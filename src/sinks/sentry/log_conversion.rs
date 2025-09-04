//! Vector log event to Sentry log conversion utilities.

use std::time::SystemTime;
use sentry::protocol::{Log, LogAttribute, LogLevel, Map, TraceId, Value};

/// Extract trace ID from log event, returning the trace ID and which field was used.
pub fn extract_trace_id(log: &vector_lib::event::LogEvent) -> (TraceId, Option<&'static str>) {
    let trace_fields = ["trace_id", "sentry.trace_id"];
    for field_name in &trace_fields {
        if let Some(trace_value) = log.get(*field_name) {
            let trace_str = trace_value.to_string_lossy();
            if let Ok(uuid) = uuid::Uuid::parse_str(&trace_str) {
                // Convert UUID to bytes and then to TraceId
                return (TraceId::from(uuid.into_bytes()), Some(*field_name));
            }
        }
    }

    // Create a zero'd out trace ID (16 bytes of zeros for UUID). This is special cased
    // during sentry ingestion.
    let default_trace_id: TraceId = TraceId::from([0u8; 16]);

    (default_trace_id, None)
}

/// Convert a Vector log event to a Sentry log.
pub fn convert_to_sentry_log(log: &vector_lib::event::LogEvent) -> Log {
    // Extract timestamp
    let timestamp = log
        .get_timestamp()
        .and_then(|ts| ts.as_timestamp())
        .map(|ts| (*ts).into())
        .unwrap_or_else(SystemTime::now);

    // Extract message
    let body = log
        .get_message()
        .map(|msg| msg.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Extract level
    let level = log
        .get("level")
        .or_else(|| log.get("severity"))
        .or_else(|| log.get("sentry.level"))
        .or_else(|| log.get("sentry.severity"))
        .map(
            |level_value| match level_value.to_string_lossy().to_lowercase().as_str() {
                "trace" => LogLevel::Trace,
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" | "warning" => LogLevel::Warn,
                "error" | "err" => LogLevel::Error,
                "fatal" | "critical" | "alert" | "emergency" => LogLevel::Fatal,
                _ => LogLevel::Info,
            },
        )
        .unwrap_or(LogLevel::Info);

    // Extract trace ID and determine which field was used
    let (trace_id, used_trace_field) = extract_trace_id(log);

    // Convert fields to attributes
    let attributes = convert_fields_to_attributes(log, used_trace_field);

    Log {
        level,
        body,
        trace_id: Some(trace_id),
        timestamp,
        severity_number: None, // We could map this from level if needed
        attributes,
    }
}

/// Convert log event fields to Sentry log attributes, excluding specified fields.
///
/// See https://develop.sentry.dev/sdk/telemetry/logs/#log-envelope-item
pub fn convert_fields_to_attributes(
    log: &vector_lib::event::LogEvent,
    used_trace_field: Option<&str>,
) -> Map<String, LogAttribute> {
    let mut attributes = Map::new();
    if let Some(fields) = log.all_event_fields() {
        for (key, value) in fields {
            let key_str = key.as_str();
            if key_str != "message"
                && key_str != "level"
                && key_str != "severity"
                && key_str != "timestamp"
                && Some(key_str) != used_trace_field
            {
                let sentry_value = match value {
                    vrl::value::Value::Bytes(b) => {
                        Value::String(String::from_utf8_lossy(b).to_string())
                    }
                    vrl::value::Value::Integer(i) => Value::Number(serde_json::Number::from(*i)),
                    vrl::value::Value::Float(f) => {
                        // Ensure we're using 64-bit floating point as per Sentry protocol
                        let float_val = f.into_inner();
                        if let Some(n) = serde_json::Number::from_f64(float_val) {
                            Value::Number(n)
                        } else {
                            // If the float can't be represented as a JSON number, convert to string
                            Value::String(float_val.to_string())
                        }
                    }
                    vrl::value::Value::Boolean(b) => Value::Bool(*b),
                    _ => Value::String(value.to_string_lossy().to_string()),
                };
                attributes.insert(key_str.to_string(), LogAttribute(sentry_value));
            }
        }
    }
    attributes
}