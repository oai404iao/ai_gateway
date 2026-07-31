//! Nonblocking application port for terminal request-log events.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use crate::{
    domain::RequestLogEvent, observability::RequestLogPipelineMetrics,
    persistence::RequestLogRepository, request_log_spool::RequestLogSpool,
};

const MONITOR_QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Request paths use this synchronous port without waiting for PostgreSQL.
/// Durable implementations may perform a bounded local append before return.
pub trait RequestLogSink: Send + Sync {
    fn try_record(&self, event: RequestLogEvent);
}

#[derive(Clone, Default)]
pub struct NoopRequestLogSink;

impl RequestLogSink for NoopRequestLogSink {
    fn try_record(&self, _: RequestLogEvent) {}
}

#[derive(Clone)]
pub struct QueueRequestLogSink {
    sender: mpsc::Sender<RequestLogEvent>,
}

impl QueueRequestLogSink {
    #[must_use]
    pub fn new(sender: mpsc::Sender<RequestLogEvent>) -> Self {
        Self { sender }
    }
}

impl RequestLogSink for QueueRequestLogSink {
    fn try_record(&self, event: RequestLogEvent) {
        let id = event.id;
        match self.sender.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(request_log_id = %id, reason = "queue_full", "request log dropped");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!(request_log_id = %id, reason = "queue_closed", "request log dropped");
            }
        }
    }
}

/// Appends every accepted terminal event to a recoverable local spool before
/// notifying the asynchronous database pipeline. A full notification queue
/// only coalesces wakeups; it never discards the durable event.
#[derive(Clone)]
pub struct DurableRequestLogSink {
    spool: Arc<RequestLogSpool>,
    wake: mpsc::Sender<()>,
    metrics: Arc<RequestLogPipelineMetrics>,
}

impl DurableRequestLogSink {
    #[must_use]
    pub(crate) fn new(
        spool: Arc<RequestLogSpool>,
        wake: mpsc::Sender<()>,
        metrics: Arc<RequestLogPipelineMetrics>,
    ) -> Self {
        Self {
            spool,
            wake,
            metrics,
        }
    }
}

impl RequestLogSink for DurableRequestLogSink {
    fn try_record(&self, event: RequestLogEvent) {
        let id = event.id;
        self.metrics.record_attempt();
        match self.spool.append(&event) {
            Ok(bytes) => {
                self.metrics.record_spooled(bytes);
                match self.wake.try_send(()) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::warn!(
                            request_log_id = %id,
                            reason = "spool_worker_closed",
                            "request log remains durable in the local spool for restart recovery"
                        );
                    }
                }
            }
            Err(error) => {
                self.metrics.record_spool_append_failure();
                tracing::error!(
                    request_log_id = %id,
                    %error,
                    reason = "spool_append_failed",
                    "request log could not cross the configured durability boundary"
                );
            }
        }
    }
}

#[derive(Clone)]
pub struct RequestLogPipelineMonitor {
    repository: RequestLogRepository,
    spool: Arc<RequestLogSpool>,
    notification_sender: mpsc::Sender<()>,
    notification_capacity: usize,
    projection_sender: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    projection_capacity: usize,
    database_capacity: u32,
    metrics: Arc<RequestLogPipelineMetrics>,
}

impl RequestLogPipelineMonitor {
    #[must_use]
    pub(crate) fn new(
        repository: RequestLogRepository,
        spool: Arc<RequestLogSpool>,
        notification_sender: mpsc::Sender<()>,
        notification_capacity: usize,
        projection_sender: mpsc::Sender<()>,
        database_capacity: u32,
        metrics: Arc<RequestLogPipelineMetrics>,
    ) -> Self {
        Self {
            repository,
            spool,
            notification_sender,
            notification_capacity,
            projection_sender: Arc::new(Mutex::new(Some(projection_sender))),
            projection_capacity: 1,
            database_capacity,
            metrics,
        }
    }

    pub(crate) async fn snapshot(&self) -> RequestLogPipelineSnapshot {
        let metrics = self.metrics.snapshot();
        let (ingress, settlement) = tokio::join!(
            tokio::time::timeout(MONITOR_QUERY_TIMEOUT, self.repository.ingest_backlog()),
            tokio::time::timeout(MONITOR_QUERY_TIMEOUT, self.repository.settlement_backlog())
        );
        let ingress = ingress.ok().and_then(Result::ok);
        let settlement = settlement.ok().and_then(Result::ok);
        let pool = self.repository.pool_status();
        RequestLogPipelineSnapshot {
            notification_queue_depth: queue_depth(
                &self.notification_sender,
                self.notification_capacity,
            ),
            notification_queue_capacity: usize_to_u64(self.notification_capacity),
            projection_queue_depth: self
                .projection_sender
                .lock()
                .ok()
                .and_then(|sender| {
                    sender
                        .as_ref()
                        .map(|sender| queue_depth(sender, self.projection_capacity))
                })
                .unwrap_or(0),
            projection_queue_capacity: usize_to_u64(self.projection_capacity),
            spool_pending_bytes: self.spool.pending_bytes(),
            ingress_backlog_rows_estimate: ingress.map(|backlog| backlog.row_count.max(0) as u64),
            ingress_oldest_age_seconds: ingress
                .and_then(|backlog| age_seconds(backlog.oldest_staged_at)),
            settlement_backlog_rows: settlement.map(|backlog| backlog.row_count.max(0) as u64),
            settlement_oldest_age_seconds: settlement
                .and_then(|backlog| age_seconds(backlog.oldest_completed_at)),
            recorded_total: metrics.recorded_total,
            spooled_total: metrics.spooled_total,
            projected_rows_total: metrics.projected_rows_total,
            projection_deferred_total: metrics.projection_deferred_total,
            settled_rows_total: metrics.settled_rows_total,
            spool_append_failures_total: metrics.spool_append_failures_total,
            ingress_failures_total: metrics.ingress_failures_total,
            projection_failures_total: metrics.projection_failures_total,
            settlement_failures_total: metrics.settlement_failures_total,
            database_pool_size: u64::from(pool.size),
            database_pool_idle: usize_to_u64(pool.idle),
            database_pool_capacity: u64::from(self.database_capacity),
        }
    }

    pub(crate) fn disconnect_projection_queue(&self) {
        if let Ok(mut sender) = self.projection_sender.lock() {
            sender.take();
        }
    }
}

fn queue_depth(sender: &mpsc::Sender<()>, capacity: usize) -> u64 {
    usize_to_u64(capacity.saturating_sub(sender.capacity()))
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn age_seconds(timestamp: Option<DateTime<Utc>>) -> Option<u64> {
    timestamp.map(|timestamp| {
        u64::try_from((Utc::now() - timestamp).num_seconds().max(0)).unwrap_or(u64::MAX)
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RequestLogPipelineSnapshot {
    pub notification_queue_depth: u64,
    pub notification_queue_capacity: u64,
    pub projection_queue_depth: u64,
    pub projection_queue_capacity: u64,
    pub spool_pending_bytes: u64,
    pub ingress_backlog_rows_estimate: Option<u64>,
    pub ingress_oldest_age_seconds: Option<u64>,
    pub settlement_backlog_rows: Option<u64>,
    pub settlement_oldest_age_seconds: Option<u64>,
    pub recorded_total: u64,
    pub spooled_total: u64,
    pub projected_rows_total: u64,
    pub projection_deferred_total: u64,
    pub settled_rows_total: u64,
    pub spool_append_failures_total: u64,
    pub ingress_failures_total: u64,
    pub projection_failures_total: u64,
    pub settlement_failures_total: u64,
    pub database_pool_size: u64,
    pub database_pool_idle: u64,
    pub database_pool_capacity: u64,
}

/// A minimal test sink for observing accepted terminal events without a DB.
#[derive(Clone, Default)]
pub struct RecordingRequestLogSink {
    events: Arc<Mutex<Vec<RequestLogEvent>>>,
}

impl RecordingRequestLogSink {
    #[must_use]
    pub fn events(&self) -> Vec<RequestLogEvent> {
        self.events
            .lock()
            .expect("recording sink lock poisoned")
            .clone()
    }
}

impl RequestLogSink for RecordingRequestLogSink {
    fn try_record(&self, event: RequestLogEvent) {
        self.events
            .lock()
            .expect("recording sink lock poisoned")
            .push(event);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rust_decimal::Decimal;
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::{QueueRequestLogSink, RequestLogSink};
    use crate::domain::{
        ApiFormat, ApiOperation, RequestBilling, RequestLogEvent, RequestLogOutcome,
        RequestLogSource, RequestPriceSnapshot, RequestProtocol, RequestUsage,
    };

    fn event() -> RequestLogEvent {
        let now = Utc::now();
        RequestLogEvent {
            id: Uuid::new_v4(),
            started_at: now,
            completed_at: now,
            user_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            request_source: RequestLogSource::Client,
            api_format: ApiFormat::OpenAiChatCompletions,
            api_operation: ApiOperation::ChatCompletions,
            request_protocol: RequestProtocol::NonStream,
            client_model: "test".into(),
            reasoning_effort: None,
            fast_mode: false,
            upstream_model: None,
            model_rule_id: None,
            channel_group_id: None,
            channel_id: None,
            model_id: None,
            outcome: RequestLogOutcome::Rejected,
            response_status_code: Some(404),
            streamed: false,
            ttft_ms: None,
            total_duration_ms: 0,
            billing: Some(RequestBilling {
                usage: Some(RequestUsage {
                    input_tokens: 3,
                    cached_input_tokens: 1,
                    cache_write_tokens: 0,
                    output_tokens: 2,
                    reasoning_tokens: 1,
                }),
                price: RequestPriceSnapshot {
                    currency: "USD".into(),
                    price_unit_tokens: 1_000_000,
                    price_effective_at: now,
                    input_unit_price: Decimal::ZERO,
                    cached_input_unit_price: Decimal::ZERO,
                    cache_write_unit_price: Decimal::ZERO,
                    output_unit_price: Decimal::ZERO,
                },
                cost_amount: Some(Decimal::ZERO),
                output_tokens_per_second: Some(Decimal::ONE),
            }),
            error_code: Some("model_not_found".into()),
            error_summary: None,
        }
    }

    #[test]
    fn saturated_queue_drops_without_waiting() {
        let (sender, mut receiver) = mpsc::channel(1);
        let sink = QueueRequestLogSink::new(sender);
        sink.try_record(event());
        sink.try_record(event());
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }
}
