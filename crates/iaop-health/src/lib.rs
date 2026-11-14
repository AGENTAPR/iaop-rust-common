#![forbid(unsafe_code)]
//! Liveness/readiness primitives for Axum services.
//! Readiness contributors are explicit and bounded — optional
//! dependencies must not gate traffic.

use axum::{Json, Router, routing::get};
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub status: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A readiness dependency probe (database, cache, required config).
pub trait ReadinessCheck: Send + Sync {
    fn name(&self) -> &'static str;
    fn check(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>;
}

/// Database readiness adapter for `sqlx::PgPool`.
pub struct PgReadiness {
    pool: sqlx::PgPool,
    timeout: Duration,
}

impl PgReadiness {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            timeout: Duration::from_secs(2),
        }
    }
}

impl ReadinessCheck for PgReadiness {
    fn name(&self) -> &'static str {
        "postgres"
    }
    fn check(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>> {
        let pool = self.pool.clone();
        let t = self.timeout;
        Box::pin(async move {
            timeout(t, sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&pool))
                .await
                .map_err(|_| "timeout".to_string())?
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}

/// Aggregates readiness checks with per-check timeouts.
pub struct HealthRegistry {
    checks: Vec<Arc<dyn ReadinessCheck>>,
    check_timeout: Duration,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self {
            checks: Vec::new(),
            check_timeout: Duration::from_secs(2),
        }
    }
    pub fn with_check(mut self, c: Arc<dyn ReadinessCheck>) -> Self {
        self.checks.push(c);
        self
    }
    pub fn check_timeout(mut self, d: Duration) -> Self {
        self.check_timeout = d;
        self
    }

    /// Runs all checks concurrently-ish (sequential is fine at this scale),
    /// each bounded by `check_timeout`.
    pub async fn readiness(&self) -> HealthStatus {
        let mut results = Vec::new();
        let mut all_ok = true;
        for c in &self.checks {
            let res = timeout(self.check_timeout, c.check()).await;
            let (ok, err) = match res {
                Ok(Ok(())) => (true, None),
                Ok(Err(e)) => (false, Some(e)),
                Err(_) => (false, Some("timeout".into())),
            };
            if !ok {
                all_ok = false;
            }
            results.push(CheckResult {
                name: c.name().to_string(),
                ok,
                error: err,
            });
        }
        HealthStatus {
            status: if all_ok { "ok" } else { "degraded" },
            checks: results,
        }
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Mounts `/health/live` and `/health/ready`.
pub fn health_router(registry: Arc<HealthRegistry>) -> Router {
    let ready = {
        let reg = registry.clone();
        Router::new().route(
            "/ready",
            get(move || {
                let reg = reg.clone();
                async move { Json(reg.readiness().await) }
            }),
        )
    };
    Router::new()
        .route(
            "/health/live",
            get(|| async {
                Json(HealthStatus {
                    status: "ok",
                    checks: vec![],
                })
            }),
        )
        .merge(ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    struct AlwaysOk;
    impl ReadinessCheck for AlwaysOk {
        fn name(&self) -> &'static str {
            "ok"
        }
        fn check(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Ok(()) })
        }
    }
    struct AlwaysFail;
    impl ReadinessCheck for AlwaysFail {
        fn name(&self) -> &'static str {
            "fail"
        }
        fn check(
            &self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + '_>>
        {
            Box::pin(async { Err("down".into()) })
        }
    }

    #[tokio::test]
    async fn readiness_aggregates() {
        let reg = HealthRegistry::new()
            .with_check(Arc::new(AlwaysOk))
            .with_check(Arc::new(AlwaysFail));
        let s = reg.readiness().await;
        assert_eq!(s.status, "degraded");
        assert_eq!(s.checks.len(), 2);
        assert!(!s.checks[1].ok);
    }

    #[tokio::test]
    async fn router_serves_endpoints() {
        let app = health_router(Arc::new(HealthRegistry::new()));
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/health/live")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
