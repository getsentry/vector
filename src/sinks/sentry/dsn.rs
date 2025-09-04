//! DSN parsing utilities for Sentry using the official Sentry SDK.

use sentry::Dsn;

/// Parse DSN and extract endpoint information for HTTP transport
pub fn parse_dsn_to_endpoint(dsn_str: &str) -> crate::Result<(String, String)> {
    let dsn = Dsn::from_str(dsn_str).map_err(|e| format!("Invalid Sentry DSN: {}", e))?;

    let endpoint = format!(
        "{}://{}/api/{}/envelope/",
        dsn.scheme(),
        dsn.host(),
        dsn.project_id()
    );

    let public_key = dsn.public_key().to_string();

    Ok((endpoint, public_key))
}
