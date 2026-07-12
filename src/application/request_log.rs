//! Nonblocking application port for terminal request-log events.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use crate::domain::RequestLogEvent;

/// Request paths use this synchronous port so persistence can never delay a
/// response or stream.
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
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::{QueueRequestLogSink, RequestLogSink};
    use crate::domain::{ApiFormat, RequestLogEvent, RequestLogOutcome};

    fn event() -> RequestLogEvent {
        let now = Utc::now();
        RequestLogEvent {
            id: Uuid::new_v4(),
            started_at: now,
            completed_at: now,
            user_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
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
            error_code: Some("model_not_found"),
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
