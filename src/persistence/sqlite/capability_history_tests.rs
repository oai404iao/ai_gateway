//! Isolated financial-history rebuild and rollback coverage.

use std::os::unix::fs::PermissionsExt;

use sqlx::{Connection, SqliteConnection};

use super::SqliteDatabase;
use crate::persistence::capability_cutover::{
    history::sqlite_retarget_history, io::sqlite_transfer,
};

async fn seed(connection: &mut SqliteConnection) {
    sqlx::raw_sql(
        "INSERT INTO users(id,display_name,email,role,status,password_hash)
         VALUES ('10000000-0000-0000-0000-000000000001','History','history@test.invalid','admin','active','test');
         INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions)
         VALUES ('20000000-0000-0000-0000-000000000001','10000000-0000-0000-0000-000000000001',
                 'History','history-test-key','active','[\"open_ai_chat_completions\"]','[\"proxy\"]');
         INSERT INTO channel_groups(id,name,api_format)
         VALUES ('30000000-0000-0000-0000-000000000001','History','open_ai_chat_completions');
         INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind)
         VALUES ('40000000-0000-0000-0000-000000000001','30000000-0000-0000-0000-000000000001',
                 'open_ai_chat_completions','History','https://history.test','none');
         INSERT INTO models(id,source_model_id,display_name,currency,price_unit_tokens,
                            price_effective_at,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price)
         VALUES ('50000000-0000-0000-0000-000000000001','history','History','USD',1000000,ag_now(),'1','0','0','2');
         INSERT INTO model_routing_profiles(id,model_id)
         VALUES ('60000000-0000-0000-0000-000000000001','50000000-0000-0000-0000-000000000001');
         INSERT INTO model_rules(id,api_format,model_routing_profile_id,enabled)
         VALUES ('70000000-0000-0000-0000-000000000001','open_ai_chat_completions',
                 '60000000-0000-0000-0000-000000000001',0);
         INSERT INTO request_metering_facts
             (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,
              request_protocol,client_model,outcome,peak_pricing,cost_amount,channel_group_id,channel_id,model_rule_id)
         VALUES ('80000000-0000-0000-0000-000000000001',ag_now(),ag_now(),
                 '10000000-0000-0000-0000-000000000001','20000000-0000-0000-0000-000000000001',
                 'client','open_ai_chat_completions','chat_completions','non_stream','history','failed',0,'0',
                 '30000000-0000-0000-0000-000000000001','40000000-0000-0000-0000-000000000001',
                 '70000000-0000-0000-0000-000000000001');
         INSERT INTO request_logs
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,
              outcome,cost_amount,channel_group_id,channel_id,model_rule_id)
         SELECT id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,
                outcome,cost_amount,channel_group_id,channel_id,model_rule_id FROM request_metering_facts;
         INSERT INTO request_settlement_pending(request_id,completed_at)
         SELECT id,completed_at FROM request_metering_facts;",
    )
    .execute(connection)
    .await
    .unwrap();
}

async fn schema(connection: &mut SqliteConnection) -> Vec<(String, String, String)> {
    sqlx::query_as(
        "SELECT type,name,sql FROM sqlite_schema
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )
    .fetch_all(connection)
    .await
    .unwrap()
}

async fn history_state(connection: &mut SqliteConnection) -> (String, String, i64) {
    let fact = sqlx::query_scalar(
        "SELECT json_object('id',id,'group',channel_group_id,'channel',channel_id,'rule',model_rule_id,
             'cost',cost_amount,'state',amount_state) FROM request_metering_facts",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let log = sqlx::query_scalar(
        "SELECT json_object('id',id,'group',channel_group_id,'channel',channel_id,'rule',model_rule_id,
             'cost',cost_amount) FROM request_logs",
    )
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    let pending = sqlx::query_scalar("SELECT count(*) FROM request_settlement_pending")
        .fetch_one(connection)
        .await
        .unwrap();
    (fact, log, pending)
}

#[tokio::test]
async fn history_rebuild_preserves_rows_guards_and_rolls_back_as_one_transaction() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let database = SqliteDatabase::open(&directory.path().join("history.sqlite"))
        .await
        .unwrap();
    database
        .migrate(&super::schema::MIGRATIONS[..4])
        .await
        .unwrap();
    let mut transaction = database.begin_write().await.unwrap();
    seed(&mut transaction).await;
    assert!(sqlite_retarget_history(&mut transaction).await.is_err());
    let original_state = history_state(&mut transaction).await;
    transaction.commit().await.unwrap();

    let pools = database.pools().unwrap();
    let mut connection = pools.writer.acquire().await.unwrap();
    connection.close_on_drop();
    let original_schema = schema(&mut connection).await;
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
    super::functions::set_transaction_time(&mut transaction)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/sqlite/0005_upstream_capabilities.sql"
    ))
    .execute(&mut *transaction)
    .await
    .unwrap();
    assert!(sqlite_retarget_history(&mut transaction).await.is_err());
    transaction.rollback().await.unwrap();
    assert_eq!(schema(&mut connection).await, original_schema);
    assert_eq!(history_state(&mut connection).await, original_state);
    for commit in [false, true] {
        let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
        super::functions::set_transaction_time(&mut transaction)
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/sqlite/0005_upstream_capabilities.sql"
        ))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlite_transfer(&mut transaction, chrono::Utc::now())
            .await
            .unwrap();
        let guards_before: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_schema WHERE type IN ('trigger','view') ORDER BY name",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
        sqlite_retarget_history(&mut transaction).await.unwrap();
        assert_eq!(history_state(&mut transaction).await, original_state);
        let guards_after: Vec<(String, String)> = sqlx::query_as(
            "SELECT name,sql FROM sqlite_schema WHERE type IN ('trigger','view') ORDER BY name",
        )
        .fetch_all(&mut *transaction)
        .await
        .unwrap();
        assert_eq!(guards_after, guards_before);
        for table in ["request_logs", "request_metering_facts"] {
            let targets: Vec<String> =
                sqlx::query_scalar("SELECT \"table\" FROM pragma_foreign_key_list(?)")
                    .bind(table)
                    .fetch_all(&mut *transaction)
                    .await
                    .unwrap();
            for registry in [
                "group_identity_registry",
                "channel_identity_registry",
                "model_rule_identity_registry",
            ] {
                assert!(targets.iter().any(|target| target == registry));
            }
        }
        assert!(
            sqlx::query("UPDATE request_metering_facts SET cost_amount='1'")
                .execute(&mut *transaction)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM request_metering_facts")
                .execute(&mut *transaction)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("DELETE FROM model_rule_identity_registry")
                .execute(&mut *transaction)
                .await
                .is_err()
        );
        if commit {
            transaction.commit().await.unwrap();
        } else {
            transaction.rollback().await.unwrap();
            assert_eq!(schema(&mut connection).await, original_schema);
            assert_eq!(history_state(&mut connection).await, original_state);
        }
    }
    let before_operations = schema(&mut connection).await;
    for commit in [false, true] {
        let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
        super::functions::set_transaction_time(&mut transaction)
            .await
            .unwrap();
        crate::persistence::capability_cutover::operation_split::storage::sqlite_apply(
            &mut transaction,
        )
        .await
        .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/sqlite/0007_logical_channel_authorization.sql"
        ))
        .execute(&mut *transaction)
        .await
        .unwrap();
        assert_eq!(history_state(&mut transaction).await, original_state);
        if commit {
            transaction.commit().await.unwrap();
        } else {
            transaction.rollback().await.unwrap();
            assert_eq!(schema(&mut connection).await, before_operations);
            assert_eq!(history_state(&mut connection).await, original_state);
        }
    }
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_optional(&mut *connection)
            .await
            .unwrap()
            .is_none()
    );
    connection.close().await.unwrap();
    drop(pools);
    let database = std::sync::Arc::new(database);
    let mut transaction = database.begin_write().await.unwrap();
    assert!(
        sqlx::query_scalar::<_, bool>("PRAGMA foreign_keys")
            .fetch_one(&mut *transaction)
            .await
            .unwrap()
    );
    sqlx::query("UPDATE channels SET name='Legacy name must not be read',updated_at=ag_now()")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE channel_groups SET name='Legacy group must not be read',updated_at=ag_now()",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query("UPDATE channel_capabilities SET status_statistics_enabled=1,updated_at=ag_now()")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_capabilities(id,channel_id,operation,available_models,status_statistics_enabled)
         SELECT ag_md5_uuid(id||'idle-image'),id,'images_generation','[\"idle-image\"]',1
         FROM upstream_channels",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    let logs = super::queries::SqliteRequestLogQueries::new(database.clone());
    let log_id = uuid::Uuid::parse_str("80000000-0000-0000-0000-000000000001").unwrap();
    let log = logs.get(log_id).await.unwrap().unwrap();
    assert_eq!(log.channel_name.as_deref(), Some("History"));
    assert_eq!(log.channel_group_name.as_deref(), Some("History"));
    assert!(
        logs.get_for_user(uuid::Uuid::new_v4(), log_id)
            .await
            .unwrap()
            .is_none()
    );
    let metering = super::queries::SqliteMeteringQueries::new(database.clone());
    let now = chrono::Utc::now();
    let report = metering
        .cost_statistics(crate::persistence::CostStatisticsFilter {
            started_at: now - chrono::Duration::days(1),
            ended_at: now + chrono::Duration::seconds(1),
            granularity: crate::persistence::StatisticsGranularity::Day,
            user_id: None,
            api_key_id: None,
            channel_id: None,
            codex_credential_id: None,
            include_channel_details: true,
        })
        .await
        .unwrap();
    assert_eq!(report.channels.len(), 1);
    assert_eq!(report.channels[0].name, "History");
    assert_eq!(report.channels[0].channel_group_name, "History");
    let status = logs
        .channel_group_status(crate::persistence::ChannelGroupStatusWindow::Last24Hours)
        .await
        .unwrap();
    assert_eq!(status.groups.len(), 2);
    assert_eq!(status.groups[0].id, status.groups[1].id);
    assert_eq!(
        status
            .models
            .iter()
            .map(|row| row.request_count)
            .sum::<i64>(),
        1
    );
    let images = status
        .groups
        .iter()
        .find(|group| group.api_format == "open_ai_images")
        .unwrap();
    assert_eq!(images.models[0].model, "idle-image");
    assert_eq!(images.models[0].request_count, 0);
    drop(logs);
    drop(metering);
    database.close().await;
}
