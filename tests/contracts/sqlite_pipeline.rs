//! SQLite S4 durable-write pipeline contracts.
//!
//! These run against a real installed business schema and exercise the public
//! S4 repository surface plus the schema-level guards behind the worker-facing
//! methods: financial-fact materialization, the independent query-log
//! projection, single-transaction settlement, and exact replay/conflict
//! behaviour. The `pub(crate)` worker methods (ingress accept/load/ack/defer,
//! metering load/materialize/defer, backlog and pool status) are covered by the
//! in-crate unit tests next to `src/persistence/sqlite/pipeline.rs`, because
//! their shared opaque receipt DTOs are crate-private exactly as in PostgreSQL.
//!
//! Host wiring (the parent-owned `tests/sqlite_foundation.rs`):
//!
//! ```ignore
//! #[path = "contracts/sqlite_pipeline.rs"]
//! mod sqlite_pipeline;
//! ```
//!
//! The module reuses the host's `database()` helper and the `sqlite-backend`
//! feature gate declared there.

use super::*;
use ai_gateway::{
    domain::{
        ApiFormat, ApiOperation, RequestBilling, RequestLogEvent, RequestLogOutcome,
        RequestLogSource, RequestPriceSnapshot, RequestProtocol, RequestUsage,
    },
    persistence::{
        MeteringWriteOutcome, RequestLogBatchInsertOutcome, RequestLogInsertOutcome,
        RequestLogSettlementOutcome,
        sqlite::{
            SqliteDatabase, SqliteMeteringRepository, SqliteRequestLogRepository,
            SqliteSettlementRepository,
        },
    },
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rust_decimal::Decimal;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

const PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA";
const USER: Uuid = Uuid::from_u128(0x801);
const KEY: Uuid = Uuid::from_u128(0x811);
const MODEL: Uuid = Uuid::from_u128(0x821);
const PROFILE: Uuid = Uuid::from_u128(0x822);
const RULE: Uuid = Uuid::from_u128(0x823);
const GROUP: Uuid = Uuid::from_u128(0x831);
const CHANNEL: Uuid = Uuid::from_u128(0x832);
const ACCESS: Uuid = Uuid::from_u128(0x833);
const CAPABILITY: Uuid = Uuid::from_u128(0x834);

struct Pipeline {
    _directory: Option<tempfile::TempDir>,
    database: Arc<SqliteDatabase>,
    logs: SqliteRequestLogRepository,
}

impl Pipeline {
    async fn new() -> Self {
        let (directory, database) = database().await;
        assert_eq!(database.install_schema().await.unwrap(), 5);
        let database = Arc::new(database);
        let logs = SqliteRequestLogRepository::new(Arc::clone(&database));
        Self {
            _directory: Some(directory),
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

    /// Runs one statement inside a write transaction and reports the failure
    /// message without committing, so schema guards stay observable.
    async fn attempt(&self, sql: &str) -> Result<(), String> {
        let mut transaction = self.database.begin_write().await.unwrap();
        match sqlx::Executor::execute(&mut *transaction, sqlx::AssertSqlSafe(sql.to_owned())).await
        {
            Ok(_) => {
                transaction.commit().await.unwrap();
                Ok(())
            }
            Err(error) => {
                transaction.rollback().await.unwrap();
                Err(error.to_string())
            }
        }
    }

    async fn scalar<T>(&self, sql: &str) -> T
    where
        for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
    {
        let mut reader = self.database.acquire_read().await.unwrap();
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(&mut *reader)
            .await
            .unwrap()
    }

    /// Balance and API-key quota exactly as stored, read as canonical TEXT so
    /// assertions never pass through a lossy numeric conversion.
    async fn accounts(&self) -> (Decimal, Decimal) {
        let mut reader = self.database.acquire_read().await.unwrap();
        let row = sqlx::query(
            "SELECT user_row.balance_amount AS balance, key_row.quota_used_amount AS quota \
             FROM users AS user_row JOIN api_keys AS key_row ON key_row.user_id=user_row.id \
             WHERE key_row.id=?",
        )
        .bind(KEY.to_string())
        .fetch_one(&mut *reader)
        .await
        .unwrap();
        (
            Decimal::from_str_exact(row.get::<String, _>("balance").as_str()).unwrap(),
            Decimal::from_str_exact(row.get::<String, _>("quota").as_str()).unwrap(),
        )
    }

    async fn seed(&self) {
        self.execute(&format!(
            "INSERT INTO users(id,email,display_name,role,status,password_hash,password_changed_at,user_group_id)
             VALUES ('{USER}','pipeline@example.test','Pipeline user','user','active','{PASSWORD_HASH}',ag_now(),'{DEFAULT_USER_GROUP_ID}');
             INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
             VALUES ('{KEY}','{USER}','Pipeline key','pipeline-key-secret','active','[\"open_ai_chat_completions\"]','[\"proxy\"]');
             INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
                 cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
             VALUES ('{MODEL}','pipeline-model','Pipeline',1000000,'1','0','0','2','2026-01-01T00:00:00.000000Z');
             INSERT INTO routing_groups(id,name) VALUES ('{GROUP}','Pipeline group');
             INSERT INTO upstream_accesses(id,name,connector_kind,base_url) VALUES ('{ACCESS}','Pipeline access','openai_compatible','https://upstream.invalid');
             INSERT INTO upstream_channels(id,group_id,access_id,name) VALUES ('{CHANNEL}','{GROUP}','{ACCESS}','Pipeline channel');
             INSERT INTO channel_capabilities(id,channel_id,operation,transports,enabled,available_models)
             VALUES ('{CAPABILITY}','{CHANNEL}','chat_completions','[\"http_json\"]',1,'[\"pipeline-model\"]');
             INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
             INSERT INTO model_operation_rules(id,model_routing_profile_id,operation,enabled)
             VALUES ('{RULE}','{PROFILE}','chat_completions',0);
             INSERT INTO group_identity_registry(id,label,canonical_group_id) VALUES ('{GROUP}','Pipeline group','{GROUP}');
             INSERT INTO channel_identity_registry(id,label,canonical_channel_id,capability_id) VALUES ('{CAPABILITY}','Pipeline channel','{CHANNEL}','{CAPABILITY}');
             INSERT INTO model_rule_identity_registry(id,label,created_at,canonical_rule_id) VALUES ('{RULE}','pipeline-model',ag_now(),'{RULE}');"
        ))
        .await;
    }

    async fn finish(self) {
        self.database.close().await;
    }
}

#[tokio::test]
async fn cancelling_each_financial_stage_preserves_its_commit_boundary() {
    for table in ["request_metering_facts", "request_logs", "users"] {
        let pipeline = Pipeline::new().await;
        pipeline.seed().await;
        let item = event(RequestLogOutcome::Succeeded);
        if table == "users" {
            pipeline
                .metering()
                .record_batch(std::slice::from_ref(&item))
                .await
                .unwrap();
        }
        let (started, ready) = tokio::sync::oneshot::channel();
        let (resume, paused) = std::sync::mpsc::sync_channel(1);
        let mut started = Some(started);
        let mut tx = pipeline.database.begin_write().await.unwrap();
        tx.lock_handle()
            .await
            .unwrap()
            .set_update_hook(move |update| {
                if update.table == table
                    && let Some(started) = started.take()
                {
                    started.send(()).unwrap();
                    paused
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                }
            });
        tx.rollback().await.unwrap();
        let logs = pipeline.logs.clone();
        let pending = item.clone();
        let task = tokio::spawn(async move {
            match table {
                "request_metering_facts" => {
                    logs.metering().record_batch(&[pending]).await.unwrap();
                }
                "request_logs" => {
                    logs.insert(&pending).await.unwrap();
                }
                _ => {
                    logs.settlements().settle(pending.id).await.unwrap();
                }
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        resume.send(()).unwrap();
        pipeline
            .database
            .begin_write()
            .await
            .unwrap()
            .rollback()
            .await
            .unwrap();
        assert_eq!(
            pipeline
                .scalar::<i64>("SELECT count(*) FROM request_logs")
                .await,
            0
        );
        assert_eq!(
            pipeline
                .scalar::<i64>("SELECT count(*) FROM request_settlements")
                .await,
            0
        );
        assert_eq!(
            pipeline
                .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
                .await,
            i64::from(table != "request_metering_facts")
        );
        assert_eq!(pipeline.accounts().await, (Decimal::ZERO, Decimal::ZERO));
        pipeline.logs.insert(&item).await.unwrap();
        pipeline.settlements().settle(item.id).await.unwrap();
        let cost = item.effective_cost_amount().unwrap();
        assert_eq!(pipeline.accounts().await, (-cost, cost));
        pipeline.database.close().await;
    }
}

#[tokio::test]
async fn financial_crash_child() {
    let Ok(path) = std::env::var("GATEWAY_S4_CRASH_DIRECTORY") else {
        return;
    };
    let root = std::path::PathBuf::from(path);
    let database = Arc::new(
        SqliteDatabase::open(&root.join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    database.install_schema().await.unwrap();
    let pipeline = Pipeline {
        _directory: None,
        logs: SqliteRequestLogRepository::new(Arc::clone(&database)),
        database,
    };
    pipeline.seed().await;
    let mut committed = event(RequestLogOutcome::Succeeded);
    committed.id = Uuid::from_u128(0x2001);
    let mut interrupted = committed.clone();
    interrupted.id = Uuid::from_u128(0x2002);
    pipeline
        .logs
        .insert_batch(&[committed.clone(), interrupted.clone()])
        .await
        .unwrap();
    pipeline.settlements().settle(committed.id).await.unwrap();
    let mut tx = pipeline.database.begin_write().await.unwrap();
    tx.lock_handle()
        .await
        .unwrap()
        .set_update_hook(move |update| {
            if update.table == "users" {
                std::fs::write(root.join("ready"), b"settlement in progress").unwrap();
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        });
    tx.rollback().await.unwrap();
    pipeline.settlements().settle(interrupted.id).await.unwrap();
    panic!("parent must kill the interrupted settlement");
}

#[tokio::test]
async fn process_crash_recovers_pending_and_replays_a_committed_receipt_once() {
    let directory = private_directory();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sqlite_pipeline::financial_crash_child",
            "--nocapture",
        ])
        .env("GATEWAY_S4_CRASH_DIRECTORY", directory.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let ready = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !directory.path().join("ready").exists() {
            if let Some(status) = child.try_wait().unwrap() {
                return Err(status);
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok(())
    })
    .await;
    let _ = child.kill();
    child.wait().unwrap();
    assert!(
        matches!(ready, Ok(Ok(()))),
        "child must reach settlement update"
    );
    let database = Arc::new(
        SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
            .await
            .unwrap(),
    );
    let pipeline = Pipeline {
        _directory: Some(directory),
        logs: SqliteRequestLogRepository::new(Arc::clone(&database)),
        database,
    };
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlements")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        1
    );
    assert!(matches!(
        pipeline
            .settlements()
            .settle(Uuid::from_u128(0x2001))
            .await
            .unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    ));
    assert_eq!(
        pipeline
            .settlements()
            .settle_pending(10)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        pipeline
            .settlements()
            .settle_pending(10)
            .await
            .unwrap()
            .is_empty()
    );
    let cost = event(RequestLogOutcome::Succeeded)
        .effective_cost_amount()
        .unwrap()
        * Decimal::from(2);
    assert_eq!(pipeline.accounts().await, (-cost, cost));
    pipeline.database.close().await;
}

fn event(outcome: RequestLogOutcome) -> RequestLogEvent {
    let now = DateTime::parse_from_rfc3339("2026-09-18T12:00:00.123456789Z")
        .unwrap()
        .with_timezone(&Utc);
    RequestLogEvent {
        id: Uuid::new_v4(),
        started_at: now,
        completed_at: now + ChronoDuration::milliseconds(2),
        user_id: USER,
        api_key_id: KEY,
        request_source: RequestLogSource::Client,
        api_format: ApiFormat::OpenAiChatCompletions,
        api_operation: ApiOperation::ChatCompletions,
        request_protocol: RequestProtocol::Sse,
        client_model: "pipeline-model".into(),
        reasoning_effort: Some("high".into()),
        fast_mode: true,
        upstream_model: Some("pipeline-model".into()),
        model_rule_id: Some(RULE),
        channel_group_id: Some(GROUP),
        channel_id: Some(CAPABILITY),
        model_id: Some(MODEL),
        outcome,
        response_status_code: Some(200),
        streamed: true,
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
            output_tokens_per_second: Some(Decimal::new(2000, 3)),
            peak_pricing: true,
        }),
        error_code: None,
        error_summary: None,
    }
}

#[tokio::test]
async fn facts_and_pending_are_written_atomically_and_facts_are_immutable() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let event = event(RequestLogOutcome::Succeeded);
    assert_eq!(
        pipeline
            .metering()
            .record_batch(std::slice::from_ref(&event))
            .await
            .unwrap(),
        vec![MeteringWriteOutcome::Accepted]
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        1
    );
    // Facts and receipts are append-only, and pending work cannot be removed or
    // changed without a committed receipt.
    for sql in [
        "UPDATE request_metering_facts SET cost_amount='0'",
        "DELETE FROM request_metering_facts",
        "UPDATE request_settlement_pending SET completed_at=ag_now()",
        "DELETE FROM request_settlement_pending",
    ] {
        assert!(pipeline.attempt(sql).await.is_err(), "{sql}");
    }
    pipeline
        .execute(&format!(
            "UPDATE users SET balance_amount=10,updated_at=ag_now() WHERE id='{USER}'"
        ))
        .await;
    assert_eq!(
        pipeline.accounts().await,
        (Decimal::from(10), Decimal::ZERO)
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn identical_financial_replay_is_accepted_and_conflict_preserves_evidence() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let event = event(RequestLogOutcome::Succeeded);
    pipeline
        .metering()
        .record_batch(std::slice::from_ref(&event))
        .await
        .unwrap();
    // The same UUID with equal financial content replays without a second fact
    // or work item, including values PostgreSQL stores at column scale.
    let mut replay = event.clone();
    replay.billing.as_mut().unwrap().cost_amount = Some(Decimal::new(999, 8).normalize());
    replay.billing.as_mut().unwrap().price.input_unit_price = Decimal::new(100, 2).normalize();
    assert_eq!(
        pipeline
            .metering()
            .record_batch(std::slice::from_ref(&replay))
            .await
            .unwrap(),
        vec![MeteringWriteOutcome::Accepted]
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        1
    );
    // Different financial content never overwrites and reports a conflict.
    let mut conflicting = event.clone();
    conflicting.billing.as_mut().unwrap().cost_amount = Some(Decimal::ONE);
    assert_eq!(
        pipeline
            .metering()
            .record_batch(std::slice::from_ref(&conflicting))
            .await
            .unwrap(),
        vec![MeteringWriteOutcome::Conflict]
    );
    assert_eq!(
        pipeline
            .scalar::<String>("SELECT cost_amount FROM request_metering_facts")
            .await,
        "0.00000999"
    );
    // Sub-microsecond digits round-trip to the same stored value, so an old
    // journal must not be mistaken for conflicting content; SQLx persists
    // microseconds on PostgreSQL too.
    let mut sub_microsecond = event.clone();
    sub_microsecond.started_at = DateTime::parse_from_rfc3339("2026-09-18T12:00:00.123456900Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        pipeline
            .metering()
            .record_batch(std::slice::from_ref(&sub_microsecond))
            .await
            .unwrap(),
        vec![MeteringWriteOutcome::Accepted]
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn replay_never_resurrects_settled_work_or_charges_twice() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let event = event(RequestLogOutcome::Succeeded);
    let cost = event.effective_cost_amount().unwrap();
    pipeline
        .logs
        .insert_batch(std::slice::from_ref(&event))
        .await
        .unwrap();
    assert_eq!(
        pipeline.settlements().settle(event.id).await.unwrap(),
        RequestLogSettlementOutcome::Settled {
            request_log_id: event.id,
            api_key_id: KEY,
            quota_used_amount: cost,
        }
    );
    let settled_accounts = pipeline.accounts().await;
    assert_eq!(settled_accounts, (-cost, cost));
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        0
    );
    // The receipt is a one-time claim and cannot be updated or deleted.
    for sql in [
        "UPDATE request_settlements SET cost_amount='0'",
        "DELETE FROM request_settlements",
    ] {
        assert!(pipeline.attempt(sql).await.is_err(), "{sql}");
    }
    // Replaying the terminal event recreates nothing: no second fact, no work
    // item, and an already-billed outcome.
    pipeline
        .logs
        .insert_batch(std::slice::from_ref(&event))
        .await
        .unwrap();
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        0
    );
    assert_eq!(
        pipeline.settlements().settle(event.id).await.unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    );
    assert_eq!(pipeline.accounts().await, settled_accounts);
    pipeline.finish().await;
}

#[tokio::test]
async fn projection_is_independent_of_settlement_and_keeps_diagnostic_conflicts() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let event = event(RequestLogOutcome::Succeeded);
    let cost = event.effective_cost_amount().unwrap();
    assert_eq!(
        pipeline.logs.insert(&event).await.unwrap(),
        RequestLogInsertOutcome::Inserted
    );
    // The query projection is a disposable view; deleting it must not remove the
    // fact, the pending work item, or a receipt.
    pipeline.execute("DELETE FROM request_logs").await;
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        1
    );
    pipeline.settlements().settle(event.id).await.unwrap();
    assert_eq!(pipeline.accounts().await, (-cost, cost));
    // Re-projection after an operator-removed view re-creates it once, while a
    // duplicate that differs only in display fields is a diagnostic conflict that
    // keeps the immutable financial facts untouched.
    assert_eq!(
        pipeline.logs.insert(&event).await.unwrap(),
        RequestLogInsertOutcome::Inserted
    );
    let mut diagnostic = event.clone();
    diagnostic.error_summary = Some("different diagnostic".into());
    let results = pipeline
        .logs
        .insert_batch(&[diagnostic, event.clone()])
        .await
        .unwrap();
    assert_eq!(
        results[0].outcome,
        RequestLogBatchInsertOutcome::DuplicateConflict
    );
    assert_eq!(
        results[1].outcome,
        RequestLogBatchInsertOutcome::ExactDuplicate
    );
    assert_eq!(pipeline.accounts().await, (-cost, cost));
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_logs")
            .await,
        1
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn invalid_status_is_isolated_from_valid_peers_in_one_batch() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let valid = event(RequestLogOutcome::Succeeded);
    let mut invalid = event(RequestLogOutcome::Failed);
    invalid.response_status_code = Some(99);
    invalid.billing = None;
    let results = pipeline
        .logs
        .insert_batch(&[valid.clone(), invalid.clone()])
        .await
        .unwrap();
    assert_eq!(results[0].outcome, RequestLogBatchInsertOutcome::Inserted);
    assert_eq!(
        results[1].outcome,
        RequestLogBatchInsertOutcome::InvalidResponseStatus { status: 99 }
    );
    // Financial facts are still durable for both events, but only the valid one is
    // projectable.
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_metering_facts")
            .await,
        2
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_logs")
            .await,
        1
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        2
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn settlement_classifies_eligibility_and_account_mismatch_without_charging() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let priced = event(RequestLogOutcome::Succeeded);
    let mut unknown = priced.clone();
    unknown.id = Uuid::new_v4();
    unknown.billing.as_mut().unwrap().cost_amount = None;
    unknown.billing.as_mut().unwrap().usage = None;
    let mut invalid = priced.clone();
    invalid.id = Uuid::new_v4();
    invalid.model_id = None;
    let mut rejected = event(RequestLogOutcome::Rejected);
    rejected.billing = None;
    let mut standalone = unknown.clone();
    standalone.id = Uuid::new_v4();
    standalone.api_operation = ApiOperation::StandaloneWebSearch;
    standalone.api_format = ApiFormat::OpenAiResponses;
    let mut zero = event(RequestLogOutcome::Failed);
    zero.billing = None;
    zero.model_id = None;
    pipeline
        .metering()
        .record_batch(&[
            priced.clone(),
            unknown.clone(),
            invalid.clone(),
            rejected.clone(),
            zero.clone(),
            standalone.clone(),
        ])
        .await
        .unwrap();
    let counts = pipeline.metering().reconciliation_counts().await.unwrap();
    assert_eq!(
        (counts.unknown, counts.invalid, counts.account_mismatch),
        (1, 1, 0)
    );
    assert_eq!(
        pipeline
            .scalar::<i64>(
                "SELECT count(*) FROM request_settlement_pending
                 WHERE request_id IN (
                     SELECT id FROM request_metering_facts
                     WHERE amount_state IN ('priced','zero_by_policy')
                 )",
            )
            .await,
        2
    );
    // Only priced and zero-by-policy facts may carry a receipt, and the receipt
    // must match the fact's exact amount and currency.
    for (id, note) in [
        (unknown.id, "unknown is not billable"),
        (invalid.id, "invalid priced evidence is not billable"),
        (rejected.id, "rejected is not billable"),
        (standalone.id, "standalone web search is not billable"),
    ] {
        let error = pipeline
            .attempt(&format!(
                "INSERT INTO request_settlements(request_id,cost_amount) VALUES ('{id}','0')"
            ))
            .await
            .expect_err(note);
        assert!(
            error.contains("request_settlements_eligibility_check"),
            "{note}: {error}"
        );
    }
    let error = pipeline
        .attempt(&format!(
            "INSERT INTO request_settlements(request_id,cost_amount,currency)
             VALUES ('{}','1','USD')",
            priced.id
        ))
        .await
        .expect_err("a receipt must match the fact amount exactly");
    assert!(
        error.contains("request_settlements_eligibility_check"),
        "{error}"
    );
    // Ineligible facts report distinct outcomes and keep their evidence.
    assert_eq!(
        pipeline.settlements().settle(unknown.id).await.unwrap(),
        RequestLogSettlementOutcome::NotBillable
    );
    assert_eq!(
        pipeline.settlements().settle(invalid.id).await.unwrap(),
        RequestLogSettlementOutcome::NotBillable
    );
    assert_eq!(
        pipeline.settlements().settle(rejected.id).await.unwrap(),
        RequestLogSettlementOutcome::NotBillable
    );
    assert_eq!(
        pipeline.settlements().settle(Uuid::new_v4()).await.unwrap(),
        RequestLogSettlementOutcome::NotFound
    );

    // An account mismatch is reported separately and never auto-charged.
    let other_user = Uuid::new_v4();
    pipeline
        .execute(&format!(
            "INSERT INTO users(id,display_name,role,status) VALUES ('{other_user}','mismatch','user','active')"
        ))
        .await;
    let mut mismatch = priced.clone();
    mismatch.id = Uuid::new_v4();
    mismatch.user_id = other_user;
    pipeline
        .metering()
        .record_batch(std::slice::from_ref(&mismatch))
        .await
        .unwrap();
    assert_eq!(
        pipeline.settlements().settle(mismatch.id).await.unwrap(),
        RequestLogSettlementOutcome::AccountMismatch
    );
    assert_eq!(
        pipeline
            .metering()
            .reconciliation_counts()
            .await
            .unwrap()
            .account_mismatch,
        1
    );

    // A bounded oldest-first scan settles exactly the eligible work: the priced
    // fact and the zero-by-policy failure, leaving unknown/invalid/mismatch alone.
    let settled = pipeline.settlements().settle_pending(4096).await.unwrap();
    assert_eq!(settled.len(), 2);
    assert_eq!(
        settled
            .iter()
            .filter(|outcome| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
            .count(),
        2
    );
    // The account-mismatch fact keeps its pending work for reconciliation rather
    // than being silently dropped or charged.
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        1
    );
    assert_eq!(
        pipeline.accounts().await,
        (
            -priced.effective_cost_amount().unwrap(),
            priced.effective_cost_amount().unwrap()
        )
    );
    // Nothing eligible remains, so a second scan is empty and idempotent.
    assert!(
        pipeline
            .settlements()
            .settle_pending(4096)
            .await
            .unwrap()
            .is_empty()
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn settlement_batches_aggregate_per_account_before_updating() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let events = (0..3)
        .map(|_| event(RequestLogOutcome::Succeeded))
        .collect::<Vec<_>>();
    let cost = events[0].effective_cost_amount().unwrap();
    pipeline.metering().record_batch(&events).await.unwrap();
    let ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
    let outcomes = pipeline.settlements().settle_batch(&ids).await.unwrap();
    assert_eq!(outcomes.len(), 3);
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
    );
    assert_eq!(
        pipeline.accounts().await,
        (-cost * Decimal::from(3), cost * Decimal::from(3))
    );
    // Duplicate ids in one call are deduplicated and never double-charged.
    let repeated = [ids[0], ids[0], ids[1]];
    let outcomes = pipeline
        .settlements()
        .settle_batch(&repeated)
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| *outcome == RequestLogSettlementOutcome::AlreadyBilled)
    );
    assert_eq!(
        pipeline.accounts().await,
        (-cost * Decimal::from(3), cost * Decimal::from(3))
    );
    pipeline.finish().await;
}

#[tokio::test]
async fn account_overflow_aborts_the_whole_settlement_transaction() {
    let pipeline = Pipeline::new().await;
    pipeline.seed().await;
    let events = (0..2)
        .map(|_| event(RequestLogOutcome::Succeeded))
        .collect::<Vec<_>>();
    let cost = events[0].effective_cost_amount().unwrap();
    pipeline.metering().record_batch(&events).await.unwrap();
    // Fill the balance to the top of the column range so the second receipt would
    // exceed numeric(24,8); the entire transaction must roll back, receipts and
    // pending included.
    pipeline
        .execute(&format!(
            "UPDATE users SET balance_amount='-9999999999999999.99999999',updated_at=ag_now() WHERE id='{USER}'"
        ))
        .await;
    let ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
    // Move one account to the maximum positive balance and require the -cost
    // subtraction to underflow the column's negative range.
    pipeline
        .execute("UPDATE api_keys SET quota_used_amount='0',updated_at=ag_now()")
        .await;
    assert!(
        pipeline.settlements().settle_batch(&ids).await.is_err(),
        "an out-of-range balance must abort, not round"
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlements")
            .await,
        0
    );
    assert_eq!(
        pipeline
            .scalar::<i64>("SELECT count(*) FROM request_settlement_pending")
            .await,
        2
    );
    assert_eq!(
        pipeline.accounts().await,
        (
            Decimal::from_str_exact("-9999999999999999.99999999").unwrap(),
            Decimal::ZERO
        )
    );
    // A normal balance still settles both receipts exactly once.
    pipeline
        .execute(&format!(
            "UPDATE users SET balance_amount='0',updated_at=ag_now() WHERE id='{USER}'"
        ))
        .await;
    let outcomes = pipeline.settlements().settle_batch(&ids).await.unwrap();
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
    );
    assert_eq!(
        pipeline.accounts().await,
        (-cost * Decimal::from(2), cost * Decimal::from(2))
    );
    pipeline.finish().await;
}
