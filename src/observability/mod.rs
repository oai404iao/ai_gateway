//! Tracing setup and, later, metrics and request-log emission.

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

/// Initializes a lossy, nonblocking stderr writer. Keeping the returned guard
/// alive flushes queued records during process shutdown.
pub fn init(filter: &str) -> WorkerGuard {
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let (writer, guard) = tracing_appender::non_blocking(std::io::stderr());

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(writer)
        .try_init();
    guard
}
