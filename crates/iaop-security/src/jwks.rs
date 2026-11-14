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

/// JWKS cache keyed by `kid`; refreshes on miss/expiry.
pub struct JwksCache<P: JwksProvider> {
    provider: Arc<P>,
    ttl: Duration,
    state: Mutex<CacheState>,
}

struct CacheState {
    keys: HashMap<String, (Jwk, Instant)>,
    fetched_at: Option<Instant>,
}

impl<P: JwksProvider> JwksCache<P> {
    pub fn new(provider: P, ttl: Duration) -> Self {
        Self {
            provider: Arc::new(provider),
            ttl,
            state: Mutex::new(CacheState {
                keys: HashMap::new(),
                fetched_at: None,
            }),
        }
    }

    /// Returns a decoding key for `kid`, refreshing the JWKS document on miss/stale.
    pub async fn decoding_key(&self, kid: &str) -> Result<DecodingKey, JwtError> {
        {
            let st = self.state.lock().await;
            if let Some((jwk, at)) = st.keys.get(kid) {
                if at.elapsed() < self.ttl {
                    return jwk_to_key(jwk);
                }
            }
            let stale = st
                .fetched_at
                .map(|t| t.elapsed() >= self.ttl)
                .unwrap_or(true);
            if !stale {
                // fresh doc but unknown kid — try once more below anyway
            }
        }
        self.refresh().await?;
        let st = self.state.lock().await;
        match st.keys.get(kid) {
            Some((jwk, _)) => jwk_to_key(jwk),
            None => Err(JwtError::UnknownKey),
        }
    }

    async fn refresh(&self) -> Result<(), JwtError> {
        let doc = self.provider.jwks().await?;
        let mut st = self.state.lock().await;
        st.keys.clear();
        for jwk in doc.keys {
            if let Some(kid) = jwk.kid.clone() {
                st.keys.insert(kid, (jwk, Instant::now()));
            }
        }
        st.fetched_at = Some(Instant::now());
        Ok(())
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
}
