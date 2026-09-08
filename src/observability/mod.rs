//! Tracing setup and low-overhead request-log pipeline metrics.

use std::sync::atomic::{AtomicU64, Ordering};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Initializes a lossy, nonblocking stderr writer. Keeping the returned guard
/// alive flushes queued records during process shutdown.
pub fn init(filter: &str) -> WorkerGuard {
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let (writer, guard) = tracing_appender::non_blocking(std::io::stderr());
    let layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(writer);

    // Keep the operator-supplied EnvFilter global. Composing it into the fmt
    // layer caused target-scoped application directives to miss some events
    // in the full runtime.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
    guard
}

#[derive(Default)]
pub(crate) struct RequestLogPipelineMetrics {
    recorded_total: AtomicU64,
    spooled_total: AtomicU64,
    spool_append_failures_total: AtomicU64,
    spool_bytes_total: AtomicU64,
    ingress_batches_total: AtomicU64,
    ingress_rows_total: AtomicU64,
    ingress_failures_total: AtomicU64,
    ingress_duration_micros_total: AtomicU64,
    ingress_duration_micros_max: AtomicU64,
    projected_rows_total: AtomicU64,
    projection_deferred_total: AtomicU64,
    projection_failures_total: AtomicU64,
    projection_duration_micros_total: AtomicU64,
    projection_duration_micros_max: AtomicU64,
    settled_rows_total: AtomicU64,
    settlement_failures_total: AtomicU64,
    settlement_duration_micros_total: AtomicU64,
    settlement_duration_micros_max: AtomicU64,
}

impl RequestLogPipelineMetrics {
    pub(crate) fn record_attempt(&self) {
        self.recorded_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_spooled(&self, bytes: u64) {
        self.spooled_total.fetch_add(1, Ordering::Relaxed);
        self.spool_bytes_total.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(crate) fn record_spool_append_failure(&self) {
        self.spool_append_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_ingress_batch(&self, rows: u64, duration_micros: u64) {
        self.ingress_batches_total.fetch_add(1, Ordering::Relaxed);
        self.ingress_rows_total.fetch_add(rows, Ordering::Relaxed);
        record_duration(
            &self.ingress_duration_micros_total,
            &self.ingress_duration_micros_max,
            duration_micros,
        );
    }

    pub(crate) fn record_ingress_failure(&self) {
        self.ingress_failures_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_projection(&self, projected: u64, deferred: u64, duration_micros: u64) {
        self.projected_rows_total
            .fetch_add(projected, Ordering::Relaxed);
        self.projection_deferred_total
            .fetch_add(deferred, Ordering::Relaxed);
        record_duration(
            &self.projection_duration_micros_total,
            &self.projection_duration_micros_max,
            duration_micros,
        );
    }

    pub(crate) fn record_projection_failure(&self) {
        self.projection_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_settlement(&self, settled: u64, duration_micros: u64) {
        self.settled_rows_total
            .fetch_add(settled, Ordering::Relaxed);
        record_duration(
            &self.settlement_duration_micros_total,
            &self.settlement_duration_micros_max,
            duration_micros,
        );
    }

    pub(crate) fn record_settlement_failure(&self) {
        self.settlement_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> RequestLogPipelineMetricsSnapshot {
        RequestLogPipelineMetricsSnapshot {
            recorded_total: self.recorded_total.load(Ordering::Relaxed),
            spooled_total: self.spooled_total.load(Ordering::Relaxed),
            spool_append_failures_total: self.spool_append_failures_total.load(Ordering::Relaxed),
            spool_bytes_total: self.spool_bytes_total.load(Ordering::Relaxed),
            ingress_batches_total: self.ingress_batches_total.load(Ordering::Relaxed),
            ingress_rows_total: self.ingress_rows_total.load(Ordering::Relaxed),
            ingress_failures_total: self.ingress_failures_total.load(Ordering::Relaxed),
            ingress_duration_micros_total: self
                .ingress_duration_micros_total
                .load(Ordering::Relaxed),
            ingress_duration_micros_max: self.ingress_duration_micros_max.load(Ordering::Relaxed),
            projected_rows_total: self.projected_rows_total.load(Ordering::Relaxed),
            projection_deferred_total: self.projection_deferred_total.load(Ordering::Relaxed),
            projection_failures_total: self.projection_failures_total.load(Ordering::Relaxed),
            projection_duration_micros_total: self
                .projection_duration_micros_total
                .load(Ordering::Relaxed),
            projection_duration_micros_max: self
                .projection_duration_micros_max
                .load(Ordering::Relaxed),
            settled_rows_total: self.settled_rows_total.load(Ordering::Relaxed),
            settlement_failures_total: self.settlement_failures_total.load(Ordering::Relaxed),
            settlement_duration_micros_total: self
                .settlement_duration_micros_total
                .load(Ordering::Relaxed),
            settlement_duration_micros_max: self
                .settlement_duration_micros_max
                .load(Ordering::Relaxed),
        }
    }
}

fn record_duration(total: &AtomicU64, maximum: &AtomicU64, duration_micros: u64) {
    total.fetch_add(duration_micros, Ordering::Relaxed);
    maximum.fetch_max(duration_micros, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RequestLogPipelineMetricsSnapshot {
    pub recorded_total: u64,
    pub spooled_total: u64,
    pub spool_append_failures_total: u64,
    pub spool_bytes_total: u64,
    pub ingress_batches_total: u64,
    pub ingress_rows_total: u64,
    pub ingress_failures_total: u64,
    pub ingress_duration_micros_total: u64,
    pub ingress_duration_micros_max: u64,
    pub projected_rows_total: u64,
    pub projection_deferred_total: u64,
    pub projection_failures_total: u64,
    pub projection_duration_micros_total: u64,
    pub projection_duration_micros_max: u64,
    pub settled_rows_total: u64,
    pub settlement_failures_total: u64,
    pub settlement_duration_micros_total: u64,
    pub settlement_duration_micros_max: u64,
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::{EnvFilter, layer::SubscriberExt};

    #[derive(Clone, Default)]
    struct BufferWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
    }

    impl BufferWriter {
        fn contents(&self) -> String {
            String::from_utf8(
                self.bytes
                    .lock()
                    .expect("test log buffer lock poisoned")
                    .clone(),
            )
            .expect("test log output is UTF-8")
        }
    }

    impl Write for BufferWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.bytes
                .lock()
                .expect("test log buffer lock poisoned")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for BufferWriter {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture(filter: &str, emit: impl FnOnce()) -> String {
        let writer = BufferWriter::default();
        let output = writer.clone();
        let layer = tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_writer(writer);
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new(filter))
            .with(layer);
        tracing::subscriber::with_default(subscriber, emit);
        output.contents()
    }

    #[test]
    fn target_scoped_filter_preserves_application_info() {
        let output = capture(
            "ai_gateway=info,ai_gateway::application::proxy=warn,tower_http=warn",
            || {
                tracing::info!(
                    target: "ai_gateway::request_log_metrics",
                    "request-log heartbeat"
                );
                tracing::info!(target: "dependency", "dependency info");
            },
        );

        assert!(output.contains("request-log heartbeat"));
        assert!(!output.contains("dependency info"));
    }
}
