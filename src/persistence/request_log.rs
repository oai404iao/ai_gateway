//! Backend-neutral request-log persistence contracts.

use chrono::{DateTime, Datelike, NaiveDate, Utc, Weekday};
use serde::Serialize;
use uuid::Uuid;

use crate::request_log_journal::EncodedRequestLog;

#[derive(Clone, Debug, Serialize)]
pub struct ConsoleRequestLog {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub user_id: Uuid,
    pub user_name: Option<String>,
    pub api_key_id: Uuid,
    pub request_source: String,
    pub api_format: String,
    pub api_operation: String,
    pub request_protocol: String,
    pub client_model: String,
    pub reasoning_effort: Option<String>,
    pub fast_mode: bool,
    pub upstream_model: Option<String>,
    pub model_rule_id: Option<Uuid>,
    pub channel_group_id: Option<Uuid>,
    pub channel_group_name: Option<String>,
    pub channel_id: Option<Uuid>,
    pub channel_name: Option<String>,
    pub outcome: String,
    pub response_status_code: Option<i16>,
    pub streamed: bool,
    pub ttft_ms: Option<i32>,
    pub total_duration_ms: Option<i32>,
    pub output_tokens_per_second: Option<rust_decimal::Decimal>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub cost_amount: Option<rust_decimal::Decimal>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub billed_at: Option<DateTime<Utc>>,
}

/// Server-side request-log filters. The Console API decodes and validates
/// query strings before constructing this typed repository input.
#[derive(Clone, Debug, Default)]
pub struct RequestLogFilter {
    pub limit: i64,
    pub user_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub model: Option<String>,
    pub api_format: Option<String>,
    pub api_operation: Option<String>,
    pub outcome: Option<String>,
    pub started_after: Option<DateTime<Utc>>,
    pub started_before: Option<DateTime<Utc>>,
    pub billed: Option<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelGroupStatusWindow {
    Last24Hours,
    Last3Days,
    Last7Days,
}

impl ChannelGroupStatusWindow {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Last24Hours => "24h",
            Self::Last3Days => "3d",
            Self::Last7Days => "7d",
        }
    }

    pub(super) const fn bucket_seconds(self) -> i64 {
        match self {
            Self::Last24Hours => 30 * 60,
            Self::Last3Days => 2 * 60 * 60,
            Self::Last7Days => 4 * 60 * 60,
        }
    }

    const fn bucket_count(self) -> i64 {
        match self {
            Self::Last24Hours => 48,
            Self::Last3Days => 36,
            Self::Last7Days => 42,
        }
    }

    pub(super) fn range(self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let bucket_seconds = self.bucket_seconds();
        let current_bucket_started_at = now.timestamp().div_euclid(bucket_seconds) * bucket_seconds;
        let started_at = current_bucket_started_at
            .saturating_sub((self.bucket_count() - 1).saturating_mul(bucket_seconds));
        (DateTime::from_timestamp(started_at, 0).unwrap_or(now), now)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatisticsGranularity {
    Hour,
    Day,
}

impl StatisticsGranularity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }

    pub(super) const fn max_range(self) -> chrono::Duration {
        match self {
            Self::Hour => chrono::Duration::days(31),
            Self::Day => chrono::Duration::days(366),
        }
    }

    pub(super) const fn bucket_seconds(self) -> i64 {
        match self {
            Self::Hour => 60 * 60,
            Self::Day => 24 * 60 * 60,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CostStatisticsFilter {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub granularity: StatisticsGranularity,
    pub user_id: Option<Uuid>,
    pub api_key_id: Option<Uuid>,
    pub channel_id: Option<Uuid>,
    pub codex_credential_id: Option<Uuid>,
    pub include_channel_details: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpendLeaderboardPeriod {
    Day,
    Week,
    Month,
}

impl SpendLeaderboardPeriod {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    #[must_use]
    pub fn current_start_at(self, now: DateTime<Utc>) -> NaiveDate {
        // Asia/Shanghai is UTC+08:00 and does not observe daylight saving time.
        let today = (now + chrono::Duration::hours(8)).date_naive();
        match self {
            Self::Day => today,
            Self::Week => {
                today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday()))
            }
            Self::Month => today
                .with_day(1)
                .expect("every calendar month has a first day"),
        }
    }

    #[must_use]
    pub fn end_after(self, period_start: NaiveDate) -> NaiveDate {
        match self {
            Self::Day => period_start
                .succ_opt()
                .expect("a supported date always has a next day"),
            Self::Week => period_start
                .checked_add_signed(chrono::Duration::days(7))
                .expect("a supported date always has a following week"),
            Self::Month => {
                let (year, month) = if period_start.month() == 12 {
                    (period_start.year() + 1, 1)
                } else {
                    (period_start.year(), period_start.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1)
                    .expect("a supported date always has a following month")
            }
        }
    }

    #[must_use]
    pub fn is_valid_start(self, period_start: NaiveDate) -> bool {
        match self {
            Self::Day => true,
            Self::Week => period_start.weekday() == Weekday::Mon,
            Self::Month => period_start.day() == 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpendLeaderboardFilter {
    pub period: SpendLeaderboardPeriod,
    pub period_start: NaiveDate,
    pub limit: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelGroupStatusReport {
    pub window: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub bucket_seconds: i64,
    pub models: Vec<ChannelGroupStatusModelMetric>,
    pub groups: Vec<ChannelGroupStatusGroup>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PersonalUsageReport {
    pub started_on: NaiveDate,
    pub ended_on: NaiveDate,
    pub total_request_count: i64,
    pub active_day_count: i64,
    pub current_streak_days: i64,
    pub longest_streak_days: i64,
    pub days: Vec<PersonalUsageDay>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PersonalUsageDay {
    pub date: NaiveDate,
    pub request_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelGroupStatusModelMetric {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelGroupStatusGroup {
    pub id: Uuid,
    pub api_format: String,
    pub name: String,
    pub enabled: bool,
    pub models: Vec<ChannelGroupStatusGroupModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelGroupStatusGroupModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
    pub history: Vec<ChannelGroupStatusBucket>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChannelGroupStatusBucket {
    pub started_at: DateTime<Utc>,
    pub request_count: i64,
    pub success_rate: Option<f64>,
    pub p90_ttft_ms: Option<f64>,
    pub p50_tps: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsReport {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub granularity: String,
    pub summary: CostStatisticsSummary,
    pub buckets: Vec<CostStatisticsBucket>,
    pub models: Vec<CostStatisticsModel>,
    pub channels: Vec<CostStatisticsChannel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsSummary {
    pub request_count: i64,
    pub priced_request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub average_rpm: f64,
    pub average_tpm: f64,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsBucket {
    pub started_at: DateTime<Utc>,
    pub request_count: i64,
    pub total_tokens: i64,
    pub cost_amount: rust_decimal::Decimal,
    pub models: Vec<CostStatisticsBucketModel>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsBucketModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsModel {
    pub api_format: String,
    pub model: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub success_rate: Option<f64>,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct CostStatisticsChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub channel_group_name: String,
    pub name: String,
    pub api_format: String,
    pub request_count: i64,
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub success_rate: Option<f64>,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendLeaderboardReport {
    pub period: String,
    pub period_start: NaiveDate,
    pub period_end: NaiveDate,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub total_cost_amount: rust_decimal::Decimal,
    pub previous_period_start: Option<NaiveDate>,
    pub next_period_start: Option<NaiveDate>,
    pub entries: Vec<SpendLeaderboardEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpendLeaderboardRefresh {
    Updated,
    AlreadyRunning,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpendLeaderboardEntry {
    pub rank: i64,
    pub user_id: Uuid,
    pub display_name: String,
    pub request_count: i64,
    pub priced_request_count: i64,
    pub total_tokens: i64,
    pub cost_amount: rust_decimal::Decimal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogInsertOutcome {
    Inserted,
    ExactDuplicate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestLogBatchInsertResult {
    pub request_log_id: Uuid,
    pub outcome: RequestLogBatchInsertOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestLogBatchInsertOutcome {
    Inserted,
    ExactDuplicate,
    DuplicateConflict,
    InvalidResponseStatus { status: u16 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequestLogSettlementOutcome {
    Settled {
        request_log_id: Uuid,
        api_key_id: Uuid,
        quota_used_amount: rust_decimal::Decimal,
    },
    AlreadyBilled,
    NotBillable,
    AccountMismatch,
    NotFound,
}

pub(crate) struct RequestLogIngestRecord {
    pub sequence: i64,
    pub request_log_id: Uuid,
    pub schema_version: i16,
    pub payload: Vec<u8>,
    pub attempt_count: i32,
}

impl RequestLogIngestRecord {
    pub(crate) fn encoded(&self) -> EncodedRequestLog {
        EncodedRequestLog {
            request_log_id: self.request_log_id,
            schema_version: self.schema_version,
            payload: self.payload.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestLogIngestBacklog {
    pub row_count: i64,
    pub oldest_staged_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestLogSettlementBacklog {
    pub row_count: i64,
    pub oldest_completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestLogPoolStatus {
    pub size: u32,
    pub idle: usize,
}
