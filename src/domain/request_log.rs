//! Safe, terminal request-log data emitted by the data plane.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiFormat, ApiOperation};

/// A single idempotent request-log row. It deliberately excludes request
/// bodies, successful response bodies, headers, and credentials. Failed
/// requests may retain a bounded, cleaned upstream response detail or
/// gateway/transport diagnostic for operator troubleshooting.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RequestLogEvent {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub api_key_id: Uuid,
    pub request_source: RequestLogSource,
    pub api_format: ApiFormat,
    pub api_operation: ApiOperation,
    pub request_protocol: RequestProtocol,
    pub client_model: String,
    /// Explicit reasoning effort requested by the client. Missing and
    /// unrecognized request shapes remain `None`.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Whether the client explicitly requested OpenAI Priority processing via
    /// `service_tier = "priority"`.
    #[serde(default)]
    pub fast_mode: bool,
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
    pub billing: Option<RequestBilling>,
    pub error_code: Option<String>,
    #[serde(deserialize_with = "deserialize_required_error_summary")]
    pub error_summary: Option<String>,
}

fn deserialize_required_error_summary<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

/// Identifies whether a row came from the OpenAI-compatible data plane, an MCP
/// adapter, or the system's periodic direct upstream test worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestLogSource {
    Client,
    Mcp,
    ScheduledTest,
}

impl RequestLogSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Mcp => "mcp",
            Self::ScheduledTest => "scheduled_test",
        }
    }
}

/// Client-visible transport used for one logical request.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestProtocol {
    NonStream,
    Sse,
    #[serde(rename = "websocket")]
    WebSocket,
}

impl RequestProtocol {
    #[must_use]
    pub const fn from_http_streamed(streamed: bool) -> Self {
        if streamed { Self::Sse } else { Self::NonStream }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonStream => "non_stream",
            Self::Sse => "sse",
            Self::WebSocket => "websocket",
        }
    }

    #[must_use]
    pub const fn is_streamed(self) -> bool {
        !matches!(self, Self::NonStream)
    }
}

/// Tokens and price facts recorded with one terminal selected-route request.
/// Price fields are copied from the immutable route snapshot rather than read
/// from the mutable `models` table after the response has completed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestBilling {
    pub usage: Option<RequestUsage>,
    pub price: RequestPriceSnapshot,
    pub cost_amount: Option<Decimal>,
    pub output_tokens_per_second: Option<Decimal>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    /// Subset of `output_tokens` spent on model reasoning. Older journal v2
    /// payloads omit this field and decode it as zero.
    #[serde(default)]
    pub reasoning_tokens: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestPriceSnapshot {
    pub currency: String,
    pub price_unit_tokens: i64,
    pub price_effective_at: DateTime<Utc>,
    pub input_unit_price: Decimal,
    pub cached_input_unit_price: Decimal,
    pub cache_write_unit_price: Decimal,
    pub output_unit_price: Decimal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
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
