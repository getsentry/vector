//! The Sentry [`vector_lib::sink::VectorSink`].
//!
//! This module contains the [`vector_lib::sink::VectorSink`] instance that is responsible for
//! taking a stream of [`vector_lib::event::Event`]s and forwarding them to Sentry.

use std::time::SystemTime;

use futures::FutureExt;
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

impl Default for SentryConfig {
    fn default() -> Self {
        Self {
            dsn: SensitiveString::from("https://key@sentry.io/project_id".to_string()),
            batch: BatchConfig::default(),
            encoding: Transformer::default(),
            acknowledgements: AcknowledgementsConfig::default(),
        }
    }
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
        let _guard = sentry::init(self.dsn.inner());

        input
            .batched(self.batch_settings.as_byte_size_config())
            .for_each(|events| async {
                let transformer = self.transformer.clone();

                for mut event in events {
                    transformer.transform(&mut event);

                    if let Event::Log(log) = event {
                        process_log_event(&log);
                    }
                }
            })
            .await;

        Ok(())
    }
}

#[async_trait::async_trait]
impl StreamSink<Event> for SentrySink {
    async fn run(self: Box<Self>, input: BoxStream<'_, Event>) -> Result<(), ()> {
        self.run_inner(input).await
    }
}

/// Process a log event and send it to Sentry using the appropriate method.
fn process_log_event(log: &vector_lib::event::LogEvent) {
    let message = log
        .get_message()
        .map(|msg| msg.to_string_lossy().into_owned())
        .unwrap_or_else(|| "".to_string());

    // Extract level and convert to Sentry level
    let level = log
        .get("level")
        .or_else(|| log.get("severity"))
        .map(
            |level_value| match level_value.to_string_lossy().to_lowercase().as_str() {
                "debug" => sentry::Level::Debug,
                "info" => sentry::Level::Info,
                "warn" | "warning" => sentry::Level::Warning,
                "error" => sentry::Level::Error,
                "fatal" | "critical" => sentry::Level::Fatal,
                _ => sentry::Level::Info,
            },
        )
        .unwrap_or(sentry::Level::Info);

    // For error level and above, create a full event with extra data
    if matches!(level, sentry::Level::Error | sentry::Level::Fatal) {
        // Create an event with additional context
        sentry::with_scope(
            |scope| {
                // Add extra data from log fields
                if let Some(fields) = log.all_event_fields() {
                    for (key, value) in fields {
                        let key_str = key.as_str();
                        // Skip message, level, and timestamp as they're handled separately
                        if key_str != "message"
                            && key_str != "level"
                            && key_str != "severity"
                            && key_str != "timestamp"
                        {
                            scope.set_extra(key_str, value.to_string_lossy().into());
                        }
                    }
                }

                // Set the timestamp if available
                if let Some(timestamp) = log.get_timestamp() {
                    if let Some(ts) = timestamp.as_timestamp() {
                        scope.set_extra(
                            "log_timestamp",
                            SystemTime::from(*ts)
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_secs()
                                .to_string()
                                .into(),
                        );
                    }
                }
            },
            || {
                sentry::capture_message(&message, level);
            },
        );
    } else {
        // For non-error levels, just send as a simple message
        sentry::capture_message(&message, level);
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
    fn test_process_log_event() {
        let mut log = LogEvent::default();
        log.insert("message", "test message");
        log.insert("level", "error");
        log.insert("custom_field", "custom_value");

        // This should not panic
        process_log_event(&log);
    }

    #[test]
    fn test_level_conversion() {
        let test_cases = vec![
            ("debug", sentry::Level::Debug),
            ("info", sentry::Level::Info),
            ("warn", sentry::Level::Warning),
            ("warning", sentry::Level::Warning),
            ("error", sentry::Level::Error),
            ("fatal", sentry::Level::Fatal),
            ("critical", sentry::Level::Fatal),
            ("unknown", sentry::Level::Info),
        ];

        for (input, expected) in test_cases {
            let mut log = LogEvent::default();
            log.insert("level", input);

            let level = log
                .get("level")
                .map(
                    |level_value| match level_value.to_string_lossy().to_lowercase().as_str() {
                        "debug" => sentry::Level::Debug,
                        "info" => sentry::Level::Info,
                        "warn" | "warning" => sentry::Level::Warning,
                        "error" => sentry::Level::Error,
                        "fatal" | "critical" => sentry::Level::Fatal,
                        _ => sentry::Level::Info,
                    },
                )
                .unwrap_or(sentry::Level::Info);

            assert_eq!(level, expected, "Failed for level: {}", input);
        }
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

#[cfg(feature = "sentry-integration-tests")]
#[cfg(test)]
mod integration_tests {
    use futures::stream;
    use std::env;
    use vector_lib::event::{BatchNotifier, BatchStatus, Event, LogEvent};

    use super::*;
    use crate::{
        config::SinkContext,
        test_util::components::{run_and_assert_sink_compliance, HTTP_SINK_TAGS},
    };

    #[tokio::test]
    async fn sentry_logs_integration_test() {
        let dsn = env::var("SENTRY_DSN").expect("SENTRY_DSN environment variable to be set");
        assert!(!dsn.is_empty(), "$SENTRY_DSN required");

        let cx = SinkContext::default();

        let config = SentryConfig {
            dsn: dsn.into(),
            ..Default::default()
        };

        // create unique test id so tests can run in parallel
        let test_id = uuid::Uuid::new_v4().to_string();

        let (sink, _) = config.build(cx).await.unwrap();

        let (batch, mut receiver) = BatchNotifier::new_with_receiver();

        let mut event1 = LogEvent::from("Test message 1 from Vector integration test")
            .with_batch_notifier(&batch);
        event1.insert("level", "error");
        event1.insert("test_id", test_id.clone());
        event1.insert("source", "vector-integration-test");
        event1.insert("component", "sentry-sink");

        let mut event2 = LogEvent::from("Test message 2 from Vector integration test")
            .with_batch_notifier(&batch);
        event2.insert("level", "warning");
        event2.insert("test_id", test_id.clone());
        event2.insert("source", "vector-integration-test");
        event2.insert("component", "sentry-sink");

        drop(batch);

        let events = vec![Event::Log(event1), Event::Log(event2)];

        run_and_assert_sink_compliance(sink, stream::iter(events), &HTTP_SINK_TAGS).await;

        // Wait for successful delivery
        assert_eq!(receiver.try_recv(), Ok(BatchStatus::Delivered));
    }
}
