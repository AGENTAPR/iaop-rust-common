#![forbid(unsafe_code)]
//! Tracing initialization and W3C trace-context propagation helpers.
//!
//! OpenTelemetry SDK wiring (OTLP exporter) is added here once the
//! collector endpoint is provisioned; the API surface stays stable.

use tracing_subscriber::EnvFilter;

/// Initializes the global tracing subscriber.
///
/// - `RUST_LOG` env var controls filtering (default: `info`).
/// - JSON output when `LOG_FORMAT=json`, otherwise human-readable.
/// - Never log tokens, secrets, or unredacted PII.
pub fn init(service_name: &str, default_filter: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.into());
    let json = std::env::var("LOG_FORMAT")
        .map(|v| v == "json")
        .unwrap_or(false);

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(false);

    if json {
        builder.json().with_current_span(true).init();
    } else {
        builder.init();
    }

    tracing::info!(service = service_name, "tracing initialized");
}
