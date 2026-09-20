//! Direct-SQL business schema contracts; no mock repositories or permissive SQLite connections.

use super::*;
use ai_gateway::persistence::sqlite::{SqliteDate, SqliteTimestamp, SqliteUuid};
use serde_json::Value;
use uuid::Uuid;

const USER: &str = "10000000-0000-0000-0000-000000000001";
const KEY: &str = "20000000-0000-0000-0000-000000000001";
const MODEL: &str = "30000000-0000-0000-0000-000000000001";
const GROUP: &str = "40000000-0000-0000-0000-000000000001";
const CHANNEL: &str = "50000000-0000-0000-0000-000000000001";
const PROFILE: &str = "60000000-0000-0000-0000-000000000001";
const RULE: &str = "70000000-0000-0000-0000-000000000001";
const REQUEST: &str = "80000000-0000-0000-0000-000000000001";
const CODEX_GROUP: &str = "40000000-0000-0000-0000-000000000002";
const CODEX_CHANNEL: &str = "50000000-0000-0000-0000-000000000002";

async fn schema() -> (tempfile::TempDir, SqliteDatabase) {
    let (directory, db) = database().await;
    assert_eq!(db.install_schema().await.unwrap(), 4);
    (directory, db)
}

async fn execute(db: &SqliteDatabase, sql: &str) -> Result<(), sqlx::Error> {
    let mut tx = db.begin_write().await.unwrap();
    sqlx::Executor::execute(&mut *tx, sqlx::AssertSqlSafe(sql.to_owned())).await?;
    tx.commit().await
}

async fn seed(db: &SqliteDatabase) {
    execute(db, &format!(
        "INSERT INTO users(id,display_name) VALUES ('{USER}','User');
         INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
         VALUES ('{KEY}','{USER}','Key','test-only-secret','active','[\"open_ai_responses\"]','[\"proxy\"]');
         INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
             cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
         VALUES ('{MODEL}','test-model','Test',1000000,'1','0','0','2','2026-01-01T00:00:00.000000Z');
         INSERT INTO channel_groups(id,name,api_format) VALUES ('{GROUP}','Group','open_ai_responses');
         INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind,available_models)
         VALUES ('{CHANNEL}','{GROUP}','open_ai_responses','Channel','https://upstream.invalid','none','[\"wire\"]');
         INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
         INSERT INTO model_rules(id,model_routing_profile_id,api_format,enabled)
         VALUES ('{RULE}','{PROFILE}','open_ai_responses',0);"
    )).await.unwrap();
}

fn routes() -> String {
    format!("INSERT INTO model_rule_routing_tiers VALUES ('{RULE}','open_ai_responses',0,'weighted_random');
        INSERT INTO model_rule_routing_candidates VALUES ('{RULE}','open_ai_responses',0,'{CHANNEL}','wire',1);")
}

async fn scalar<T>(db: &SqliteDatabase, query: &str) -> T
where
    for<'r> T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> + Send + Unpin,
{
    let mut reader = db.acquire_read().await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(query.to_owned()))
        .fetch_one(&mut *reader)
        .await
        .unwrap()
}

#[tokio::test]
async fn complete_baseline_has_all_columns_constraints_and_seeds_and_reopens() {
    let (directory, db) = schema().await;
    let inventory: Value =
        serde_json::from_str(include_str!("../fixtures/sqlite-schema-inventory.json")).unwrap();
    let mut reader = db.acquire_read().await.unwrap();
    for (table, expected) in inventory.as_object().unwrap() {
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA table_xinfo('{table}')"
        )))
        .fetch_all(&mut *reader)
        .await
        .unwrap();
        let columns: Vec<String> = rows.iter().map(|row| row.get("name")).collect();
        assert_eq!(
            serde_json::to_value(columns).unwrap(),
            expected["columns"],
            "{table}"
        );
        let sql: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_schema WHERE name=? AND type='table'")
                .bind(table)
                .fetch_one(&mut *reader)
                .await
                .unwrap();
        assert!(sql.ends_with("STRICT"), "{table}");
        for name in expected["checks"]
            .as_array()
            .unwrap()
            .iter()
            .chain(expected["constraints"].as_array().unwrap())
        {
            let name = name.as_str().unwrap();
            if name == "request_log_ingest_pkey" {
                continue;
            }
            assert!(sql.contains(name), "missing {table}.{name}");
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM user_groups WHERE system_role IN ('user','admin')",
    )
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
            .fetch_one(&mut *reader)
            .await
            .unwrap(),
        "ok"
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *reader)
            .await
            .unwrap()
            .is_empty()
    );
    drop(reader);
    seed(&db).await;
    assert_eq!(db.install_schema().await.unwrap(), 0);
    db.close().await;
    let reopened = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    assert_eq!(reopened.install_schema().await.unwrap(), 0);
    reopened.close().await;
}

#[tokio::test]
async fn complete_baseline_and_guards_rollback_as_one_pending_batch() {
    let (_directory, db) = database().await;
    let migrations = [
        SqliteMigration {
            version: 1,
            description: "baseline",
            sql: include_str!("../../migrations/sqlite/0001_baseline.sql"),
        },
        SqliteMigration {
            version: 2,
            description: "guards",
            sql: include_str!("../../migrations/sqlite/0002_guards.sql"),
        },
        SqliteMigration {
            version: 3,
            description: "failure",
            sql: "SELECT deliberate_missing_function()",
        },
    ];
    assert!(db.migrate(&migrations).await.is_err());
    assert!(!table_exists(&db, "users").await);
    assert!(!table_exists(&db, "request_metering_facts").await);
    assert!(!table_exists(&db, "_gateway_routing_assertions").await);
    assert_eq!(db.install_schema().await.unwrap(), 4);
    db.close().await;
}

#[tokio::test]
async fn storage_rejects_lossy_types_invalid_arrays_and_wrong_shapes() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    for assignment in [
        "quota_limit_amount='10000000000000000'",
        "quota_limit_amount='0.000000001'",
        "quota_limit_amount='1.0'",
        "quota_limit_amount='-1'",
        "allowed_api_formats='[]'",
        "allowed_api_formats='[null]'",
        "allowed_api_formats='[\"not-a-format\"]'",
        "allowed_group_ids='[\"bad-uuid\"]'",
        "allowed_channel_ids='[null]'",
        "permissions='[\"root\"]'",
        "permissions='[null]'",
        "permissions='[]'",
        "is_system=2",
        "requests_per_minute=2147483648",
        "expires_at='2026-01-01 00:00:00'",
        "name=printf('%101s','x')",
        "name='prefix'||char(0)||'suffix'",
        "updated_at='not-a-date'",
    ] {
        assert!(
            execute(
                &db,
                &format!("UPDATE api_keys SET updated_at=ag_now(), {assignment} WHERE id='{KEY}'")
            )
            .await
            .is_err(),
            "{assignment}"
        );
    }
    for assignment in [
        "input_unit_price='1000000000000'",
        "input_unit_price='0.0000000000001'",
        "advanced_billing='[]'",
        "currency='EUR'",
        "price_effective_at='2026-02-30T00:00:00.000000Z'",
    ] {
        assert!(
            execute(
                &db,
                &format!("UPDATE models SET updated_at=ag_now(), {assignment} WHERE id='{MODEL}'")
            )
            .await
            .is_err(),
            "{assignment}"
        );
    }
    execute(
        &db,
        &format!("UPDATE users SET updated_at=ag_now(),balance_amount='-9999999999999999.99999999' WHERE id='{USER}'"),
    )
    .await
    .unwrap();
    let balance: SqliteDecimal = scalar(
        &db,
        &format!("SELECT balance_amount FROM users WHERE id='{USER}'"),
    )
    .await;
    assert_eq!(
        balance.0,
        Decimal::from_str("-9999999999999999.99999999").unwrap()
    );
    db.close().await;
}

#[tokio::test]
async fn json_storage_rejects_postgres_incompatible_unicode() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    for document in [
        r#"{"value":"\ud800"}"#,
        r#"{"\udc00":"value"}"#,
        r#"{"nested":["\u0000"]}"#,
        r#"{"\u0000":1}"#,
        r#"{"x":"\u0000","x":1}"#,
        r#"{"outer":{"x":["\u0000"],"x":1},"outer":{}}"#,
        r#"{"x":"\ud800","x":1}"#,
    ] {
        let mut tx = db.begin_write().await.unwrap();
        assert!(
            sqlx::query("UPDATE models SET updated_at=ag_now(),source_payload=? WHERE id=?")
                .bind(document)
                .bind(MODEL)
                .execute(&mut *tx)
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
    }
    let mut tx = db.begin_write().await.unwrap();
    sqlx::query("UPDATE models SET updated_at=ag_now(),source_payload=? WHERE id=?")
        .bind(r#"{"emoji":"\ud83d\ude00"}"#)
        .bind(MODEL)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn normalized_log_replay_preserves_on_conflict_semantics() {
    use ai_gateway::domain::{ApiFormat, ApiOperation};
    let (_directory, db) = schema().await;
    seed(&db).await;
    for format in ApiFormat::ALL {
        let id = Uuid::new_v4().to_string();
        let operation = ApiOperation::legacy_default(format);
        for _ in 0..2 {
            let mut tx = db.begin_write().await.unwrap();
            sqlx::query("INSERT INTO request_logs(
                id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,outcome)
                VALUES (?,ag_now(),ag_now(),?,?,?,?,?,'rejected') ON CONFLICT(id) DO NOTHING")
                .bind(&id).bind(USER).bind(KEY).bind(format.as_str()).bind(operation.as_str())
                .bind("model").execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
    }
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM request_logs").await,
        3
    );
    db.close().await;
}

fn failure_kind(error: sqlx::Error) -> ai_gateway::persistence::StorageFailureKind {
    match ai_gateway::persistence::RepositoryError::from(error) {
        ai_gateway::persistence::RepositoryError::Storage(error) => error.kind(),
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn sqlite_errors_keep_input_dependency_conflict_and_internal_distinct() {
    use ai_gateway::persistence::StorageFailureKind as Kind;
    let (directory, db) = schema().await;
    seed(&db).await;
    for (assignment, expected) in [
        (
            format!("channel_group_id='{REQUEST}'"),
            Kind::RoutingDependency,
        ),
        ("channel_group_id='bad-uuid'".into(), Kind::InvalidInput),
        ("api_format='not-a-format'".into(), Kind::InvalidInput),
        ("upstream_auth_kind='not-auth'".into(), Kind::InvalidInput),
    ] {
        let sql =
            format!("UPDATE channels SET updated_at=ag_now(),{assignment} WHERE id='{CHANNEL}'");
        assert_eq!(
            failure_kind(execute(&db, &sql).await.unwrap_err()),
            expected
        );
    }
    assert_eq!(
        failure_kind(execute(&db, "SELECT no_such_function()").await.unwrap_err()),
        Kind::Internal
    );
    execute(&db,&format!("INSERT INTO proxies(id,name,proxy_url) VALUES ('{REQUEST}','Proxy','http://proxy.invalid');
        UPDATE channels SET updated_at=ag_now(),proxy_id='{REQUEST}' WHERE id='{CHANNEL}';")).await.unwrap();
    assert_eq!(
        failure_kind(execute(&db, "DELETE FROM proxies").await.unwrap_err()),
        Kind::RoutingDependency
    );
    let tx = db.begin_write().await.unwrap();
    let mut competing = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(directory.path().join("gateway.sqlite"))
            .busy_timeout(Duration::ZERO),
    )
    .await
    .unwrap();
    let error = competing.begin_with("BEGIN IMMEDIATE").await.err().unwrap();
    assert_eq!(failure_kind(error), Kind::Conflict);
    competing.close().await.unwrap();
    tx.rollback().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn routing_shapes_are_deferred_and_helper_rows_cannot_bypass_them() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    assert!(
        execute(
            &db,
            &format!(
                "UPDATE model_rules SET updated_at=ag_now(),enabled=1 WHERE id='{RULE}';
        UPDATE _gateway_routing_assertions SET valid=1 WHERE rule_id='{RULE}';"
            )
        )
        .await
        .is_err()
    );
    assert!(
        execute(
            &db,
            &format!("UPDATE model_rules SET updated_at=ag_now(),enabled=1 WHERE id='{RULE}'")
        )
        .await
        .is_err()
    );
    execute(
        &db,
        &format!(
            "UPDATE model_rules SET updated_at=ag_now(),enabled=1 WHERE id='{RULE}'; {}",
            routes()
        ),
    )
    .await
    .unwrap();
    assert!(
        execute(
            &db,
            &format!("DELETE FROM model_rule_routing_candidates WHERE model_rule_id='{RULE}'")
        )
        .await
        .is_err()
    );
    execute(
        &db,
        &format!(
            "DELETE FROM model_rule_routing_tiers WHERE model_rule_id='{RULE}'; {}",
            routes()
        ),
    )
    .await
    .unwrap();
    for sql in [
        format!("DELETE FROM _gateway_routing_assertions WHERE rule_id='{RULE}'"),
        "DELETE FROM _gateway_true".into(),
        "UPDATE _gateway_true SET value=0".into(),
        format!("UPDATE _gateway_routing_assertions SET valid=0 WHERE rule_id='{RULE}'"),
        format!("UPDATE model_rule_routing_candidates SET model_rule_id='{PROFILE}'"),
    ] {
        assert!(execute(&db, &sql).await.is_err(), "{sql}");
    }
    assert!(execute(&db,&format!("INSERT INTO model_rule_routing_candidates VALUES ('{RULE}','open_ai_responses',0,'{CHANNEL}','wire',9)")).await.is_err());
    execute(&db,&format!("INSERT INTO model_rule_routing_candidates VALUES ('{RULE}','open_ai_responses',0,'{CHANNEL}','another-wire',9)")).await.unwrap();
    execute(
        &db,
        &format!("DELETE FROM model_routing_profiles WHERE id='{PROFILE}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM _gateway_routing_assertions").await,
        0
    );
    db.close().await;
}

#[tokio::test]
async fn normalized_writes_keep_one_transaction_timestamp_and_require_functions() {
    let (directory, db) = schema().await;
    seed(&db).await;
    let mut tx = db.begin_write().await.unwrap();
    assert!(sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE users SET display_name='Renamed',updated_at='2001-01-01T00:00:00.000000Z' WHERE id='{USER}'"))).execute(&mut *tx).await.is_err());
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE users SET display_name='Renamed',updated_at=ag_now() WHERE id='{USER}'"
    )))
    .execute(&mut *tx)
    .await
    .unwrap();
    let stamp: String = sqlx::query_scalar("SELECT ag_now()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let persisted: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT updated_at FROM users WHERE id='{USER}'"
    )))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(persisted, stamp);
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE models SET updated_at=ag_now(),display_name='Renamed Model' WHERE id='{MODEL}'"
    )))
    .execute(&mut *tx)
    .await
    .unwrap();
    let model_stamp: String = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT updated_at FROM models WHERE id='{MODEL}'"
    )))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(model_stamp, stamp);
    tx.commit().await.unwrap();
    db.close().await;
    let mut raw = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(directory.path().join("gateway.sqlite")),
    )
    .await
    .unwrap();
    assert!(
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE users SET balance_amount='1' WHERE id='{USER}'"
        )))
        .execute(&mut raw)
        .await
        .is_err()
    );
    raw.close().await.unwrap();
}

#[tokio::test]
async fn draft_rule_identity_update_and_cascade_preserve_assertion_coverage() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    execute(
        &db,
        &format!("UPDATE model_rules SET updated_at=ag_now(),id='{REQUEST}' WHERE id='{RULE}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        scalar::<String>(&db, "SELECT rule_id FROM _gateway_routing_assertions").await,
        REQUEST
    );
    assert!(
        execute(
            &db,
            &format!("UPDATE model_rules SET updated_at=ag_now(),enabled=1 WHERE id='{REQUEST}'")
        )
        .await
        .is_err()
    );
    execute(
        &db,
        &format!("DELETE FROM model_routing_profiles WHERE id='{PROFILE}'"),
    )
    .await
    .unwrap();
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM _gateway_routing_assertions").await,
        0
    );
    db.close().await;
}

#[tokio::test]
async fn domain_types_roundtrip_with_exact_storage_contracts() {
    let (_directory, db) = schema().await;
    let mut tx = db.begin_write().await.unwrap();
    let id = SqliteUuid(Uuid::parse_str(USER).unwrap());
    let time = SqliteTimestamp(
        chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123456Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    );
    let date = SqliteDate(chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap());
    let row = sqlx::query("SELECT ? AS id, ? AS time, ? AS date")
        .bind(id)
        .bind(time)
        .bind(date)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(row.get::<SqliteUuid, _>("id"), id);
    assert_eq!(row.get::<SqliteTimestamp, _>("time"), time);
    assert_eq!(row.get::<SqliteDate, _>("date"), date);
    assert!(
        sqlx::query_scalar::<_, SqliteTimestamp>("SELECT '2026-01-02T03:04:05Z'")
            .fetch_one(&mut *tx)
            .await
            .is_err()
    );
    assert!(
        sqlx::query_scalar::<_, SqliteUuid>("SELECT '10000000000000000000000000000001'")
            .fetch_one(&mut *tx)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.close().await;
}

fn fact(outcome: &str, cost: &str) -> String {
    format!("INSERT INTO request_metering_facts (
        id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
        request_protocol,client_model,outcome,cost_amount,peak_pricing)
        VALUES ('{REQUEST}','2026-01-01T00:00:00.000000Z','2026-01-01T00:00:01.000000Z',
        '{USER}','{KEY}','client','open_ai_responses','responses','non_stream','test-model','{outcome}',{cost},0);")
}

#[tokio::test]
async fn financial_guards_keep_facts_receipts_pending_and_ingress_independent() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    execute(&db, &fact("failed", "'0'")).await.unwrap();
    execute(&db,&format!("INSERT INTO request_settlement_pending VALUES ('{REQUEST}','2026-01-01T00:00:01.000000Z');
        INSERT INTO request_log_ingest(request_log_id,schema_version,payload) VALUES ('{REQUEST}',1,x'01');")).await.unwrap();
    for sql in [
        "UPDATE request_metering_facts SET cost_amount='1'".into(),
        "DELETE FROM request_metering_facts".into(),
        "DELETE FROM request_settlement_pending".into(),
        "UPDATE request_settlement_pending SET completed_at=ag_now()".into(),
        "DELETE FROM request_log_ingest".into(),
        format!("INSERT INTO request_settlements(request_id,cost_amount) VALUES ('{REQUEST}','1')"),
    ] {
        assert!(execute(&db, &sql).await.is_err(), "{sql}");
    }
    execute(
        &db,
        &format!(
            "INSERT INTO request_settlements(request_id,cost_amount) VALUES ('{REQUEST}','0');
        DELETE FROM request_settlement_pending WHERE request_id='{REQUEST}';"
        ),
    )
    .await
    .unwrap();
    for sql in [
        "DELETE FROM request_settlements",
        "UPDATE request_settlements SET settled_at=ag_now()",
    ] {
        assert!(execute(&db, sql).await.is_err());
    }
    execute(&db,&format!("INSERT INTO request_logs(id,started_at,completed_at,user_id,api_key_id,
        api_format,api_operation,client_model,outcome,cost_amount) VALUES ('{REQUEST}','2026-01-01T00:00:00.000000Z',
        '2026-01-01T00:00:01.000000Z','{USER}','{KEY}','open_ai_responses','responses','test-model','failed','0');")).await.unwrap();
    assert!(
        execute(&db, "DELETE FROM request_log_ingest")
            .await
            .is_err()
    );
    execute(&db,"UPDATE request_log_ingest SET metered_at=ag_now(); DELETE FROM request_log_ingest; DELETE FROM request_logs;").await.unwrap();
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM request_settlements").await,
        1
    );
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM request_metering_facts").await,
        1
    );
    db.close().await;
}

#[tokio::test]
async fn unknown_invalid_and_rejected_facts_never_receive_settlement_receipts() {
    for (outcome, cost, state) in [
        ("succeeded", "NULL", "unknown"),
        ("succeeded", "'1'", "invalid"),
        ("rejected", "NULL", "not_applicable"),
    ] {
        let (_directory, db) = schema().await;
        seed(&db).await;
        execute(&db, &fact(outcome, cost)).await.unwrap();
        assert_eq!(
            scalar::<String>(&db, "SELECT amount_state FROM request_metering_facts").await,
            state
        );
        assert!(execute(&db,&format!("INSERT INTO request_settlements(request_id,cost_amount) VALUES ('{REQUEST}','0')")).await.is_err());
        db.close().await;
    }
}

#[tokio::test]
async fn priced_receipts_require_exact_snapshot_amount_currency_and_key_ownership() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    execute(&db,&format!(
        "INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,request_source,
            api_format,api_operation,request_protocol,client_model,model_id,outcome,
            currency,price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price,
            cache_write_unit_price,output_unit_price,cost_amount,peak_pricing)
        VALUES ('{REQUEST}',ag_now(),ag_now(),'{USER}','{KEY}','client','open_ai_responses','responses',
            'non_stream','test-model','{MODEL}','succeeded','USD',1000000,ag_now(),'1','0','0','2','1.5',0);
        INSERT INTO users(id,display_name) VALUES ('{PROFILE}','Another user');"
    )).await.unwrap();
    for (amount, currency) in [("1.6", "USD"), ("1.5", "EUR")] {
        assert!(
            execute(
                &db,
                &format!(
                    "INSERT INTO request_settlements(request_id,cost_amount,currency)
            VALUES ('{REQUEST}','{amount}','{currency}')"
                )
            )
            .await
            .is_err()
        );
    }
    execute(
        &db,
        &format!("UPDATE api_keys SET updated_at=ag_now(),user_id='{PROFILE}' WHERE id='{KEY}'"),
    )
    .await
    .unwrap();
    assert!(
        execute(
            &db,
            &format!(
                "INSERT INTO request_settlements(request_id,cost_amount,currency)
        VALUES ('{REQUEST}','1.5','USD')"
            )
        )
        .await
        .is_err()
    );
    execute(&db,&format!("UPDATE api_keys SET updated_at=ag_now(),user_id='{USER}' WHERE id='{KEY}';
        INSERT INTO request_settlements(request_id,cost_amount,currency) VALUES ('{REQUEST}','1.5','USD');")).await.unwrap();
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM request_settlements").await,
        1
    );
    db.close().await;
}

async fn codex(db: &SqliteDatabase) {
    execute(db,&format!("INSERT INTO connector_pools(id,connector_kind) VALUES ('{CODEX_GROUP}','codex_oauth');
        INSERT INTO channel_groups(id,name,api_format,connector_kind,connector_pool_id)
        VALUES ('{CODEX_GROUP}','Codex','open_ai_responses','codex_oauth','{CODEX_GROUP}');
        INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind)
        VALUES ('{CODEX_CHANNEL}','{CODEX_GROUP}','open_ai_responses','Account','https://codex.invalid','none');
        INSERT INTO codex_oauth_credentials(channel_id,channel_group_id,connector_pool_id,label,account_id,user_id,
            id_token,access_token,refresh_token,last_refreshed_at)
        VALUES ('{CODEX_CHANNEL}','{CODEX_GROUP}','{CODEX_GROUP}','Account','account','provider-user','test-id','test-access','test-refresh',ag_now());")).await.unwrap();
}

#[tokio::test]
async fn codex_projections_share_only_intended_state_and_keep_images_disabled() {
    let (_directory, db) = schema().await;
    codex(&db).await;
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM connector_pools").await,
        1
    );
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM codex_oauth_credential_channels").await,
        2
    );
    assert_eq!(
        scalar::<i64>(
            &db,
            "SELECT enabled FROM channel_groups WHERE api_format='open_ai_images'"
        )
        .await,
        0
    );
    assert_eq!(scalar::<String>(&db,&format!("SELECT connector_pool_id FROM codex_oauth_credentials WHERE channel_id='{CODEX_CHANNEL}'")).await,CODEX_GROUP);
    execute(&db,&format!("UPDATE channel_groups SET updated_at=ag_now(),sharing_only=1 WHERE id='{CODEX_GROUP}';
        UPDATE channels SET updated_at=ag_now(),name='Updated',billing_multiplier='1.25',supports_websocket=1 WHERE id='{CODEX_CHANNEL}';")).await.unwrap();
    assert_eq!(
        scalar::<i64>(
            &db,
            "SELECT count(*) FROM channel_groups WHERE sharing_only=1"
        )
        .await,
        2
    );
    assert_eq!(
        scalar::<String>(
            &db,
            "SELECT billing_multiplier FROM channels WHERE api_format='open_ai_images'"
        )
        .await,
        "1.25"
    );
    assert_eq!(
        scalar::<i64>(
            &db,
            "SELECT supports_websocket FROM channels WHERE api_format='open_ai_images'"
        )
        .await,
        0
    );
    execute(&db,&format!("UPDATE codex_oauth_credentials SET updated_at=ag_now(),deleted_at=ag_now() WHERE channel_id='{CODEX_CHANNEL}'")).await.unwrap();
    assert_eq!(
        scalar::<i64>(
            &db,
            "SELECT enabled FROM channels WHERE api_format='open_ai_images'"
        )
        .await,
        0
    );
    db.close().await;
}

#[tokio::test]
async fn sharing_binding_identity_and_seat_numbers_are_protected() {
    let (_directory, db) = schema().await;
    codex(&db).await;
    execute(&db,&format!("INSERT INTO codex_sharing_groups(id,credential_id,provider_account_id,provider_user_id,
        name,enabled,seats,primary_limit_amount,secondary_limit_amount,request_reservation_amount,
        user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests,group_max_concurrent_requests)
        VALUES ('{REQUEST}','{CODEX_CHANNEL}','account','provider-user','Car',1,'[null,null]','1','2','0.01',1,1,1,1);")).await.unwrap();
    for sql in [
        "UPDATE codex_sharing_groups SET updated_at=ag_now(),seats='[null]'".to_owned(),
        "UPDATE codex_sharing_groups SET updated_at=ag_now(),provider_user_id='different'".into(),
        "UPDATE codex_sharing_groups SET updated_at=ag_now(),primary_limit_amount='1000000000000'"
            .into(),
        format!(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),account_id='different' WHERE channel_id='{CODEX_CHANNEL}'"
        ),
        format!(
            "UPDATE codex_oauth_credentials SET updated_at=ag_now(),deleted_at=ag_now() WHERE channel_id='{CODEX_CHANNEL}'"
        ),
    ] {
        assert!(execute(&db, &sql).await.is_err(), "{sql}");
    }
    execute(
        &db,
        "UPDATE codex_sharing_groups SET updated_at=ag_now(),seats='[null,null,null]',enabled=0",
    )
    .await
    .unwrap();
    db.close().await;
}

#[tokio::test]
async fn partial_uniques_keep_tombstones_reusable_and_null_credentials_unique() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    codex(&db).await;
    execute(&db,&format!(
        "INSERT INTO channel_groups(id,name,api_format) VALUES ('{REQUEST}','Reusable','open_ai_responses');
         UPDATE channel_groups SET updated_at=ag_now(),enabled=0,deleted_at=ag_now(),deleted_by='{USER}' WHERE id='{REQUEST}';
         INSERT INTO channel_groups(id,name,api_format) VALUES ('{PROFILE}','Reusable','open_ai_responses');
         UPDATE codex_oauth_credentials SET updated_at=ag_now(),account_id=NULL WHERE channel_id='{CODEX_CHANNEL}';"
    )).await.unwrap();
    let result=execute(&db,&format!(
        "INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind)
             VALUES ('{REQUEST}','{CODEX_GROUP}','open_ai_responses','Duplicate','https://codex.invalid','none');
         INSERT INTO codex_oauth_credentials(channel_id,channel_group_id,connector_pool_id,label,account_id,
             user_id,id_token,access_token,refresh_token,last_refreshed_at)
             VALUES ('{REQUEST}','{CODEX_GROUP}','{CODEX_GROUP}','Duplicate',NULL,
                 'provider-user','test-id','test-access','test-refresh',ag_now());"
    )).await;
    assert_eq!(
        failure_kind(result.unwrap_err()),
        ai_gateway::persistence::StorageFailureKind::InvalidInput
    );
    assert_eq!(
        scalar::<i64>(
            &db,
            &format!("SELECT count(*) FROM channels WHERE id='{REQUEST}'")
        )
        .await,
        0
    );
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM codex_oauth_credentials").await,
        1
    );
    db.close().await;
}

#[tokio::test]
async fn tombstones_cannot_be_hard_deleted_restored_or_newly_referenced() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    for table in [
        "users",
        "user_groups",
        "api_keys",
        "channels",
        "channel_groups",
        "models",
    ] {
        assert!(
            execute(&db, &format!("DELETE FROM {table}")).await.is_err(),
            "{table}"
        );
    }
    execute(
        &db,
        &format!(
            "UPDATE models SET updated_at=ag_now(),enabled=0,deleted_at=ag_now(),deleted_by='{USER}' WHERE id='{MODEL}'"
        ),
    )
    .await
    .unwrap();
    for sql in [
        format!(
            "UPDATE models SET updated_at=ag_now(),deleted_at=NULL,deleted_by=NULL WHERE id='{MODEL}'"
        ),
        format!("UPDATE model_rules SET updated_at=ag_now(),enabled=1 WHERE id='{RULE}'"),
        format!(
            "UPDATE channels SET updated_at=ag_now(),test_model='wire',test_pricing_model_id='{MODEL}' WHERE id='{CHANNEL}'"
        ),
        format!(
            "UPDATE model_routing_profiles SET updated_at=ag_now(),model_id='{MODEL}' WHERE id='{PROFILE}'"
        ),
    ] {
        assert!(execute(&db, &sql).await.is_err(), "{sql}");
    }
    assert!(execute(&db,&format!("UPDATE channel_groups SET updated_at=ag_now(),enabled=0,deleted_at=ag_now(),deleted_by='{USER}' WHERE id='{GROUP}'")).await.is_err());
    execute(&db,&format!("UPDATE channels SET updated_at=ag_now(),enabled=0,base_url='https://deleted.invalid',
        available_models='[]',deleted_at=ag_now(),deleted_by='{USER}' WHERE id='{CHANNEL}';
        UPDATE channel_groups SET updated_at=ag_now(),enabled=0,deleted_at=ag_now(),deleted_by='{USER}' WHERE id='{GROUP}';")).await.unwrap();
    assert!(
        execute(
            &db,
            &format!(
                "UPDATE channels SET updated_at=ag_now(),name='Restored' WHERE id='{CHANNEL}'"
            )
        )
        .await
        .is_err()
    );
    assert!(
        execute(
            &db,
            &format!(
                "UPDATE channel_groups SET updated_at=ag_now(),name='Restored' WHERE id='{GROUP}'"
            )
        )
        .await
        .is_err()
    );
    db.close().await;
}

#[tokio::test]
async fn identity_audit_quota_catalog_and_leaderboard_constraints_are_live() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    codex(&db).await;
    execute(&db,&format!(
        "INSERT INTO api_key_policies(id,name) VALUES ('{REQUEST}','Policy');
         INSERT INTO config_templates(id,name,document) VALUES ('{REQUEST}','Template','{{}}');
         INSERT INTO proxies(id,name,proxy_url) VALUES ('{REQUEST}','Proxy','socks5h://proxy.invalid');
         INSERT INTO user_sessions(id,user_id,refresh_token_hash,expires_at)
             VALUES ('{REQUEST}','{USER}',x'01','2100-01-01T00:00:00.000000Z');
         INSERT INTO user_invitations(id,user_id,invited_by,token_hash,expires_at)
             VALUES ('{REQUEST}','{USER}','{USER}',x'01','2100-01-01T00:00:00.000000Z');
         INSERT INTO registration_invitation_codes(id,name,code_hash,user_group_id,created_by,max_uses)
             VALUES ('{REQUEST}','Invitation',x'01','00000000-0000-0000-0000-000000000101','{USER}',1);
         INSERT INTO audit_logs(id,actor_user_id,actor_type,action,object_type,object_id,source_ip_prefix)
             VALUES ('{REQUEST}','{USER}','user','test','user','{USER}','192.0.2.0/24');
         INSERT INTO system_settings(setting_key,value) VALUES ('runtime','{{}}');
         INSERT INTO user_group_codex_quota_visibility(user_group_id,channel_group_id)
             VALUES ('00000000-0000-0000-0000-000000000101','{CODEX_GROUP}');
         INSERT INTO codex_oauth_flows(id,actor_user_id,channel_group_id,label,quota_threshold_percent,
             redirect_uri,state_hash,code_verifier,expires_at)
             VALUES ('{REQUEST}','{USER}','{CODEX_GROUP}','Flow',95,'http://localhost:1455/auth/callback',
                 zeroblob(32),printf('%043d',0),'2100-01-01T00:00:00.000000Z');
         INSERT INTO codex_quota_window_periods(id,credential_id,window_kind,window_seconds,
             started_at,scheduled_reset_at,initial_used_percent,last_used_percent,first_observed_at,last_observed_at)
             VALUES ('{REQUEST}','{CODEX_CHANNEL}','primary',300,'2026-01-01T00:00:00.000000Z',
                 '2026-01-01T00:05:00.000000Z',0,0,ag_now(),ag_now());
         INSERT INTO codex_quota_reset_events(id,credential_id,actor_user_id,requested_at,outcome,windows_reset,correlation_id)
             VALUES ('{REQUEST}','{CODEX_CHANNEL}','{USER}',ag_now(),'no_credit',0,'{REQUEST}');
         INSERT INTO codex_sharing_ledger VALUES (1,'{REQUEST}');
         INSERT INTO spend_leaderboard_periods VALUES ('day','2026-01-01','2026-01-02',ag_now(),'1');
         INSERT INTO spend_leaderboard_entries VALUES ('day','2026-01-01','{USER}',1,1,1,1,'1');"
    )).await.unwrap();
    for sql in [
        "UPDATE user_sessions SET purpose='admin'",
        "UPDATE user_sessions SET expires_at='2000-01-01T00:00:00.000000Z'",
        "UPDATE user_invitations SET expires_at='2000-01-01T00:00:00.000000Z'",
        "UPDATE registration_invitation_codes SET updated_at=ag_now(),used_count=2",
        "UPDATE config_templates SET updated_at=ag_now(),document='[]'",
        "UPDATE proxies SET updated_at=ag_now(),proxy_url='file:///invalid'",
        "UPDATE codex_oauth_flows SET state_hash=x'00'",
        "UPDATE codex_oauth_flows SET quota_threshold_percent=101",
        "UPDATE codex_quota_window_periods SET updated_at=ag_now(),ended_at='2025-01-01T00:00:00.000000Z',reset_reason='manual'",
        "UPDATE codex_quota_window_periods SET updated_at=ag_now(),last_used_percent=101",
        "UPDATE codex_quota_reset_events SET windows_reset=3",
        "UPDATE codex_sharing_ledger SET singleton=0",
        "UPDATE spend_leaderboard_entries SET priced_request_count=2",
        "UPDATE spend_leaderboard_periods SET period_end='2026-01-01'",
        "UPDATE audit_logs SET action='changed'",
        "DELETE FROM audit_logs",
        "UPDATE system_settings SET updated_at=ag_now(),value='[]'",
    ] {
        assert!(execute(&db, sql).await.is_err(), "{sql}");
    }
    execute(&db, "DELETE FROM spend_leaderboard_periods")
        .await
        .unwrap();
    assert_eq!(
        scalar::<i64>(&db, "SELECT count(*) FROM spend_leaderboard_entries").await,
        0
    );
    db.close().await;
}
