#![forbid(unsafe_code)]
//! Integration event envelope and serialization helpers.
//!
//! Canonical schema: `iaop-event-contracts` `envelope.schema.json`.
//! Domain events must be translated to integration events at the
//! application boundary; internal domain objects are never published.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Standard integration-event envelope (v1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event_id: Uuid,
    pub event_type: String,
    pub event_version: u32,
    pub source: String,
    pub aggregate_id: Option<Uuid>,
    pub aggregate_version: Option<u64>,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub payload: serde_json::Value,
}

impl EventEnvelope {
    /// Creates an envelope; assigns `event_id` and `occurred_at`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_type: impl Into<String>,
        event_version: u32,
        source: impl Into<String>,
        aggregate_id: Option<Uuid>,
        aggregate_version: Option<u64>,
        correlation_id: Option<Uuid>,
        causation_id: Option<Uuid>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            event_id: Uuid::now_v7(),
            event_type: event_type.into(),
            event_version,
            source: source.into(),
            aggregate_id,
            aggregate_version,
            occurred_at: Utc::now(),
            correlation_id,
            causation_id,
            payload,
        }
    }

    /// Serializes to the canonical JSON wire format.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// Validates required envelope invariants before publishing.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.event_type.is_empty() {
            return Err("event_type must not be empty");
        }
        if self.event_version == 0 {
            return Err("event_version must be >= 1");
        }
        if self.source.is_empty() {
            return Err("source must not be empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_roundtrips() {
        let e = EventEnvelope::new(
            "agent.activated",
            1,
            "iaop-agent-service",
            None,
            None,
            None,
            None,
            json!({}),
        );
        let s = e.to_json().unwrap();
        let back: EventEnvelope = serde_json::from_str(&s).unwrap();
        assert_eq!(back.event_type, "agent.activated");
        assert!(back.validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_version() {
        let mut e = EventEnvelope::new("x.y", 0, "svc", None, None, None, None, json!({}));
        assert!(e.validate().is_err());
        e.event_version = 1;
        assert!(e.validate().is_ok());
    }
}
