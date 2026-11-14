#![forbid(unsafe_code)]
//! JWT validation utilities for OIDC/Keycloak tokens.
//!
//! Validates signature (via supplied JWK), issuer, audience, and expiry.
//! JWKS fetching/rotation is the caller's responsibility (port trait
//! [`JwksProvider`]); this crate stays transport-free.

pub mod axum;
pub mod jwks;

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("malformed token")]
    Malformed,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("token expired")]
    Expired,
    #[error("invalid issuer")]
    InvalidIssuer,
    #[error("invalid audience")]
    InvalidAudience,
    #[error("validation failed: {0}")]
    Validation(String),
    #[error("jwks unavailable")]
    KeyUnavailable,
    #[error("unknown signing key id")]
    UnknownKey,
}

/// Standard OIDC claims we rely on.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: serde_json::Value,
    pub exp: usize,
    pub iat: Option<usize>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub realm_access: Option<RealmAccess>,
    #[serde(default)]
    pub resource_access: Option<std::collections::HashMap<String, ClientAccess>>,
    #[serde(default)]
    pub azp: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RealmAccess {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Claims {
    /// Space-separated scopes from the `scope` claim.
    pub fn scopes(&self) -> Vec<&str> {
        self.scope
            .as_deref()
            .map(|s| s.split_whitespace().collect())
            .unwrap_or_default()
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes().contains(&scope)
    }

    pub fn has_role(&self, role: &str) -> bool {
        self.realm_access
            .as_ref()
            .map(|r| r.roles.iter().any(|x| x == role))
            .unwrap_or(false)
    }
}

/// Supplies JWKs for a token's `kid`. Implemented by adapters that fetch
/// and cache the issuer's JWKS (with rotation support).
pub trait JwksProvider: Send + Sync {
    fn jwk_for_kid(&self, kid: &str) -> Option<JwkKey>;
}

/// Minimal RSA JWK (modulus + exponent) sufficient for RS256 validation.
#[derive(Debug, Clone)]
pub struct JwkKey {
    pub kid: String,
    pub n: String,
    pub e: String,
}

/// Validates a JWT against issuer/audience and a JWK.
pub fn validate_token(
    token: &str,
    jwk: &JwkKey,
    issuer: &str,
    audience: &str,
) -> Result<Claims, JwtError> {
    let header = decode_header(token).map_err(|_| JwtError::Malformed)?;
    if header.alg != Algorithm::RS256 {
        return Err(JwtError::Validation(format!(
            "unsupported alg {:?}",
            header.alg
        )));
    }

    let decoding_key = DecodingKey::from_rsa_components(&jwk.n, &jwk.e)
        .map_err(|_| JwtError::Validation("invalid JWK components".into()))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[issuer]);
    validation.set_audience(&[audience]);
    validation.validate_exp = true;

    let data = decode::<Claims>(token, &decoding_key, &validation).map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtError::Expired,
        jsonwebtoken::errors::ErrorKind::InvalidIssuer => JwtError::InvalidIssuer,
        jsonwebtoken::errors::ErrorKind::InvalidAudience => JwtError::InvalidAudience,
        jsonwebtoken::errors::ErrorKind::InvalidSignature => JwtError::InvalidSignature,
        _ => JwtError::Validation(e.to_string()),
    })?;

    Ok(data.claims)
}
