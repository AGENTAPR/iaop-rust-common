#!/usr/bin/env python3
"""Generate the IAOP Axum microservice scaffold into a repository directory.

Usage: python scaffold.py <repo_dir> <service_name> <service_title> <db_name> [--bff]
"""
import os, sys, json

RUST_VER = "1.100.0"

# Dependency versions (pinned majors; lockfiles generated per repo)
DEPS = {
    "axum": '0.8',
    "tokio": '1',
    "serde": '1',
    "serde_json": '1',
    "uuid": '1',
    "chrono": '0.4',
    "thiserror": '2',
    "tracing": '0.1',
    "tracing-subscriber": '0.3',
    "sqlx": '0.8',
    "utoipa": '5',
    "tower": '0.5',
    "anyhow": '1',
}

def w(root, path, content):
    p = os.path.join(root, path.replace('/', os.sep))
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, 'w', newline='\n') as f:
        f.write(content)
    print('  +', path)

# ---------------------------------------------------------------- templates

DOCKERIGNORE = '''target/
.git/
.gitignore
*.log
.env
.env.*
!.env.example
node_modules/
'''

GITIGNORE = '''# Rust
/target/
**/*.rs.bk
*.pdb
*.log

# Env / secrets
.env
.env.*
!.env.example

# IDE
.idea/
.vscode/
*.swp

# OS
.DS_Store
Thumbs.db
'''

def rust_toolchain():
    return f'''[toolchain]
channel = "{RUST_VER}"
profile = "minimal"
components = ["rustfmt", "clippy"]
'''

def workspace_toml(svc_us, binary):
    return f'''[workspace]
resolver = "3"
members = [
    "crates/domain",
    "crates/application",
    "crates/infrastructure",
    "crates/api",
]

[workspace.package]
edition = "2024"
rust-version = "1.85"
license = "UNLICENSED"
publish = false

[workspace.dependencies]
{svc_us}-domain = {{ path = "crates/domain" }}
{svc_us}-application = {{ path = "crates/application" }}
{svc_us}-infrastructure = {{ path = "crates/infrastructure" }}
{svc_us}-api = {{ path = "crates/api" }}
serde = {{ version = "{DEPS['serde']}", features = ["derive"] }}
serde_json = "{DEPS['serde_json']}"
uuid = {{ version = "{DEPS['uuid']}", features = ["v4", "v7", "serde"] }}
chrono = {{ version = "{DEPS['chrono']}", features = ["serde"] }}
thiserror = "{DEPS['thiserror']}"
tracing = "{DEPS['tracing']}"
tokio = {{ version = "{DEPS['tokio']}", features = ["full"] }}
sqlx = {{ version = "{DEPS['sqlx']}", features = [
    "runtime-tokio-rustls",
    "postgres",
    "uuid",
    "chrono",
    "json",
    "migrate",
] }}
axum = "{DEPS['axum']}"
tower = "{DEPS['tower']}"
utoipa = "{DEPS['utoipa']}"
anyhow = "{DEPS['anyhow']}"

[package]
name = "{binary}"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
{svc_us}-domain.workspace = true
{svc_us}-application.workspace = true
{svc_us}-infrastructure.workspace = true
{svc_us}-api.workspace = true
tokio.workspace = true
axum.workspace = true
tracing.workspace = true
tracing-subscriber = {{ version = "{DEPS['tracing-subscriber']}", features = ["env-filter", "json"] }}
anyhow.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true

[dev-dependencies]
tower = {{ workspace = true, features = ["util"] }}
tokio = {{ workspace = true, features = ["macros", "rt-multi-thread"] }}
'''

def crate_toml(crate_name, binary, extra_deps="", dev_deps=""):
    return f'''[package]
name = "{crate_name}"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
{extra_deps}{dev_deps}'''

DOMAIN_LIB = '''#![forbid(unsafe_code)]
//! Domain layer for {title}.
//!
//! This crate contains the pure domain model: entities, value objects,
//! aggregates, domain events, and domain errors. It MUST NOT depend on
//! Axum, SQLx, HTTP clients, Keycloak, message brokers, or any other
//! infrastructure concern.
//!
//! Dependency rule: `domain` has no dependencies on sibling crates.

pub mod entity_id;
pub mod error;

pub use entity_id::EntityId;
pub use error::DomainError;
'''

ENTITY_ID = '''use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for domain entities and aggregates.
///
/// Wraps a UUIDv7 so identifiers are time-ordered and globally unique
/// without exposing the raw UUID type throughout the domain model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(Uuid);

impl EntityId {
    /// Generates a new time-ordered (UUIDv7) identifier.
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for EntityId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_id_roundtrips_through_string() {
        let id = EntityId::new();
        let parsed: EntityId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn entity_id_is_unique() {
        assert_ne!(EntityId::new(), EntityId::new());
    }
}
'''

DOMAIN_ERROR = '''use thiserror::Error;

/// Domain-level error type.
///
/// Mapped to API problem details by the api crate. Never contains
/// infrastructure details (SQL, HTTP status, stack traces).
#[derive(Debug, Error)]
pub enum DomainError {
    #[error("validation failed: {0}")]
    Validation(String),

    #[error("resource not found")]
    NotFound,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("operation not permitted in current state")]
    InvalidState,
}

impl DomainError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }
}
'''

APPLICATION_LIB = '''#![forbid(unsafe_code)]
//! Application layer for {title}.
//!
//! Contains use cases (commands and queries) and the ports they depend on.
//! Depends only on the `domain` crate — never on Axum, SQLx, or other
//! infrastructure. Adapters in `infrastructure` implement the ports.

pub mod ports;
pub mod service_info;
'''

SERVICE_INFO = '''use serde::{Deserialize, Serialize};

/// Query: return static service metadata for `GET /api/v1/system/info`.
#[derive(Debug, Clone)]
pub struct GetServiceInfoQuery {
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
}

/// Response DTO produced by the use case (transport-agnostic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub service: String,
    pub version: String,
    pub environment: String,
    pub status: &'static str,
}

impl GetServiceInfoQuery {
    pub fn execute(self) -> ServiceInfo {
        ServiceInfo {
            service: self.service_name,
            version: self.service_version,
            environment: self.environment,
            status: "ok",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_service_metadata() {
        let info = GetServiceInfoQuery {
            service_name: "svc".into(),
            service_version: "0.1.0".into(),
            environment: "local".into(),
        }
        .execute();
        assert_eq!(info.service, "svc");
        assert_eq!(info.status, "ok");
    }
}
'''

PORTS = '''//! Ports (interfaces) that the application layer requires from the
//! outside world. Implementations live in `infrastructure`.
//!
//! Convention: a port is only introduced when a use case actually needs it.
//! Do not add ports speculatively.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use {svc_us}_domain::EntityId;

/// Canonical integration-event envelope published through the outbox.
///
/// Mirrors the `iaop-event-contracts` envelope schema. Domain events are
/// translated to integration events at the application boundary — internal
/// domain objects are never published directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationEvent {
    pub event_id: Uuid,
    pub event_type: String,
    pub event_version: u32,
    pub source: String,
    pub aggregate_id: Option<EntityId>,
    pub aggregate_version: Option<u64>,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
#[error("failed to publish event: {0}")]
pub struct PublishError(pub String);

/// Port: publish an integration event durably (implemented via the
/// transactional outbox in `infrastructure::persistence`).
pub trait EventPublisher: Send + Sync {
    fn publish<'a>(
        &'a self,
        event: &'a IntegrationEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), PublishError>> + Send + 'a>>;
}
'''

INFRA_LIB = '''#![forbid(unsafe_code)]
//! Infrastructure layer for {title}.
//!
//! Adapters implementing application ports: configuration, PostgreSQL
//! persistence (pool, migrations, outbox/inbox), messaging helpers,
//! and security utilities. Depends on `application` and `domain`.

pub mod config;
pub mod messaging;
pub mod persistence;
pub mod security;
'''

CONFIG_RS = '''//! Typed application configuration loaded from environment variables.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    MissingVar(&'static str),
    #[error("invalid value for {0}: {1}")]
    InvalidValue(&'static str, String),
}

/// Service configuration. Secrets (e.g. DATABASE_URL) are never logged;
/// use [`AppConfig::redacted_summary`] for safe diagnostics.
#[derive(Debug, Clone)]
pub struct AppConfig {
    pub service_name: String,
    pub service_version: String,
    pub environment: String,
    pub bind_addr: String,
    /// Optional: services without persistence leave this unset and
    /// `/health/ready` does not probe a database.
    pub database_url: Option<String>,
    pub database_max_connections: u32,
    pub request_timeout: Duration,
}

impl AppConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            service_name: env_or("SERVICE_NAME", "{binary}"),
            service_version: env_or("SERVICE_VERSION", env!("CARGO_PKG_VERSION")),
            environment: env_or("ENVIRONMENT", "local"),
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            database_url: std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty()),
            database_max_connections: env_parse("DATABASE_MAX_CONNECTIONS", 10)?,
            request_timeout: Duration::from_secs(env_parse("REQUEST_TIMEOUT_SECS", 30)?),
        })
    }

    /// Diagnostics-safe summary: never includes secrets.
    pub fn redacted_summary(&self) -> String {
        format!(
            "service={} version={} env={} bind={} db={} timeout={}s",
            self.service_name,
            self.service_version,
            self.environment,
            self.bind_addr,
            if self.database_url.is_some() { "configured" } else { "disabled" },
            self.request_timeout.as_secs(),
        )
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T: std::str::FromStr>(key: &'static str, default: T) -> Result<T, ConfigError> {
    match std::env::var(key) {
        Ok(v) => v
            .parse::<T>()
            .map_err(|_| ConfigError::InvalidValue(key, v)),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_loads_with_defaults() {
        let cfg = AppConfig::from_env().expect("defaults must be valid");
        assert!(!cfg.service_name.is_empty());
        assert!(cfg.redacted_summary().contains("db="));
    }
}
'''

PERSISTENCE_RS = '''//! PostgreSQL persistence: connection pool, migrations, and the
//! transactional outbox/inbox tables defined in `migrations/`.
//!
//! This service owns its database exclusively. No cross-service queries,
//! joins, or foreign keys are permitted.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Establish a connection pool. Returns `None`-safe callers should check
/// `database_url` presence before calling.
pub async fn connect_pool(
    database_url: &str,
    max_connections: u32,
) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(database_url)
        .await
}

/// Apply pending SQLx migrations from the service-root `migrations/` dir.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

/// Readiness probe: verifies the pool can reach PostgreSQL.
pub async fn ping(pool: &PgPool) -> bool {
    sqlx::query("SELECT 1")
        .fetch_one(pool)
        .await
        .is_ok()
}

/// Outbox record mirroring the `outbox_events` table.
/// The background publisher (added with the first event-producing use
/// case) claims unpublished rows, publishes them to the broker, and sets
/// `published_at`.
#[derive(Debug, Clone)]
pub struct OutboxRecord {
    pub event_id: uuid::Uuid,
    pub aggregate_id: Option<uuid::Uuid>,
    pub aggregate_type: Option<String>,
    pub aggregate_version: Option<i64>,
    pub event_type: String,
    pub event_version: i32,
    pub payload: serde_json::Value,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub publish_attempts: i32,
    pub last_error: Option<String>,
}
'''

MESSAGING_RS = '''//! Messaging helpers and the outbox/inbox model.
//!
//! Integration events use the canonical envelope from `iaop-event-contracts`.
//! Delivery is at-least-once; consumers must be idempotent (inbox table).
//! Exactly-once delivery is NOT claimed.
//!
//! Publisher implementation is added together with the first event-producing
//! use case; the `EventPublisher` port is defined in the application layer.

use {svc_us}_application::ports::IntegrationEvent;

/// Validates that an event can be represented as an outbox row.
pub fn validate_event(event: &IntegrationEvent) -> Result<(), String> {
    if event.event_type.is_empty() {
        return Err("event_type must not be empty".into());
    }
    if event.event_version == 0 {
        return Err("event_version must be >= 1".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn event() -> IntegrationEvent {
        IntegrationEvent {
            event_id: Uuid::now_v7(),
            event_type: "agent.activated".into(),
            event_version: 1,
            source: "test".into(),
            aggregate_id: None,
            aggregate_version: None,
            occurred_at: Utc::now(),
            correlation_id: None,
            causation_id: None,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn valid_event_passes() {
        assert!(validate_event(&event()).is_ok());
    }

    #[test]
    fn empty_event_type_fails() {
        let mut e = event();
        e.event_type = String::new();
        assert!(validate_event(&e).is_err());
    }
}
'''

SECURITY_RS = '''//! Security utilities: token extraction helpers and public-path policy.
//!
//! JWT/JWKS validation is centralized in `iaop-security` (iaop-rust-common);
//! services adopt it once tagged releases exist. This module only provides
//! transport-level helpers that contain no cryptographic logic.

/// Extracts the bearer token from an `Authorization` header value.
/// Returns `None` for missing, malformed, or non-bearer headers.
/// Never logs the token.
pub fn bearer_token(authorization: Option<&str>) -> Option<&str> {
    let value = authorization?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        let token = token.trim();
        if token.is_empty() { None } else { Some(token) }
    } else {
        None
    }
}

/// Paths that bypass authentication. Health and API-docs endpoints are the
/// only permitted public paths; everything else requires a valid token.
pub fn is_public_path(path: &str) -> bool {
    matches!(
        path,
        "/health/live" | "/health/ready" | "/api/docs/openapi.json"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_bearer_token() {
        assert_eq!(bearer_token(Some("Bearer abc.def.ghi")), Some("abc.def.ghi"));
        assert_eq!(bearer_token(Some("bearer  tok ")), Some("tok"));
    }

    #[test]
    fn rejects_missing_or_malformed() {
        assert_eq!(bearer_token(None), None);
        assert_eq!(bearer_token(Some("Basic abc")), None);
        assert_eq!(bearer_token(Some("Bearer ")), None);
    }

    #[test]
    fn public_paths_are_limited() {
        assert!(is_public_path("/health/live"));
        assert!(!is_public_path("/api/v1/agents"));
        assert!(!is_public_path("/health/ready/extra"));
    }
}
'''

API_LIB = '''#![forbid(unsafe_code)]
//! API layer (inbound adapter) for {title}.
//!
//! Axum router, HTTP handlers, DTOs, middleware, and error mapping.
//! Depends on `application` and `infrastructure` for wiring types.

pub mod dto;
pub mod error_mapping;
pub mod middleware;
pub mod openapi;
pub mod routes;
pub mod state;

pub use routes::build_router;
pub use state::AppState;
'''

API_STATE = '''use std::sync::Arc;

use sqlx::PgPool;
use {svc_us}_infrastructure::config::AppConfig;

/// Shared application state injected into handlers via `State`.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    /// `None` when DATABASE_URL is not configured; `/health/ready`
    /// reports accordingly instead of failing.
    pub db: Option<PgPool>,
}
'''

API_DTO = '''use serde::Serialize;

/// Wire format for `GET /health/live` and `/health/ready`.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

/// Wire format for `GET /api/v1/system/info`.
#[derive(Debug, Serialize)]
pub struct SystemInfoResponse {
    pub service: String,
    pub version: String,
    pub environment: String,
    pub status: &'static str,
}
'''

API_ERRORS = '''//! RFC 9457 Problem Details error mapping.
//!
//! Never expose stack traces, SQL errors, or internal details to clients.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub correlation_id: Option<Uuid>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, title: &'static str, detail: impl Into<String>) -> Self {
        Self { status, code, title, detail: detail.into(), correlation_id: None }
    }

    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR", "Validation failed", detail)
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", "Resource not found", "The requested resource does not exist")
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        // Detail is logged server-side; clients get a generic message.
        tracing::error!(error = %detail.into(), "internal error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", "Internal error", "An unexpected error occurred")
    }

    pub fn with_correlation(mut self, id: Option<Uuid>) -> Self {
        self.correlation_id = id;
        self
    }
}

#[derive(Serialize)]
struct ProblemDetails {
    #[serde(rename = "type")]
    problem_type: String,
    title: String,
    status: u16,
    detail: String,
    code: String,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    correlation_id: Option<Uuid>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ProblemDetails {
            problem_type: format!("https://api.iaop.internal/problems/{}", self.code.to_lowercase()),
            title: self.title.to_string(),
            status: self.status.as_u16(),
            detail: self.detail,
            code: self.code.to_string(),
            correlation_id: self.correlation_id,
        };
        (self.status, Json(body)).into_response()
    }
}
'''

API_MIDDLEWARE = '''//! HTTP middleware: request IDs, tracing, and request timeouts.

use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::header::{self, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::Instrument;
use uuid::Uuid;

use crate::error_mapping::ApiError;
use crate::state::AppState;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const CORRELATION_ID_HEADER: &str = "x-correlation-id";

/// Adds a request/correlation ID, emits a tracing span with method/path/
/// latency, and enforces the configured request timeout.
pub async fn request_context(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .unwrap_or_else(Uuid::now_v7);

    request.extensions_mut().insert(RequestContext {
        request_id,
        correlation_id: request_id,
    });

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        correlation_id = %request_id,
        method = %method,
        path = %path,
    );

    async move {
        let start = Instant::now();
        let response = match tokio::time::timeout(state.config.request_timeout, next.run(request)).await {
            Ok(resp) => resp,
            Err(_) => ApiError::new(
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "REQUEST_TIMEOUT",
                "Request timeout",
                "The request exceeded the allowed processing time",
            )
            .with_correlation(Some(request_id))
            .into_response(),
        };
        let latency_ms = start.elapsed().as_millis() as u64;
        tracing::info!(latency_ms, status = response.status().as_u16(), "request completed");
        let mut response = response;
        if let Ok(v) = HeaderValue::from_str(&request_id.to_string()) {
            response
                .headers_mut()
                .insert(header::HeaderName::from_static("x-request-id"), v);
        }
        response
    }
    .instrument(span)
    .await
}

/// Per-request identifiers propagated to logs and downstream calls.
#[derive(Debug, Clone, Copy)]
pub struct RequestContext {
    pub request_id: Uuid,
    pub correlation_id: Uuid,
}

impl RequestContext {
    /// Retrieves the context inserted by [`request_context`].
    pub fn from_request(req: &Request) -> Option<Self> {
        req.extensions().get::<RequestContext>().copied()
    }
}
'''

API_ROUTES = '''//! HTTP route handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use utoipa::OpenApi;
use {svc_us}_application::service_info::GetServiceInfoQuery;
use {svc_us}_infrastructure::persistence;

use crate::dto::{HealthResponse, SystemInfoResponse};
use crate::middleware;
use crate::openapi::ApiDoc;
use crate::state::AppState;

/// Builds the service router with all baseline endpoints.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/api/v1/system/info", get(system_info))
        .route("/api/docs/openapi.json", get(openapi_json))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::request_context,
        ))
        .with_state(state)
}

/// Liveness: process is up. Must NOT depend on external systems.
async fn live() -> impl IntoResponse {
    (StatusCode::OK, Json(HealthResponse { status: "ok" }))
}

/// Readiness: dependencies required to serve traffic.
/// - No DB configured: ready (DB intentionally disabled).
/// - DB configured: ready only if a query succeeds.
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match &state.db {
        Some(pool) => {
            if persistence::ping(pool).await {
                (StatusCode::OK, Json(HealthResponse { status: "ready" }))
            } else {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(HealthResponse { status: "degraded" }),
                )
            }
        }
        None => (StatusCode::OK, Json(HealthResponse { status: "ready" })),
    }
}

async fn system_info(State(state): State<AppState>) -> impl IntoResponse {
    let info = GetServiceInfoQuery {
        service_name: state.config.service_name.clone(),
        service_version: state.config.service_version.clone(),
        environment: state.config.environment.clone(),
    }
    .execute();
    Json(SystemInfoResponse {
        service: info.service,
        version: info.version,
        environment: info.environment,
        status: info.status,
    })
}

async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}
'''

API_OPENAPI = '''//! OpenAPI 3.1 document generation.
//!
//! Baseline document is generated from `utoipa`. Path annotations are added
//! as real endpoints are implemented; `iaop-api-contracts` remains the
//! canonical contract source of truth.

use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "{title} API",
        version = "0.1.0",
        description = "IAOP {title} service API. Contracts are governed by iaop-api-contracts."
    ),
    servers((url = "/"))
)]
pub struct ApiDoc;
'''

MAIN_RS = '''//! Composition root for {title}.
//!
//! Wires configuration, persistence, and the HTTP router, then serves with
//! graceful shutdown on SIGINT/SIGTERM.

use std::sync::Arc;

use anyhow::Context;
use {svc_us}_api::{build_router, AppState};
use {svc_us}_infrastructure::config::AppConfig;
use {svc_us}_infrastructure::persistence;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = AppConfig::from_env().context("failed to load configuration")?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .json()
        .init();

    tracing::info!(config = %config.redacted_summary(), "starting service");

    let db = match &config.database_url {
        Some(url) => {
            let pool = persistence::connect_pool(url, config.database_max_connections)
                .await
                .context("failed to connect to database")?;
            persistence::run_migrations(&pool)
                .await
                .context("failed to run migrations")?;
            tracing::info!("database migrations applied");
            Some(pool)
        }
        None => {
            tracing::warn!("DATABASE_URL not set; running without persistence");
            None
        }
    };

    let state = AppState {
        config: Arc::new(config.clone()),
        db,
    };
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    tracing::info!("shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
'''

ENV_EXAMPLE = '''# Service configuration (see crates/infrastructure/src/config.rs)
SERVICE_NAME={binary}
SERVICE_VERSION=0.1.0
ENVIRONMENT=local
BIND_ADDR=0.0.0.0:8080

# Optional persistence - omit to run without a database
# DATABASE_URL=postgres://iaop:iaop@localhost:5432/{db}
# DATABASE_MAX_CONNECTIONS=10

# Request timeout
REQUEST_TIMEOUT_SECS=30

# Logging
RUST_LOG=info,{svc_us}=debug
'''

DOCKERFILE = '''# syntax=docker/dockerfile:1
FROM rust:{ver}-slim-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --locked --bin {binary}

FROM debian:bookworm-slim AS runtime
RUN useradd --system --uid 10001 --create-home appuser
WORKDIR /app
COPY --from=builder /app/target/release/{binary} /app/{binary}
USER appuser
EXPOSE 8080
ENV RUST_LOG=info
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \\
    CMD ["/app/{binary}", "--healthcheck"] || exit 1
ENTRYPOINT ["/app/{binary}"]
'''

# NOTE: healthcheck flag not implemented; adjust Dockerfile to use wget? Keep simple:
DOCKERFILE = DOCKERFILE.replace('''HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \\
    CMD ["/app/{binary}", "--healthcheck"] || exit 1
''', '''HEALTHCHECK --interval=30s --timeout=5s --start-period=10s \\
    CMD ["sh", "-c", "wget -qO- http://127.0.0.1:${PORT:-8080}/health/live >/dev/null 2>&1 || exit 1"]
''')

MIGRATION_OUTBOX = '''-- Transactional outbox: events are written in the same transaction as the
-- state change, then relayed to the broker by the publisher.
-- At-least-once delivery; consumers must deduplicate via inbox_events.

CREATE TABLE IF NOT EXISTS outbox_events (
    event_id            UUID PRIMARY KEY,
    aggregate_id        UUID,
    aggregate_type      TEXT,
    aggregate_version   BIGINT,
    event_type          TEXT        NOT NULL,
    event_version       INTEGER     NOT NULL DEFAULT 1,
    payload             JSONB       NOT NULL,
    occurred_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    published_at        TIMESTAMPTZ,
    publish_attempts    INTEGER     NOT NULL DEFAULT 0,
    last_error          TEXT
);

CREATE INDEX IF NOT EXISTS idx_outbox_unpublished
    ON outbox_events (created_at)
    WHERE published_at IS NULL;
'''

MIGRATION_INBOX = '''-- Inbox / deduplication: processed integration events are recorded so
-- redeliveries are idempotent. Unique on (consumer_name, event_id).

CREATE TABLE IF NOT EXISTS inbox_events (
    event_id        UUID        NOT NULL,
    consumer_name   TEXT        NOT NULL,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at    TIMESTAMPTZ,
    status          TEXT        NOT NULL DEFAULT 'received'
                    CHECK (status IN ('received', 'processed', 'failed')),
    last_error      TEXT,
    PRIMARY KEY (consumer_name, event_id)
);

CREATE INDEX IF NOT EXISTS idx_inbox_status
    ON inbox_events (status, received_at);
'''

INTEGRATION_TEST = '''//! Smoke integration test: the router serves health and info endpoints
//! without a database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use {svc_us}_api::{build_router, AppState};
use {svc_us}_infrastructure::config::AppConfig;
use std::sync::Arc;

fn test_state() -> AppState {
    AppState {
        config: Arc::new(
            AppConfig::from_env().expect("test config must load"),
        ),
        db: None,
    }
}

#[tokio::test]
async fn health_live_returns_ok() {
    let app = build_router(test_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn system_info_returns_metadata() {
    let app = build_router(test_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/system/info")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
'''

CI_WORKFLOW = '''name: Main CI

# IAOP CI/CD policy: workflows run ONLY on pushes to main.
# No pull_request / pull_request_target / schedule / tag triggers.
# Every job is additionally guarded by a ref check.

on:
  push:
    branches:
      - main

permissions:
  contents: read

concurrency:
  group: main-ci-${{ github.repository }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  rust:
    name: Rust quality gates
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install pinned Rust toolchain
        run: rustup show

      - name: Rust cache
        uses: Swatinem/rust-cache@v2
        with:
          workspaces: . -> target

      - name: Format check
        run: cargo fmt --all -- --check

      - name: Clippy
        run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

      - name: Unit + integration tests
        run: cargo test --workspace --locked

      - name: Build release binary
        run: cargo build --release --locked

      - name: Dependency audit
        run: |
          cargo install cargo-audit --locked --quiet || true
          cargo audit || echo "cargo-audit unavailable or advisories found - review output"

  container:
    name: Container build verification
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build image (no push)
        run: docker build -t {binary}:ci .
'''

VERIFY_PS1 = '''<#
.SYNOPSIS
    Local verification for {title} - mirrors CI quality gates.
    Run from the repository root on Windows PowerShell.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$failed = $false

function Step($name, $script) {
    Write-Host "`n=== $name ===" -ForegroundColor Cyan
    & $script
    if ($LASTEXITCODE -ne 0) { $script:failed = $true; Write-Warning "$name FAILED" }
    else { Write-Host "$name OK" -ForegroundColor Green }
}

Step 'cargo fmt --check' { cargo fmt --all -- --check }
Step 'cargo clippy'      { cargo clippy --workspace --all-targets --all-features --locked -- -D warnings }
Step 'cargo test'        { cargo test --workspace --locked }
Step 'cargo build'       { cargo build --workspace --locked }

if ($failed) { exit 1 }
Write-Host "`nAll checks passed." -ForegroundColor Green
'''

DOC_ARCH = '''# {title} - Architecture

Hexagonal / clean architecture layout:

```text
crates/
  domain/          pure domain model - no infra deps
  application/     use cases + ports
  infrastructure/  adapters: config, postgres, messaging, security
  api/             axum inbound adapter: routes, dto, middleware, errors
src/main.rs        composition root
```

## Dependency rule

```
api            -> application, infrastructure, domain
infrastructure -> application, domain
application    -> domain
domain         -> (nothing)
```

The domain crate forbids unsafe code and has zero infrastructure dependencies.

## Status

Scaffold only - no business use cases implemented yet.
'''

DOC_DB = '''# {title} - Database

- Engine: PostgreSQL (database-per-service; this service owns its DB exclusively).
- Migrations: SQLx, timestamp-prefixed files in `migrations/`, applied at startup
  via `sqlx::migrate!` when `DATABASE_URL` is configured.
- Templates included: `outbox_events` (transactional outbox) and
  `inbox_events` (consumer deduplication).

## Conventions

- `UUID` primary keys, `TIMESTAMPTZ` instants, `NUMERIC` money, `JSONB` only when justified.
- Expand-and-contract schema evolution; no destructive auto-migrations in production.
- Production migration runs require explicit authorization - never automatic on deploy.
'''

DOC_API = '''# {title} - API

Base path: `/api/v1`. OpenAPI 3.1 document: `GET /api/docs/openapi.json`.

## Baseline endpoints

| Method | Path                  | Purpose            |
|--------|-----------------------|--------------------|
| GET    | /health/live          | Liveness probe     |
| GET    | /health/ready         | Readiness probe    |
| GET    | /api/v1/system/info   | Service metadata   |

## Standards

- RFC 3339 UTC timestamps, UUID identifiers, typed pagination, RFC 9457
  problem-details errors (`type`, `title`, `status`, `detail`, `code`,
  `correlationId`).
- Contracts governed by `iaop-api-contracts`.
'''

# ---------------------------------------------------------------- driver

def scaffold(root, svc, title, db, binary=None):
    binary = binary or svc
    svc_us = svc.replace('-', '_')
    print(f'== {svc} -> {root}')

    w(root, 'rust-toolchain.toml', rust_toolchain())
    w(root, 'Cargo.toml', workspace_toml(svc_us, binary))

    # domain
    w(root, f'crates/domain/Cargo.toml', crate_toml(f'{svc_us}-domain', binary,
        extra_deps='''serde.workspace = true
uuid.workspace = true
thiserror.workspace = true
'''))
    w(root, 'crates/domain/src/lib.rs', DOMAIN_LIB.replace('{title}', title))
    w(root, 'crates/domain/src/entity_id.rs', ENTITY_ID)
    w(root, 'crates/domain/src/error.rs', DOMAIN_ERROR)

    # application
    w(root, 'crates/application/Cargo.toml', crate_toml(f'{svc_us}-application', binary,
        extra_deps=f'''{svc_us}-domain.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
'''))
    w(root, 'crates/application/src/lib.rs', APPLICATION_LIB.replace('{title}', title))
    w(root, 'crates/application/src/service_info.rs', SERVICE_INFO)
    w(root, 'crates/application/src/ports.rs', PORTS.replace('{svc_us}', svc_us))

    # infrastructure
    w(root, 'crates/infrastructure/Cargo.toml', crate_toml(f'{svc_us}-infrastructure', binary,
        extra_deps=f'''{svc_us}-application.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true
tokio.workspace = true
sqlx.workspace = true
'''))
    w(root, 'crates/infrastructure/src/lib.rs', INFRA_LIB.replace('{title}', title))
    w(root, 'crates/infrastructure/src/config.rs', CONFIG_RS.replace('{binary}', binary).replace('{svc_us}', svc_us))
    w(root, 'crates/infrastructure/src/persistence.rs', PERSISTENCE_RS)
    w(root, 'crates/infrastructure/src/messaging.rs', MESSAGING_RS.replace('{svc_us}', svc_us))
    w(root, 'crates/infrastructure/src/security.rs', SECURITY_RS)

    # api
    w(root, 'crates/api/Cargo.toml', crate_toml(f'{svc_us}-api', binary,
        extra_deps=f'''{svc_us}-application.workspace = true
{svc_us}-infrastructure.workspace = true
{svc_us}-domain.workspace = true
serde.workspace = true
serde_json.workspace = true
uuid.workspace = true
chrono.workspace = true
tracing.workspace = true
tokio.workspace = true
sqlx.workspace = true
axum.workspace = true
utoipa.workspace = true
tower.workspace = true
'''))
    w(root, 'crates/api/src/lib.rs', API_LIB.replace('{title}', title))
    w(root, 'crates/api/src/state.rs', API_STATE.replace('{svc_us}', svc_us))
    w(root, 'crates/api/src/dto.rs', API_DTO)
    w(root, 'crates/api/src/error_mapping.rs', API_ERRORS)
    w(root, 'crates/api/src/middleware.rs', API_MIDDLEWARE)
    w(root, 'crates/api/src/routes.rs', API_ROUTES.replace('{svc_us}', svc_us))
    w(root, 'crates/api/src/openapi.rs', API_OPENAPI.replace('{title}', title))

    # integration test target (tests/<name>/main.rs is auto-discovered)
    w(root, 'tests/integration/main.rs', INTEGRATION_TEST.replace('{svc_us}', svc_us))

    # root binary
    w(root, 'src/main.rs', MAIN_RS.replace('{title}', title).replace('{svc_us}', svc_us))

    # migrations
    w(root, 'migrations/20261113000001_create_outbox_events.sql', MIGRATION_OUTBOX)
    w(root, 'migrations/20261113000002_create_inbox_events.sql', MIGRATION_INBOX)

    # env / docker / ci / verify / docs
    w(root, '.env.example', ENV_EXAMPLE.replace('{binary}', binary).replace('{db}', db).replace('{svc_us}', svc_us))
    w(root, 'Dockerfile', DOCKERFILE.replace('{ver}', RUST_VER).replace('{binary}', binary))
    w(root, '.dockerignore', DOCKERIGNORE)
    w(root, '.gitignore', GITIGNORE)
    w(root, '.github/workflows/main-ci.yml', CI_WORKFLOW.replace('{binary}', binary))
    w(root, 'tools/verify-local.ps1', VERIFY_PS1.replace('{title}', title))
    w(root, 'docs/architecture.md', DOC_ARCH.replace('{title}', title))
    w(root, 'docs/database.md', DOC_DB.replace('{title}', title))
    w(root, 'docs/api.md', DOC_API.replace('{title}', title))

if __name__ == '__main__':
    root, svc, title, db = sys.argv[1:5]
    scaffold(root, svc, title, db)
