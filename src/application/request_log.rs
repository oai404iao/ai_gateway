//! Nonblocking application port for terminal request-log events.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::{
    domain::RequestLogEvent, observability::RequestLogPipelineMetrics,
    request_log_spool::RequestLogSpool,
};

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
        ApiFormat, RequestBilling, RequestLogEvent, RequestLogOutcome, RequestLogSource,
        RequestPriceSnapshot, RequestUsage,
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
            client_model: "test".into(),
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
