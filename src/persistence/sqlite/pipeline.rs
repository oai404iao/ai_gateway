//! SQLite durable request-log write pipeline: low-index ingress, immutable
//! metering facts, the independent query-log projection, and single-transaction
//! settlement.
//!
//! The PostgreSQL implementations stay the behavioural contract; this module
//! reproduces their outcomes on the database's single `BEGIN IMMEDIATE` writer,
//! where no concurrent claim exists to arbitrate and no row-lock ordering has to
//! be reproduced:
//!
//! * facts, their settlement work items, and the ingress metering marker commit
//!   in one transaction, and only newly created facts create work;
//! * a settlement receipt is the sole claim; balance/quota writes and pending
//!   removal happen in the same transaction and only for receipts created by
//!   that transaction;
//! * replaying an identical event is accepted, replaying different financial
//!   content conflicts and never overwrites, and the query projection never owns
//!   settlement.
//!
//! Amounts stay exact `Decimal` values bound through the S2 column adapters.
//! No aggregate, balance, or quota calculation uses floats, SQLite numeric
//! affinity, or SQL `SUM`; account arithmetic is checked before the write and
//! the schema still enforces column precision, so an out-of-range result aborts
//! the whole settlement transaction instead of silently rounding.
//!
//! Row mapping stays backend-private: shared neutral DTOs are converted from
//! SQLite-specific row structs, and `SqliteUuid`/`SqliteTimestamp`/`SqliteAmount`
//! restore the canonical encodings PostgreSQL's driver would produce.

use std::{collections::BTreeMap, collections::HashMap, collections::HashSet, sync::Arc};

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Map, Value, json};
use sqlx::{FromRow, QueryBuilder, Sqlite, Transaction};
use uuid::Uuid;

use crate::{
    domain::RequestLogEvent,
    persistence::{
        IngestReceipt, MeteringReconciliationCounts, MeteringWriteOutcome, RepositoryError,
        RequestLogBatchInsertOutcome, RequestLogBatchInsertResult, RequestLogIngestBacklog,
        RequestLogIngestRecord, RequestLogInsertOutcome, RequestLogPoolStatus,
        RequestLogSettlementBacklog, RequestLogSettlementOutcome,
    },
    request_log_journal::EncodedRequestLog,
};

use super::{
    SqliteAmount, SqliteDatabase, SqliteOpenError, SqliteTimestamp, SqliteTokenRate,
    SqliteUnitPrice, SqliteUuid,
};

/// Column order is derived from one field list so the inserted columns and the
/// `json_extract` expressions can never drift apart.
const FACT_FIELDS: [&str; 30] = [
    "id",
    "started_at",
    "completed_at",
    "user_id",
    "api_key_id",
    "request_source",
    "api_format",
    "api_operation",
    "request_protocol",
    "client_model",
    "upstream_model",
    "model_rule_id",
    "channel_group_id",
    "channel_id",
    "model_id",
    "outcome",
    "input_tokens",
    "cached_input_tokens",
    "cache_write_tokens",
    "output_tokens",
    "reasoning_tokens",
    "currency",
    "price_unit_tokens",
    "price_effective_at",
    "input_unit_price",
    "cached_input_unit_price",
    "cache_write_unit_price",
    "output_unit_price",
    "cost_amount",
    "peak_pricing",
];

const LOG_FIELDS: [&str; 39] = [
    "id",
    "started_at",
    "completed_at",
    "user_id",
    "api_key_id",
    "request_source",
    "api_format",
    "api_operation",
    "request_protocol",
    "client_model",
    "upstream_model",
    "model_rule_id",
    "channel_group_id",
    "channel_id",
    "outcome",
    "response_status_code",
    "streamed",
    "ttft_ms",
    "total_duration_ms",
    "output_tokens_per_second",
    "input_tokens",
    "cached_input_tokens",
    "cache_write_tokens",
    "output_tokens",
    "model_id",
    "currency",
    "price_unit_tokens",
    "price_effective_at",
    "input_unit_price",
    "cached_input_unit_price",
    "cache_write_unit_price",
    "output_unit_price",
    "cost_amount",
    "error_code",
    "error_summary",
    "reasoning_tokens",
    "reasoning_effort",
    "fast_mode",
    "peak_pricing",
];

/// SQLite has no array parameters and a build-dependent `SQLITE_MAX_VARIABLE_NUMBER`
/// (the minimum any build may assume is 999). Multi-row statements are split into
/// chunks that stay far below that minimum; one write transaction keeps each call
/// all-or-nothing the way PostgreSQL's single `= ANY($1)` statement is.
const BIND_CHUNK: usize = 300;

fn columns(fields: &[&str]) -> String {
    fields.join(",")
}

fn values(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| format!("json_extract(entry.value,'$.{field}')"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds one upsert statement whose input rows travel as a single JSON array.
/// `WHERE TRUE` removes the parser ambiguity SQLite documents for
/// `INSERT ... SELECT ... ON CONFLICT`, and `ORDER BY ordinal` preserves the
/// caller's input order for duplicate ids inside one batch. The primary-key
/// conflict target means a replayed row inserts nothing and therefore is never
/// charged, projected, or acknowledged twice.
fn batch_insert(table: &str, fields: &[&str]) -> String {
    format!(
        "INSERT INTO {table} ({columns}) SELECT {values} FROM json_each(?) AS entry WHERE TRUE \
         ORDER BY json_extract(entry.value,'$.ordinal') ON CONFLICT(id) DO NOTHING \
         RETURNING id",
        columns = columns(fields),
        values = values(fields),
    )
}

/// Oldest-first recovery scan over outstanding work only. It is driven by
/// `request_settlement_pending_order_idx`, so settled history never expands the
/// scan; the eligible-key join skips account-mismatch rows without touching the
/// fact table for work that is not yet claimable.
const PENDING_SCAN: &str = "SELECT fact.id
     FROM request_settlement_pending AS pending
     JOIN request_metering_facts AS fact ON fact.id=pending.request_id
     JOIN api_keys AS key ON key.id = fact.api_key_id AND key.user_id = fact.user_id
     WHERE fact.amount_state IN ('priced','zero_by_policy')
     ORDER BY pending.completed_at, pending.request_id
     LIMIT ?";

const FACT_TABLE: &str = "request_metering_facts";
const LOG_TABLE: &str = "request_logs";

/// The PostgreSQL implementations record retry deadlines as `now() + interval`,
/// which keeps microsecond precision. SQLite's date functions only format whole
/// seconds, so the deadline is derived from the transaction clock (`ag_now()`)
/// and written back as a canonical microsecond timestamp instead of truncating.
async fn retry_deadline(
    transaction: &mut Transaction<'_, Sqlite>,
    retry_seconds: i64,
) -> Result<String, RepositoryError> {
    let now: SqliteTimestamp = sqlx::query_scalar("SELECT ag_now()")
        .fetch_one(&mut **transaction)
        .await?;
    let delay =
        chrono::Duration::try_seconds(retry_seconds.max(1)).ok_or(RepositoryError::Validation)?;
    let deadline = now
        .0
        .checked_add_signed(delay)
        .ok_or(RepositoryError::Validation)?;
    Ok(super::types::timestamp(deadline))
}

fn uuid_array(ids: &[Uuid]) -> String {
    Value::Array(ids.iter().map(|id| json!(id.to_string())).collect()).to_string()
}

fn push_receipt_list(builder: &mut QueryBuilder<Sqlite>, receipts: &[IngestReceipt]) {
    let mut separated = builder.separated(", ");
    for receipt in receipts {
        separated.push_bind(*receipt);
    }
}

fn open_failure(error: SqliteOpenError) -> RepositoryError {
    RepositoryError::from(sqlx::Error::Configuration(Box::new(error)))
}

/// Quantizes exactly like the PostgreSQL column type would (`numeric(p,s)`
/// rounds half away from zero) and rejects a value the column cannot hold, so
/// an event that would overflow is an error instead of an approximation.
fn column_text(value: Decimal, scale: u32) -> Result<String, RepositoryError> {
    let quantized = value.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
    let validated = match scale {
        4 => SqliteTokenRate::new(quantized).err(),
        8 => SqliteAmount::new(quantized).err(),
        12 => SqliteUnitPrice::new(quantized).err(),
        _ => Some("unsupported SQLite decimal scale".into()),
    };
    validated.map_or_else(
        || Ok(quantized.normalize().to_string()),
        |_| Err(RepositoryError::Validation),
    )
}

fn amount_text(value: Decimal) -> Result<String, RepositoryError> {
    column_text(value, 8)
}

fn price_text(value: Decimal) -> Result<String, RepositoryError> {
    column_text(value, 12)
}

fn rate_text(value: Decimal) -> Result<String, RepositoryError> {
    column_text(value, 4)
}

/// Mirrors the SQLite timestamp adapter so duplicate comparison observes exactly
/// the precision PostgreSQL's microsecond timestamp binding persists.
fn normalize_timestamp(value: DateTime<Utc>) -> DateTime<Utc> {
    super::types::parse_timestamp(&super::types::timestamp(value)).unwrap_or(value)
}

/// Durable ingress plus the idempotent query-log projection.
#[derive(Clone)]
pub struct SqliteRequestLogRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteRequestLogRepository {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    #[must_use]
    pub fn metering(&self) -> SqliteMeteringRepository {
        SqliteMeteringRepository::new(Arc::clone(&self.database))
    }

    #[must_use]
    pub fn settlements(&self) -> SqliteSettlementRepository {
        SqliteSettlementRepository::new(Arc::clone(&self.database))
    }

    /// Inserts one terminal event without changing schema-owned defaults.
    ///
    /// A duplicate id is successful only if every field owned by this event is
    /// identical after microsecond timestamp normalization.
    pub async fn insert(
        &self,
        event: &RequestLogEvent,
    ) -> Result<RequestLogInsertOutcome, RepositoryError> {
        let result = self
            .insert_batch(std::slice::from_ref(event))
            .await?
            .into_iter()
            .next()
            .expect("one input event produces one batch result");
        match result.outcome {
            RequestLogBatchInsertOutcome::Inserted => Ok(RequestLogInsertOutcome::Inserted),
            RequestLogBatchInsertOutcome::ExactDuplicate => {
                Ok(RequestLogInsertOutcome::ExactDuplicate)
            }
            RequestLogBatchInsertOutcome::DuplicateConflict => {
                Err(RepositoryError::DuplicateConflict { id: event.id })
            }
            RequestLogBatchInsertOutcome::InvalidResponseStatus { status } => {
                Err(RepositoryError::InvalidResponseStatus { status })
            }
        }
    }

    /// Persists financial facts before attempting the independent log projection.
    /// A projection error never rolls back already accepted financial evidence.
    pub async fn insert_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<RequestLogBatchInsertResult>, RepositoryError> {
        let accepted = self.metering().record_batch(events).await?;
        let projectable = events
            .iter()
            .zip(&accepted)
            .filter_map(|(event, outcome)| {
                (*outcome == MeteringWriteOutcome::Accepted).then_some(event.clone())
            })
            .collect::<Vec<_>>();
        let mut projected = self.project_batch(&projectable).await?.into_iter();
        Ok(events
            .iter()
            .zip(accepted)
            .map(|(event, outcome)| match outcome {
                MeteringWriteOutcome::Accepted => projected
                    .next()
                    .expect("projection preserves input cardinality"),
                MeteringWriteOutcome::Conflict => RequestLogBatchInsertResult {
                    request_log_id: event.id,
                    outcome: RequestLogBatchInsertOutcome::DuplicateConflict,
                },
            })
            .collect())
    }

    /// Projects events whose independent financial facts are already durable.
    ///
    /// Per-event validation and duplicate classification stay isolated so one
    /// malformed status or conflicting duplicate does not hide valid peers.
    pub(crate) async fn project_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<RequestLogBatchInsertResult>, RepositoryError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        let mut outcomes = vec![None; events.len()];
        let mut valid = Vec::with_capacity(events.len());
        let mut first_valid_index = HashMap::<Uuid, usize>::with_capacity(events.len());
        for (index, event) in events.iter().enumerate() {
            let status = match event
                .response_status_code
                .map(validate_response_status)
                .transpose()
            {
                Ok(status) => status,
                Err(RepositoryError::InvalidResponseStatus { status }) => {
                    outcomes[index] =
                        Some(RequestLogBatchInsertOutcome::InvalidResponseStatus { status });
                    continue;
                }
                Err(error) => return Err(error),
            };
            first_valid_index.entry(event.id).or_insert(index);
            valid.push((index, status));
        }

        if !valid.is_empty() {
            let entries = valid
                .iter()
                .enumerate()
                .map(|(ordinal, (index, status))| {
                    log_object(&events[*index], *status, ordinal)
                        .map(|entry| (events[*index].id, entry))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let carrier = Value::Array(
                entries
                    .iter()
                    .map(|(_, entry)| Value::Object(entry.clone()))
                    .collect(),
            )
            .to_string();

            let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
            let inserted = sqlx::query_scalar::<_, SqliteUuid>(sqlx::AssertSqlSafe(batch_insert(
                LOG_TABLE,
                &LOG_FIELDS,
            )))
            .bind(&carrier)
            .fetch_all(&mut *transaction)
            .await?
            .into_iter()
            .map(|id| id.0)
            .collect::<HashSet<_>>();

            let mut needs_existing = HashSet::new();
            for (index, _) in &valid {
                let event = &events[*index];
                if inserted.contains(&event.id) && first_valid_index.get(&event.id) == Some(index) {
                    outcomes[*index] = Some(RequestLogBatchInsertOutcome::Inserted);
                } else {
                    needs_existing.insert(event.id);
                }
            }

            if !needs_existing.is_empty() {
                let ids = needs_existing.iter().copied().collect::<Vec<_>>();
                let stored = sqlx::query_as::<_, RequestLogRow>(
                    "SELECT id,started_at,completed_at,user_id,api_key_id,request_source,api_format,\
                            api_operation,request_protocol,client_model,upstream_model,model_rule_id,\
                            channel_group_id,channel_id,outcome,response_status_code,streamed,ttft_ms,\
                            total_duration_ms,output_tokens_per_second,input_tokens,cached_input_tokens,\
                            cache_write_tokens,output_tokens,reasoning_tokens,model_id,currency,\
                            price_unit_tokens,price_effective_at,input_unit_price,\
                            cached_input_unit_price,cache_write_unit_price,output_unit_price,\
                            cost_amount,error_code,error_summary,reasoning_effort,fast_mode,peak_pricing \
                     FROM request_logs WHERE id IN (SELECT value FROM json_each(?))",
                )
                .bind(uuid_array(&ids))
                .fetch_all(&mut *transaction)
                .await?
                .into_iter()
                .map(|row| (row.id.0, row))
                .collect::<HashMap<_, _>>();

                for (index, status) in &valid {
                    if outcomes[*index].is_some() {
                        continue;
                    }
                    let event = &events[*index];
                    let stored = stored
                        .get(&event.id)
                        .ok_or(RepositoryError::DuplicateDisappeared { id: event.id })?;
                    outcomes[*index] = Some(if stored.matches(event, *status) {
                        RequestLogBatchInsertOutcome::ExactDuplicate
                    } else {
                        RequestLogBatchInsertOutcome::DuplicateConflict
                    });
                }
            }
            transaction.commit().await?;
        }

        Ok(events
            .iter()
            .zip(outcomes)
            .map(|(event, outcome)| RequestLogBatchInsertResult {
                request_log_id: event.id,
                outcome: outcome.expect("every batch input receives an outcome"),
            })
            .collect())
    }

    /// Appends encoded terminal events to the low-index durable ingress table.
    ///
    /// Duplicates are intentional: a checkpoint replay may re-send rows, while
    /// the final `request_logs` primary key remains the idempotency boundary.
    pub(crate) async fn accept_batch(
        &self,
        records: &[EncodedRequestLog],
    ) -> Result<u64, RepositoryError> {
        if records.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let mut accepted = 0u64;
        for chunk in records.chunks(BIND_CHUNK) {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "INSERT INTO request_log_ingest(request_log_id,schema_version,payload) ",
            );
            builder.push_values(chunk, |mut row, record| {
                row.push_bind(SqliteUuid(record.request_log_id))
                    .push_bind(record.schema_version)
                    .push_bind(record.payload.clone());
            });
            accepted += builder
                .build()
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        }
        transaction.commit().await?;
        Ok(accepted)
    }

    /// Oldest-first slice of ingress rows that still need financial facts.
    pub(crate) async fn load_ingest_batch(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let rows = sqlx::query_as::<_, IngestRecordRow>(
            "SELECT sequence,request_log_id,schema_version,payload,attempt_count \
             FROM request_log_ingest \
             WHERE metered_at IS NOT NULL AND next_attempt_at <= ag_now() \
             ORDER BY sequence LIMIT ?",
        )
        .bind(limit.max(1))
        .fetch_all(&mut *connection)
        .await?;
        Ok(rows.into_iter().map(IngestRecordRow::into_record).collect())
    }

    /// Removes projected ingress rows. The schema's ack guard rejects any row
    /// whose financial fact or query projection is still missing.
    pub(crate) async fn acknowledge_ingest(
        &self,
        sequences: &[IngestReceipt],
    ) -> Result<u64, RepositoryError> {
        if sequences.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let mut acknowledged = 0u64;
        for chunk in sequences.chunks(BIND_CHUNK) {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "DELETE FROM request_log_ingest WHERE metered_at IS NOT NULL AND sequence IN (",
            );
            push_receipt_list(&mut builder, chunk);
            builder.push(")");
            acknowledged += builder
                .build()
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        }
        transaction.commit().await?;
        Ok(acknowledged)
    }

    /// Reschedules failed projection work with its independent retry budget.
    pub(crate) async fn defer_ingest(
        &self,
        sequences: &[IngestReceipt],
        error_code: &str,
        retry_after_seconds: i64,
    ) -> Result<u64, RepositoryError> {
        if sequences.is_empty() {
            return Ok(0);
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let deadline = retry_deadline(&mut transaction, retry_after_seconds).await?;
        let mut deferred = 0u64;
        for chunk in sequences.chunks(BIND_CHUNK) {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "UPDATE request_log_ingest SET attempt_count=attempt_count+1, next_attempt_at=",
            );
            builder
                .push_bind(deadline.clone())
                .push(", last_error_code=")
                .push_bind(error_code.to_owned())
                .push(" WHERE sequence IN (");
            push_receipt_list(&mut builder, chunk);
            builder.push(")");
            deferred += builder
                .build()
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        }
        transaction.commit().await?;
        Ok(deferred)
    }

    pub(crate) async fn ingest_backlog(&self) -> Result<RequestLogIngestBacklog, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let row = sqlx::query_as::<_, IngestBacklogRow>(
            "SELECT COALESCE(
                 (SELECT sequence FROM request_log_ingest ORDER BY sequence DESC LIMIT 1)
                 - (SELECT sequence FROM request_log_ingest ORDER BY sequence LIMIT 1) + 1,
                 0
             ) AS row_count,
             (SELECT staged_at FROM request_log_ingest ORDER BY sequence LIMIT 1) AS oldest_staged_at",
        )
        .fetch_one(&mut *connection)
        .await?;
        Ok(RequestLogIngestBacklog {
            row_count: row.row_count,
            oldest_staged_at: row.oldest_staged_at.map(|value| value.0),
        })
    }

    /// SQLite shares both database pools with the control plane rather than owning a log pool.
    pub(crate) fn pool_status(&self) -> RequestLogPoolStatus {
        let Ok(pools) = self.database.pools() else {
            return RequestLogPoolStatus {
                size: 0,
                idle: 0,
                capacity: 0,
            };
        };
        RequestLogPoolStatus {
            size: pools.writer.size().saturating_add(pools.readers.size()),
            idle: pools
                .writer
                .num_idle()
                .saturating_add(pools.readers.num_idle()),
            capacity: pools
                .writer
                .options()
                .get_max_connections()
                .saturating_add(pools.readers.options().get_max_connections()),
        }
    }
}

/// Immutable financial facts, independent of the request-log projection.
#[derive(Clone)]
pub struct SqliteMeteringRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteMeteringRepository {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    pub async fn reconciliation_counts(
        &self,
    ) -> Result<MeteringReconciliationCounts, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        Ok(sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM request_metering_facts WHERE amount_state='unknown') AS unknown,
                (SELECT count(*) FROM request_metering_facts WHERE amount_state='invalid') AS invalid,
                (SELECT count(*) FROM request_settlement_pending AS pending
                 JOIN request_metering_facts AS fact ON fact.id=pending.request_id
                 LEFT JOIN api_keys AS key ON key.id=fact.api_key_id
                 WHERE key.user_id IS NOT fact.user_id) AS account_mismatch",
        )
        .fetch_one(&mut *connection)
        .await?)
    }

    pub async fn record_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let outcomes = write_facts(&mut transaction, events).await?;
        transaction.commit().await?;
        Ok(outcomes)
    }

    pub(crate) async fn load_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let rows = sqlx::query_as::<_, IngestRecordRow>(
            "SELECT sequence,request_log_id,schema_version,payload, \
                    metering_attempt_count AS attempt_count \
             FROM request_log_ingest \
             WHERE metered_at IS NULL AND metering_next_attempt_at <= ag_now() \
             ORDER BY sequence LIMIT ?",
        )
        .bind(limit.max(1))
        .fetch_all(&mut *connection)
        .await?;
        Ok(rows.into_iter().map(IngestRecordRow::into_record).collect())
    }

    pub(crate) async fn materialize(
        &self,
        receipts: &[IngestReceipt],
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        if receipts.len() != events.len() {
            return Err(RepositoryError::Validation);
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let outcomes = write_facts(&mut transaction, events).await?;
        let accepted = receipts
            .iter()
            .zip(&outcomes)
            .filter_map(|(receipt, outcome)| {
                (*outcome == MeteringWriteOutcome::Accepted).then_some(*receipt)
            })
            .collect::<Vec<_>>();
        for chunk in accepted.chunks(BIND_CHUNK) {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "UPDATE request_log_ingest SET metered_at=ag_now() \
                 WHERE metered_at IS NULL AND sequence IN (",
            );
            push_receipt_list(&mut builder, chunk);
            builder.push(")");
            builder.build().execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(outcomes)
    }

    pub(crate) async fn defer(
        &self,
        receipts: &[IngestReceipt],
        code: &str,
        retry_seconds: i64,
    ) -> Result<(), RepositoryError> {
        if receipts.is_empty() {
            return Ok(());
        }
        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let deadline = retry_deadline(&mut transaction, retry_seconds).await?;
        for chunk in receipts.chunks(BIND_CHUNK) {
            let mut builder = QueryBuilder::<Sqlite>::new(
                "UPDATE request_log_ingest SET metering_attempt_count=metering_attempt_count+1, \
                 metering_next_attempt_at=",
            );
            builder
                .push_bind(deadline.clone())
                .push(", metering_last_error_code=")
                .push_bind(code.to_owned())
                .push(" WHERE metered_at IS NULL AND sequence IN (");
            push_receipt_list(&mut builder, chunk);
            builder.push(")");
            builder.build().execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

/// Single-transaction settlement claims.
#[derive(Clone)]
pub struct SqliteSettlementRepository {
    database: Arc<SqliteDatabase>,
}

impl SqliteSettlementRepository {
    #[must_use]
    pub fn new(database: Arc<SqliteDatabase>) -> Self {
        Self { database }
    }

    /// Claims and applies one eligible financial fact in a single transaction.
    pub async fn settle(
        &self,
        request_log_id: Uuid,
    ) -> Result<RequestLogSettlementOutcome, RepositoryError> {
        Ok(self
            .settle_batch(&[request_log_id])
            .await?
            .into_iter()
            .next()
            .expect("one request-log id produces one settlement outcome")
            .1)
    }

    /// Claims and applies a set of billable facts with batched account updates
    /// in one transaction.
    ///
    /// Costs are aggregated per user and API key before those account rows are
    /// updated, and only receipts created by this transaction may move money or
    /// remove pending work. The returned vector is deduplicated by request-log id
    /// while preserving first-seen input order.
    pub async fn settle_batch(
        &self,
        request_log_ids: &[Uuid],
    ) -> Result<Vec<(Uuid, RequestLogSettlementOutcome)>, RepositoryError> {
        let mut seen = HashSet::with_capacity(request_log_ids.len());
        let request_log_ids = request_log_ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect::<Vec<_>>();
        if request_log_ids.is_empty() {
            return Ok(Vec::new());
        }
        let carrier = uuid_array(&request_log_ids);

        let mut transaction = self.database.begin_write().await.map_err(open_failure)?;
        let claimed = sqlx::query_as::<_, ClaimedRow>(
            "INSERT INTO request_settlements(request_id,cost_amount,currency)
             SELECT fact.id,fact.cost_amount,fact.currency
             FROM request_metering_facts AS fact
             JOIN api_keys AS key ON key.id=fact.api_key_id AND key.user_id=fact.user_id
             WHERE fact.id IN (SELECT value FROM json_each(?))
               AND fact.amount_state IN ('priced','zero_by_policy')
               AND NOT EXISTS (
                   SELECT 1 FROM request_settlements AS receipt WHERE receipt.request_id=fact.id
               )
             ORDER BY fact.id
             ON CONFLICT(request_id) DO NOTHING
             RETURNING request_id, cost_amount",
        )
        .bind(&carrier)
        .fetch_all(&mut *transaction)
        .await?;

        let stored = sqlx::query_as::<_, SettlementFactRow>(
            "SELECT fact.id,fact.user_id,fact.api_key_id,fact.cost_amount,
                    receipt.settled_at,key.user_id AS api_key_user_id
             FROM request_metering_facts AS fact
             LEFT JOIN request_settlements AS receipt ON receipt.request_id=fact.id
             LEFT JOIN api_keys AS key ON key.id=fact.api_key_id
             WHERE fact.id IN (SELECT value FROM json_each(?))",
        )
        .bind(&carrier)
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| (row.id.0, row))
        .collect::<HashMap<_, _>>();

        let first_claim = claimed.first().map(|row| row.request_id.0);
        let mut user_costs = BTreeMap::<Uuid, Decimal>::new();
        let mut api_key_costs = BTreeMap::<(Uuid, Uuid), Decimal>::new();
        for row in &claimed {
            let fact = stored.get(&row.request_id.0).ok_or(
                RepositoryError::SettlementClaimInvalidated {
                    id: row.request_id.0,
                },
            )?;
            let user_total = user_costs.entry(fact.user_id.0).or_default();
            *user_total = user_total
                .checked_add(row.cost_amount.0)
                .ok_or(RepositoryError::Validation)?;
            let key_total = api_key_costs
                .entry((fact.api_key_id.0, fact.user_id.0))
                .or_default();
            *key_total = key_total
                .checked_add(row.cost_amount.0)
                .ok_or(RepositoryError::Validation)?;
        }

        let mut quota_by_api_key = HashMap::new();
        if !claimed.is_empty() {
            let claim_invalidated = || RepositoryError::SettlementClaimInvalidated {
                id: first_claim.expect("claimed work exists"),
            };
            // Ascending account ids keep the write order deterministic, matching
            // the PostgreSQL repository's lock ordering.
            for (user_id, cost) in &user_costs {
                let current = sqlx::query_scalar::<_, SqliteAmount>(
                    "SELECT balance_amount FROM users WHERE id=?",
                )
                .bind(SqliteUuid(*user_id))
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or_else(claim_invalidated)?;
                let next = current
                    .0
                    .checked_sub(*cost)
                    .ok_or(RepositoryError::Validation)?;
                let next = SqliteAmount::new(next).map_err(|_| RepositoryError::Validation)?;
                let updated = sqlx::query_scalar::<_, SqliteUuid>(
                    "UPDATE users SET balance_amount=?,updated_at=ag_now() \
                     WHERE id=? RETURNING id",
                )
                .bind(next)
                .bind(SqliteUuid(*user_id))
                .fetch_optional(&mut *transaction)
                .await?;
                if updated.is_none() {
                    return Err(claim_invalidated());
                }
            }
            for ((api_key_id, user_id), cost) in &api_key_costs {
                let current = sqlx::query_scalar::<_, SqliteAmount>(
                    "SELECT quota_used_amount FROM api_keys WHERE id=?",
                )
                .bind(SqliteUuid(*api_key_id))
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or_else(claim_invalidated)?;
                let next = current
                    .0
                    .checked_add(*cost)
                    .ok_or(RepositoryError::Validation)?;
                let next = SqliteAmount::new(next).map_err(|_| RepositoryError::Validation)?;
                let updated = sqlx::query_as::<_, UpdatedApiKeyRow>(
                    "UPDATE api_keys SET quota_used_amount=?,updated_at=ag_now() \
                     WHERE id=? AND user_id=? RETURNING quota_used_amount",
                )
                .bind(next)
                .bind(SqliteUuid(*api_key_id))
                .bind(SqliteUuid(*user_id))
                .fetch_optional(&mut *transaction)
                .await?
                .ok_or_else(claim_invalidated)?;
                quota_by_api_key.insert(*api_key_id, updated.quota_used_amount.0);
            }
        }

        let mut settled = HashMap::<Uuid, RequestLogSettlementOutcome>::new();
        for row in &claimed {
            let fact = stored.get(&row.request_id.0).ok_or(
                RepositoryError::SettlementClaimInvalidated {
                    id: row.request_id.0,
                },
            )?;
            let quota_used_amount = quota_by_api_key.get(&fact.api_key_id.0).copied().ok_or(
                RepositoryError::SettlementClaimInvalidated {
                    id: row.request_id.0,
                },
            )?;
            settled.insert(
                row.request_id.0,
                RequestLogSettlementOutcome::Settled {
                    request_log_id: row.request_id.0,
                    api_key_id: fact.api_key_id.0,
                    quota_used_amount,
                },
            );
        }

        let outcomes = request_log_ids
            .iter()
            .map(|id| {
                let outcome = settled.get(id).cloned().unwrap_or_else(|| {
                    stored
                        .get(id)
                        .map_or(RequestLogSettlementOutcome::NotFound, settlement_outcome)
                });
                (*id, outcome)
            })
            .collect::<Vec<_>>();

        if !claimed.is_empty() {
            let claimed = claimed
                .iter()
                .map(|row| row.request_id.0)
                .collect::<Vec<_>>();
            sqlx::query(
                "DELETE FROM request_settlement_pending \
                 WHERE request_id IN (SELECT value FROM json_each(?))",
            )
            .bind(uuid_array(&claimed))
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(outcomes)
    }

    /// Reconciles a bounded oldest-first slice of durable, eligible work.
    ///
    /// Unknown, invalid-price and account-mismatch facts stay for reconciliation
    /// instead of blocking eligible pending work; the id snapshot is read from a
    /// read snapshot like PostgreSQL's separate claim scan, and `settle_batch`
    /// re-validates every candidate under the write transaction.
    pub async fn settle_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogSettlementOutcome>, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let ids = sqlx::query_scalar::<_, SqliteUuid>(PENDING_SCAN)
            .bind(limit.max(1))
            .fetch_all(&mut *connection)
            .await?;
        drop(connection);
        Ok(self
            .settle_batch(&ids.into_iter().map(|id| id.0).collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect())
    }

    pub(crate) async fn settlement_backlog(
        &self,
    ) -> Result<RequestLogSettlementBacklog, RepositoryError> {
        let mut connection = self.database.acquire_read().await.map_err(open_failure)?;
        let row = sqlx::query_as::<_, SettlementBacklogRow>(
            "SELECT count(*) AS row_count, min(pending.completed_at) AS oldest_completed_at
             FROM request_settlement_pending AS pending
             JOIN request_metering_facts AS fact ON fact.id=pending.request_id
             JOIN api_keys AS key
               ON key.id = fact.api_key_id
              AND key.user_id = fact.user_id
             WHERE fact.amount_state IN ('priced','zero_by_policy')",
        )
        .fetch_one(&mut *connection)
        .await?;
        Ok(RequestLogSettlementBacklog {
            row_count: row.row_count,
            oldest_completed_at: row.oldest_completed_at.map(|value| value.0),
        })
    }
}

/// Writes immutable facts and queues work only for facts created by this
/// transaction.
///
/// Facts are inserted in input order and compared afterwards. A replayed event
/// must match the stored fact in every field except the derived `amount_state`;
/// a mismatch is reported as a conflict and never overwrites or double-charges.
/// Work items are created from the newly inserted ids only, so a replay cannot
/// resurrect work a committed settlement already removed.
async fn write_facts(
    transaction: &mut Transaction<'_, Sqlite>,
    events: &[RequestLogEvent],
) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let entries = events
        .iter()
        .enumerate()
        .map(|(ordinal, event)| fact_object(event, ordinal).map(|entry| (event.id, entry)))
        .collect::<Result<Vec<_>, _>>()?;

    let mut inserted = Vec::with_capacity(events.len());
    for chunk in entries.chunks(BIND_CHUNK) {
        let carrier = Value::Array(
            chunk
                .iter()
                .map(|(_, entry)| Value::Object(entry.clone()))
                .collect(),
        )
        .to_string();
        inserted.extend(
            sqlx::query_scalar::<_, SqliteUuid>(sqlx::AssertSqlSafe(batch_insert(
                FACT_TABLE,
                &FACT_FIELDS,
            )))
            .bind(&carrier)
            .fetch_all(&mut **transaction)
            .await?
            .into_iter()
            .map(|id| id.0),
        );
    }

    if !inserted.is_empty() {
        sqlx::query(
            "INSERT INTO request_settlement_pending(request_id,completed_at)
             SELECT id,completed_at FROM request_metering_facts
             WHERE id IN (SELECT value FROM json_each(?))
               AND amount_state IN ('priced','zero_by_policy')",
        )
        .bind(uuid_array(&inserted))
        .execute(&mut **transaction)
        .await?;
    }

    // Compare every input id, not only the newly inserted ones: a replay is the
    // normal reason why nothing was inserted, and its stored fact still has to be
    // compared. Decoded stored facts let the S2 column adapters decide equality
    // exactly the way PostgreSQL's typed comparisons do, including microsecond
    // timestamps and column scale.
    let mut compared_ids = Vec::with_capacity(events.len());
    let mut seen = HashSet::with_capacity(events.len());
    for event in events {
        if seen.insert(event.id) {
            compared_ids.push(event.id);
        }
    }
    let stored = sqlx::query_as::<_, FactRow>(
        "SELECT id,started_at,completed_at,user_id,api_key_id,request_source,api_format,\
                api_operation,request_protocol,client_model,upstream_model,model_rule_id,\
                channel_group_id,channel_id,model_id,outcome,input_tokens,cached_input_tokens,\
                cache_write_tokens,output_tokens,reasoning_tokens,currency,price_unit_tokens,\
                price_effective_at,input_unit_price,cached_input_unit_price,\
                cache_write_unit_price,output_unit_price,cost_amount,peak_pricing \
         FROM request_metering_facts \
         WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(uuid_array(&compared_ids))
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|row| (row.id.0, row))
    .collect::<HashMap<_, _>>();
    if stored.len() != compared_ids.len() {
        return Err(RepositoryError::Validation);
    }

    let mut outcomes = Vec::with_capacity(events.len());
    for event in events {
        let stored = stored
            .get(&event.id)
            .ok_or(RepositoryError::DuplicateDisappeared { id: event.id })?;
        outcomes.push(if stored.matches(event) {
            MeteringWriteOutcome::Accepted
        } else {
            MeteringWriteOutcome::Conflict
        });
    }
    Ok(outcomes)
}

/// Stored immutable fact used only for replay equality.
#[derive(FromRow)]
struct FactRow {
    id: SqliteUuid,
    started_at: SqliteTimestamp,
    completed_at: SqliteTimestamp,
    user_id: SqliteUuid,
    api_key_id: SqliteUuid,
    request_source: String,
    api_format: String,
    api_operation: String,
    request_protocol: String,
    client_model: String,
    upstream_model: Option<String>,
    model_rule_id: Option<SqliteUuid>,
    channel_group_id: Option<SqliteUuid>,
    channel_id: Option<SqliteUuid>,
    model_id: Option<SqliteUuid>,
    outcome: String,
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    currency: Option<String>,
    price_unit_tokens: Option<i64>,
    price_effective_at: Option<SqliteTimestamp>,
    input_unit_price: Option<SqliteUnitPrice>,
    cached_input_unit_price: Option<SqliteUnitPrice>,
    cache_write_unit_price: Option<SqliteUnitPrice>,
    output_unit_price: Option<SqliteUnitPrice>,
    cost_amount: Option<SqliteAmount>,
    peak_pricing: bool,
}

impl FactRow {
    /// An accepted replay must own every stored column except the derived
    /// `amount_state`; any difference preserves the original fact and reports a
    /// conflict rather than overwriting evidence.
    fn matches(&self, event: &RequestLogEvent) -> bool {
        let quantize = |value: Decimal, scale| {
            value.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero)
        };
        let billing = event.billing.as_ref();
        let usage = billing.and_then(|billing| billing.usage);
        let price = billing.map(|billing| &billing.price);
        self.started_at.0 == normalize_timestamp(event.started_at)
            && self.completed_at.0 == normalize_timestamp(event.completed_at)
            && self.user_id.0 == event.user_id
            && self.api_key_id.0 == event.api_key_id
            && self.request_source == event.request_source.as_str()
            && self.api_format == event.api_format.as_str()
            && self.api_operation == event.api_operation.as_str()
            && self.request_protocol == event.request_protocol.as_str()
            && self.client_model == event.client_model
            && self.upstream_model == event.upstream_model
            && self.model_rule_id.map(|value| value.0) == event.model_rule_id
            && self.channel_group_id.map(|value| value.0) == event.channel_group_id
            && self.channel_id.map(|value| value.0) == event.channel_id
            && self.model_id.map(|value| value.0) == event.model_id
            && self.outcome == event.outcome.as_str()
            && self.input_tokens == usage.map(|usage| usage.input_tokens)
            && self.cached_input_tokens == usage.map(|usage| usage.cached_input_tokens)
            && self.cache_write_tokens == usage.map(|usage| usage.cache_write_tokens)
            && self.output_tokens == usage.map(|usage| usage.output_tokens)
            && self.reasoning_tokens == usage.map(|usage| usage.reasoning_tokens)
            && self.currency == price.map(|price| price.currency.clone())
            && self.price_unit_tokens == price.map(|price| price.price_unit_tokens)
            && self.price_effective_at.map(|value| value.0)
                == price.map(|price| normalize_timestamp(price.price_effective_at))
            && self.input_unit_price.map(|value| value.0)
                == price.map(|price| quantize(price.input_unit_price, 12))
            && self.cached_input_unit_price.map(|value| value.0)
                == price.map(|price| quantize(price.cached_input_unit_price, 12))
            && self.cache_write_unit_price.map(|value| value.0)
                == price.map(|price| quantize(price.cache_write_unit_price, 12))
            && self.output_unit_price.map(|value| value.0)
                == price.map(|price| quantize(price.output_unit_price, 12))
            && self.cost_amount.map(|value| value.0)
                == event
                    .effective_cost_amount()
                    .map(|value| quantize(value, 8))
            && self.peak_pricing == billing.is_some_and(|billing| billing.peak_pricing)
    }
}

/// Encodes one event as the JSON object bound to `batch_insert`. Text that is
/// already column precision is emitted verbatim; only caller-supplied amounts
/// are quantized, and both paths stay inside the canonical decimal alphabet the
/// S2 decoder accepts.
fn fact_object(
    event: &RequestLogEvent,
    ordinal: usize,
) -> Result<Map<String, Value>, RepositoryError> {
    let billing = event.billing.as_ref();
    let usage = billing.and_then(|billing| billing.usage);
    let price = billing.map(|billing| &billing.price);
    let mut entry = Map::with_capacity(FACT_FIELDS.len() + 1);
    entry.insert("ordinal".into(), json!(ordinal as i64));
    entry.insert("id".into(), json!(event.id.to_string()));
    entry.insert(
        "started_at".into(),
        json!(super::types::timestamp(event.started_at)),
    );
    entry.insert(
        "completed_at".into(),
        json!(super::types::timestamp(event.completed_at)),
    );
    entry.insert("user_id".into(), json!(event.user_id.to_string()));
    entry.insert("api_key_id".into(), json!(event.api_key_id.to_string()));
    entry.insert(
        "request_source".into(),
        json!(event.request_source.as_str()),
    );
    entry.insert("api_format".into(), json!(event.api_format.as_str()));
    entry.insert("api_operation".into(), json!(event.api_operation.as_str()));
    entry.insert(
        "request_protocol".into(),
        json!(event.request_protocol.as_str()),
    );
    entry.insert("client_model".into(), json!(event.client_model));
    entry.insert("upstream_model".into(), json!(event.upstream_model));
    entry.insert(
        "model_rule_id".into(),
        json!(uuid_text(event.model_rule_id)),
    );
    entry.insert(
        "channel_group_id".into(),
        json!(uuid_text(event.channel_group_id)),
    );
    entry.insert("channel_id".into(), json!(uuid_text(event.channel_id)));
    entry.insert("model_id".into(), json!(uuid_text(event.model_id)));
    entry.insert("outcome".into(), json!(event.outcome.as_str()));
    entry.insert(
        "input_tokens".into(),
        json!(usage.map(|usage| usage.input_tokens)),
    );
    entry.insert(
        "cached_input_tokens".into(),
        json!(usage.map(|usage| usage.cached_input_tokens)),
    );
    entry.insert(
        "cache_write_tokens".into(),
        json!(usage.map(|usage| usage.cache_write_tokens)),
    );
    entry.insert(
        "output_tokens".into(),
        json!(usage.map(|usage| usage.output_tokens)),
    );
    entry.insert(
        "reasoning_tokens".into(),
        json!(usage.map(|usage| usage.reasoning_tokens)),
    );
    entry.insert("currency".into(), json!(price.map(|price| &price.currency)));
    entry.insert(
        "price_unit_tokens".into(),
        json!(price.map(|price| price.price_unit_tokens)),
    );
    entry.insert(
        "price_effective_at".into(),
        json!(price.map(|price| super::types::timestamp(price.price_effective_at))),
    );
    entry.insert(
        "input_unit_price".into(),
        price
            .map(|price| price_text(price.input_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cached_input_unit_price".into(),
        price
            .map(|price| price_text(price.cached_input_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cache_write_unit_price".into(),
        price
            .map(|price| price_text(price.cache_write_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "output_unit_price".into(),
        price
            .map(|price| price_text(price.output_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cost_amount".into(),
        event
            .effective_cost_amount()
            .map(amount_text)
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "peak_pricing".into(),
        json!(billing.is_some_and(|billing| billing.peak_pricing)),
    );
    Ok(entry)
}

/// Encodes one event as the JSON object bound to `batch_insert` for `request_logs`.
fn log_object(
    event: &RequestLogEvent,
    response_status_code: Option<i16>,
    ordinal: usize,
) -> Result<Map<String, Value>, RepositoryError> {
    let billing = event.billing.as_ref();
    let usage = billing.and_then(|billing| billing.usage);
    let price = billing.map(|billing| &billing.price);
    let mut entry = Map::with_capacity(LOG_FIELDS.len() + 1);
    entry.insert("ordinal".into(), json!(ordinal as i64));
    entry.insert("id".into(), json!(event.id.to_string()));
    entry.insert(
        "started_at".into(),
        json!(super::types::timestamp(event.started_at)),
    );
    entry.insert(
        "completed_at".into(),
        json!(super::types::timestamp(event.completed_at)),
    );
    entry.insert("user_id".into(), json!(event.user_id.to_string()));
    entry.insert("api_key_id".into(), json!(event.api_key_id.to_string()));
    entry.insert(
        "request_source".into(),
        json!(event.request_source.as_str()),
    );
    entry.insert("api_format".into(), json!(event.api_format.as_str()));
    entry.insert("api_operation".into(), json!(event.api_operation.as_str()));
    entry.insert(
        "request_protocol".into(),
        json!(event.request_protocol.as_str()),
    );
    entry.insert("client_model".into(), json!(event.client_model));
    entry.insert("upstream_model".into(), json!(event.upstream_model));
    entry.insert(
        "model_rule_id".into(),
        json!(uuid_text(event.model_rule_id)),
    );
    entry.insert(
        "channel_group_id".into(),
        json!(uuid_text(event.channel_group_id)),
    );
    entry.insert("channel_id".into(), json!(uuid_text(event.channel_id)));
    entry.insert("outcome".into(), json!(event.outcome.as_str()));
    entry.insert("response_status_code".into(), json!(response_status_code));
    entry.insert("streamed".into(), json!(event.streamed));
    entry.insert("ttft_ms".into(), json!(event.ttft_ms));
    entry.insert("total_duration_ms".into(), json!(event.total_duration_ms));
    entry.insert(
        "output_tokens_per_second".into(),
        billing
            .and_then(|billing| billing.output_tokens_per_second)
            .map(rate_text)
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "input_tokens".into(),
        json!(usage.map(|usage| usage.input_tokens)),
    );
    entry.insert(
        "cached_input_tokens".into(),
        json!(usage.map(|usage| usage.cached_input_tokens)),
    );
    entry.insert(
        "cache_write_tokens".into(),
        json!(usage.map(|usage| usage.cache_write_tokens)),
    );
    entry.insert(
        "output_tokens".into(),
        json!(usage.map(|usage| usage.output_tokens)),
    );
    entry.insert("model_id".into(), json!(uuid_text(event.model_id)));
    entry.insert("currency".into(), json!(price.map(|price| &price.currency)));
    entry.insert(
        "price_unit_tokens".into(),
        json!(price.map(|price| price.price_unit_tokens)),
    );
    entry.insert(
        "price_effective_at".into(),
        json!(price.map(|price| super::types::timestamp(price.price_effective_at))),
    );
    entry.insert(
        "input_unit_price".into(),
        price
            .map(|price| price_text(price.input_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cached_input_unit_price".into(),
        price
            .map(|price| price_text(price.cached_input_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cache_write_unit_price".into(),
        price
            .map(|price| price_text(price.cache_write_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "output_unit_price".into(),
        price
            .map(|price| price_text(price.output_unit_price))
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert(
        "cost_amount".into(),
        event
            .effective_cost_amount()
            .map(amount_text)
            .transpose()?
            .map_or(Value::Null, Value::String),
    );
    entry.insert("error_code".into(), json!(event.error_code));
    entry.insert("error_summary".into(), json!(event.error_summary));
    entry.insert(
        "reasoning_tokens".into(),
        json!(usage.map(|usage| usage.reasoning_tokens)),
    );
    entry.insert("reasoning_effort".into(), json!(event.reasoning_effort));
    entry.insert("fast_mode".into(), json!(event.fast_mode));
    entry.insert(
        "peak_pricing".into(),
        json!(billing.is_some_and(|billing| billing.peak_pricing)),
    );
    Ok(entry)
}

fn uuid_text(value: Option<Uuid>) -> Option<String> {
    value.map(|value| value.to_string())
}

fn settlement_outcome(row: &SettlementFactRow) -> RequestLogSettlementOutcome {
    if row.settled_at.is_some() {
        return RequestLogSettlementOutcome::AlreadyBilled;
    }
    if row.cost_amount.is_none() {
        return RequestLogSettlementOutcome::NotBillable;
    }
    if row.api_key_user_id != Some(row.user_id) {
        return RequestLogSettlementOutcome::AccountMismatch;
    }
    // A known cost without complete pricing evidence is not a billable claim.
    RequestLogSettlementOutcome::NotBillable
}

fn validate_response_status(status: u16) -> Result<i16, RepositoryError> {
    if !(100..=599).contains(&status) {
        return Err(RepositoryError::InvalidResponseStatus { status });
    }
    i16::try_from(status).map_err(|_| RepositoryError::InvalidResponseStatus { status })
}

#[derive(FromRow)]
struct IngestRecordRow {
    sequence: IngestReceipt,
    request_log_id: SqliteUuid,
    schema_version: i16,
    payload: Vec<u8>,
    attempt_count: i32,
}

impl IngestRecordRow {
    fn into_record(self) -> RequestLogIngestRecord {
        RequestLogIngestRecord {
            sequence: self.sequence,
            request_log_id: self.request_log_id.0,
            schema_version: self.schema_version,
            payload: self.payload,
            attempt_count: self.attempt_count,
        }
    }
}

#[derive(FromRow)]
struct IngestBacklogRow {
    row_count: i64,
    oldest_staged_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct SettlementBacklogRow {
    row_count: i64,
    oldest_completed_at: Option<SqliteTimestamp>,
}

#[derive(FromRow)]
struct ClaimedRow {
    request_id: SqliteUuid,
    cost_amount: SqliteAmount,
}

#[derive(FromRow)]
struct UpdatedApiKeyRow {
    quota_used_amount: SqliteAmount,
}

#[derive(FromRow)]
struct SettlementFactRow {
    id: SqliteUuid,
    user_id: SqliteUuid,
    api_key_id: SqliteUuid,
    cost_amount: Option<SqliteAmount>,
    settled_at: Option<SqliteTimestamp>,
    api_key_user_id: Option<SqliteUuid>,
}

/// Stored projection row used only for duplicate classification; JSON columns are
/// read as TEXT because `ag_json_valid` already owns their validity.
#[derive(FromRow)]
struct RequestLogRow {
    id: SqliteUuid,
    started_at: SqliteTimestamp,
    completed_at: SqliteTimestamp,
    user_id: SqliteUuid,
    api_key_id: SqliteUuid,
    request_source: String,
    api_format: String,
    api_operation: String,
    request_protocol: String,
    client_model: String,
    upstream_model: Option<String>,
    model_rule_id: Option<SqliteUuid>,
    channel_group_id: Option<SqliteUuid>,
    channel_id: Option<SqliteUuid>,
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
    model_id: Option<SqliteUuid>,
    currency: Option<String>,
    price_unit_tokens: Option<i64>,
    price_effective_at: Option<SqliteTimestamp>,
    input_unit_price: Option<SqliteUnitPrice>,
    cached_input_unit_price: Option<SqliteUnitPrice>,
    cache_write_unit_price: Option<SqliteUnitPrice>,
    output_unit_price: Option<SqliteUnitPrice>,
    cost_amount: Option<SqliteAmount>,
    error_code: Option<String>,
    error_summary: Option<String>,
    reasoning_effort: Option<String>,
    fast_mode: bool,
    peak_pricing: bool,
}

impl RequestLogRow {
    fn matches(&self, event: &RequestLogEvent, response_status_code: Option<i16>) -> bool {
        let billing = event.billing.as_ref();
        let usage = billing.and_then(|billing| billing.usage);
        let price = billing.map(|billing| &billing.price);
        self.started_at.0 == normalize_timestamp(event.started_at)
            && self.completed_at.0 == normalize_timestamp(event.completed_at)
            && self.user_id.0 == event.user_id
            && self.api_key_id.0 == event.api_key_id
            && self.request_source == event.request_source.as_str()
            && self.api_format == event.api_format.as_str()
            && self.api_operation == event.api_operation.as_str()
            && self.request_protocol == event.request_protocol.as_str()
            && self.client_model == event.client_model
            && self.upstream_model == event.upstream_model
            && self.model_rule_id.map(|value| value.0) == event.model_rule_id
            && self.channel_group_id.map(|value| value.0) == event.channel_group_id
            && self.channel_id.map(|value| value.0) == event.channel_id
            && self.outcome == event.outcome.as_str()
            && self.response_status_code == response_status_code
            && self.streamed == event.streamed
            && self.ttft_ms == event.ttft_ms
            && self.total_duration_ms == Some(event.total_duration_ms)
            && self.output_tokens_per_second.map(|value| value.0)
                == billing.and_then(|billing| billing.output_tokens_per_second)
            && self.input_tokens == usage.map(|usage| usage.input_tokens)
            && self.cached_input_tokens == usage.map(|usage| usage.cached_input_tokens)
            && self.cache_write_tokens == usage.map(|usage| usage.cache_write_tokens)
            && self.output_tokens == usage.map(|usage| usage.output_tokens)
            && self.reasoning_tokens == usage.map(|usage| usage.reasoning_tokens)
            && self.model_id.map(|value| value.0) == event.model_id
            && self.currency == price.map(|price| price.currency.clone())
            && self.price_unit_tokens == price.map(|price| price.price_unit_tokens)
            && self.price_effective_at.map(|value| value.0)
                == price.map(|price| normalize_timestamp(price.price_effective_at))
            && self.input_unit_price.map(|value| value.0)
                == price.map(|price| price.input_unit_price)
            && self.cached_input_unit_price.map(|value| value.0)
                == price.map(|price| price.cached_input_unit_price)
            && self.cache_write_unit_price.map(|value| value.0)
                == price.map(|price| price.cache_write_unit_price)
            && self.output_unit_price.map(|value| value.0)
                == price.map(|price| price.output_unit_price)
            && self.cost_amount.map(|value| value.0) == event.effective_cost_amount()
            && self.error_code == event.error_code
            && self.error_summary == event.error_summary
            && self.reasoning_effort == event.reasoning_effort
            && self.fast_mode == event.fast_mode
            && self.peak_pricing == billing.is_some_and(|billing| billing.peak_pricing)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::domain::{
        ApiFormat, ApiOperation, RequestBilling, RequestLogOutcome, RequestLogSource,
        RequestPriceSnapshot, RequestProtocol, RequestUsage,
    };
    use sqlx::Row;

    const USER: Uuid = Uuid::from_u128(0x901);
    const KEY: Uuid = Uuid::from_u128(0x911);
    const MODEL: Uuid = Uuid::from_u128(0x921);
    const PROFILE: Uuid = Uuid::from_u128(0x922);
    const RULE: Uuid = Uuid::from_u128(0x923);
    const GROUP: Uuid = Uuid::from_u128(0x931);
    const CHANNEL: Uuid = Uuid::from_u128(0x932);

    struct Fixture {
        _directory: tempfile::TempDir,
        database: Arc<SqliteDatabase>,
        logs: SqliteRequestLogRepository,
    }

    impl Fixture {
        async fn new() -> Self {
            let directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
                .await
                .unwrap();
            assert_eq!(database.install_schema().await.unwrap(), 3);
            let database = Arc::new(database);
            let logs = SqliteRequestLogRepository::new(Arc::clone(&database));
            Self {
                _directory: directory,
                database,
                logs,
            }
        }

        fn metering(&self) -> SqliteMeteringRepository {
            self.logs.metering()
        }

        fn settlements(&self) -> SqliteSettlementRepository {
            self.logs.settlements()
        }

        async fn execute(&self, sql: &str) {
            let mut transaction = self.database.begin_write().await.unwrap();
            sqlx::Executor::execute(&mut *transaction, sqlx::AssertSqlSafe(sql.to_owned()))
                .await
                .unwrap();
            transaction.commit().await.unwrap();
        }

        async fn scalar<T: for<'r> sqlx::Decode<'r, Sqlite> + sqlx::Type<Sqlite> + Send + Unpin>(
            &self,
            sql: &str,
        ) -> T {
            let mut reader = self.database.acquire_read().await.unwrap();
            sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
                .fetch_one(&mut *reader)
                .await
                .unwrap()
        }

        async fn seed(&self) {
            self.execute(&format!(
                "INSERT INTO users(id,display_name) VALUES ('{USER}','Pipeline unit user');
                 INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
                 VALUES ('{KEY}','{USER}','Unit key','unit-key-secret','active',
                         '[\"open_ai_chat_completions\"]','[\"proxy\"]');
                 INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
                     cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
                 VALUES ('{MODEL}','pipeline-model','Pipeline',1000000,'1','0','0','2',
                         '2026-01-01T00:00:00.000000Z');
                 INSERT INTO channel_groups(id,name,api_format)
                 VALUES ('{GROUP}','Unit group','open_ai_chat_completions');
                 INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind,available_models)
                 VALUES ('{CHANNEL}','{GROUP}','open_ai_chat_completions','Unit channel',
                         'https://upstream.invalid','none','[\"pipeline-model\"]');
                 INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
                 INSERT INTO model_rules(id,model_routing_profile_id,api_format,enabled)
                 VALUES ('{RULE}','{PROFILE}','open_ai_chat_completions',0);"
            ))
            .await;
        }
    }

    fn event() -> RequestLogEvent {
        let now = "2026-09-18T12:00:00.123456789Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        RequestLogEvent {
            id: Uuid::new_v4(),
            started_at: now,
            completed_at: now + chrono::Duration::seconds(1),
            user_id: USER,
            api_key_id: KEY,
            request_source: RequestLogSource::Client,
            api_format: ApiFormat::OpenAiChatCompletions,
            api_operation: ApiOperation::ChatCompletions,
            request_protocol: RequestProtocol::NonStream,
            client_model: "pipeline-model".into(),
            reasoning_effort: None,
            fast_mode: false,
            upstream_model: Some("pipeline-model".into()),
            model_rule_id: Some(RULE),
            channel_group_id: Some(GROUP),
            channel_id: Some(CHANNEL),
            model_id: Some(MODEL),
            outcome: RequestLogOutcome::Succeeded,
            response_status_code: Some(200),
            streamed: false,
            ttft_ms: Some(1),
            total_duration_ms: 2,
            billing: Some(RequestBilling {
                usage: Some(RequestUsage {
                    input_tokens: 10,
                    cached_input_tokens: 2,
                    cache_write_tokens: 1,
                    output_tokens: 4,
                    reasoning_tokens: 1,
                }),
                price: RequestPriceSnapshot {
                    currency: "USD".into(),
                    price_unit_tokens: 1_000_000,
                    price_effective_at: now,
                    input_unit_price: Decimal::new(100, 2),
                    cached_input_unit_price: Decimal::new(20, 2),
                    cache_write_unit_price: Decimal::new(30, 2),
                    output_unit_price: Decimal::new(200, 2),
                },
                cost_amount: Some(Decimal::new(999, 8)),
                output_tokens_per_second: None,
                peak_pricing: false,
            }),
            error_code: None,
            error_summary: None,
        }
    }

    fn encoded(events: &[RequestLogEvent]) -> Vec<EncodedRequestLog> {
        events
            .iter()
            .map(|event| EncodedRequestLog::encode(event).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn ingress_round_trip_keeps_facts_and_projection_independent() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let events = (0..3).map(|_| event()).collect::<Vec<_>>();
        assert_eq!(fixture.logs.accept_batch(&[]).await.unwrap(), 0);
        assert_eq!(
            fixture.logs.accept_batch(&encoded(&events)).await.unwrap(),
            3
        );
        // A replayed ingress batch is accepted again: the PRIMARY KEY is not the
        // idempotency boundary for durable ingress.
        assert_eq!(
            fixture.logs.accept_batch(&encoded(&events)).await.unwrap(),
            3
        );
        let backlog = fixture.logs.ingest_backlog().await.unwrap();
        assert_eq!(backlog.row_count, 6);
        assert!(backlog.oldest_staged_at.is_some());
        let status = fixture.logs.pool_status();
        assert!(status.size >= 1 && status.idle <= status.size as usize);

        let pending = fixture.metering().load_pending(100).await.unwrap();
        assert_eq!(pending.len(), 6);
        assert!(pending.iter().all(|row| row.attempt_count == 0));
        assert_eq!(pending[0].encoded().decode().unwrap().id, events[0].id);

        // Only the metered duplicates publish; the unmetered ones remain.
        let metered = &pending[..3];
        let receipts = metered.iter().map(|row| row.sequence).collect::<Vec<_>>();
        assert_eq!(
            fixture
                .metering()
                .materialize(&receipts, &events)
                .await
                .unwrap(),
            vec![MeteringWriteOutcome::Accepted; 3]
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_log_ingest WHERE metered_at IS NULL")
                .await,
            3
        );
        let projectable = fixture.logs.load_ingest_batch(100).await.unwrap();
        assert_eq!(projectable.len(), 3);
        // Acking before the query projection exists is rejected by the schema
        // guard, exactly like PostgreSQL's deferred projection precondition.
        assert!(
            fixture
                .logs
                .acknowledge_ingest(&[receipts[0]])
                .await
                .is_err()
        );
        let results = fixture.logs.project_batch(&events).await.unwrap();
        assert!(
            results
                .iter()
                .all(|result| result.outcome == RequestLogBatchInsertOutcome::Inserted)
        );
        assert_eq!(
            fixture
                .logs
                .acknowledge_ingest(
                    &projectable
                        .iter()
                        .map(|row| row.sequence)
                        .collect::<Vec<_>>()
                )
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_log_ingest")
                .await,
            3
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
                .await,
            3
        );
    }

    #[tokio::test]
    async fn materialize_rolls_back_facts_and_pending_when_the_ready_flag_fails() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let event = event();
        fixture
            .logs
            .accept_batch(&encoded(std::slice::from_ref(&event)))
            .await
            .unwrap();
        let pending = fixture.metering().load_pending(10).await.unwrap();
        fixture
            .execute(
                "CREATE TRIGGER unit_reject_ready BEFORE UPDATE ON request_log_ingest
                 WHEN OLD.metered_at IS NULL AND NEW.metered_at IS NOT NULL
                 BEGIN SELECT RAISE(ABORT, 'unit_reject_ready'); END;",
            )
            .await;
        assert!(
            fixture
                .metering()
                .materialize(&[pending[0].sequence], std::slice::from_ref(&event))
                .await
                .is_err()
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
                .await,
            0
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
                .await,
            0
        );
        assert_eq!(
            fixture
                .scalar::<i64>(
                    "SELECT count(*) FROM request_log_ingest WHERE metered_at IS NOT NULL"
                )
                .await,
            0
        );
        fixture.execute("DROP TRIGGER unit_reject_ready").await;
        assert_eq!(
            fixture
                .metering()
                .materialize(&[pending[0].sequence], std::slice::from_ref(&event))
                .await
                .unwrap(),
            vec![MeteringWriteOutcome::Accepted]
        );
    }

    #[tokio::test]
    async fn metering_and_projection_retries_are_tracked_separately() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let event = event();
        fixture
            .logs
            .accept_batch(&encoded(std::slice::from_ref(&event)))
            .await
            .unwrap();
        let receipt = fixture.metering().load_pending(10).await.unwrap()[0].sequence;

        fixture
            .metering()
            .defer(&[receipt], "metering_write_failed", 3600)
            .await
            .unwrap();
        assert!(
            fixture
                .metering()
                .load_pending(10)
                .await
                .unwrap()
                .is_empty(),
            "a deferred row is not due yet"
        );
        fixture
            .execute("UPDATE request_log_ingest SET metering_next_attempt_at=ag_now()")
            .await;
        let pending = fixture.metering().load_pending(10).await.unwrap();
        assert_eq!(pending[0].attempt_count, 1);
        assert_eq!(
            fixture
                .scalar::<String>("SELECT metering_last_error_code FROM request_log_ingest")
                .await,
            "metering_write_failed"
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT attempt_count FROM request_log_ingest")
                .await,
            0,
            "the projection retry budget is independent"
        );

        fixture
            .metering()
            .materialize(&[receipt], std::slice::from_ref(&event))
            .await
            .unwrap();
        fixture
            .logs
            .defer_ingest(&[receipt], "isolated_insert_failed", 3600)
            .await
            .unwrap();
        assert!(fixture.logs.load_ingest_batch(10).await.unwrap().is_empty());
        fixture
            .execute("UPDATE request_log_ingest SET next_attempt_at=ag_now()")
            .await;
        let projectable = fixture.logs.load_ingest_batch(10).await.unwrap();
        assert_eq!(projectable[0].attempt_count, 1);
        assert_eq!(
            fixture
                .scalar::<String>("SELECT last_error_code FROM request_log_ingest")
                .await,
            "isolated_insert_failed"
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT metering_attempt_count FROM request_log_ingest")
                .await,
            1
        );
    }

    #[tokio::test]
    async fn duplicate_occurrences_inside_one_batch_are_classified_per_input() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let event = event();
        let mut conflicting = event.clone();
        conflicting.billing.as_mut().unwrap().cost_amount = Some(Decimal::ONE);
        // The first occurrence inserts; the second is recognised as a duplicate of
        // the just-written row instead of being reported as a new insert.
        assert_eq!(
            fixture
                .metering()
                .record_batch(&[event.clone(), event.clone(), conflicting.clone()])
                .await
                .unwrap(),
            vec![
                MeteringWriteOutcome::Accepted,
                MeteringWriteOutcome::Accepted,
                MeteringWriteOutcome::Conflict,
            ]
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
                .await,
            1,
            "a replayed occurrence must not create a second work item"
        );
        let results = fixture
            .logs
            .project_batch(&[event.clone(), event.clone(), conflicting])
            .await
            .unwrap();
        assert_eq!(
            results
                .iter()
                .map(|result| result.outcome)
                .collect::<Vec<_>>(),
            vec![
                RequestLogBatchInsertOutcome::Inserted,
                RequestLogBatchInsertOutcome::ExactDuplicate,
                RequestLogBatchInsertOutcome::DuplicateConflict,
            ]
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_logs")
                .await,
            1
        );
    }

    #[tokio::test]
    async fn batches_larger_than_one_bind_chunk_still_use_one_transaction() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let count = BIND_CHUNK * 2 + 5;
        let events = (0..count).map(|_| event()).collect::<Vec<_>>();
        assert_eq!(
            fixture.logs.accept_batch(&encoded(&events)).await.unwrap(),
            count as u64
        );
        let pending = fixture.metering().load_pending(count as i64).await.unwrap();
        assert_eq!(pending.len(), count);
        let receipts = pending.iter().map(|row| row.sequence).collect::<Vec<_>>();
        assert_eq!(
            fixture
                .metering()
                .materialize(&receipts, &events)
                .await
                .unwrap(),
            vec![MeteringWriteOutcome::Accepted; count]
        );
        let results = fixture.logs.project_batch(&events).await.unwrap();
        assert_eq!(results.len(), count);
        assert!(
            results
                .iter()
                .all(|result| result.outcome == RequestLogBatchInsertOutcome::Inserted)
        );
        assert_eq!(
            fixture.logs.acknowledge_ingest(&receipts).await.unwrap(),
            count as u64
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_log_ingest")
                .await,
            0
        );
        // The settlement batch is chunked too, and reports one outcome per id.
        let ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
        let outcomes = fixture.settlements().settle_batch(&ids).await.unwrap();
        assert_eq!(outcomes.len(), count);
        assert!(
            outcomes
                .iter()
                .all(|(_, outcome)| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
        );
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
                .await,
            0
        );
    }

    #[tokio::test]
    async fn mapped_columns_decode_to_canonical_domain_values() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let mut event = event();
        event.id = Uuid::from_u128(0xdead_beef);
        event.started_at = "2026-09-18T12:00:00.999999900Z".parse().unwrap();
        event.completed_at = "2026-09-18T12:00:01.000000100Z".parse().unwrap();
        fixture
            .metering()
            .record_batch(std::slice::from_ref(&event))
            .await
            .unwrap();
        let mut reader = fixture.database.acquire_read().await.unwrap();
        let row = sqlx::query(
            "SELECT id,started_at,completed_at,cost_amount,input_unit_price,peak_pricing,amount_state
             FROM request_metering_facts",
        )
        .fetch_one(&mut *reader)
        .await
        .unwrap();
        assert_eq!(
            row.get::<String, _>("id"),
            "00000000-0000-0000-0000-0000deadbeef"
        );
        assert_eq!(
            row.get::<String, _>("started_at"),
            "2026-09-18T12:00:00.999999Z"
        );
        assert_eq!(
            row.get::<String, _>("completed_at"),
            "2026-09-18T12:00:01.000000Z"
        );
        assert_eq!(row.get::<String, _>("cost_amount"), "0.00000999");
        assert_eq!(row.get::<String, _>("input_unit_price"), "1");
        assert_eq!(row.get::<i64, _>("peak_pricing"), 0);
        assert_eq!(row.get::<String, _>("amount_state"), "priced");
    }

    #[test]
    fn fact_and_log_encoders_cover_every_bindable_column() {
        let event = event();
        let fact = fact_object(&event, 3).unwrap();
        assert_eq!(
            fact.keys().filter(|key| key.as_str() != "ordinal").count(),
            FACT_FIELDS.len()
        );
        for field in FACT_FIELDS {
            assert!(fact.contains_key(field), "{field}");
        }
        assert_eq!(fact["ordinal"], json!(3));
        let log = log_object(&event, Some(200), 0).unwrap();
        assert_eq!(log.len(), LOG_FIELDS.len() + 1);
        for field in LOG_FIELDS {
            assert!(log.contains_key(field), "{field}");
        }
        // An out-of-range amount is rejected instead of being silently rounded.
        let mut overflow = event;
        overflow.billing.as_mut().unwrap().cost_amount = Some(Decimal::MAX);
        assert!(fact_object(&overflow, 0).is_err());
    }

    #[test]
    fn canonical_helpers_match_the_sqlite_column_encodings() {
        assert_eq!(amount_text(Decimal::new(999, 8)).unwrap(), "0.00000999");
        assert_eq!(amount_text(Decimal::new(10, 0)).unwrap(), "10");
        assert_eq!(price_text(Decimal::new(100, 2)).unwrap(), "1");
        assert_eq!(rate_text(Decimal::new(2000, 3)).unwrap(), "2");
        assert_eq!(
            normalize_timestamp(
                "2026-09-18T12:00:00.123456789Z"
                    .parse::<DateTime<Utc>>()
                    .unwrap()
            ),
            "2026-09-18T12:00:00.123456Z"
                .parse::<DateTime<Utc>>()
                .unwrap()
        );
        assert!(uuid_text(None).is_none());
        assert_eq!(uuid_text(Some(KEY)).unwrap(), KEY.to_string());
    }
    #[tokio::test]
    async fn recovery_scan_stays_bounded_after_settled_history_grows() {
        let fixture = Fixture::new().await;
        fixture.seed().await;
        let history = (0..500).map(|_| event()).collect::<Vec<_>>();
        fixture.metering().record_batch(&history).await.unwrap();
        let ids = history.iter().map(|event| event.id).collect::<Vec<_>>();
        let outcomes = fixture.settlements().settle_batch(&ids).await.unwrap();
        assert_eq!(outcomes.len(), history.len());
        assert_eq!(
            fixture
                .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
                .await,
            0
        );

        let current = event();
        fixture
            .metering()
            .record_batch(std::slice::from_ref(&current))
            .await
            .unwrap();
        let mut plan = String::new();
        {
            let mut reader = fixture.database.acquire_read().await.unwrap();
            for row in sqlx::query(sqlx::AssertSqlSafe(format!(
                "EXPLAIN QUERY PLAN {PENDING_SCAN}"
            )))
            .fetch_all(&mut *reader)
            .await
            .unwrap()
            {
                plan.push_str(&row.get::<String, _>("detail"));
                plan.push('\n');
            }
        }
        // The scan is driven by the pending queue index, not by the fact table.
        assert!(
            plan.contains("request_settlement_pending_order_idx"),
            "unexpected recovery plan:\n{plan}"
        );
        assert!(
            !plan.contains("SCAN request_metering_facts"),
            "settled history must not expand the pending scan:\n{plan}"
        );
        assert_eq!(
            fixture.settlements().settle_pending(1).await.unwrap().len(),
            1
        );
        assert!(
            fixture
                .settlements()
                .settle_pending(1)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
