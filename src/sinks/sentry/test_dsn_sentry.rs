#[cfg(test)]
mod tests {
    #[test]
    fn test_sentry_dsn_parsing() {
        // Test what DSN types are available from sentry crate
        let dsn_str = "https://public_key@sentry.io/project_id";

        // Check if we can use sentry's Dsn
        if let Ok(parsed) = dsn_str.parse::<sentry::Dsn>() {
            println!("Successfully parsed with sentry::Dsn");
            println!("Host: {}", parsed.host());
            println!("Project ID: {}", parsed.project_id());
            println!("Public key: {}", parsed.public_key());
        } else {
            println!("Failed to parse with sentry::Dsn");
        }
    }
}
