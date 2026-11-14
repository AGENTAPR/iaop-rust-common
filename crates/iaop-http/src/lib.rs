#![forbid(unsafe_code)]
//! Transport-level HTTP primitives: request IDs, pagination types,
//! and response metadata. Framework-agnostic (no axum dependency).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Correlation/request identifier propagated across services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Offset/page pagination request parameters.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PageRequest {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_size")]
    pub size: u32,
}

fn default_page() -> u32 {
    1
}
fn default_size() -> u32 {
    20
}

pub const MAX_PAGE_SIZE: u32 = 200;

impl PageRequest {
    /// Validates and clamps the request; returns SQL offset/limit.
    pub fn bounds(&self) -> Result<(u32, u32), String> {
        if self.page == 0 {
            return Err("page must be >= 1".into());
        }
        if self.size == 0 || self.size > MAX_PAGE_SIZE {
            return Err(format!("size must be between 1 and {MAX_PAGE_SIZE}"));
        }
        let offset = self.page.saturating_sub(1).saturating_mul(self.size);
        Ok((offset, self.size))
    }
}

/// Paginated response envelope.
#[derive(Debug, Serialize)]
pub struct Page<T: Serialize> {
    pub items: Vec<T>,
    pub page: u32,
    pub size: u32,
    pub total: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_bounds_compute_offset() {
        let p = PageRequest { page: 3, size: 10 };
        assert_eq!(p.bounds().unwrap(), (20, 10));
    }

    #[test]
    fn page_rejects_oversize() {
        let p = PageRequest {
            page: 1,
            size: MAX_PAGE_SIZE + 1,
        };
        assert!(p.bounds().is_err());
    }
}
