//! Populated sharing upgrades quarantine routes without rewriting financial evidence.

use super::TestDatabase;
use ai_gateway::persistence::{MIGRATOR, run_migrations};
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(feature = "sqlite-backend")]
use ai_gateway::persistence::sqlite::{SqliteDatabase, SqliteMigration};
#[cfg(feature = "sqlite-backend")]
#[path = "../../src/persistence/sqlite/schema.rs"]
mod sqlite_migrations;

enum Backend<'a> {
    Postgres(&'a PgPool),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(&'a SqliteDatabase),
}

impl Backend<'_> {
    async fn execute(&self, sql: &str) {
        match self {
            Self::Postgres(pool) => {
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
                    .execute(*pool)
                    .await
                    .unwrap();
            }
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => {
                let mut tx = database.begin_write().await.unwrap();
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql.replace("now()", "ag_now()")))
                    .execute(&mut *tx)
                    .await
                    .unwrap();
                tx.commit().await.unwrap();
            }
        }
    }

    async fn rows(&self, sql: &str) -> Vec<String> {
        match self {
            Self::Postgres(pool) => sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
                .fetch_all(*pool)
                .await
                .unwrap(),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(database) => sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
                .fetch_all(&mut *database.acquire_read().await.unwrap())
                .await
                .unwrap(),
        }
    }

    async fn preserved_state(&self) -> Vec<Vec<String>> {
        let mut state = Vec::new();
        for query in [
            "SELECT CAST(id AS TEXT)||':'||CAST(balance_amount AS TEXT) FROM users ORDER BY id",
            "SELECT CAST(id AS TEXT)||':'||CAST(quota_used_amount AS TEXT) FROM api_keys ORDER BY id",
            "SELECT CAST(api_key_id AS TEXT)||':'||CAST(channel_id AS TEXT)||':'||origin_kind||':'||CAST(origin_id AS TEXT) FROM api_key_channel_grants ORDER BY 1",
            "SELECT CAST(policy_id AS TEXT)||':'||CAST(channel_id AS TEXT)||':'||origin_kind||':'||CAST(origin_id AS TEXT) FROM api_key_policy_channel_grants ORDER BY 1",
            "SELECT CAST(id AS TEXT)||':'||outcome||':'||COALESCE(CAST(cost_amount AS TEXT),'unknown') FROM request_metering_facts ORDER BY id",
            "SELECT CAST(request_id AS TEXT)||':'||CAST(cost_amount AS TEXT) FROM request_settlements ORDER BY request_id",
            "SELECT CAST(request_id AS TEXT) FROM request_settlement_pending ORDER BY request_id",
            "SELECT CAST(channel_id AS TEXT)||':'||access_token||':'||refresh_token||':'||CAST(refresh_generation AS TEXT) FROM codex_oauth_credentials ORDER BY channel_id",
        ] {
            state.push(self.rows(query).await);
        }
        state
    }
}

fn id(value: u128) -> Uuid {
    Uuid::from_u128(0x7200 + value)
}

async fn seed(database: &Backend<'_>) {
    let user = id(1);
    let key = id(2);
    let policy = id(3);
    let group = id(4);
    let access = id(5);
    let ordinary_access = id(6);
    let (formats, permissions) = match database {
        Backend::Postgres(_) => ("'{open_ai_responses}'", "'{proxy}'"),
        #[cfg(feature = "sqlite-backend")]
        Backend::Sqlite(_) => ("'[\"open_ai_responses\"]'", "'[\"proxy\"]'"),
    };
    database.execute(&format!(
        "INSERT INTO users(id,display_name,status,balance_amount) VALUES ('{user}','Upgrade user','active','12.34');
         INSERT INTO api_keys(id,user_id,name,secret_value,status,allowed_api_formats,permissions,quota_used_amount)
         VALUES ('{key}','{user}','Upgrade key','synthetic-upgrade-key','active',{formats},{permissions},'5.67');
         INSERT INTO api_key_policies(id,name) VALUES ('{policy}','Upgrade policy');
         INSERT INTO routing_groups(id,name) VALUES ('{group}','Upgrade group');
         INSERT INTO upstream_accesses(id,name,connector_kind,base_url)
         VALUES ('{access}','Codex','codex','https://codex.invalid'),
                ('{ordinary_access}','Ordinary','general','https://ordinary.invalid');"
    )).await;
    for (value, account, member) in [
        (10, Some("bound"), "bound-user"),
        (11, Some("bound"), "bound-user"),
        (12, None, "marked-user"),
        (13, None, "marked-user"),
        (14, Some("ordinary"), "ordinary-user"),
    ] {
        let credential = id(value);
        let account = account.map_or("NULL".into(), |account| format!("'{account}'"));
        database.execute(&format!(
            "INSERT INTO upstream_credentials(id,name,kind,connector_kind,allowed_base_urls)
             VALUES ('{credential}','Credential {value}','codex_oauth','codex','[\"https://codex.invalid\"]');
             INSERT INTO codex_oauth_credentials(channel_id,label,account_id,user_id,id_token,access_token,refresh_token,last_refreshed_at)
             VALUES ('{credential}','Credential {value}',{account},'{member}','synthetic-id','synthetic-access','synthetic-refresh',now());"
        )).await;
    }
    for (value, credential, marked) in [
        (20, Some(10), false),
        (21, Some(10), false),
        (22, Some(11), false),
        (23, Some(12), true),
        (24, Some(12), false),
        (25, Some(13), false),
        (26, Some(14), false),
        (27, None, false),
    ] {
        let channel = id(value);
        let selected_access = if credential.is_some() {
            access
        } else {
            ordinary_access
        };
        let credential = credential.map_or("NULL".into(), |value| format!("'{}'", id(value)));
        database.execute(&format!(
            "INSERT INTO upstream_channels(id,group_id,access_id,credential_id,name,sharing_only)
             VALUES ('{channel}','{group}','{selected_access}',{credential},'Channel {value}',{marked});
             INSERT INTO api_key_channel_grants(api_key_id,channel_id,origin_kind,origin_id)
             VALUES ('{key}','{channel}','channel','{channel}');
             INSERT INTO api_key_policy_channel_grants(policy_id,channel_id,origin_kind,origin_id)
             VALUES ('{policy}','{channel}','channel','{channel}');"
        )).await;
    }
    database.execute(&format!(
        "INSERT INTO codex_sharing_groups(id,credential_id,channel_id,provider_account_id,provider_user_id,
          name,enabled,seats,primary_limit_amount,secondary_limit_amount,request_reservation_amount,
          user_requests_per_minute,group_requests_per_minute,user_max_concurrent_requests,group_max_concurrent_requests)
         VALUES ('{}','{}','{}','bound','bound-user','Stopped car',false,'[null]','1','2','0.01',1,1,1,1);
         INSERT INTO codex_sharing_ledger(singleton,ledger_id) VALUES (true,'{}');
         INSERT INTO request_metering_facts(id,started_at,completed_at,user_id,api_key_id,request_source,
          api_format,api_operation,request_protocol,client_model,outcome,cost_amount,peak_pricing)
         VALUES ('{}',now(),now(),'{user}','{key}','client','open_ai_responses','responses','non_stream','model','failed','0',false),
                ('{}',now(),now(),'{user}','{key}','client','open_ai_responses','responses','non_stream','model','succeeded',NULL,false),
                ('{}',now(),now(),'{user}','{key}','client','open_ai_responses','responses','non_stream','model','cancelled','0',false);
         INSERT INTO request_settlements(request_id,cost_amount,currency) VALUES ('{}','0',NULL);
         INSERT INTO request_settlement_pending(request_id,completed_at) VALUES ('{}',now());",
        id(30),id(10),id(20),id(31),id(40),id(41),id(42),id(40),id(42)
    )).await;
}

async fn verify(database: &Backend<'_>, before: Vec<Vec<String>>) {
    assert_eq!(database.preserved_state().await, before);
    assert_eq!(
        database.rows("SELECT CAST(id AS TEXT) FROM upstream_channels WHERE enabled AND deleted_at IS NULL ORDER BY id").await,
        [id(26).to_string(), id(27).to_string()]
    );
    assert_eq!(
        database.rows("SELECT CAST(id AS TEXT) FROM upstream_channels WHERE NOT enabled AND deleted_at IS NULL ORDER BY id").await,
        (20..=25).map(|value| id(value).to_string()).collect::<Vec<_>>()
    );
    let objects = match database {
        Backend::Postgres(_) => database.rows(
            "SELECT relname FROM pg_class WHERE relnamespace='public'::regnamespace AND relname LIKE '%sharing%'
             UNION ALL SELECT proname FROM pg_proc WHERE pronamespace='public'::regnamespace AND proname LIKE '%sharing%'
             UNION ALL SELECT tgname FROM pg_trigger WHERE NOT tgisinternal AND tgname LIKE '%sharing%'
             UNION ALL SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND column_name='sharing_only'"
        ).await,
        #[cfg(feature = "sqlite-backend")]
        Backend::Sqlite(_) => database.rows(
            "SELECT name FROM sqlite_schema WHERE name LIKE '%sharing%'
             UNION ALL SELECT name FROM pragma_table_info('upstream_channels') WHERE name='sharing_only'"
        ).await,
    };
    assert!(objects.is_empty(), "{objects:?}");
}

#[tokio::test]
async fn postgres_sharing_removal_quarantines_aliases_and_preserves_grants_and_facts() {
    use ai_gateway::persistence::capability_cutover::{activation, operation_split::storage};
    use sqlx::migrate::Migrate;

    let database = TestDatabase::new_unmigrated().await;
    let mut previous = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations"))
        .await
        .unwrap();
    previous.migrations = previous
        .iter()
        .filter(|migration| migration.version <= 64)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    previous.run(&database.pool).await.unwrap();
    let mut tx = database.pool.begin().await.unwrap();
    for migration in MIGRATOR
        .iter()
        .filter(|migration| (65..=71).contains(&migration.version))
    {
        if migration.version == 66 {
            storage::pg_prepare(&mut tx).await.unwrap();
        }
        (*tx).apply("_sqlx_migrations", migration).await.unwrap();
        if migration.version == 65 {
            activation::postgres(&mut tx).await.unwrap();
        }
        if migration.version == 66 {
            storage::pg_validate(&mut tx).await.unwrap();
        }
    }
    tx.commit().await.unwrap();
    let backend = Backend::Postgres(&database.pool);
    seed(&backend).await;
    let before = backend.preserved_state().await;
    run_migrations(&database.pool).await.unwrap();
    verify(&backend, before).await;
    run_migrations(&database.pool).await.unwrap();
    assert_eq!(
        backend
            .rows("SELECT CAST(max(version) AS TEXT) FROM _sqlx_migrations")
            .await,
        ["72"]
    );
    database.cleanup().await;
}

#[cfg(feature = "sqlite-backend")]
#[tokio::test]
async fn sqlite_sharing_removal_preserves_pending_intent_and_restores_update_fence() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let database = SqliteDatabase::open(&directory.path().join("gateway.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        database
            .migrate(&sqlite_migrations::MIGRATIONS[..11])
            .await
            .unwrap(),
        11
    );
    let backend = Backend::Sqlite(&database);
    seed(&backend).await;
    backend
        .execute(&format!(
            "INSERT INTO _gateway_codex_operations(credential_id,attempt_id,kind,generation)
         VALUES ('{}','{}','quota_reset',0);",
            id(10),
            id(50)
        ))
        .await;
    let intent_query = "SELECT credential_id||':'||attempt_id||':'||kind||':'||CAST(generation AS TEXT)||':'||started_at FROM _gateway_codex_operations";
    let intent = backend.rows(intent_query).await;
    let before = backend.preserved_state().await;
    assert_eq!(database.install_schema().await.unwrap(), 1);
    verify(&backend, before).await;
    assert_eq!(backend.rows(intent_query).await, intent);
    let mut tx = database.begin_write().await.unwrap();
    let error = sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE upstream_channels SET enabled=true,updated_at=ag_now() WHERE id='{}'",
        id(20)
    )))
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert!(error.to_string().contains("codex_operation_pending"));
    tx.rollback().await.unwrap();
    assert_eq!(database.install_schema().await.unwrap(), 0);
    assert_eq!(backend.rows(intent_query).await, intent);
    database.close().await;
}
