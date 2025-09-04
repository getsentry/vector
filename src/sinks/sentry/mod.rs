//! The Sentry [`vector_lib::sink::VectorSink`].
//!
//! This module contains the [`vector_lib::sink::VectorSink`] instance that is responsible for
//! taking a stream of [`vector_lib::event::Event`]s and forwarding them to Sentry via HTTP.

mod config;
mod dsn;
mod encoder;
mod envelope;
mod log_conversion;
mod request_builder;
mod service;
mod sink;

pub use config::SentryConfig;

#[cfg(test)]
mod tests;