use super::*;
use ai_gateway::persistence::{
    CostStatisticsFilter, MeteringQueries, MeteringRepository, MeteringWriteOutcome,
    SettlementRepository, StatisticsGranularity,
};
use rust_decimal::Decimal;
use serde_json::json;

#[path = "rehearsal.rs"]
mod rehearsal;

async fn accounts(pool: &PgPool, key: Uuid) -> (Decimal, Decimal) {
    sqlx::query_as(
        "SELECT account.balance_amount,key.quota_used_amount
         FROM users AS account JOIN api_keys AS key ON key.user_id=account.id WHERE key.id=$1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn wait_for_receipts(pool: &PgPool, ids: &[Uuid], expected: i64) {
    timeout(Duration::from_secs(10), async {
        loop {
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM request_settlements WHERE request_id=ANY($1)",
            )
            .bind(ids)
            .fetch_one(pool)
            .await
            .unwrap();
            if count == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("financial receipts must progress independently of logs");
}

#[tokio::test]
async fn locked_or_invalid_log_projection_does_not_block_settlement_or_sharing_recovery() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let directory = tempfile::tempdir().unwrap();
    let config = RequestLoggingConfig {
        spool_directory: directory.path().into(),
        settlement_interval_milliseconds: 10,
        shutdown_drain_seconds: 1,
        ..RequestLoggingConfig::default()
    };
    let repository = RequestLogRepository::new(database.pool.clone());
    let queries = MeteringQueries::new(database.pool.clone());
    let valid = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut invalid_display = valid.clone();
    invalid_display.id = Uuid::new_v4();
    invalid_display.response_status_code = Some(99);
    let ids = [valid.id, invalid_display.id];
    let cost = valid.effective_cost_amount().unwrap();
    let mut locked = database.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE request_logs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locked)
        .await
        .unwrap();
    let (sink, worker) = DurableRequestLogWorker::start(repository.clone(), &config)
        .await
        .unwrap();
    sink.try_record(valid.clone());
    sink.try_record(invalid_display);
    wait_for_receipts(&database.pool, &ids, 2).await;
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (-cost * Decimal::from(2), cost * Decimal::from(2))
    );
    assert_eq!(
        queries.sharing_completed_costs(&ids).await.unwrap().len(),
        2
    );
    let report = queries
        .cost_statistics(CostStatisticsFilter {
            started_at: valid.started_at - chrono::Duration::seconds(1),
            ended_at: valid.completed_at + chrono::Duration::seconds(1),
            granularity: StatisticsGranularity::Hour,
            user_id: Some(seed.user),
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: true,
        })
        .await
        .unwrap();
    assert_eq!(report.summary.cost_amount, cost * Decimal::from(2));
    let projected: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
        .fetch_one(&mut *locked)
        .await
        .unwrap();
    assert_eq!(projected, 0);
    drop(sink);
    worker.shutdown().await;
    locked.rollback().await.unwrap();

    let (sink, worker) = DurableRequestLogWorker::start(repository, &config)
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        loop {
            let projected: i64 =
                sqlx::query_scalar("SELECT count(*) FROM request_logs WHERE id=$1")
                    .bind(valid.id)
                    .fetch_one(&database.pool)
                    .await
                    .unwrap();
            let bad: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM request_log_ingest
                 WHERE request_log_id=$1 AND metered_at IS NOT NULL
                   AND last_error_code='invalid_response_status'",
            )
            .bind(ids[1])
            .fetch_one(&database.pool)
            .await
            .unwrap();
            if projected == 1 && bad == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    sink.try_record(valid.clone());
    let mut financial_conflict = valid.clone();
    financial_conflict.billing.as_mut().unwrap().cost_amount = Some(Decimal::ONE);
    sink.try_record(financial_conflict);
    let mut display_conflict = valid;
    display_conflict.error_summary = Some("different diagnostic".into());
    sink.try_record(display_conflict);
    timeout(Duration::from_secs(10), async {
        loop {
            let financial: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM request_log_ingest
                 WHERE request_log_id=$1 AND metered_at IS NULL
                   AND metering_last_error_code='financial_replay_conflict'",
            )
            .bind(ids[0])
            .fetch_one(&database.pool)
            .await
            .unwrap();
            let display: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM request_log_ingest
                 WHERE request_log_id=$1 AND metered_at IS NOT NULL
                   AND last_error_code='duplicate_conflict'",
            )
            .bind(ids[0])
            .fetch_one(&database.pool)
            .await
            .unwrap();
            if financial == 1 && display == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(sink);
    worker.shutdown().await;
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (-cost * Decimal::from(2), cost * Decimal::from(2))
    );
    assert_eq!(
        queries.sharing_completed_costs(&ids).await.unwrap().len(),
        2
    );
    database.cleanup().await;
}

#[tokio::test]
async fn ingress_readiness_and_financial_facts_commit_atomically() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::raw_sql(
        "CREATE FUNCTION contract_reject_metering_flag() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
             IF OLD.metered_at IS NULL AND NEW.metered_at IS NOT NULL THEN
                 RAISE EXCEPTION 'injected metering acknowledgement failure';
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER contract_reject_metering_flag BEFORE UPDATE ON request_log_ingest
         FOR EACH ROW EXECUTE FUNCTION contract_reject_metering_flag();",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let directory = tempfile::tempdir().unwrap();
    let config = RequestLoggingConfig {
        spool_directory: directory.path().into(),
        settlement_interval_milliseconds: 10,
        shutdown_drain_seconds: 2,
        ..RequestLoggingConfig::default()
    };
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), &config)
            .await
            .unwrap();
    sink.try_record(event.clone());
    timeout(Duration::from_secs(10), async {
        loop {
            let deferred: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM request_log_ingest WHERE request_log_id=$1
                 AND metered_at IS NULL AND metering_attempt_count>0)",
            )
            .bind(event.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
            if deferred {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM request_metering_facts),
                (SELECT count(*) FROM request_settlements)",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(counts, (0, 0));
    let old_ack = sqlx::query("DELETE FROM request_log_ingest WHERE request_log_id=$1")
        .bind(event.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        old_ack.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (Decimal::ZERO, Decimal::ZERO)
    );
    sqlx::raw_sql("DROP FUNCTION contract_reject_metering_flag() CASCADE")
        .execute(&database.pool)
        .await
        .unwrap();
    wait_for_receipts(&database.pool, &[event.id], 1).await;
    drop(sink);
    worker.shutdown().await;
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (
            -event.effective_cost_amount().unwrap(),
            event.effective_cost_amount().unwrap()
        )
    );
    database.cleanup().await;
}

#[tokio::test]
async fn facts_and_receipts_enforce_eligibility_and_survive_projection_deletion() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let metering = repository.metering();
    let priced = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut unknown = priced.clone();
    unknown.id = Uuid::new_v4();
    unknown.billing.as_mut().unwrap().usage = None;
    unknown.billing.as_mut().unwrap().cost_amount = None;
    let mut invalid = priced.clone();
    invalid.id = Uuid::new_v4();
    invalid.model_id = None;
    let mut rejected = request_log_event(&seed, RequestLogOutcome::Rejected);
    rejected.billing = None;
    let mut search = unknown.clone();
    search.id = Uuid::new_v4();
    search.api_format = ApiFormat::OpenAiResponses;
    search.api_operation = ApiOperation::StandaloneWebSearch;
    let mut zero = request_log_event(&seed, RequestLogOutcome::Failed);
    zero.billing = None;
    zero.model_id = None;
    metering
        .record_batch(&[
            priced.clone(),
            unknown.clone(),
            invalid.clone(),
            rejected.clone(),
            zero.clone(),
            search.clone(),
        ])
        .await
        .unwrap();
    let counts = metering.reconciliation_counts().await.unwrap();
    assert_eq!(
        (counts.unknown, counts.invalid, counts.account_mismatch),
        (1, 1, 0)
    );
    for event in [&unknown, &invalid, &rejected, &search, &priced] {
        let error = sqlx::query(
            "INSERT INTO request_settlements(request_id,cost_amount,currency) VALUES($1,123,'USD')",
        )
        .bind(event.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().constraint(),
            Some("request_settlements_eligibility_check")
        );
    }
    let outcomes = repository.settlements().settle_pending(10).await.unwrap();
    assert_eq!(outcomes.len(), 2);
    let before = accounts(&database.pool, seed.key).await;
    assert_eq!(
        before,
        (
            -priced.effective_cost_amount().unwrap(),
            priced.effective_cost_amount().unwrap()
        )
    );
    for statement in [
        "UPDATE request_metering_facts SET cost_amount=0 WHERE id=$1",
        "DELETE FROM request_metering_facts WHERE id=$1",
        "UPDATE request_settlements SET cost_amount=0 WHERE request_id=$1",
        "DELETE FROM request_settlements WHERE request_id=$1",
    ] {
        let error = sqlx::query(statement)
            .bind(priced.id)
            .execute(&database.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("P0001")
        );
    }
    repository.insert(&priced).await.unwrap();
    sqlx::query("DELETE FROM request_logs WHERE id=$1")
        .bind(priced.id)
        .execute(&database.pool)
        .await
        .unwrap();
    repository.insert(&priced).await.unwrap();
    assert_eq!(
        repository.settlements().settle(priced.id).await.unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    );
    assert_eq!(accounts(&database.pool, seed.key).await, before);
    let error = sqlx::query("UPDATE request_logs SET billed_at=now() WHERE id=$1")
        .bind(priced.id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42703")
    );
    database.cleanup().await;
}

async fn legacy_database() -> TestDatabase {
    let database = TestDatabase::new_unmigrated().await;
    let mut previous = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations"))
        .await
        .unwrap();
    previous.migrations = previous
        .iter()
        .filter(|migration| migration.version <= 62)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    previous.run(&database.pool).await.unwrap();
    database
}

async fn insert_legacy_event(pool: &PgPool, event: &RequestLogEvent, billed: bool) {
    let mut row = serde_json::to_value(event).unwrap();
    row["api_operation"] = json!(
        ai_gateway::persistence::capability_cutover::legacy_settings::operation_name(
            event.api_operation
        )
    );
    let billing = row.as_object_mut().unwrap().remove("billing").unwrap();
    if let Some(billing) = billing.as_object() {
        for (key, value) in billing {
            if key == "price" || key == "usage" {
                if let Some(fields) = value.as_object() {
                    row.as_object_mut().unwrap().extend(fields.clone());
                }
            } else {
                row[key] = value.clone();
            }
        }
    }
    row["cost_amount"] = json!(event.effective_cost_amount());
    row["started_at"] = json!(
        event
            .started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    );
    row["completed_at"] = json!(
        event
            .completed_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    );
    row["price_effective_at"] = json!(event.billing.as_ref().map(|billing| {
        billing
            .price
            .price_effective_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    }));
    row["attempts"] = json!([]);
    row["peak_pricing"] = json!(
        event
            .billing
            .as_ref()
            .is_some_and(|billing| billing.peak_pricing)
    );
    row["billed_at"] = json!(billed.then(|| {
        event
            .completed_at
            .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    }));
    sqlx::query("INSERT INTO request_logs SELECT (jsonb_populate_record(NULL::request_logs,$1)).*")
        .bind(row)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn cutover_backfills_receipts_without_charging_and_replays_legacy_ingress_once() {
    let database = legacy_database().await;
    let seed = seed(&database.pool).await;
    let mut billed = request_log_event(&seed, RequestLogOutcome::Succeeded);
    billed.started_at = "2026-09-16T01:00:00.123456789Z".parse().unwrap();
    billed.completed_at = billed.started_at + chrono::Duration::seconds(1);
    billed.billing.as_mut().unwrap().price.price_effective_at = billed.started_at;
    let pending = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut unknown = request_log_event(&seed, RequestLogOutcome::Succeeded);
    unknown.billing.as_mut().unwrap().usage = None;
    unknown.billing.as_mut().unwrap().cost_amount = None;
    let mut failed = request_log_event(&seed, RequestLogOutcome::Failed);
    failed.billing = None;
    let mut rejected = request_log_event(&seed, RequestLogOutcome::Rejected);
    rejected.billing = None;
    for (event, settled) in [
        (&billed, true),
        (&pending, false),
        (&unknown, false),
        (&failed, false),
        (&rejected, false),
    ] {
        insert_legacy_event(&database.pool, event, settled).await;
    }
    sqlx::query("UPDATE users SET balance_amount=10 WHERE id=$1")
        .bind(seed.user)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET quota_used_amount=$1 WHERE id=$2")
        .bind(billed.effective_cost_amount())
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let before = accounts(&database.pool, seed.key).await;
    let queued = request_log_event(&seed, RequestLogOutcome::Succeeded);
    for event in [&billed, &billed, &queued] {
        let mut payload = serde_json::to_value(event).unwrap();
        payload["api_operation"] = json!(
            ai_gateway::persistence::capability_cutover::legacy_settings::operation_name(
                event.api_operation
            )
        );
        sqlx::query(
            "INSERT INTO request_log_ingest(request_log_id,schema_version,payload) VALUES($1,6,$2)",
        )
        .bind(event.id)
        .bind(serde_json::to_vec(&payload).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();
    }
    run_migrations(&database.pool).await.unwrap();
    assert_eq!(accounts(&database.pool, seed.key).await, before);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM request_metering_facts),(SELECT count(*) FROM request_settlements)",
    ).fetch_one(&database.pool).await.unwrap();
    assert_eq!(counts, (5, 1));
    let settled: DateTime<Utc> =
        sqlx::query_scalar("SELECT settled_at FROM request_settlements WHERE request_id=$1")
            .bind(billed.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        settled.timestamp_micros(),
        billed.completed_at.timestamp_micros()
    );
    let directory = tempfile::tempdir().unwrap();
    let config = RequestLoggingConfig {
        spool_directory: directory.path().into(),
        settlement_interval_milliseconds: 10,
        shutdown_drain_seconds: 2,
        ..RequestLoggingConfig::default()
    };
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), &config)
            .await
            .unwrap();
    wait_for_receipts(
        &database.pool,
        &[billed.id, pending.id, failed.id, queued.id],
        4,
    )
    .await;
    drop(sink);
    worker.shutdown().await;
    let increment =
        pending.effective_cost_amount().unwrap() + queued.effective_cost_amount().unwrap();
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (before.0 - increment, before.1 + increment)
    );
    let ingress: i64 = sqlx::query_scalar("SELECT count(*) FROM request_log_ingest")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(ingress, 0);
    assert_eq!(
        MeteringRepository::new(database.pool.clone())
            .reconciliation_counts()
            .await
            .unwrap()
            .unknown,
        1
    );
    database.cleanup().await;
}

#[tokio::test]
async fn cutover_waits_for_an_inflight_legacy_claim_before_copying_receipts() {
    let database = legacy_database().await;
    let seed = seed(&database.pool).await;
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    insert_legacy_event(&database.pool, &event, false).await;
    let cost = event.effective_cost_amount().unwrap();
    let mut legacy_claim = database.pool.begin().await.unwrap();
    sqlx::query("UPDATE request_logs SET billed_at=now() WHERE id=$1")
        .bind(event.id)
        .execute(&mut *legacy_claim)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET balance_amount=balance_amount-$1 WHERE id=$2")
        .bind(cost)
        .bind(seed.user)
        .execute(&mut *legacy_claim)
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET quota_used_amount=quota_used_amount+$1 WHERE id=$2")
        .bind(cost)
        .bind(seed.key)
        .execute(&mut *legacy_claim)
        .await
        .unwrap();
    let pool = database.pool.clone();
    let migration = tokio::spawn(async move { run_migrations(&pool).await });
    timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='request_logs'::regclass
                 AND mode='AccessExclusiveLock' AND NOT granted)",
            )
            .fetch_one(&database.pool)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!migration.is_finished());
    legacy_claim.commit().await.unwrap();
    timeout(Duration::from_secs(10), migration)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        SettlementRepository::new(database.pool.clone())
            .settle(event.id)
            .await
            .unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    );
    assert_eq!(accounts(&database.pool, seed.key).await, (-cost, cost));
    database.cleanup().await;
}

#[tokio::test]
async fn invalid_historical_receipt_aborts_the_entire_cutover() {
    let database = legacy_database().await;
    let seed = seed(&database.pool).await;
    let other_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,display_name,role,status) VALUES($1,'mismatched','user','active')",
    )
    .bind(other_user)
    .execute(&database.pool)
    .await
    .unwrap();
    let mut event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    event.user_id = other_user;
    insert_legacy_event(&database.pool, &event, true).await;
    let before = accounts(&database.pool, seed.key).await;
    assert!(run_migrations(&database.pool).await.is_err());
    let state: (i64, bool, bool, bool) = sqlx::query_as(
        "SELECT (SELECT max(version) FROM _sqlx_migrations),
                to_regclass('request_metering_facts') IS NULL,
                to_regclass('request_settlements') IS NULL,
                EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='request_logs' AND column_name='billed_at')",
    ).fetch_one(&database.pool).await.unwrap();
    assert_eq!(state, (62, true, true, true));
    assert_eq!(accounts(&database.pool, seed.key).await, before);
    database.cleanup().await;
}

#[tokio::test]
async fn settlement_does_not_wait_for_foreign_key_share_locks() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    MeteringRepository::new(database.pool.clone())
        .record_batch(std::slice::from_ref(&event))
        .await
        .unwrap();
    let mut foreign_keys = database.pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM users WHERE id=$1 FOR KEY SHARE")
        .bind(seed.user)
        .fetch_one(&mut *foreign_keys)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM api_keys WHERE id=$1 FOR KEY SHARE")
        .bind(seed.key)
        .fetch_one(&mut *foreign_keys)
        .await
        .unwrap();
    let settlements = SettlementRepository::new(database.pool.clone());
    let outcome = timeout(Duration::from_secs(2), settlements.settle(event.id))
        .await
        .expect("non-key balance updates must be compatible with FK readers")
        .unwrap();
    assert!(matches!(
        outcome,
        RequestLogSettlementOutcome::Settled { .. }
    ));
    foreign_keys.rollback().await.unwrap();
    database.cleanup().await;
}

#[tokio::test]
async fn recovery_scans_only_outstanding_work_not_settled_history() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let metering = MeteringRepository::new(database.pool.clone());
    let settlements = SettlementRepository::new(database.pool.clone());
    let template = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let history = (0..2_000)
        .map(|_| {
            let mut event = template.clone();
            event.id = Uuid::new_v4();
            event
        })
        .collect::<Vec<_>>();
    metering.record_batch(&history).await.unwrap();
    assert_eq!(
        settlements.settle_pending(4096).await.unwrap().len(),
        history.len()
    );
    let empty: i64 = sqlx::query_scalar("SELECT count(*) FROM request_settlement_pending")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(empty, 0);
    metering.record_batch(&history).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM request_settlement_pending")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );
    let current = request_log_event(&seed, RequestLogOutcome::Succeeded);
    metering
        .record_batch(std::slice::from_ref(&current))
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM request_settlement_pending WHERE request_id=$1")
            .bind(current.id)
            .execute(&database.pool)
            .await
            .is_err()
    );
    sqlx::raw_sql(
        "ANALYZE request_metering_facts; ANALYZE request_settlement_pending; ANALYZE api_keys;",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    let plan: serde_json::Value = sqlx::query_scalar(
        "EXPLAIN (ANALYZE,FORMAT JSON)
         SELECT fact.id FROM request_settlement_pending AS pending
         JOIN request_metering_facts AS fact ON fact.id=pending.request_id
         JOIN api_keys AS key ON key.id=fact.api_key_id AND key.user_id=fact.user_id
         WHERE fact.amount_state IN ('priced','zero_by_policy')
         ORDER BY pending.completed_at,pending.request_id LIMIT 1",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    fn check_plan(node: &serde_json::Value) {
        if node["Relation Name"] == "request_metering_facts" {
            assert!(
                node["Actual Rows"].as_f64().unwrap() * node["Actual Loops"].as_f64().unwrap()
                    <= 1.0,
                "{node}"
            );
        }
        if let Some(children) = node["Plans"].as_array() {
            for child in children {
                check_plan(child);
            }
        }
    }
    check_plan(&plan[0]["Plan"]);
    assert_eq!(settlements.settle_pending(1).await.unwrap().len(), 1);
    assert!(settlements.settle_pending(1).await.unwrap().is_empty());
    database.cleanup().await;
}

#[tokio::test]
async fn conflicting_facts_and_account_mismatches_remain_visible_without_second_charge() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = MeteringRepository::new(database.pool.clone());
    let first = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut conflict = first.clone();
    conflict.billing.as_mut().unwrap().cost_amount = Some(Decimal::ONE);
    let outcomes = repository
        .record_batch(&[first.clone(), conflict])
        .await
        .unwrap();
    assert_eq!(
        outcomes,
        vec![
            MeteringWriteOutcome::Accepted,
            MeteringWriteOutcome::Conflict
        ]
    );
    let other_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users(id,display_name,role,status) VALUES($1,'mismatch','user','active')",
    )
    .bind(other_user)
    .execute(&database.pool)
    .await
    .unwrap();
    let mut mismatch = first.clone();
    mismatch.id = Uuid::new_v4();
    mismatch.user_id = other_user;
    repository.record_batch(&[mismatch.clone()]).await.unwrap();
    let settlements = SettlementRepository::new(database.pool.clone());
    assert_eq!(
        settlements.settle(mismatch.id).await.unwrap(),
        RequestLogSettlementOutcome::AccountMismatch
    );
    settlements.settle(first.id).await.unwrap();
    assert_eq!(
        repository
            .reconciliation_counts()
            .await
            .unwrap()
            .account_mismatch,
        1
    );
    assert_eq!(
        accounts(&database.pool, seed.key).await,
        (
            -first.effective_cost_amount().unwrap(),
            first.effective_cost_amount().unwrap()
        )
    );
    database.cleanup().await;
}
