use sentry;

fn main() {
    let dsn_str = "https://public_key@sentry.io/project_id";

    // Try to parse using sentry SDK
    match sentry::Dsn::from_str(dsn_str) {
        Ok(dsn) => {
            println!("Parsed DSN successfully");
            println!("Host: {:?}", dsn.host());
            println!("Project ID: {:?}", dsn.project_id());
            println!("Public Key: {:?}", dsn.public_key());
            println!("Scheme: {:?}", dsn.scheme());
        }
        Err(e) => {
            println!("Failed to parse DSN: {:?}", e);
        }
    }
}
