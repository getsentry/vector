//! HTTP request builder for Sentry envelopes.

use bytes::Bytes;
use std::io;

use crate::{
    codecs::Transformer,
    sinks::{prelude::*, util::http::HttpRequest},
};

use super::encoder::SentryEncoder;

/// HTTP request builder for Sentry envelopes
#[derive(Clone)]
pub struct SentryRequestBuilder {
    pub encoder: SentryEncoder,
    pub compression: Compression,
}

impl SentryRequestBuilder {
    pub fn new(dsn: String, transformer: Transformer) -> Self {
        Self {
            encoder: SentryEncoder::new(transformer, dsn),
            compression: Compression::None,
        }
    }
}

impl RequestBuilder<Vec<Event>> for SentryRequestBuilder {
    type Metadata = EventFinalizers;
    type Events = Vec<Event>;
    type Encoder = SentryEncoder;
    type Payload = Bytes;
    type Request = HttpRequest<()>;
    type Error = io::Error;

    fn compression(&self) -> Compression {
        self.compression
    }

    fn encoder(&self) -> &Self::Encoder {
        &self.encoder
    }

    fn split_input(
        &self,
        mut events: Vec<Event>,
    ) -> (Self::Metadata, RequestMetadataBuilder, Self::Events) {
        let finalizers = events.take_finalizers();
        let builder = RequestMetadataBuilder::from_events(&events);
        (finalizers, builder, events)
    }

    fn build_request(
        &self,
        metadata: Self::Metadata,
        request_metadata: RequestMetadata,
        payload: EncodeResult<Self::Payload>,
    ) -> Self::Request {
        HttpRequest::new(payload.into_payload(), metadata, request_metadata, ())
    }
}