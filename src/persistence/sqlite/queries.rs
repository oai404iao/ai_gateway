//! SQLite log views and immutable-fact statistics, preserving the shared report projections.
//! Money uses exact scaled accumulation; percentile sorting and leaderboard staging use disk.

use super::aggregate::CostSum;
use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use futures_util::TryStreamExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{Connection, FromRow, QueryBuilder, Sqlite};
use uuid::Uuid;

use crate::{
    domain::ApiFormat,
    persistence::{
        ChannelGroupStatusBucket, ChannelGroupStatusGroup, ChannelGroupStatusGroupModel,
        ChannelGroupStatusModelMetric, ChannelGroupStatusReport, ChannelGroupStatusWindow,
        ConsoleRequestLog, CostStatisticsChannel, CostStatisticsFilter, CostStatisticsReport,
        CostStatisticsSummary, PersonalUsageReport, RepositoryError, RequestLogFilter,
        SpendLeaderboardEntry, SpendLeaderboardFilter, SpendLeaderboardRefresh,
        SpendLeaderboardReport,
        postgres_control_plane::{
            CostBucketMetricRow, CostModelMetricRow, PersonalUsageDayRow, fold_cost_buckets,
            fold_cost_models, fold_personal_usage, redact_self_service_request_log, success_rate,
        },
    },
};

use super::{
    SqliteAmount, SqliteDatabase, SqliteDate, SqliteOpenError, SqliteTimestamp, SqliteTokenRate,
    SqliteUuid,
};

fn open_failure(error: SqliteOpenError) -> RepositoryError {
    RepositoryError::from(sqlx::Error::Configuration(Box::new(error)))
}

fn checked_add_count(target: &mut i64, value: i64) -> Result<(), RepositoryError> {
    *target = target
        .checked_add(value)
        .ok_or(RepositoryError::Validation)?;
    Ok(())
}

/// `json_each` is SQLite's bounded replacement for a PostgreSQL array
/// parameter; the carrier is a JSON array of canonical UUID strings.
fn uuid_array(ids: &[Uuid]) -> String {
    Value::Array(ids.iter().map(|id| json!(id.to_string())).collect()).to_string()
}

/// Reads the bounded Console log projection. The `_for_user` variants add the
/// ownership predicate and channel-attribution redaction.
#[derive(Clone)]
pub struct SqliteRequestLogQueries {
    database: Arc<SqliteDatabase>,
}

impl SqliteRequestLogQueries {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    #[must_use]
    pub fn metering(&self) -> SqliteMeteringQueries {
        SqliteMeteringQueries::new(Arc::clone(&self.database))
    }

    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        let mut logs = self.query_logs(Some(user_id), filter).await?;
        for log in &mut logs {
            redact_self_service_request_log(log);
        }
        Ok(logs)
    }

    pub async fn get_for_user(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        let mut log = self.query_log(id, Some(user_id)).await?;
        if let Some(log) = &mut log {
            redact_self_service_request_log(log);
        }
        Ok(log)
    }

    pub async fn list_all(
        &self,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        self.query_logs(None, filter).await
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        self.query_log(id, None).await
    }

    async fn query_log(
        &self,
        id: Uuid,
        owner_user_id: Option<Uuid>,
    ) -> Result<Option<ConsoleRequestLog>, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let row = match owner_user_id {
            Some(user_id) => {
                sqlx::query_as::<_, ConsoleRequestLogRow>(sqlx::AssertSqlSafe(format!(
                    "{CONSOLE_REQUEST_LOG_SELECT} WHERE log.id = ? AND log.user_id = ?"
                )))
                .bind(SqliteUuid(id))
                .bind(SqliteUuid(user_id))
                .fetch_optional(&mut *connection)
                .await?
            }
            None => {
                sqlx::query_as::<_, ConsoleRequestLogRow>(sqlx::AssertSqlSafe(format!(
                    "{CONSOLE_REQUEST_LOG_SELECT} WHERE log.id = ?"
                )))
                .bind(SqliteUuid(id))
                .fetch_optional(&mut *connection)
                .await?
            }
        };
        Ok(row.map(ConsoleRequestLogRow::into_log))
    }

    async fn query_logs(
        &self,
        owner_user_id: Option<Uuid>,
        filter: RequestLogFilter,
    ) -> Result<Vec<ConsoleRequestLog>, RepositoryError> {
        validate_request_log_filter(&filter)?;
        let mut query =
            QueryBuilder::<Sqlite>::new(format!("{CONSOLE_REQUEST_LOG_SELECT} WHERE TRUE"));
        if let Some(user_id) = owner_user_id {
            query
                .push(" AND log.user_id = ")
                .push_bind(SqliteUuid(user_id));
        }
        if let Some(user_id) = filter.user_id {
            query
                .push(" AND log.user_id = ")
                .push_bind(SqliteUuid(user_id));
        }
        if let Some(api_key_id) = filter.api_key_id {
            query
                .push(" AND log.api_key_id = ")
                .push_bind(SqliteUuid(api_key_id));
        }
        if let Some(model) = filter.model {
            query
                .push(" AND (log.client_model = ")
                .push_bind(model.clone())
                .push(" OR log.upstream_model = ")
                .push_bind(model)
                .push(")");
        }
        if let Some(api_format) = filter.api_format {
            query.push(" AND log.api_format = ").push_bind(api_format);
        }
        if let Some(api_operation) = filter.api_operation {
            query
                .push(" AND log.api_operation = ")
                .push_bind(api_operation);
        }
        if let Some(outcome) = filter.outcome {
            query.push(" AND log.outcome = ").push_bind(outcome);
        }
        if let Some(started_after) = filter.started_after {
            query
                .push(" AND log.started_at >= ")
                .push_bind(SqliteTimestamp(started_after));
        }
        if let Some(started_before) = filter.started_before {
            query
                .push(" AND log.started_at <= ")
                .push_bind(SqliteTimestamp(started_before));
        }
        if let Some(billed) = filter.billed {
            if billed {
                query.push(" AND receipt.settled_at IS NOT NULL");
            } else {
                query.push(" AND receipt.settled_at IS NULL");
            }
        }
        query
            .push(" ORDER BY log.started_at DESC, log.id DESC LIMIT ")
            .push_bind(filter.limit.clamp(1, 100));
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        Ok(query
            .build_query_as::<ConsoleRequestLogRow>()
            .fetch_all(&mut *connection)
            .await?
            .into_iter()
            .map(ConsoleRequestLogRow::into_log)
            .collect())
    }

    /// Aggregates the channel-group status view from `request_logs`.
    ///
    /// The accumulators are seeded from each tracked group's configured
    /// `available_models`, so an idle model still reports zero data exactly like
    /// the PostgreSQL `LEFT JOIN LATERAL unnest(...)` seed.
    pub async fn channel_group_status(
        &self,
        window: ChannelGroupStatusWindow,
    ) -> Result<ChannelGroupStatusReport, RepositoryError> {
        let ended_at = Utc::now();
        let (started_at, ended_at) = window.range(ended_at);
        let bucket_seconds = window.bucket_seconds();
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let mut transaction = connection.begin().await?;

        let mut groups = Vec::<StatusGroupBuilder>::new();
        let mut group_indexes = BTreeMap::<(Uuid, String), usize>::new();
        let mut overall_models = BTreeMap::<(String, String), StatusMetricAccumulator>::new();
        let tracked = sqlx::query_as::<_, TrackedChannelGroupRow>(
            "SELECT channel_group.id AS id, \
                    CASE capability.operation \
                        WHEN 'chat_completions' THEN 'open_ai_chat_completions' \
                        WHEN 'images_generation' THEN 'open_ai_images' \
                        WHEN 'images_edit' THEN 'open_ai_images' \
                        ELSE 'open_ai_responses' END AS api_format, \
                    channel_group.name AS name, channel_group.enabled AS enabled, \
                    capability.available_models AS available_models \
             FROM routing_groups AS channel_group \
             JOIN upstream_channels AS channel \
               ON channel.group_id = channel_group.id AND channel.deleted_at IS NULL \
             JOIN channel_capabilities AS capability ON capability.channel_id=channel.id \
               AND capability.deleted_at IS NULL AND capability.status_statistics_enabled \
             WHERE channel_group.deleted_at IS NULL \
             ORDER BY channel_group.name, channel_group.id, api_format",
        )
        .fetch_all(&mut *transaction)
        .await?;
        for group in tracked {
            let group_key = (group.id.0, group.api_format.clone());
            let index = match group_indexes.get(&group_key).copied() {
                Some(index) => index,
                None => {
                    group_indexes.insert(group_key, groups.len());
                    groups.push(StatusGroupBuilder {
                        id: group.id.0,
                        api_format: group.api_format.clone(),
                        name: group.name,
                        enabled: group.enabled,
                        models: BTreeMap::new(),
                    });
                    groups.len() - 1
                }
            };
            let Some(models) = group.available_models.as_deref() else {
                continue;
            };
            let values: Vec<String> =
                serde_json::from_str(models).map_err(|_| RepositoryError::Validation)?;
            for model in values {
                overall_models
                    .entry((group.api_format.clone(), model.clone()))
                    .or_default();
                groups[index]
                    .models
                    .entry((group.api_format.clone(), model))
                    .or_default();
            }
        }

        let mut group_models = BTreeMap::<(Uuid, String, String), StatusMetricAccumulator>::new();
        let mut history = BTreeMap::<(Uuid, String, String, i64), StatusMetricAccumulator>::new();
        {
            let mut stream = sqlx::query_as::<_, StatusMetricRow>(include_str!("status.sql"))
                .bind(SqliteTimestamp(started_at))
                .bind(SqliteTimestamp(ended_at))
                .bind(bucket_seconds)
                .fetch(&mut *transaction);
            while let Some(row) = stream.try_next().await? {
                let metric = StatusMetricAccumulator {
                    request_count: row.request_count,
                    success_rate_request_count: row.success_rate_request_count,
                    succeeded_count: row.succeeded_count,
                    p90_ttft_ms: row.p90_ttft_ms,
                    p50_tps: row.p50_tps,
                };
                match row.scope {
                    0 => {
                        overall_models.insert((row.api_format, row.model), metric);
                    }
                    1 => {
                        group_models.insert(
                            (
                                row.group_id.ok_or(RepositoryError::Validation)?.0,
                                row.api_format,
                                row.model,
                            ),
                            metric,
                        );
                    }
                    2 => {
                        history.insert(
                            (
                                row.group_id.ok_or(RepositoryError::Validation)?.0,
                                row.api_format,
                                row.model,
                                row.bucket,
                            ),
                            metric,
                        );
                    }
                    _ => return Err(RepositoryError::Validation),
                }
            }
        }
        transaction.commit().await?;

        for ((group_id, api_format, model), accumulator) in group_models {
            let Some(index) = group_indexes.get(&(group_id, api_format.clone())).copied() else {
                continue;
            };
            groups[index]
                .models
                .insert((api_format, model), accumulator);
        }

        let mut report_groups = Vec::with_capacity(groups.len());
        for group in groups {
            let mut models = Vec::with_capacity(group.models.len());
            for ((api_format, model), accumulator) in &group.models {
                let mut history_buckets = Vec::new();
                for ((_, _, _, bucket), bucket_metric) in history.range(
                    (group.id, api_format.clone(), model.clone(), i64::MIN)
                        ..=(group.id, api_format.clone(), model.clone(), i64::MAX),
                ) {
                    let started_at =
                        DateTime::from_timestamp(*bucket, 0).ok_or(RepositoryError::Validation)?;
                    let summary = bucket_metric.summary();
                    history_buckets.push(ChannelGroupStatusBucket {
                        started_at,
                        request_count: summary.request_count,
                        success_rate: summary.success_rate,
                        p90_ttft_ms: summary.p90_ttft_ms,
                        p50_tps: summary.p50_tps,
                    });
                }
                let summary = accumulator.summary();
                models.push(ChannelGroupStatusGroupModel {
                    api_format: api_format.clone(),
                    model: model.clone(),
                    request_count: summary.request_count,
                    success_rate: summary.success_rate,
                    p90_ttft_ms: summary.p90_ttft_ms,
                    p50_tps: summary.p50_tps,
                    history: history_buckets,
                });
            }
            report_groups.push(ChannelGroupStatusGroup {
                id: group.id,
                api_format: group.api_format,
                name: group.name,
                enabled: group.enabled,
                models,
            });
        }

        let models = overall_models
            .into_iter()
            .map(|((api_format, model), accumulator)| {
                let summary = accumulator.summary();
                ChannelGroupStatusModelMetric {
                    api_format,
                    model,
                    request_count: summary.request_count,
                    success_rate: summary.success_rate,
                    p90_ttft_ms: summary.p90_ttft_ms,
                    p50_tps: summary.p50_tps,
                }
            })
            .collect();

        Ok(ChannelGroupStatusReport {
            window: window.as_str().into(),
            started_at,
            ended_at,
            bucket_seconds,
            models,
            groups: report_groups,
        })
    }
}

/// Mirrors the PostgreSQL filter validation exactly, including treating a
/// whitespace-only model and a reversed time range as invalid.
fn validate_request_log_filter(filter: &RequestLogFilter) -> Result<(), RepositoryError> {
    if filter
        .api_format
        .as_deref()
        .is_some_and(|value| ApiFormat::parse(value).is_none())
        || filter.api_operation.as_deref().is_some_and(|value| {
            !matches!(
                value,
                "chat_completions"
                    | "responses"
                    | "standalone_web_search"
                    | "images_generation"
                    | "images_edit"
            )
        })
        || filter.outcome.as_deref().is_some_and(|value| {
            !matches!(value, "succeeded" | "failed" | "rejected" | "cancelled")
        })
        || filter
            .model
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 300)
        || filter.started_after > filter.started_before
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

const CONSOLE_REQUEST_LOG_SELECT: &str = "SELECT log.id,log.started_at,log.completed_at,log.user_id,\
     request_user.display_name AS user_name,log.api_key_id,log.request_source,log.api_format AS api_format,\
     log.api_operation,log.request_protocol,log.client_model,log.reasoning_effort,log.fast_mode,\
     log.upstream_model,log.model_rule_id,log.channel_group_id,\
     channel_group.label AS channel_group_name,log.channel_id,channel.label AS channel_name,log.outcome,\
     log.response_status_code,log.streamed,log.ttft_ms,log.total_duration_ms,\
     log.output_tokens_per_second,log.input_tokens,log.cached_input_tokens,log.cache_write_tokens,\
     log.output_tokens,log.reasoning_tokens,log.cost_amount,log.peak_pricing,log.error_code,\
     log.error_summary,receipt.settled_at AS billed_at \
     FROM request_logs AS log \
     LEFT JOIN request_settlements AS receipt ON receipt.request_id=log.id \
     JOIN users AS request_user ON request_user.id=log.user_id \
     LEFT JOIN group_identity_registry AS channel_group ON channel_group.id=log.channel_group_id \
     LEFT JOIN channel_identity_registry AS channel ON channel.id=log.channel_id";

#[derive(FromRow)]
struct ConsoleRequestLogRow {
    id: SqliteUuid,
    started_at: SqliteTimestamp,
    completed_at: SqliteTimestamp,
    user_id: SqliteUuid,
    user_name: Option<String>,
    api_key_id: SqliteUuid,
    request_source: String,
    api_format: String,
    api_operation: String,
    request_protocol: String,
    client_model: String,
    reasoning_effort: Option<String>,
    fast_mode: bool,
    upstream_model: Option<String>,
    model_rule_id: Option<SqliteUuid>,
    channel_group_id: Option<SqliteUuid>,
    channel_group_name: Option<String>,
    channel_id: Option<SqliteUuid>,
    channel_name: Option<String>,
    outcome: String,
    response_status_code: Option<i16>,
    streamed: bool,
    ttft_ms: Option<i32>,
    total_duration_ms: Option<i32>,
    output_tokens_per_second: Option<SqliteTokenRate>,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cost_amount: Option<SqliteAmount>,
    peak_pricing: bool,
    error_code: Option<String>,
    error_summary: Option<String>,
    billed_at: Option<SqliteTimestamp>,
}

impl ConsoleRequestLogRow {
    fn into_log(self) -> ConsoleRequestLog {
        ConsoleRequestLog {
            id: self.id.0,
            started_at: self.started_at.0,
            completed_at: self.completed_at.0,
            user_id: self.user_id.0,
            user_name: self.user_name,
            api_key_id: self.api_key_id.0,
            request_source: self.request_source,
            api_format: self.api_format,
            api_operation: self.api_operation,
            request_protocol: self.request_protocol,
            client_model: self.client_model,
            reasoning_effort: self.reasoning_effort,
            fast_mode: self.fast_mode,
            upstream_model: self.upstream_model,
            model_rule_id: self.model_rule_id.map(|value| value.0),
            channel_group_id: self.channel_group_id.map(|value| value.0),
            channel_group_name: self.channel_group_name,
            channel_id: self.channel_id.map(|value| value.0),
            channel_name: self.channel_name,
            outcome: self.outcome,
            response_status_code: self.response_status_code,
            streamed: self.streamed,
            ttft_ms: self.ttft_ms,
            total_duration_ms: self.total_duration_ms,
            output_tokens_per_second: self.output_tokens_per_second.map(|value| value.0),
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_write_tokens: self.cache_write_tokens,
            output_tokens: self.output_tokens,
            reasoning_tokens: self.reasoning_tokens,
            cost_amount: self.cost_amount.map(|value| value.0),
            peak_pricing: self.peak_pricing,
            error_code: self.error_code,
            error_summary: self.error_summary,
            billed_at: self.billed_at.map(|value| value.0),
        }
    }
}

#[derive(FromRow)]
struct TrackedChannelGroupRow {
    id: SqliteUuid,
    api_format: String,
    name: String,
    enabled: bool,
    available_models: Option<String>,
}

#[derive(FromRow)]
struct StatusMetricRow {
    scope: i64,
    group_id: Option<SqliteUuid>,
    api_format: String,
    model: String,
    bucket: i64,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

struct StatusGroupBuilder {
    id: Uuid,
    api_format: String,
    name: String,
    enabled: bool,
    models: BTreeMap<(String, String), StatusMetricAccumulator>,
}

#[derive(Default)]
struct StatusMetricAccumulator {
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

struct StatusMetricSummary {
    request_count: i64,
    success_rate: Option<f64>,
    p90_ttft_ms: Option<f64>,
    p50_tps: Option<f64>,
}

impl StatusMetricAccumulator {
    fn summary(&self) -> StatusMetricSummary {
        StatusMetricSummary {
            request_count: self.request_count,
            success_rate: success_rate(self.success_rate_request_count, self.succeeded_count),
            p90_ttft_ms: self.p90_ttft_ms,
            p50_tps: self.p50_tps,
        }
    }
}

/// PostgreSQL orders the `api_format` enum by declaration order, which is not
/// the byte order of its text form. Reproduce the enum order for the one query
/// that orders by format.
fn api_format_rank(value: &str) -> usize {
    ApiFormat::ALL
        .iter()
        .position(|format| format.as_str() == value)
        .unwrap_or(usize::MAX)
}

/// Immutable-fact cost aggregation, personal usage, spend-leaderboard snapshots,
/// and the sharing recovery read.
#[derive(Clone)]
pub struct SqliteMeteringQueries {
    database: Arc<SqliteDatabase>,
}

impl SqliteMeteringQueries {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// Fixed 365-day UTC activity window for one user, reading financial facts
    /// rather than the log projection.
    pub async fn personal_usage(
        &self,
        user_id: Uuid,
        ended_on: NaiveDate,
    ) -> Result<PersonalUsageReport, RepositoryError> {
        let started_on = ended_on
            .checked_sub_signed(ChronoDuration::days(364))
            .ok_or(RepositoryError::Validation)?;
        let ended_exclusive_on = ended_on.succ_opt().ok_or(RepositoryError::Validation)?;
        let started_at = DateTime::from_naive_utc_and_offset(
            started_on
                .and_hms_opt(0, 0, 0)
                .ok_or(RepositoryError::Validation)?,
            Utc,
        );
        let ended_at = DateTime::from_naive_utc_and_offset(
            ended_exclusive_on
                .and_hms_opt(0, 0, 0)
                .ok_or(RepositoryError::Validation)?,
            Utc,
        );
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let rows = sqlx::query_as::<_, PersonalUsageDayRow>(
            "SELECT substr(log.started_at,1,10) AS date, count(*) AS request_count \
             FROM request_metering_facts AS log \
             WHERE log.user_id = ?1 \
               AND log.request_source = 'client' \
               AND log.started_at >= ?2 AND log.started_at < ?3 \
             GROUP BY substr(log.started_at,1,10) \
             ORDER BY substr(log.started_at,1,10)",
        )
        .bind(SqliteUuid(user_id))
        .bind(SqliteTimestamp(started_at))
        .bind(SqliteTimestamp(ended_at))
        .fetch_all(&mut *connection)
        .await?;
        Ok(fold_personal_usage(rows, started_on, ended_on))
    }

    pub async fn cost_statistics(
        &self,
        filter: CostStatisticsFilter,
    ) -> Result<CostStatisticsReport, RepositoryError> {
        let duration = filter.ended_at.signed_duration_since(filter.started_at);
        if duration <= ChronoDuration::zero()
            || duration > filter.granularity.max_range()
            || (filter.channel_id.is_some() && filter.codex_credential_id.is_some())
        {
            return Err(RepositoryError::Validation);
        }

        let mut summary = CostSummaryAccumulator::default();
        let mut buckets = BTreeMap::<(i64, String, String), CostAccumulator>::new();
        let mut models = BTreeMap::<(String, String), CostModelAccumulator>::new();
        let mut channels = BTreeMap::<(Uuid, Uuid, String), CostChannelAccumulator>::new();

        let bucket_seconds = filter.granularity.bucket_seconds();
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT log.started_at AS started_at, log.api_format AS api_format, \
                    COALESCE(log.upstream_model, log.client_model) AS model, \
                    log.outcome AS outcome, log.input_tokens AS input_tokens, \
                    log.cached_input_tokens AS cached_input_tokens, \
                    log.cache_write_tokens AS cache_write_tokens, \
                    log.output_tokens AS output_tokens, log.cost_amount AS cost_amount, \
                    log.channel_id AS channel_id, log.channel_group_id AS channel_group_id, \
                    channel.label AS channel_name, \
                    channel_group.label AS channel_group_name \
             FROM request_metering_facts AS log \
             LEFT JOIN channel_identity_registry AS channel ON channel.id = log.channel_id \
             LEFT JOIN group_identity_registry AS channel_group \
               ON channel_group.id = log.channel_group_id \
             WHERE log.started_at >= ",
        );
        query.push_bind(SqliteTimestamp(filter.started_at));
        query.push(" AND log.started_at < ");
        query.push_bind(SqliteTimestamp(filter.ended_at));
        if let Some(user_id) = filter.user_id {
            query
                .push(" AND log.user_id = ")
                .push_bind(SqliteUuid(user_id));
        }
        if let Some(api_key_id) = filter.api_key_id {
            query
                .push(" AND log.api_key_id = ")
                .push_bind(SqliteUuid(api_key_id));
        }
        if let Some(channel_id) = filter.channel_id {
            query
                .push(" AND log.channel_id = ")
                .push_bind(SqliteUuid(channel_id));
        }
        if let Some(credential_id) = filter.codex_credential_id {
            query
                .push(
                    " AND log.channel_id IN (SELECT identity.id \
                     FROM channel_identity_registry AS identity \
                     WHERE identity.codex_credential_id = ",
                )
                .push_bind(SqliteUuid(credential_id))
                .push(")");
        }
        {
            let mut stream = query
                .build_query_as::<CostFactRow>()
                .fetch(&mut *connection);
            while let Some(row) = stream.try_next().await? {
                let bucket =
                    row.started_at.0.timestamp().div_euclid(bucket_seconds) * bucket_seconds;
                summary.add(&row)?;
                buckets
                    .entry((bucket, row.api_format.clone(), row.model.clone()))
                    .or_default()
                    .add(&row)?;
                models
                    .entry((row.api_format.clone(), row.model.clone()))
                    .or_default()
                    .add(&row)?;
                if filter.include_channel_details
                    && let (Some(channel_id), Some(channel_group_id)) =
                        (row.channel_id, row.channel_group_id)
                {
                    channels
                        .entry((channel_id.0, channel_group_id.0, row.api_format.clone()))
                        .or_default()
                        .add(&row)?;
                }
            }
        }

        let bucket_rows = buckets
            .into_iter()
            .map(|((bucket, api_format, model), accumulator)| {
                Ok(CostBucketMetricRow {
                    bucket_started_at: DateTime::from_timestamp(bucket, 0)
                        .ok_or(RepositoryError::Validation)?,
                    model,
                    api_format,
                    request_count: accumulator.request_count,
                    total_tokens: accumulator.total_tokens,
                    cost_amount: accumulator.cost_amount.finish()?,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        let model_rows = models
            .into_iter()
            .map(|((api_format, model), accumulator)| {
                Ok(CostModelMetricRow {
                    model,
                    api_format,
                    request_count: accumulator.request_count,
                    success_rate_request_count: accumulator.success_rate_request_count,
                    succeeded_count: accumulator.succeeded_count,
                    total_tokens: accumulator.total_tokens,
                    input_tokens: accumulator.input_tokens,
                    cached_input_tokens: accumulator.cached_input_tokens,
                    cache_write_tokens: accumulator.cache_write_tokens,
                    output_tokens: accumulator.output_tokens,
                    cost_amount: accumulator.cost_amount.finish()?,
                })
            })
            .collect::<Result<Vec<_>, RepositoryError>>()?;

        let mut channel_details = channels
            .into_values()
            .map(CostChannelAccumulator::finish)
            .collect::<Result<Vec<_>, RepositoryError>>()?;
        channel_details.sort_by(|left, right| {
            (
                &left.channel_group_name,
                &left.name,
                api_format_rank(&left.api_format),
            )
                .cmp(&(
                    &right.channel_group_name,
                    &right.name,
                    api_format_rank(&right.api_format),
                ))
        });

        let duration_minutes = duration.num_milliseconds().max(1) as f64 / 60_000.0;
        Ok(CostStatisticsReport {
            started_at: filter.started_at,
            ended_at: filter.ended_at,
            granularity: filter.granularity.as_str().into(),
            summary: CostStatisticsSummary {
                request_count: summary.request_count,
                priced_request_count: summary.priced_request_count,
                total_tokens: summary.total_tokens,
                input_tokens: summary.input_tokens,
                cached_input_tokens: summary.cached_input_tokens,
                cache_write_tokens: summary.cache_write_tokens,
                output_tokens: summary.output_tokens,
                average_rpm: summary.request_count as f64 / duration_minutes,
                average_tpm: summary.total_tokens as f64 / duration_minutes,
                cost_amount: summary.cost_amount.finish()?,
            },
            buckets: fold_cost_buckets(
                bucket_rows,
                filter.started_at,
                filter.ended_at,
                filter.granularity,
            ),
            models: fold_cost_models(model_rows),
            channels: channel_details,
        })
    }

    /// Rebuilds the Asia/Shanghai day, ISO-week, and calendar-month user-spend
    /// snapshots from immutable financial facts. SQLite is single-instance, so
    /// the whole rebuild runs inside the one `BEGIN IMMEDIATE` writer instead of
    /// an advisory lock; Console reads only the snapshot tables afterwards.
    pub async fn refresh_spend_leaderboard_snapshots(
        &self,
    ) -> Result<SpendLeaderboardRefresh, RepositoryError> {
        super::leaderboard::refresh(&self.database).await
    }

    pub async fn spend_leaderboard(
        &self,
        filter: SpendLeaderboardFilter,
    ) -> Result<SpendLeaderboardReport, RepositoryError> {
        if !(1..=100).contains(&filter.limit) || !filter.period.is_valid_start(filter.period_start)
        {
            return Err(RepositoryError::Validation);
        }
        let period = filter.period.as_str();
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let mut transaction = connection.begin().await?;
        let snapshot = sqlx::query_as::<_, SpendLeaderboardPeriodRow>(
            "SELECT period_end AS period_end, refreshed_at AS refreshed_at, \
                    total_cost_amount AS total_cost_amount \
             FROM spend_leaderboard_periods WHERE period = ? AND period_start = ?",
        )
        .bind(period)
        .bind(SqliteDate(filter.period_start))
        .fetch_optional(&mut *transaction)
        .await?;
        let rows = sqlx::query_as::<_, SpendLeaderboardRow>(
            "SELECT entry.rank AS rank, entry.user_id AS user_id, \
                    account_user.display_name AS display_name, \
                    entry.request_count AS request_count, \
                    entry.priced_request_count AS priced_request_count, \
                    entry.total_tokens AS total_tokens, entry.cost_amount AS cost_amount \
             FROM spend_leaderboard_entries AS entry \
             JOIN users AS account_user ON account_user.id = entry.user_id \
             WHERE entry.period = ? AND entry.period_start = ? \
             ORDER BY entry.rank LIMIT ?",
        )
        .bind(period)
        .bind(SqliteDate(filter.period_start))
        .bind(filter.limit)
        .fetch_all(&mut *transaction)
        .await?;
        let previous_period_start = sqlx::query_scalar::<_, Option<SqliteDate>>(
            "SELECT max(period_start) FROM spend_leaderboard_periods \
             WHERE period = ? AND period_start < ?",
        )
        .bind(period)
        .bind(SqliteDate(filter.period_start))
        .fetch_one(&mut *transaction)
        .await?;
        let next_period_start = sqlx::query_scalar::<_, Option<SqliteDate>>(
            "SELECT min(period_start) FROM spend_leaderboard_periods \
             WHERE period = ? AND period_start > ?",
        )
        .bind(period)
        .bind(SqliteDate(filter.period_start))
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(SpendLeaderboardReport {
            period: period.into(),
            period_start: filter.period_start,
            period_end: snapshot.as_ref().map_or_else(
                || filter.period.end_after(filter.period_start),
                |row| row.period_end.0,
            ),
            refreshed_at: snapshot.as_ref().map(|row| row.refreshed_at.0),
            total_cost_amount: snapshot
                .as_ref()
                .map_or(Decimal::ZERO, |row| row.total_cost_amount.0),
            previous_period_start: previous_period_start.map(|value| value.0),
            next_period_start: next_period_start.map(|value| value.0),
            entries: rows
                .into_iter()
                .map(|row| SpendLeaderboardEntry {
                    rank: row.rank,
                    user_id: row.user_id.0,
                    display_name: row.display_name,
                    request_count: row.request_count,
                    priced_request_count: row.priced_request_count,
                    total_tokens: row.total_tokens,
                    cost_amount: row.cost_amount.0,
                })
                .collect(),
        })
    }

    /// Reads the exact completed cost of at most 1000 request UUIDs.
    ///
    /// Only `priced` and `zero_by_policy` facts carry a settlement cost; the
    /// sharing WAL consumes this one-way evidence read and never touches the
    /// ordinary balance.
    pub async fn sharing_completed_costs(
        &self,
        ids: &[Uuid],
    ) -> Result<Vec<(Uuid, Decimal)>, RepositoryError> {
        if ids.len() > 1000 {
            return Err(RepositoryError::Validation);
        }
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let rows = sqlx::query_as::<_, (SqliteUuid, SqliteAmount)>(
            "SELECT id, cost_amount FROM request_metering_facts \
             WHERE id IN (SELECT value FROM json_each(?)) \
               AND amount_state IN ('priced','zero_by_policy')",
        )
        .bind(uuid_array(ids))
        .fetch_all(&mut *connection)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, cost_amount)| (id.0, cost_amount.0))
            .collect())
    }
}

#[derive(FromRow)]
struct CostFactRow {
    started_at: SqliteTimestamp,
    api_format: String,
    model: String,
    outcome: String,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cost_amount: Option<SqliteAmount>,
    channel_id: Option<SqliteUuid>,
    channel_group_id: Option<SqliteUuid>,
    channel_name: Option<String>,
    channel_group_name: Option<String>,
}

/// PostgreSQL computes `COALESCE(input_tokens,0) + COALESCE(output_tokens,0)`
/// with `bigint` arithmetic; a sum outside the column range must fail instead of
/// wrapping.
fn fact_total_tokens(row: &CostFactRow) -> Result<i64, RepositoryError> {
    row.input_tokens
        .unwrap_or(0)
        .checked_add(row.output_tokens.unwrap_or(0))
        .ok_or(RepositoryError::Validation)
}

/// Sums `cost_amount` only when the row is priced, matching `count(cost_amount)`
/// and `COALESCE(sum(cost_amount),0)`.
fn add_cost(target: &mut CostSum, row: &CostFactRow) -> Result<bool, RepositoryError> {
    let Some(cost_amount) = row.cost_amount else {
        return Ok(false);
    };
    target.add(cost_amount.0);
    Ok(true)
}

fn add_usage(
    row: &CostFactRow,
    total_tokens: &mut i64,
    input_tokens: &mut i64,
    cached_input_tokens: &mut i64,
    cache_write_tokens: &mut i64,
    output_tokens: &mut i64,
) -> Result<(), RepositoryError> {
    checked_add_count(total_tokens, fact_total_tokens(row)?)?;
    for (target, value) in [
        (input_tokens, row.input_tokens),
        (cached_input_tokens, row.cached_input_tokens),
        (cache_write_tokens, row.cache_write_tokens),
        (output_tokens, row.output_tokens),
    ] {
        if let Some(value) = value {
            checked_add_count(target, value)?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct CostSummaryAccumulator {
    request_count: i64,
    priced_request_count: i64,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: CostSum,
}

impl CostSummaryAccumulator {
    fn add(&mut self, row: &CostFactRow) -> Result<(), RepositoryError> {
        checked_add_count(&mut self.request_count, 1)?;
        if add_cost(&mut self.cost_amount, row)? {
            checked_add_count(&mut self.priced_request_count, 1)?;
        }
        add_usage(
            row,
            &mut self.total_tokens,
            &mut self.input_tokens,
            &mut self.cached_input_tokens,
            &mut self.cache_write_tokens,
            &mut self.output_tokens,
        )
    }
}

#[derive(Default)]
struct CostAccumulator {
    request_count: i64,
    total_tokens: i64,
    cost_amount: CostSum,
}

impl CostAccumulator {
    fn add(&mut self, row: &CostFactRow) -> Result<(), RepositoryError> {
        checked_add_count(&mut self.request_count, 1)?;
        add_cost(&mut self.cost_amount, row)?;
        checked_add_count(&mut self.total_tokens, fact_total_tokens(row)?)
    }
}

#[derive(Default)]
struct CostModelAccumulator {
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: CostSum,
}

impl CostModelAccumulator {
    fn add(&mut self, row: &CostFactRow) -> Result<(), RepositoryError> {
        checked_add_count(&mut self.request_count, 1)?;
        if row.outcome != "cancelled" {
            checked_add_count(&mut self.success_rate_request_count, 1)?;
        }
        if row.outcome == "succeeded" {
            checked_add_count(&mut self.succeeded_count, 1)?;
        }
        add_cost(&mut self.cost_amount, row)?;
        add_usage(
            row,
            &mut self.total_tokens,
            &mut self.input_tokens,
            &mut self.cached_input_tokens,
            &mut self.cache_write_tokens,
            &mut self.output_tokens,
        )
    }
}

#[derive(Default)]
struct CostChannelAccumulator {
    channel_group_name: String,
    name: String,
    api_format: String,
    request_count: i64,
    success_rate_request_count: i64,
    succeeded_count: i64,
    total_tokens: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cost_amount: CostSum,
    id: Uuid,
    channel_group_id: Uuid,
}

impl CostChannelAccumulator {
    /// The PostgreSQL channel query inner-joins `channels` and
    /// `channel_groups`, so a fact whose attribution cannot be resolved is
    /// excluded from the channel breakdown. Callers select only rows with both
    /// ids present and the join supplies the names.
    fn add(&mut self, row: &CostFactRow) -> Result<(), RepositoryError> {
        let (Some(channel_id), Some(channel_group_id)) = (row.channel_id, row.channel_group_id)
        else {
            return Ok(());
        };
        let (Some(name), Some(group_name)) =
            (row.channel_name.as_ref(), row.channel_group_name.as_ref())
        else {
            return Ok(());
        };
        self.id = channel_id.0;
        self.channel_group_id = channel_group_id.0;
        self.name.clone_from(name);
        self.channel_group_name.clone_from(group_name);
        self.api_format.clone_from(&row.api_format);
        checked_add_count(&mut self.request_count, 1)?;
        if row.outcome != "cancelled" {
            checked_add_count(&mut self.success_rate_request_count, 1)?;
        }
        if row.outcome == "succeeded" {
            checked_add_count(&mut self.succeeded_count, 1)?;
        }
        add_cost(&mut self.cost_amount, row)?;
        add_usage(
            row,
            &mut self.total_tokens,
            &mut self.input_tokens,
            &mut self.cached_input_tokens,
            &mut self.cache_write_tokens,
            &mut self.output_tokens,
        )
    }

    fn finish(self) -> Result<CostStatisticsChannel, RepositoryError> {
        Ok(CostStatisticsChannel {
            id: self.id,
            channel_group_id: self.channel_group_id,
            channel_group_name: self.channel_group_name,
            name: self.name,
            api_format: self.api_format,
            request_count: self.request_count,
            total_tokens: self.total_tokens,
            input_tokens: self.input_tokens,
            cached_input_tokens: self.cached_input_tokens,
            cache_write_tokens: self.cache_write_tokens,
            output_tokens: self.output_tokens,
            success_rate: success_rate(self.success_rate_request_count, self.succeeded_count),
            cost_amount: self.cost_amount.finish()?,
        })
    }
}

#[derive(FromRow)]
struct SpendLeaderboardPeriodRow {
    period_end: SqliteDate,
    refreshed_at: SqliteTimestamp,
    total_cost_amount: SqliteAmount,
}

#[derive(FromRow)]
struct SpendLeaderboardRow {
    rank: i64,
    user_id: SqliteUuid,
    display_name: String,
    request_count: i64,
    priced_request_count: i64,
    total_tokens: i64,
    cost_amount: SqliteAmount,
}
