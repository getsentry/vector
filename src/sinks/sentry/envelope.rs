//! Sentry envelope creation utilities.

use chrono;
use sentry::protocol::Log;
use serde_json;

use sentry::Dsn;
use std::str::FromStr;

/// Create a Sentry envelope containing log items
pub fn create_sentry_envelope(logs: Vec<Log>, dsn_str: &str) -> crate::Result<String> {
    let _dsn = Dsn::from_str(dsn_str).map_err(|e| format!("Invalid Sentry DSN: {}", e))?;

    // Create envelope header
    let envelope_header = serde_json::json!({
        "dsn": dsn_str,
        "sent_at": chrono::Utc::now().to_rfc3339(),
        "sdk": {
            "name": "sentry.vector.sink",
            "version": "0.1.0"
        }
    });

    // Create log item header
    let log_item_header = serde_json::json!({
        "type": "logs",
        "item_count": logs.len(),
        "content_type": "application/vnd.sentry.items.log+json"
    });

    // Create log item payload
    let log_item_payload = serde_json::json!({
        "items": logs
    });

    // Build envelope as newline-delimited format
    let envelope = format!(
        "{}\n{}\n{}\n",
        serde_json::to_string(&envelope_header)
            .map_err(|e| format!("Failed to serialize envelope header: {}", e))?,
        serde_json::to_string(&log_item_header)
            .map_err(|e| format!("Failed to serialize log item header: {}", e))?,
        serde_json::to_string(&log_item_payload)
            .map_err(|e| format!("Failed to serialize log item payload: {}", e))?
    );

    Ok(envelope)
}