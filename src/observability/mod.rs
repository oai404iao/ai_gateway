//! Tracing setup and, later, metrics and request-log emission.

use tracing_subscriber::EnvFilter;

pub fn init(filter: &str) {
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}
