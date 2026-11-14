#![forbid(unsafe_code)]
//! Durable idempotency-key infrastructure. Storage-agnostic core +
//! PostgreSQL adapter. Business semantics stay in the service.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum IdempotencyError {
    #[error("invalid idempotency key: {0}")]
    InvalidKey(String),
    #[error("idempotency key conflict: same key, different request fingerprint")]
    Conflict,
    #[error("storage error: {0}")]
    Storage(String),
}

/// Validates an `Idempotency-Key` header value: 1..=255 chars,
/// restricted to safe token characters.
pub fn validate_key(key: &str) -> Result<(), IdempotencyError> {
    if key.is_empty() || key.len() > 255 {
        return Err(IdempotencyError::InvalidKey("length 1..=255".into()));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(IdempotencyError::InvalidKey("unsafe characters".into()));
    }
    Ok(())
}

/// Deterministic fingerprint of a canonical request payload.
pub fn fingerprint(payload: &[u8]) -> [u8; 32] {
    Sha256::digest(payload).into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reservation {
    /// First sighting — caller proceeds and must complete/release.
    Acquired { record_id: Uuid },
    /// Same key + same fingerprint already completed — replay stored result.
    Replay { result: Vec<u8>, status: u16 },
    /// Key is in-flight by another request.
    InFlight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyRecord {
    pub record_id: Uuid,
    pub scope: String,
    pub key: String,
    pub fingerprint: Vec<u8>,
    pub status: String,
    pub response_status: Option<i32>,
    pub response_body: Option<Vec<u8>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Durable storage port.
pub trait IdempotencyStore {
    fn reserve(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        ttl: chrono::Duration,
    ) -> impl std::future::Future<Output = Result<Reservation, IdempotencyError>> + Send;
    fn complete(
        &self,
        record_id: Uuid,
        status: u16,
        body: &[u8],
    ) -> impl std::future::Future<Output = Result<(), IdempotencyError>> + Send;
    fn release(
        &self,
        record_id: Uuid,
    ) -> impl std::future::Future<Output = Result<(), IdempotencyError>> + Send;
}

/// PostgreSQL implementation. Table contract:
/// ```sql
/// CREATE TABLE idempotency_records (
///   record_id UUID PRIMARY KEY,
///   scope TEXT NOT NULL,
///   key TEXT NOT NULL,
///   fingerprint BYTEA NOT NULL,
///   status TEXT NOT NULL,          -- RESERVED|COMPLETED|FAILED
///   response_status INT,
///   response_body BYTEA,
///   created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
///   expires_at TIMESTAMPTZ NOT NULL,
///   UNIQUE (scope, key)
/// );
/// ```
pub struct PgIdempotencyStore {
    pool: PgPool,
}

impl PgIdempotencyStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IdempotencyStore for PgIdempotencyStore {
    async fn reserve(
        &self,
        scope: &str,
        key: &str,
        fingerprint: &[u8; 32],
        ttl: chrono::Duration,
    ) -> Result<Reservation, IdempotencyError> {
        validate_key(key)?;
        let record_id = Uuid::now_v7();
        let expires = chrono::Utc::now() + ttl;
        // Try insert; on conflict inspect existing row.
        let inserted = sqlx::query(
            "INSERT INTO idempotency_records (record_id, scope, key, fingerprint, status, created_at, expires_at)
             VALUES ($1,$2,$3,$4,'RESERVED', now(), $5)
             ON CONFLICT (scope, key) DO NOTHING",
        )
        .bind(record_id)
        .bind(scope)
        .bind(key)
        .bind(&fingerprint[..])
        .bind(expires)
        .execute(&self.pool)
        .await
        .map_err(|e| IdempotencyError::Storage(e.to_string()))?;

        if inserted.rows_affected() == 1 {
            return Ok(Reservation::Acquired { record_id });
        }

        // Existing row: compare fingerprint and status.
        let row = sqlx::query_as::<_, (Vec<u8>, String, Option<i32>, Option<Vec<u8>>)>(
            "SELECT fingerprint, status, response_status, response_body
             FROM idempotency_records WHERE scope=$1 AND key=$2",
        )
        .bind(scope)
        .bind(key)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| IdempotencyError::Storage(e.to_string()))?;

        if row.0 != fingerprint[..] {
            return Err(IdempotencyError::Conflict);
        }
        match row.1.as_str() {
            "COMPLETED" => Ok(Reservation::Replay {
                status: row.2.unwrap_or(200) as u16,
                result: row.3.unwrap_or_default(),
            }),
            _ => Ok(Reservation::InFlight),
        }
    }

    async fn complete(
        &self,
        record_id: Uuid,
        status: u16,
        body: &[u8],
    ) -> Result<(), IdempotencyError> {
        sqlx::query(
            "UPDATE idempotency_records SET status='COMPLETED', response_status=$2, response_body=$3 WHERE record_id=$1",
        )
        .bind(record_id)
        .bind(status as i32)
        .bind(body)
        .execute(&self.pool)
        .await
        .map_err(|e| IdempotencyError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn release(&self, record_id: Uuid) -> Result<(), IdempotencyError> {
        sqlx::query("DELETE FROM idempotency_records WHERE record_id=$1 AND status='RESERVED'")
            .bind(record_id)
            .execute(&self.pool)
            .await
            .map_err(|e| IdempotencyError::Storage(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation() {
        assert!(validate_key("abc-123_X.Y").is_ok());
        assert!(validate_key("").is_err());
        assert!(validate_key("bad key!").is_err());
        assert!(validate_key(&"x".repeat(256)).is_err());
    }

    #[test]
    fn fingerprint_is_deterministic() {
        assert_eq!(fingerprint(b"a"), fingerprint(b"a"));
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }
}
