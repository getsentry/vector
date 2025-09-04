//! HTTP service for sending Sentry requests.

use bytes::Bytes;
use http::{Request, Uri};
use snafu::ResultExt;

use crate::sinks::{
    HTTPRequestBuilderSnafu,
    util::http::{HttpRequest, HttpServiceRequestBuilder},
};

use super::dsn::parse_dsn_to_endpoint;

#[derive(Debug, Clone)]
pub struct SentryServiceRequestBuilder {
    pub uri: Uri,
    pub public_key: String,
}

impl SentryServiceRequestBuilder {
    pub fn new(dsn: &str) -> crate::Result<Self> {
        let (endpoint, public_key) = parse_dsn_to_endpoint(dsn)?;
        let uri = endpoint.parse::<Uri>()
            .map_err(|e| format!("Invalid Sentry endpoint URI: {}", e))?;
        
        Ok(Self {
            uri,
            public_key,
        })
    }
}

impl HttpServiceRequestBuilder<()> for SentryServiceRequestBuilder {
    fn build(&self, mut request: HttpRequest<()>) -> Result<Request<Bytes>, crate::Error> {
        let builder = Request::post(&self.uri)
            .header("Content-Type", "application/x-sentry-envelope")
            .header("X-Sentry-Auth", format!(
                "Sentry sentry_version=7, sentry_client=sentry.vector.sink/0.1.0, sentry_key={}",
                self.public_key
            ))
            .header("User-Agent", "sentry.vector.sink/0.1.0");

        builder
            .body(request.take_payload())
            .context(HTTPRequestBuilderSnafu)
            .map_err(Into::into)
    }
}