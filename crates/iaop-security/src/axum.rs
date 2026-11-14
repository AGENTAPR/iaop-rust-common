//! Axum integration: bearer extraction, auth middleware, principal.
//! 401 = unauthenticated; 403 = authenticated but unauthorized.

use crate::{
    Claims, JwtError,
    jwks::{JwksProvider, JwtValidator},
};
use axum::{
    Json,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AuthenticatedPrincipal {
    pub subject: String,
    pub issuer: String,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub realm_roles: Vec<String>,
    /// Audience-scoped client roles (Keycloak `resource_access[<audience>]`).
    pub client_roles: Vec<String>,
    /// Effective permissions = realm roles ∪ client roles for trusted audience.
    pub permissions: Vec<String>,
}

impl AuthenticatedPrincipal {
    pub fn has_permission(&self, p: &str) -> bool {
        self.permissions.iter().any(|x| x == p)
    }
    pub fn has_scope(&self, s: &str) -> bool {
        self.scopes.iter().any(|x| x == s)
    }
}

impl From<Claims> for AuthenticatedPrincipal {
    fn from(c: Claims) -> Self {
        let scopes: Vec<String> = c.scopes().iter().map(|s| s.to_string()).collect();
        let realm_roles = c
            .realm_access
            .as_ref()
            .map(|r| r.roles.clone())
            .unwrap_or_default();
        let mut client_roles = Vec::new();
        if let Some(ra) = &c.resource_access {
            // merge roles only for clients listed in token audience
            let auds: Vec<String> = match &c.aud {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                _ => vec![],
            };
            for client in auds {
                if let Some(entry) = ra.get(&client) {
                    client_roles.extend(entry.roles.clone());
                }
            }
        }
        let mut permissions = realm_roles.clone();
        permissions.extend(client_roles.iter().cloned());
        permissions.sort();
        permissions.dedup();
        Self {
            subject: c.sub,
            issuer: c.iss,
            client_id: c.azp.or(c.client_id),
            scopes,
            realm_roles,
            client_roles,
            permissions,
        }
    }
}

#[derive(Debug, Serialize)]
struct AuthErrorBody {
    code: &'static str,
    detail: &'static str,
}

fn unauthorized(detail: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(AuthErrorBody {
            code: "UNAUTHORIZED",
            detail,
        }),
    )
        .into_response()
}

fn forbidden(detail: &'static str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(AuthErrorBody {
            code: "FORBIDDEN",
            detail,
        }),
    )
        .into_response()
}

/// Extractor: `Authorization: Bearer <jwt>` -> [`AuthenticatedPrincipal`].
/// Requires the validator to have been run by [`auth_middleware`] first
/// (principal stored in request extensions), or performs inline validation
/// when constructed with a validator.
pub struct AuthUser(pub AuthenticatedPrincipal);

impl FromRequestParts<Arc<AuthConfig>> for AuthUser {
    type Rejection = Response;

    #[allow(clippy::result_large_err)]
    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AuthConfig>,
    ) -> Result<Self, Self::Rejection> {
        // Prefer middleware-injected principal.
        if let Some(p) = parts.extensions.get::<AuthenticatedPrincipal>() {
            return Ok(Self(p.clone()));
        }
        let token = bearer_token(parts).ok_or_else(|| unauthorized("missing bearer token"))?;
        let claims = state
            .validator
            .validate(token)
            .await
            .map_err(|_| unauthorized("invalid token"))?;
        let principal = AuthenticatedPrincipal::from(claims);
        Ok(Self(principal))
    }
}

fn bearer_token(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Shared auth configuration holding the validator.
pub struct AuthConfig {
    pub validator: Arc<dyn TokenValidator>,
}

/// Object-safe validator abstraction for middleware state.
pub trait TokenValidator: Send + Sync {
    fn validate<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Claims, JwtError>> + Send + 'a>>;
}

impl<P: JwksProvider> TokenValidator for JwtValidator<P> {
    fn validate<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Claims, JwtError>> + Send + 'a>>
    {
        Box::pin(async move { self.validate(token).await })
    }
}

/// Middleware: validates bearer token, injects principal into extensions.
/// Missing/invalid token -> 401. Downstream handlers get `AuthUser`.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AuthConfig>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(token) = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ").map(str::to_owned))
    else {
        return unauthorized("missing bearer token");
    };
    match state.validator.validate(&token).await {
        Ok(claims) => {
            req.extensions_mut()
                .insert(AuthenticatedPrincipal::from(claims));
            next.run(req).await
        }
        Err(_) => unauthorized("invalid token"),
    }
}

/// Authorization helper: require a permission on the authenticated principal.
#[allow(clippy::result_large_err)]
pub fn require_permission(principal: &AuthenticatedPrincipal, perm: &str) -> Result<(), Response> {
    if principal.has_permission(perm) {
        Ok(())
    } else {
        Err(forbidden("insufficient permissions"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn principal_merges_audience_client_roles_only() {
        let claims: Claims = serde_json::from_value(serde_json::json!({
            "sub": "u1", "iss": "iss", "aud": ["acct", "iaop-agent-api"],
            "exp": 9999999999i64, "iat": 0i64, "nbf": 0i64,
            "scope": "openid",
            "realm_access": {"roles": ["IAOP_AGENT"]},
            "resource_access": {
                "iaop-agent-api": {"roles": ["agent:read", "agent:activate"]},
                "other-app": {"roles": ["admin:all"]}
            }
        }))
        .unwrap();
        let p = AuthenticatedPrincipal::from(claims);
        assert!(p.has_permission("agent:read"));
        assert!(p.has_permission("agent:activate"));
        assert!(p.has_permission("IAOP_AGENT"));
        // unrelated client role must NOT leak into permissions
        assert!(!p.has_permission("admin:all"));
        assert_eq!(p.client_roles.len(), 2);
    }
}
