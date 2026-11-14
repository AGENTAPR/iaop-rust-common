#![deny(unsafe_code)]
//! Reusable test utilities: env guards, id builders, HTTP test helpers.
//! No business fixtures — only technical/generic helpers.

use std::sync::{Mutex, OnceLock};

/// Serializes env-mutating tests within a process.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub struct EnvGuard {
    _permit: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    /// Acquires the env lock and snapshots listed vars.
    pub fn acquire(keys: &[&str]) -> Self {
        let lock = ENV_LOCK.get_or_init(|| Mutex::new(()));
        let permit = lock.lock().unwrap();
        let saved = keys
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        Self {
            _permit: permit,
            saved,
        }
    }

    #[allow(unsafe_code)]
    pub fn set(&self, key: &str, value: &str) {
        // SAFETY: tests using EnvGuard are serialized by the internal mutex,
        // so no concurrent env mutation races within this process.
        unsafe { std::env::set_var(key, value) };
    }

    #[allow(unsafe_code)]
    pub fn remove(&self, key: &str) {
        // SAFETY: same serialization guarantee as `set`.
        unsafe { std::env::remove_var(key) };
    }
}

#[allow(unsafe_code)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(val) => unsafe { std::env::set_var(k, val) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

/// Deterministic UUID builders for tests.
pub struct TestIds;

impl TestIds {
    pub fn uuid() -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }
    pub fn fixed(seed: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(seed)
    }
    pub fn agent_like() -> uuid::Uuid {
        Self::fixed(0xA6E7_0000_0000_0000_0000_0000_0000_0001)
    }
}

/// Extracts a JSON body from an axum `Response` in tests.
pub async fn body_json<T: serde::de::DeserializeOwned>(resp: axum::response::Response) -> T {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body");
    serde_json::from_slice(&bytes).expect("parse json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_guard_restores() {
        let g = EnvGuard::acquire(&["IAOP_TEST_TMP"]);
        g.set("IAOP_TEST_TMP", "x");
        assert_eq!(std::env::var("IAOP_TEST_TMP").unwrap(), "x");
        drop(g);
        assert!(std::env::var("IAOP_TEST_TMP").is_err());
    }

    #[test]
    fn fixed_ids_deterministic() {
        assert_eq!(TestIds::fixed(7), TestIds::fixed(7));
        assert_ne!(TestIds::fixed(7), TestIds::fixed(8));
    }
}
