//! The Sentry [`vector_lib::sink::VectorSink`].
//!
//! This module contains the [`vector_lib::sink::VectorSink`] instance that is responsible for
//! taking a stream of [`vector_lib::event::Event`]s and forwarding them to Sentry.

use std::time::SystemTime;

use futures::FutureExt;
use sentry::protocol::{Log, LogAttribute, LogLevel, Map, TraceId, Value};
use vector_lib::configurable::configurable_component;
use vector_lib::sensitive_string::SensitiveString;
use vrl::value::Kind;

use crate::sinks::prelude::*;

/// Configuration for the `sentry` sink.
#[configurable_component(sink("sentry", "Deliver log events to Sentry."))]
#[derive(Clone, Debug)]
pub struct SentryConfig {
    /// The Sentry DSN (Data Source Name) to send logs to.
    #[configurable(metadata(docs::examples = "${SENTRY_DSN}"))]
    #[configurable(metadata(docs::examples = "https://key@sentry.io/project_id"))]
    dsn: SensitiveString,

    #[configurable(derived)]
    #[serde(default)]
    batch: BatchConfig<SentryDefaultBatchSettings>,

    #[configurable(derived)]
    #[serde(default, skip_serializing_if = "crate::serde::is_default")]
    encoding: Transformer,

    #[configurable(derived)]
    #[serde(
        default,
        deserialize_with = "crate::serde::bool_or_struct",
        skip_serializing_if = "crate::serde::is_default"
    )]
    acknowledgements: AcknowledgementsConfig,
}

#[derive(Clone, Copy, Debug, Default)]
struct SentryDefaultBatchSettings;

impl SinkBatchSettings for SentryDefaultBatchSettings {
    const MAX_EVENTS: Option<usize> = Some(100);
    const MAX_BYTES: Option<usize> = None;
    const TIMEOUT_SECS: f64 = 1.0;
}

impl GenerateConfig for SentryConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(r#"dsn = "${SENTRY_DSN}""#).unwrap()
    }
}

#[async_trait::async_trait]
#[typetag::serde(name = "sentry")]
impl SinkConfig for SentryConfig {
    async fn build(&self, _cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let batch_settings = self.batch.validate()?.into_batcher_settings()?;

        let sink = SentrySink::new(self.dsn.clone(), self.encoding.clone(), batch_settings);

        let healthcheck = healthcheck(self.dsn.clone()).boxed();

        Ok((VectorSink::from_event_streamsink(sink), healthcheck))
    }

    fn input(&self) -> Input {
        let requirement = Requirement::empty().optional_meaning("timestamp", Kind::timestamp());

        Input::log().with_schema_requirement(requirement)
    }

    fn acknowledgements(&self) -> &AcknowledgementsConfig {
        &self.acknowledgements
    }
}

pub(super) struct SentrySink {
    dsn: SensitiveString,
    transformer: Transformer,
    batch_settings: BatcherSettings,
}

impl SentrySink {
    pub(super) const fn new(
        dsn: SensitiveString,
        transformer: Transformer,
        batch_settings: BatcherSettings,
    ) -> Self {
        Self {
            dsn,
            transformer,
            batch_settings,
        }
    }

    async fn run_inner(self: Box<Self>, input: BoxStream<'_, Event>) -> Result<(), ()> {
        // Initialize Sentry client
        let _guard = sentry::init((
            self.dsn.inner(),
            sentry::ClientOptions {
                enable_logs: true,
                ..Default::default()
            },
        ));

        input
            .batched(self.batch_settings.as_byte_size_config())
            .for_each(|events| async {
                let transformer = self.transformer.clone();

                for mut event in events {
                    transformer.transform(&mut event);

                    if let Event::Log(log) = event {
                        let sentry_log = convert_to_sentry_log(&log);
                        // Use the current hub to capture the log
                        let hub = sentry::Hub::current();
                        hub.capture_log(sentry_log);
                    }
                }
            })
            .await;

        // Use the current hub to flush
        let hub = sentry::Hub::current();
        hub.client().map(|client| {
            client.close(Some(std::time::Duration::from_secs(5)));
        });

        Ok(())
    }
}

#[async_trait::async_trait]
impl StreamSink<Event> for SentrySink {
    async fn run(self: Box<Self>, input: BoxStream<'_, Event>) -> Result<(), ()> {
        self.run_inner(input).await
    }
}

/// Extract trace ID from log event, returning the trace ID and which field was used.
fn extract_trace_id(log: &vector_lib::event::LogEvent) -> (TraceId, Option<&'static str>) {
    if let Some(trace_value) = log.get("trace_id") {
        let trace_str = trace_value.to_string_lossy();
        if let Ok(uuid) = uuid::Uuid::parse_str(&trace_str) {
            // Convert UUID to bytes and then to TraceId
            (TraceId::from(uuid.into_bytes()), Some("trace_id"))
        } else {
            (TraceId::default(), None)
        }
    } else if let Some(trace_value) = log.get("sentry.trace_id") {
        let trace_str = trace_value.to_string_lossy();
        if let Ok(uuid) = uuid::Uuid::parse_str(&trace_str) {
            (TraceId::from(uuid.into_bytes()), Some("sentry.trace_id"))
        } else {
            (TraceId::default(), None)
        }
    } else {
        // Fall back to default trace ID if no trace_id fields found
        (TraceId::default(), None)
    }
}

/// Convert log event fields to Sentry log attributes, excluding specified fields.
fn convert_fields_to_attributes(
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
                        LogAttribute::String(String::from_utf8_lossy(b).to_string())
                    }
                    vrl::value::Value::Integer(i) => {
                        LogAttribute::Number(serde_json::Number::from(*i))
                    }
                    vrl::value::Value::Float(f) => {
                        // Ensure we're using 64-bit floating point as per Sentry protocol
                        let float_val = f.into_inner();
                        if let Some(n) = serde_json::Number::from_f64(float_val) {
                            LogAttribute::Number(n)
                        } else {
                            // If the float can't be represented as a JSON number, convert to string
                            LogAttribute::String(float_val.to_string())
                        }
                    }
                    vrl::value::Value::Boolean(b) => LogAttribute::Bool(*b),
                    _ => LogAttribute::String(value.to_string_lossy().to_string()),
                };
                attributes.insert(key_str.to_string(), sentry_value);
            }
        }
    }
    attributes
}

/// Convert a Vector log event to a Sentry log.
fn convert_to_sentry_log(log: &vector_lib::event::LogEvent) -> Log {
    use chrono::{DateTime, Utc};

    // Extract timestamp
    let timestamp = log
        .get_timestamp()
        .and_then(|ts| ts.as_timestamp())
        .map(|ts| DateTime::<Utc>::from(*ts).into())
        .unwrap_or_else(SystemTime::now);

    // Extract message
    let body = log
        .get_message()
        .map(|msg| msg.to_string_lossy().into_owned())
        .unwrap_or_else(|| "".to_string());

    // Extract level
    let level = log
        .get("level")
        .or_else(|| log.get("severity"))
        .map(
            |level_value| match level_value.to_string_lossy().to_lowercase().as_str() {
                "debug" => LogLevel::Debug,
                "info" => LogLevel::Info,
                "warn" | "warning" => LogLevel::Warn,
                "error" => LogLevel::Error,
                "fatal" | "critical" => LogLevel::Fatal,
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

async fn healthcheck(dsn: SensitiveString) -> crate::Result<()> {
    // Simple healthcheck - just verify we can initialize Sentry
    let _guard = sentry::init(dsn.inner());

    // If we get here without panicking, initialization succeeded
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroUsize;
    use std::time::Duration;
    use vector_lib::event::LogEvent;

    #[test]
    fn test_generate_config() {
        let config = SentryConfig::generate_config();
        assert!(config.get("dsn").is_some());
    }

    #[test]
    fn test_convert_to_sentry_log() {
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("level", "error");
        log.insert("custom_field", "custom_value");

        let sentry_log = convert_to_sentry_log(&log);

        assert_eq!(sentry_log.body, "test message");
        assert_eq!(sentry_log.level, LogLevel::Error);
        assert_eq!(
            sentry_log.attributes.get("custom_field"),
            Some(&Value::String("custom_value".to_string()))
        );
    }

    #[test]
    fn test_level_conversion() {
        let test_cases = vec![
            ("debug", LogLevel::Debug),
            ("info", LogLevel::Info),
            ("warn", LogLevel::Warn),
            ("warning", LogLevel::Warn),
            ("error", LogLevel::Error),
            ("fatal", LogLevel::Fatal),
            ("critical", LogLevel::Fatal),
            ("unknown", LogLevel::Info),
        ];

        for (input, expected) in test_cases {
            let mut log = LogEvent::default();
            log.insert("level", input);

            let sentry_log = convert_to_sentry_log(&log);
            assert_eq!(sentry_log.level, expected, "Failed for level: {}", input);
        }
    }

    #[test]
    fn test_trace_id_extraction() {
        // Test with valid trace_id field
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("trace_id", "550e8400-e29b-41d4-a716-446655440000");

        let sentry_log = convert_to_sentry_log(&log);
        assert!(sentry_log.trace_id.is_some());

        // Test with valid sentry.trace_id field
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("sentry.trace_id", "550e8400-e29b-41d4-a716-446655440000");

        let sentry_log = convert_to_sentry_log(&log);
        assert!(sentry_log.trace_id.is_some());

        // Test with invalid trace_id (should fall back to default trace ID and keep in attributes)
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("trace_id", "invalid-uuid");

        let sentry_log = convert_to_sentry_log(&log);
        assert!(sentry_log.trace_id.is_some());
        // Should be the default UUID
        assert_eq!(sentry_log.trace_id, Some(TraceId::default()));
        // Invalid trace_id should be preserved in attributes
        assert!(sentry_log.attributes.contains_key("trace_id"));
        assert_eq!(
            sentry_log.attributes.get("trace_id"),
            Some(&Value::String("invalid-uuid".to_string()))
        );

        // Test with invalid sentry.trace_id (should fall back to default trace ID and keep in attributes)
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("sentry.trace_id", "invalid-sentry-uuid");

        let sentry_log = convert_to_sentry_log(&log);
        assert!(sentry_log.trace_id.is_some());
        // Should be the default UUID
        assert_eq!(sentry_log.trace_id, Some(TraceId::default()));
        // Invalid sentry.trace_id should be preserved in attributes
        assert!(sentry_log.attributes.contains_key("sentry.trace_id"));
        assert_eq!(
            sentry_log.attributes.get("sentry.trace_id"),
            Some(&Value::String("invalid-sentry-uuid".to_string()))
        );

        // Test with no trace_id field (should use default trace ID)
        let mut log = LogEvent::default();
        log.insert("message", "test message");

        let sentry_log = convert_to_sentry_log(&log);
        assert!(sentry_log.trace_id.is_some());
        assert_eq!(sentry_log.trace_id, Some(TraceId::default()));
    }

    #[test]
    fn test_trace_id_attribute_handling() {
        // Test that valid trace_id is excluded from attributes but invalid ones are kept
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("trace_id", "550e8400-e29b-41d4-a716-446655440000"); // valid UUID
        log.insert("custom_field", "custom_value");

        let sentry_log = convert_to_sentry_log(&log);

        // Valid trace_id should be used and excluded from attributes
        assert!(!sentry_log.attributes.contains_key("trace_id"));
        assert!(sentry_log.attributes.contains_key("custom_field"));

        // Test with sentry.trace_id preference (trace_id takes priority)
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("trace_id", "550e8400-e29b-41d4-a716-446655440000"); // valid UUID
        log.insert("sentry.trace_id", "other-trace-id"); // invalid, should be kept
        log.insert("custom_field", "custom_value");

        let sentry_log = convert_to_sentry_log(&log);

        // trace_id should be used (takes priority) and excluded from attributes
        assert!(!sentry_log.attributes.contains_key("trace_id"));
        // sentry.trace_id should be kept since it wasn't used
        assert!(sentry_log.attributes.contains_key("sentry.trace_id"));
        assert!(sentry_log.attributes.contains_key("custom_field"));

        // Test with only sentry.trace_id and it's valid
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("sentry.trace_id", "550e8400-e29b-41d4-a716-446655440000"); // valid UUID
        log.insert("custom_field", "custom_value");

        let sentry_log = convert_to_sentry_log(&log);

        // sentry.trace_id should be used and excluded from attributes
        assert!(!sentry_log.attributes.contains_key("sentry.trace_id"));
        assert!(sentry_log.attributes.contains_key("custom_field"));
    }

    #[tokio::test]
    async fn test_sentry_sink_creation() {
        let dsn = SensitiveString::from("https://key@sentry.io/project_id".to_string());
        let transformer = Transformer::default();
        let batch_settings = BatcherSettings::new(
            Duration::from_secs(1),
            NonZeroUsize::new(1024).unwrap(),
            NonZeroUsize::new(100).unwrap(),
        );

        let sink = SentrySink::new(dsn, transformer, batch_settings);

        // Just verify that the sink can be created
        assert!(std::mem::size_of_val(&sink) > 0);
    }
}
