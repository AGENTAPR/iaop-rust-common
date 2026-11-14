//! JWKS retrieval, caching, and RS256 validation (Keycloak/OIDC).
//! Ported from the proven iaop-agent-service implementation.

use crate::JwtError;
use base64::Engine;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,
    #[serde(rename = "use", default)]
    pub key_use: Option<String>,
    pub kid: Option<String>,
    pub alg: Option<String>,
    pub n: Option<String>,
    pub e: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwksDocument {
    keys: Vec<Jwk>,
}

/// Supplies JWKS documents; implementations may fetch remotely or serve fixtures.
pub trait JwksProvider: Send + Sync {
    fn jwks(&self) -> impl std::future::Future<Output = Result<JwksDocument, JwtError>> + Send;
}

/// Remote provider using `reqwest` against an OIDC `jwks_uri`.
pub struct RemoteJwksProvider {
    jwks_url: String,
    http: reqwest::Client,
}

impl RemoteJwksProvider {
    pub fn new(jwks_url: impl Into<String>) -> Self {
        Self {
            jwks_url: jwks_url.into(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

impl JwksProvider for RemoteJwksProvider {
    async fn jwks(&self) -> Result<JwksDocument, JwtError> {
        self.http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|_| JwtError::KeyUnavailable)?
            .json::<JwksDocument>()
            .await
            .map_err(|_| JwtError::KeyUnavailable)
    }
}

/// JWKS cache keyed by `kid`; refreshes on miss/expiry with rate-limiting and negative caching.
pub struct JwksCache<P: JwksProvider> {
    provider: Arc<P>,
    ttl: Duration,
    /// Keys older than `ttl` but younger than `stale_ttl` may still verify
    /// when the upstream JWKS fetch fails (bounded stale-key policy).
    stale_ttl: Duration,
    negative_ttl: Duration,
    min_refresh_interval: Duration,
    state: Mutex<CacheState>,
    /// Singleflight: only one task refreshes JWKS at a time; concurrent
    /// misses wait, then re-check the cache (double-checked locking).
    refresh_lock: tokio::sync::Mutex<()>,
}

struct CacheState {
    keys: HashMap<String, (Jwk, Instant)>,
    fetched_at: Option<Instant>,
    negative_cache: HashMap<String, Instant>,
}

impl<P: JwksProvider> JwksCache<P> {
    pub fn new(provider: P, ttl: Duration) -> Self {
        Self {
            provider: Arc::new(provider),
            ttl,
            stale_ttl: ttl.saturating_mul(3),
            negative_ttl: Duration::from_secs(30),
            min_refresh_interval: Duration::from_secs(10),
            state: Mutex::new(CacheState {
                keys: HashMap::new(),
                fetched_at: None,
                negative_cache: HashMap::new(),
            }),
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Returns a decoding key for `kid`, refreshing the JWKS document on miss/stale
    /// with negative caching and refresh rate-limiting to prevent JWKS storms.
    pub async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, JwtError> {
        let mut st = self.state.lock().await;

        // 1. Check positive cache
        if let Some((jwk, at)) = st.keys.get(kid) {
            if at.elapsed() < self.ttl {
                return jwk_to_key(jwk);
            }
        }

        // 2. Check negative cache (known invalid kid within cooldown)
        if let Some(at) = st.negative_cache.get(kid) {
            if at.elapsed() < self.negative_ttl {
                return Err(JwtError::UnknownKey);
            }
        }

        // 3. Rate-limit refresh: if JWKS was fetched recently, don't spam upstream
        if let Some(last_fetch) = st.fetched_at {
            if last_fetch.elapsed() < self.min_refresh_interval {
                st.negative_cache.insert(kid.to_string(), Instant::now());
                return Err(JwtError::UnknownKey);
            }
        }
        drop(st);

        // 4. Singleflight refresh: concurrent misses serialize here; the
        //    winner fetches, losers re-check the cache afterwards.
        let _refresh_guard = self.refresh_lock.lock().await;

        // Double-check: another task may have refreshed while we waited.
        {
            let mut st = self.state.lock().await;
            if let Some((jwk, at)) = st.keys.get(kid) {
                if at.elapsed() < self.ttl {
                    return jwk_to_key(jwk);
                }
            }
            if let Some(neg_at) = st.negative_cache.get(kid) {
                if neg_at.elapsed() < self.negative_ttl {
                    return Err(JwtError::UnknownKey);
                }
            }
            if let Some(last_fetch) = st.fetched_at {
                if last_fetch.elapsed() < self.min_refresh_interval {
                    st.negative_cache.insert(kid.to_string(), Instant::now());
                    return Err(JwtError::UnknownKey);
                }
            }
        }

        match self.provider.jwks().await {
            Ok(doc) => {
                let mut st = self.state.lock().await;
                st.keys.clear();
                for jwk in doc.keys {
                    if let Some(k) = jwk.kid.clone() {
                        st.keys.insert(k, (jwk, Instant::now()));
                    }
                }
                st.fetched_at = Some(Instant::now());

                // Bounded negative cache cleanup
                if st.negative_cache.len() > 500 {
                    st.negative_cache
                        .retain(|_, at| at.elapsed() < self.negative_ttl);
                }

                match st.keys.get(kid) {
                    Some((jwk, _)) => jwk_to_key(jwk),
                    None => {
                        st.negative_cache.insert(kid.to_string(), Instant::now());
                        Err(JwtError::UnknownKey)
                    }
                }
            }
            Err(e) => {
                // Stale-key policy: if the provider is down but we hold a
                // recently-expired key within the stale window, keep serving
                // it rather than failing every request.
                let st = self.state.lock().await;
                if let Some((jwk, at)) = st.keys.get(kid) {
                    if at.elapsed() < self.stale_ttl {
                        return jwk_to_key(jwk);
                    }
                }
                Err(e)
            }
        }
    }
}

fn jwk_to_key(jwk: &Jwk) -> Result<DecodingKey, JwtError> {
    if jwk.kty != "RSA" {
        return Err(JwtError::InvalidSignature);
    }
    let n = jwk.n.as_deref().ok_or(JwtError::InvalidSignature)?;
    let e = jwk.e.as_deref().ok_or(JwtError::InvalidSignature)?;
    let n_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(n)
        .map_err(|_| JwtError::InvalidSignature)?;
    let e_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(e)
        .map_err(|_| JwtError::InvalidSignature)?;
    DecodingKey::from_rsa_components(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(n_bytes),
        &base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(e_bytes),
    )
    .map_err(|_| JwtError::InvalidSignature)
}

/// Full JWT validator: signature via JWKS + iss/aud/exp/nbf.
pub struct JwtValidator<P: JwksProvider> {
    cache: JwksCache<P>,
    issuer: String,
    audience: String,
    leeway_secs: u64,
}

impl<P: JwksProvider> JwtValidator<P> {
    pub fn new(provider: P, issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            cache: JwksCache::new(provider, Duration::from_secs(300)),
            issuer: issuer.into(),
            audience: audience.into(),
            leeway_secs: 30,
        }
    }

    pub fn with_leeway(mut self, secs: u64) -> Self {
        self.leeway_secs = secs;
        self
    }

    pub async fn validate<T: for<'de> Deserialize<'de>>(&self, token: &str) -> Result<T, JwtError> {
        let header = decode_header(token).map_err(|_| JwtError::Malformed)?;
        if header.alg != Algorithm::RS256 {
            return Err(JwtError::InvalidSignature);
        }
        let kid = header.kid.ok_or(JwtError::Malformed)?;
        let key = self.cache.decoding_key(&kid).await?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = self.leeway_secs;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        decode::<T>(token, &key, &validation)
            .map(|d| d.claims)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
                jsonwebtoken::errors::ErrorKind::InvalidAudience => JwtError::InvalidAudience,
                jsonwebtoken::errors::ErrorKind::InvalidIssuer => JwtError::InvalidIssuer,
                _ => JwtError::InvalidSignature,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jwks::JwksDocument;

    struct EmptyProvider;
    impl JwksProvider for EmptyProvider {
        async fn jwks(&self) -> Result<JwksDocument, JwtError> {
            Ok(JwksDocument { keys: vec![] })
        }
    }

    #[tokio::test]
    async fn rejects_garbage_token() {
        let v = JwtValidator::new(EmptyProvider, "iss", "aud");
        assert!(matches!(
            v.validate::<serde_json::Value>("not-a-jwt").await,
            Err(JwtError::Malformed)
        ));
    }

    #[test]
    fn jwk_to_key_rejects_non_rsa() {
        let jwk = Jwk {
            kty: "EC".into(),
            key_use: None,
            kid: None,
            alg: None,
            n: None,
            e: None,
        };
        assert!(jwk_to_key(&jwk).is_err());
    }

    #[tokio::test]
    async fn rate_limits_unknown_kid_and_negative_caches() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct CountingProvider(Arc<AtomicUsize>);
        impl JwksProvider for CountingProvider {
            async fn jwks(&self) -> Result<JwksDocument, JwtError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(JwksDocument { keys: vec![] })
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let cache = JwksCache::new(CountingProvider(counter.clone()), Duration::from_secs(300));

        // First call fetches from upstream
        let res1 = cache.decoding_key("unknown-1").await;
        assert!(matches!(res1, Err(JwtError::UnknownKey)));
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Immediate second call with different unknown kid hits min_refresh_interval and does NOT call upstream
        let res2 = cache.decoding_key("unknown-2").await;
        assert!(matches!(res2, Err(JwtError::UnknownKey)));
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Third call with first unknown kid hits negative cache directly
        let res3 = cache.decoding_key("unknown-1").await;
        assert!(matches!(res3, Err(JwtError::UnknownKey)));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_unknown_kids_trigger_single_refresh() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct SlowProvider(Arc<AtomicUsize>);
        impl JwksProvider for SlowProvider {
            async fn jwks(&self) -> Result<JwksDocument, JwtError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(JwksDocument { keys: vec![] })
            }
        }

        let counter = Arc::new(AtomicUsize::new(0));
        let cache = std::sync::Arc::new(JwksCache::new(
            SlowProvider(counter.clone()),
            Duration::from_secs(300),
        ));

        let mut handles = Vec::new();
        for i in 0..20 {
            let c = cache.clone();
            handles.push(tokio::spawn(async move {
                c.decoding_key(&format!("kid-{i}")).await
            }));
        }
        for h in handles {
            assert!(matches!(h.await.unwrap(), Err(JwtError::UnknownKey)));
        }
        // Singleflight + rate limiting: far fewer upstream fetches than callers.
        assert!(counter.load(Ordering::SeqCst) <= 2);
    }
}
