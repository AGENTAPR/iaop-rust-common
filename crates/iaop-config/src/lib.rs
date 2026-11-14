#![forbid(unsafe_code)]
//! Typed configuration helpers: environment variables with validation,
//! safe defaults, and secret redaction.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}: {1}")]
    Invalid(&'static str, String),
}

/// Returns the env var or `default` when unset/empty.
pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// Returns the env var or fails with [`ConfigError::Missing`].
pub fn require_env(key: &'static str) -> Result<String, ConfigError> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or(ConfigError::Missing(key))
}

/// Parses an env var, falling back to `default` when unset.
pub fn env_parse<T: std::str::FromStr>(key: &'static str, default: T) -> Result<T, ConfigError> {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v.parse::<T>().map_err(|_| ConfigError::Invalid(key, v)),
        _ => Ok(default),
    }
}

/// Parses an env var, failing when unset or invalid.
pub fn require_parse<T: std::str::FromStr>(key: &'static str) -> Result<T, ConfigError> {
    let raw = require_env(key)?;
    raw.parse::<T>().map_err(|_| ConfigError::Invalid(key, raw))
}

/// Masks a secret for safe logging: shows only length and last 4 chars.
pub fn redact_secret(secret: &str) -> String {
    let len = secret.len();
    if len <= 4 {
        "****".to_string()
    } else {
        format!("****{}", &secret[len - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_or_uses_default() {
        assert_eq!(env_or("IAOP_DEFINITELY_UNSET_VAR", "fallback"), "fallback");
    }

    #[test]
    fn require_env_fails_when_missing() {
        assert!(matches!(
            require_env("IAOP_DEFINITELY_UNSET_VAR"),
            Err(ConfigError::Missing(_))
        ));
    }

    #[test]
    fn redact_hides_all_but_suffix() {
        assert_eq!(redact_secret("supersecretvalue"), "****alue");
        assert_eq!(redact_secret("ab"), "****");
    }
}
