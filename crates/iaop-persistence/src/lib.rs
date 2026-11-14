#![forbid(unsafe_code)]
//! PostgreSQL connection pool, migration, and readiness helpers (sqlx).
//! No generic repository abstraction — services own their SQL.

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool, migrate::Migrator};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("invalid database url: {0}")]
    InvalidUrl(String),
    #[error("database connection failed: {0}")]
    Connect(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migration(String),
}

/// Pool/timing configuration for a PostgreSQL service.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_lifetime: Duration,
}

impl DbConfig {
    /// Builds pool options with sensible bounded defaults.
    pub fn pool(&self) -> PgPoolOptions {
        PgPoolOptions::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(self.acquire_timeout)
            .idle_timeout(self.idle_timeout)
            .max_lifetime(self.max_lifetime)
    }
}

impl Default for DbConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(600),
            max_lifetime: Duration::from_secs(1800),
        }
    }
}

/// Creates a pool, verifying connectivity with a bounded timeout.
pub async fn connect(cfg: &DbConfig) -> Result<PgPool, DbError> {
    let opts = PgConnectOptions::from_str(&cfg.url)
        .map_err(|e| DbError::InvalidUrl(e.to_string()))?
        .disable_statement_logging();
    let pool = cfg.pool().connect_with(opts).await?;
    Ok(pool)
}

/// Runs embedded migrations; `migrations` dir resolved by caller via Migrator.
pub async fn run_migrations(pool: &PgPool, migrator: &Migrator) -> Result<(), DbError> {
    migrator
        .run(pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))
}

/// Readiness probe: `SELECT 1` bounded by pool acquire timeout.
pub async fn ping(pool: &PgPool) -> Result<(), DbError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(pool)
        .await?;
    Ok(())
}

/// Classifies sqlx errors as transient (retryable) vs permanent.
pub fn is_transient(err: &sqlx::Error) -> bool {
    if matches!(
        err,
        sqlx::Error::PoolTimedOut
            | sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::WorkerCrashed
            | sqlx::Error::BeginFailed
    ) {
        return true;
    }
    if let Some(db) = err.as_database_error() {
        if let Some(code) = db.code() {
            return matches!(
                code.as_ref(),
                "57P01"
                    | "57P02"
                    | "57P03"
                    | "08000"
                    | "08001"
                    | "08003"
                    | "08004"
                    | "08006"
                    | "53300"
            );
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_url_is_rejected() {
        let cfg = DbConfig {
            url: "not-a-url".into(),
            ..Default::default()
        };
        // connect is async; validate parse path directly.
        assert!(PgConnectOptions::from_str(&cfg.url).is_err());
    }

    #[test]
    fn transient_classifier_covers_pool_timeout() {
        let e = sqlx::Error::PoolTimedOut;
        assert!(is_transient(&e));
    }
}
