//! Safe, terminal request-log data emitted by the data plane.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::ApiFormat;

/// A single idempotent request-log row. It deliberately excludes request and
/// response bodies, headers, credentials, and raw transport errors.
#[derive(Clone, Debug)]
pub struct RequestLogEvent {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub api_key_id: Uuid,
    pub api_format: ApiFormat,
    pub client_model: String,
    pub upstream_model: Option<String>,
    pub model_rule_id: Option<Uuid>,
    pub channel_group_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub model_id: Option<Uuid>,
    pub outcome: RequestLogOutcome,
    pub response_status_code: Option<u16>,
    pub streamed: bool,
    pub ttft_ms: Option<i32>,
    pub total_duration_ms: i32,
    pub error_code: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogOutcome {
    Succeeded,
    Failed,
    Rejected,
    Cancelled,
}

impl RequestLogOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
}
