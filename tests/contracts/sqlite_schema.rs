//! Business schema storage, financial guards, and reopen contracts.

use super::*;
use ai_gateway::persistence::sqlite::{SqliteDate, SqliteTimestamp, SqliteUuid};
use serde_json::Value;
use uuid::Uuid;

const USER: &str = "10000000-0000-0000-0000-000000000001";
const KEY: &str = "20000000-0000-0000-0000-000000000001";
const MODEL: &str = "30000000-0000-0000-0000-000000000001";
const ACCESS: &str = "90000000-0000-0000-0000-000000000001";
const CAPABILITY: &str = "90000000-0000-0000-0000-000000000002";
const GROUP: &str = "40000000-0000-0000-0000-000000000001";
const CHANNEL: &str = "50000000-0000-0000-0000-000000000001";
const PROFILE: &str = "60000000-0000-0000-0000-000000000001";
const RULE: &str = "70000000-0000-0000-0000-000000000001";
const REQUEST: &str = "80000000-0000-0000-0000-000000000001";
const CODEX_GROUP: &str = "40000000-0000-0000-0000-000000000002";
const CODEX_CHANNEL: &str = "50000000-0000-0000-0000-000000000002";

async fn schema() -> (tempfile::TempDir, SqliteDatabase) {
    let (directory, db) = database().await;
    assert_eq!(db.install_schema().await.unwrap(), 1);
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
         INSERT INTO api_keys(id,user_id,name,secret_value,status,permissions)
         VALUES ('{KEY}','{USER}','Key','test-only-secret','active','[\"proxy\"]');
         INSERT INTO models(id,source_model_id,display_name,price_unit_tokens,input_unit_price,
             cached_input_unit_price,cache_write_unit_price,output_unit_price,price_effective_at)
         VALUES ('{MODEL}','test-model','Test',1000000,'1','0','0','2','2026-01-01T00:00:00.000000Z');
         INSERT INTO routing_groups(id,name) VALUES ('{GROUP}','Group');
         INSERT INTO upstream_accesses(id,name,base_url,connector_kind) VALUES ('{ACCESS}','Access','https://upstream.invalid','general');
         INSERT INTO upstream_channels(id,group_id,access_id,name) VALUES ('{CHANNEL}','{GROUP}','{ACCESS}','Channel');
         INSERT INTO channel_capabilities(id,channel_id,operation) VALUES ('{CAPABILITY}','{CHANNEL}','responses');
         INSERT INTO model_routing_profiles(id,model_id) VALUES ('{PROFILE}','{MODEL}');
         INSERT INTO model_operation_rules(id,model_routing_profile_id,operation,enabled)
         VALUES ('{RULE}','{PROFILE}','responses',0);"
    )).await.unwrap();
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
async fn complete_baseline_preserves_columns_storage_guards_and_seeds_and_reopens() {
    let (directory, db) = schema().await;
    let inventory: Value =
        serde_json::from_str(include_str!("../fixtures/canonical-schema-inventory.json")).unwrap();
    let mut reader = db.acquire_read().await.unwrap();
    for (table, expected) in inventory.as_object().unwrap() {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_xinfo(?) ORDER BY cid")
                .bind(table)
                .fetch_all(&mut *reader)
                .await
                .unwrap();
        assert_eq!(
            serde_json::to_value(columns).unwrap(),
            expected["columns"],
            "{table}"
        );
        let strict: i64 = sqlx::query_scalar(
            "SELECT strict FROM pragma_table_list(?) WHERE schema='main' AND type='table'",
        )
        .bind(table)
        .fetch_one(&mut *reader)
        .await
        .unwrap();
        assert_eq!(strict, 1, "{table}");
    }
    let key_fks: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT \"from\",\"table\",\"to\",on_delete FROM pragma_foreign_key_list('api_keys') ORDER BY \"from\"",
    )
    .fetch_all(&mut *reader)
    .await
    .unwrap();
    assert_eq!(
        key_fks,
        ["deleted_by", "user_id"].map(|column| (
            column.to_owned(),
            "users".to_owned(),
            "id".to_owned(),
            "RESTRICT".to_owned(),
        ))
    );
    let index: (i64, i64) = sqlx::query_as(
        "SELECT \"unique\",partial FROM pragma_index_list('api_keys') WHERE name='api_keys_active_user_name_idx'",
    )
    .fetch_one(&mut *reader)
    .await
    .unwrap();
    assert_eq!(index, (1, 1));
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT name FROM pragma_index_info('api_keys_active_user_name_idx') ORDER BY seqno",
        )
        .fetch_all(&mut *reader)
        .await
        .unwrap(),
        ["user_id", "name"]
    );
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
    for (format, status, rejected_by) in [
        (
            "open_ai_images",
            200,
            Some("request_logs_api_operation_format_check"),
        ),
        (
            "open_ai_responses",
            600,
            Some("request_logs_response_status_code_check"),
        ),
        ("open_ai_responses", 200, None),
    ] {
        let result = execute(
            &db,
            &format!(
                "INSERT INTO request_logs(id,started_at,completed_at,user_id,api_key_id,
                api_format,api_operation,client_model,outcome,response_status_code)
             VALUES ('{REQUEST}',ag_now(),ag_now(),'{USER}','{KEY}',
                '{format}','web_search','test-model','succeeded',{status})"
            ),
        )
        .await;
        if let Some(constraint) = rejected_by {
            let error = result.unwrap_err();
            let error = error.as_database_error().unwrap();
            assert_eq!(error.code().as_deref(), Some("275"), "{error}");
            assert!(error.message().ends_with(constraint), "{error}");
        } else {
            result.unwrap();
        }
    }
    assert_eq!(db.install_schema().await.unwrap(), 0);
    db.close().await;
    let reopened = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    assert_eq!(reopened.install_schema().await.unwrap(), 0);
    reopened.close().await;
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
async fn credentials_and_accesses_enforce_explicit_plugin_ownership() {
    let (_directory, database) = schema().await;
    let mut tx = database.begin_write().await.unwrap();
    sqlx::query(
        "INSERT INTO upstream_credentials(id,name,kind,connector_kind,secret,allowed_base_urls)
         VALUES('10000000-0000-0000-0000-000000000003','plugin','bearer','acme','secret','[\"https://example.test\"]')",
    ).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO upstream_accesses(id,name,connector_kind,base_url)
         VALUES('20000000-0000-0000-0000-000000000001','plugin','acme','https://example.test')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "UPDATE upstream_credentials SET connector_kind='general',updated_at=ag_now()
         WHERE id='10000000-0000-0000-0000-000000000003'",
        )
        .execute(&mut *tx)
        .await
        .is_err()
    );
    for (kind, connector) in [
        ("bearer", "codex"),
        ("codex_oauth", "acme"),
        ("bearer", "BAD"),
    ] {
        assert!(sqlx::query(
            "INSERT INTO upstream_credentials(id,name,kind,connector_kind,secret,allowed_base_urls)
             VALUES('10000000-0000-0000-0000-000000000004','invalid',?,?,?,'[\"https://example.test\"]')",
        ).bind(kind).bind(connector).bind((kind == "bearer").then_some("secret"))
            .execute(&mut *tx).await.is_err());
    }
    tx.rollback().await.unwrap();
    database.close().await;
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
    use ai_gateway::domain::ApiOperation;
    let (_directory, db) = schema().await;
    seed(&db).await;
    for operation in [
        ApiOperation::ChatCompletions,
        ApiOperation::Responses,
        ApiOperation::ImagesGeneration,
    ] {
        let format = operation.api_format();
        let id = Uuid::new_v4().to_string();
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
async fn sqlite_errors_keep_input_conflict_and_internal_distinct() {
    use ai_gateway::persistence::StorageFailureKind as Kind;
    let (directory, db) = schema().await;
    seed(&db).await;
    for assignment in [
        format!("group_id='{REQUEST}'"),
        "group_id='bad-uuid'".into(),
        "enabled=9".into(),
    ] {
        let sql = format!(
            "UPDATE upstream_channels SET updated_at=ag_now(),{assignment} WHERE id='{CHANNEL}'"
        );
        assert_eq!(
            failure_kind(execute(&db, &sql).await.unwrap_err()),
            Kind::InvalidInput
        );
    }
    assert_eq!(
        failure_kind(execute(&db, "SELECT no_such_function()").await.unwrap_err()),
        Kind::Internal
    );
    execute(&db,&format!("INSERT INTO proxies(id,name,proxy_url) VALUES ('{REQUEST}','Proxy','http://proxy.invalid');
        UPDATE upstream_accesses SET updated_at=ag_now(),proxy_id='{REQUEST}' WHERE id='{ACCESS}';")).await.unwrap();
    assert_eq!(
        failure_kind(execute(&db, "DELETE FROM proxies").await.unwrap_err()),
        Kind::InvalidInput
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
    execute(db, &format!(
        "INSERT INTO routing_groups(id,name) VALUES ('{CODEX_GROUP}','Codex');
         INSERT INTO upstream_credentials(id,name,kind,connector_kind,allowed_base_urls)
         VALUES ('{CODEX_CHANNEL}','Account','codex_oauth','codex','[\"https://codex.invalid\"]');
         INSERT INTO codex_oauth_credentials(channel_id,label,account_id,user_id,id_token,access_token,refresh_token,last_refreshed_at)
         VALUES ('{CODEX_CHANNEL}','Account','account','provider-user','test-id','test-access','test-refresh',ag_now());"
    )).await.unwrap();
}

#[tokio::test]
async fn active_group_names_are_unique_and_tombstones_release_the_name() {
    let (_directory, db) = schema().await;
    seed(&db).await;
    let insert = format!("INSERT INTO routing_groups(id,name) VALUES ('{REQUEST}','Group')");
    assert!(execute(&db, &insert).await.is_err());
    execute(&db, &format!("UPDATE routing_groups SET updated_at=ag_now(),enabled=0,deleted_at=ag_now() WHERE id='{GROUP}'")).await.unwrap();
    execute(&db, &insert).await.unwrap();
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
        "upstream_channels",
        "channel_capabilities",
        "routing_groups",
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
        format!("UPDATE model_operation_rules SET updated_at=ag_now(),enabled=1 WHERE id='{RULE}'"),
        format!(
            "UPDATE channel_capabilities SET updated_at=ag_now(),test_model='wire',test_pricing_model_id='{MODEL}' WHERE id='{CAPABILITY}'"
        ),
        format!(
            "UPDATE model_routing_profiles SET updated_at=ag_now(),model_id='{MODEL}' WHERE id='{PROFILE}'"
        ),
    ] {
        assert!(execute(&db, &sql).await.is_err(), "{sql}");
    }
    execute(&db, &format!("UPDATE upstream_channels SET updated_at=ag_now(),enabled=0,deleted_at=ag_now() WHERE id='{CHANNEL}';
        UPDATE routing_groups SET updated_at=ag_now(),enabled=0,deleted_at=ag_now() WHERE id='{GROUP}';")).await.unwrap();
    assert!(
        execute(
            &db,
            &format!(
                "UPDATE upstream_channels SET updated_at=ag_now(),name='Restored' WHERE id='{CHANNEL}'"
            )
        )
        .await
        .is_err()
    );
    assert!(
        execute(
            &db,
            &format!(
                "UPDATE routing_groups SET updated_at=ag_now(),name='Restored' WHERE id='{GROUP}'"
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
         INSERT INTO audit_logs(id,actor_user_id,actor_type,action,object_type,object_id)
             VALUES ('{REQUEST}','{USER}','user','test','user','{USER}');
         INSERT INTO system_settings(setting_key,value) VALUES ('runtime','{{}}');
         INSERT INTO user_group_codex_quota_visibility(user_group_id,channel_group_id)
             VALUES ('00000000-0000-0000-0000-000000000101','{CODEX_GROUP}');
         INSERT INTO codex_oauth_flows(id,actor_user_id,label,quota_threshold_percent,
             state_hash,code_verifier,expires_at)
             VALUES ('{REQUEST}','{USER}','Flow',95,
                 zeroblob(32),printf('%043d',0),'2100-01-01T00:00:00.000000Z');
         INSERT INTO codex_quota_window_periods(id,credential_id,window_kind,window_seconds,
             started_at,scheduled_reset_at,initial_used_percent,last_used_percent,first_observed_at,last_observed_at)
             VALUES ('{REQUEST}','{CODEX_CHANNEL}','primary',300,'2026-01-01T00:00:00.000000Z',
                 '2026-01-01T00:05:00.000000Z',0,0,ag_now(),ag_now());
         INSERT INTO codex_quota_reset_events(id,credential_id,actor_user_id,requested_at,outcome,windows_reset,correlation_id)
             VALUES ('{REQUEST}','{CODEX_CHANNEL}','{USER}',ag_now(),'no_credit',0,'{REQUEST}');
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
