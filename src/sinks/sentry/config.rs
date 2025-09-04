//! Configuration for the `sentry` sink.

use futures::FutureExt;
use tower::ServiceBuilder;
use vector_lib::configurable::configurable_component;
use vector_lib::sensitive_string::SensitiveString;
use vrl::value::Kind;

use crate::{
    codecs::EncodingConfigWithFraming,
    http::HttpClient,
    sinks::{
        prelude::*,
        util::http::{HttpService, RequestConfig, http_response_retry_logic},
    },
};

use super::{
    request_builder::SentryRequestBuilder, service::SentryServiceRequestBuilder, sink::SentrySink,
};

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

    #[serde(flatten)]
    pub encoding: EncodingConfigWithFraming,

    #[configurable(derived)]
    #[serde(default)]
    pub request: RequestConfig,

    #[configurable(derived)]
    pub tls: Option<TlsConfig>,

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

impl SentryConfig {
    fn build_http_client(&self, cx: &SinkContext) -> crate::Result<HttpClient> {
        let tls = TlsSettings::from_options(self.tls.as_ref())?;
        Ok(HttpClient::new(tls, cx.proxy())?)
    }
}

impl GenerateConfig for SentryConfig {
    fn generate_config() -> toml::Value {
        toml::from_str(r#"dsn = "${SENTRY_DSN}""#).unwrap()
    }
}

async fn healthcheck(dsn: SensitiveString) -> crate::Result<()> {
    // Basic DSN validation - try to parse it
    use sentry::Dsn;
    use std::str::FromStr;
    
    Dsn::from_str(dsn.inner()).map_err(|e| format!("Invalid Sentry DSN: {}", e))?;
    Ok(())
}

#[async_trait::async_trait]
#[typetag::serde(name = "sentry")]
impl SinkConfig for SentryConfig {
    async fn build(&self, cx: SinkContext) -> crate::Result<(VectorSink, Healthcheck)> {
        let batch_settings = self.batch.validate()?.into_batcher_settings()?;
        let transformer = self.encoding.transformer();

        // Build request builder
        let request_builder = SentryRequestBuilder::new(self.dsn.inner().to_string(), transformer);

        // Build service request builder
        let service_request_builder = SentryServiceRequestBuilder::new(self.dsn.inner())?;

        // Build HTTP client
        let client = self.build_http_client(&cx)?;

        // Create HTTP service
        let request_limits = self.request.tower.into_settings();
        let service = ServiceBuilder::new()
            .settings(request_limits, http_response_retry_logic())
            .service(HttpService::new(client, service_request_builder));

        // Create sink
        let sentry_sink = SentrySink::new(service, batch_settings, request_builder);

        let boxed_healthcheck = healthcheck(self.dsn.clone()).boxed();

        Ok((
            VectorSink::from_event_streamsink(sentry_sink),
            boxed_healthcheck,
        ))
    }

    fn input(&self) -> Input {
        let requirement = Requirement::empty().optional_meaning("timestamp", Kind::timestamp());

        Input::log().with_schema_requirement(requirement)
    }

    fn acknowledgements(&self) -> &AcknowledgementsConfig {
        &self.acknowledgements
    }
}
