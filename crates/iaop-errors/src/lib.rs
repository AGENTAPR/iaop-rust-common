#![forbid(unsafe_code)]
//! Generic technical error types and safe API error serialization.
//!
//! Framework-agnostic: HTTP layers map [`ProblemDetails`] onto their own
//! response types. No business semantics belong here.

use serde::Serialize;
use uuid::Uuid;

/// Stable machine-readable error code (e.g. `VALIDATION_ERROR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ErrorCode(pub &'static str);

impl ErrorCode {
    pub const VALIDATION: Self = Self("VALIDATION_ERROR");
    pub const NOT_FOUND: Self = Self("NOT_FOUND");
    pub const CONFLICT: Self = Self("CONFLICT");
    pub const UNAUTHORIZED: Self = Self("UNAUTHORIZED");
    pub const FORBIDDEN: Self = Self("FORBIDDEN");
    pub const RATE_LIMITED: Self = Self("RATE_LIMITED");
    pub const INTERNAL: Self = Self("INTERNAL_ERROR");
}

/// RFC 9457 Problem Details payload.
#[derive(Debug, Clone, Serialize)]
pub struct ProblemDetails {
    #[serde(rename = "type")]
    pub problem_type: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub code: ErrorCode,
    #[serde(rename = "correlationId", skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

impl ProblemDetails {
    /// Creates a problem detail for the given HTTP status.
    /// `detail` must already be safe for clients (no internals).
    pub fn new(
        status: u16,
        title: impl Into<String>,
        code: ErrorCode,
        detail: Option<String>,
    ) -> Self {
        Self {
            problem_type: format!(
                "https://api.iaop.internal/problems/{}",
                code.0.to_lowercase()
            ),
            title: title.into(),
            status,
            detail,
            code,
            correlation_id: None,
            instance: None,
        }
    }

    pub fn with_correlation(mut self, id: Uuid) -> Self {
        self.correlation_id = Some(id);
        self
    }

    pub fn with_instance(mut self, path: impl Into<String>) -> Self {
        self.instance = Some(path.into());
        self
    }
}
