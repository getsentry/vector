//! Encoding for the `sentry` sink.

use bytes::Bytes;
use std::io;

use crate::{
    codecs::Transformer,
    sinks::{
        prelude::*,
        util::encoding::{Encoder as SinkEncoder, write_all},
    },
};

use super::{envelope::create_sentry_envelope, log_conversion::convert_to_sentry_log};

#[derive(Clone)]
pub struct SentryEncoder {
    pub transformer: Transformer,
    pub dsn: String,
}

impl SentryEncoder {
    pub fn new(transformer: Transformer, dsn: String) -> Self {
        Self { transformer, dsn }
    }
}

impl SinkEncoder<Vec<Event>> for SentryEncoder {
    fn encode_input(
        &self,
        events: Vec<Event>,
        writer: &mut dyn io::Write,
    ) -> io::Result<(usize, GroupedCountByteSize)> {
        let mut byte_size = telemetry().create_request_count_byte_size();
        let n_events = events.len();
        let mut logs = Vec::with_capacity(n_events);

        for mut event in events {
            self.transformer.transform(&mut event);
            byte_size.add_event(&event, event.estimated_json_encoded_size_of());

            if let Event::Log(log) = event {
                logs.push(convert_to_sentry_log(&log));
            }
        }

        // Create Sentry envelope
        let envelope_body = create_sentry_envelope(logs, &self.dsn)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let body = Bytes::from(envelope_body);

        write_all(writer, n_events, body.as_ref()).map(|()| (body.len(), byte_size))
    }
}
