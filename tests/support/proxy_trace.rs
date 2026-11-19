use tempfile::NamedTempFile;

// One capture per integration-test binary. Other gateway tasks can register
// the same tracing callsite first, so use the process-wide subscriber.
pub fn capture() -> NamedTempFile {
    let trace = NamedTempFile::new().unwrap();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("ai_gateway::application::proxy=debug")
        .with_writer(trace.reopen().unwrap())
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    trace
}
