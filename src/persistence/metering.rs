//! Immutable financial facts, independent of the request-log projection.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::domain::RequestLogEvent;

use super::RepositoryError;
use super::postgres_control_plane::{IngestReceipt, RequestLogIngestRecord};

const FACT_COLUMNS: &str = "id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,\
     request_protocol,client_model,upstream_model,model_rule_id,channel_group_id,channel_id,\
     model_id,outcome,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,\
     reasoning_tokens,currency,price_unit_tokens,price_effective_at,input_unit_price,\
     cached_input_unit_price,cache_write_unit_price,output_unit_price,cost_amount,peak_pricing";

#[derive(Clone)]
pub struct MeteringRepository {
    pool: PgPool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeteringWriteOutcome {
    Accepted,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRow)]
pub struct MeteringReconciliationCounts {
    pub unknown: i64,
    pub invalid: i64,
    pub account_mismatch: i64,
}

impl MeteringRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn reconciliation_counts(
        &self,
    ) -> Result<MeteringReconciliationCounts, RepositoryError> {
        Ok(sqlx::query_as(
            "SELECT
                (SELECT count(*) FROM request_metering_facts WHERE amount_state='unknown') AS unknown,
                (SELECT count(*) FROM request_metering_facts WHERE amount_state='invalid') AS invalid,
                (SELECT count(*) FROM request_settlement_pending AS pending
                 JOIN request_metering_facts AS fact ON fact.id=pending.request_id
                 LEFT JOIN api_keys AS key ON key.id=fact.api_key_id
                 WHERE key.user_id IS DISTINCT FROM fact.user_id) AS account_mismatch",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn record_batch(
        &self,
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        let mut transaction = self.pool.begin().await?;
        let outcomes = write_facts(&mut transaction, events).await?;
        transaction.commit().await?;
        Ok(outcomes)
    }

    pub(crate) async fn load_pending(
        &self,
        limit: i64,
    ) -> Result<Vec<RequestLogIngestRecord>, RepositoryError> {
        Ok(sqlx::query_as(
            "SELECT sequence,request_log_id,schema_version,payload,
                    metering_attempt_count AS attempt_count
             FROM request_log_ingest
             WHERE metered_at IS NULL AND metering_next_attempt_at <= now()
             ORDER BY sequence LIMIT $1",
        )
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await?)
    }

    pub(crate) async fn materialize(
        &self,
        receipts: &[IngestReceipt],
        events: &[RequestLogEvent],
    ) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
        if receipts.len() != events.len() {
            return Err(RepositoryError::Validation);
        }
        let mut transaction = self.pool.begin().await?;
        let outcomes = write_facts(&mut transaction, events).await?;
        let accepted = receipts
            .iter()
            .zip(&outcomes)
            .filter_map(|(receipt, outcome)| {
                (*outcome == MeteringWriteOutcome::Accepted).then_some(*receipt)
            })
            .collect::<Vec<_>>();
        sqlx::query(
            "UPDATE request_log_ingest SET metered_at=now()
             WHERE sequence=ANY($1) AND metered_at IS NULL",
        )
        .bind(accepted)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(outcomes)
    }

    pub(crate) async fn defer(
        &self,
        receipts: &[IngestReceipt],
        code: &str,
        retry_seconds: i64,
    ) -> Result<(), RepositoryError> {
        sqlx::query(
            "UPDATE request_log_ingest SET
                 metering_attempt_count=metering_attempt_count+1,
                 metering_next_attempt_at=now()+make_interval(secs => $2),
                 metering_last_error_code=$3
             WHERE sequence=ANY($1) AND metered_at IS NULL",
        )
        .bind(receipts)
        .bind(retry_seconds.max(1))
        .bind(code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

async fn write_facts(
    transaction: &mut Transaction<'_, Postgres>,
    events: &[RequestLogEvent],
) -> Result<Vec<MeteringWriteOutcome>, RepositoryError> {
    if events.is_empty() {
        return Ok(Vec::new());
    }
    let input = Value::Array(events.iter().map(fact_input).collect());
    let inserted: Vec<Uuid> = sqlx::query_scalar(&format!(
        "INSERT INTO request_metering_facts ({FACT_COLUMNS})
         SELECT {FACT_COLUMNS}
         FROM jsonb_populate_recordset(NULL::request_metering_facts,$1)
         ON CONFLICT (id) DO NOTHING RETURNING id"
    ))
    .bind(&input)
    .fetch_all(&mut **transaction)
    .await?;
    // Only new facts create work. A replay must not resurrect work concurrently
    // removed by a committed settlement receipt.
    sqlx::query(
        "INSERT INTO request_settlement_pending(request_id,completed_at)
         SELECT id,completed_at FROM request_metering_facts
         WHERE id=ANY($1) AND amount_state IN ('priced','zero_by_policy')",
    )
    .bind(inserted)
    .execute(&mut **transaction)
    .await?;
    let matches: Vec<bool> = sqlx::query_scalar(
        "SELECT (to_jsonb(stored)-'amount_state') = (to_jsonb(incoming)-'amount_state')
         FROM jsonb_array_elements($1) WITH ORDINALITY AS input(value,ordinal)
         CROSS JOIN LATERAL jsonb_populate_record(NULL::request_metering_facts,input.value) AS incoming
         JOIN request_metering_facts AS stored ON stored.id=incoming.id
         ORDER BY input.ordinal",
    ).bind(&input).fetch_all(&mut **transaction).await?;
    if matches.len() != events.len() {
        return Err(RepositoryError::Validation);
    }
    Ok(matches
        .into_iter()
        .map(|equal| {
            if equal {
                MeteringWriteOutcome::Accepted
            } else {
                MeteringWriteOutcome::Conflict
            }
        })
        .collect())
}

fn fact_input(event: &RequestLogEvent) -> Value {
    let price = event.billing.as_ref().map(|billing| &billing.price);
    let usage = event.billing.as_ref().and_then(|billing| billing.usage);
    json!({
        "id": event.id,
        "started_at": timestamp(event.started_at),
        "completed_at": timestamp(event.completed_at),
        "user_id": event.user_id,
        "api_key_id": event.api_key_id,
        "request_source": event.request_source.as_str(),
        "api_format": event.api_format.as_str(),
        "api_operation": event.api_operation.as_str(),
        "request_protocol": event.request_protocol.as_str(),
        "client_model": event.client_model,
        "upstream_model": event.upstream_model,
        "model_rule_id": event.model_rule_id,
        "channel_group_id": event.channel_group_id,
        "channel_id": event.channel_id,
        "model_id": event.model_id,
        "outcome": event.outcome.as_str(),
        "input_tokens": usage.map(|usage| usage.input_tokens),
        "cached_input_tokens": usage.map(|usage| usage.cached_input_tokens),
        "cache_write_tokens": usage.map(|usage| usage.cache_write_tokens),
        "output_tokens": usage.map(|usage| usage.output_tokens),
        "reasoning_tokens": usage.map(|usage| usage.reasoning_tokens),
        "currency": price.map(|price| &price.currency),
        "price_unit_tokens": price.map(|price| price.price_unit_tokens),
        "price_effective_at": price.map(|price| timestamp(price.price_effective_at)),
        "input_unit_price": price.map(|price| price.input_unit_price),
        "cached_input_unit_price": price.map(|price| price.cached_input_unit_price),
        "cache_write_unit_price": price.map(|price| price.cache_write_unit_price),
        "output_unit_price": price.map(|price| price.output_unit_price),
        "cost_amount": event.effective_cost_amount(),
        "peak_pricing": event.billing.as_ref().is_some_and(|billing| billing.peak_pricing),
    })
}

// SQLx bindings truncate to microseconds; PG JSON timestamp casts would round
// sub-microsecond digits and make historical replay appear to conflict.
fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}
