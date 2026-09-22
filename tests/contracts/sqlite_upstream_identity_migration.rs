//! Populated pre-identity databases upgrade atomically without merging secrets or Codex IDs.

use super::database;
use super::sqlite_schema::IDENTITY_MIGRATIONS;
use ai_gateway::persistence::sqlite::{SqliteDatabase, SqliteMigration};

async fn legacy_database() -> (tempfile::TempDir, SqliteDatabase) {
    let (directory, database) = database().await;
    database
        .migrate(&[
            SqliteMigration {
                version: 1,
                description: "business schema after PostgreSQL 0063",
                sql: include_str!("../../migrations/sqlite/0001_baseline.sql"),
            },
            SqliteMigration {
                version: 2,
                description: "business constraints and derived projections",
                sql: include_str!("../../migrations/sqlite/0002_guards.sql"),
            },
            SqliteMigration {
                version: 3,
                description: "durable Codex external-operation fences",
                sql: include_str!("../../migrations/sqlite/0003_codex_operations.sql"),
            },
        ])
        .await
        .unwrap();
    (directory, database)
}

#[tokio::test]
async fn populated_identity_upgrade_preserves_authentication_and_codex_projections() {
    let (_directory, database) = legacy_database().await;
    let group = uuid::Uuid::from_u128(1).to_string();
    let mut tx = database.begin_write().await.unwrap();
    sqlx::query("INSERT INTO channel_groups(id,name,api_format,enabled) VALUES(?,'legacy','open_ai_chat_completions',1)")
        .bind(&group).execute(&mut *tx).await.unwrap();
    for (id, kind, header, secret) in [
        (2, "bearer", None, Some("same-test-secret")),
        (3, "bearer", None, Some("same-test-secret")),
        (4, "header", Some("x-api-key"), Some("header-test-secret")),
        (5, "none", None, None),
    ] {
        sqlx::query("INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind,upstream_auth_header_name,upstream_api_key)
            VALUES(?,?,'open_ai_chat_completions',?,'https://upstream.test/',?,?,?)")
            .bind(uuid::Uuid::from_u128(id).to_string()).bind(&group).bind(format!("channel-{id}"))
            .bind(kind).bind(header).bind(secret).execute(&mut *tx).await.unwrap();
    }
    let pool = uuid::Uuid::from_u128(10).to_string();
    let codex_group = uuid::Uuid::from_u128(11).to_string();
    let codex = uuid::Uuid::from_u128(12).to_string();
    sqlx::query("INSERT INTO connector_pools(id,connector_kind) VALUES(?,'codex_oauth')")
        .bind(&pool)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO channel_groups(id,name,api_format,connector_kind,connector_pool_id,enabled)
        VALUES(?,'legacy codex','open_ai_responses','codex_oauth',?,1)",
    )
    .bind(&codex_group)
    .bind(&pool)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO channels(id,channel_group_id,api_format,name,base_url,upstream_auth_kind,supports_websocket)
        VALUES(?,?,'open_ai_responses','legacy codex account','https://codex.test','none',1)")
        .bind(&codex).bind(&codex_group).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO codex_oauth_credentials(channel_id,channel_group_id,label,account_id,user_id,
        id_token,access_token,refresh_token,last_refreshed_at,refresh_generation,connector_pool_id)
        VALUES(?,?,'legacy account','legacy-account','legacy-user','id-test-token','access-test-token','refresh-test-token',ag_now(),7,?)")
        .bind(&codex).bind(&codex_group).bind(&pool).execute(&mut *tx).await.unwrap();
    let before: Vec<(String, String)> = sqlx::query_as(
        "SELECT api_format,channel_id FROM codex_oauth_credential_channels ORDER BY api_format",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(database.migrate(IDENTITY_MIGRATIONS).await.unwrap(), 1);
    let mut connection = database.acquire_read().await.unwrap();
    let after: Vec<(String, String)> = sqlx::query_as(
        "SELECT api_format,channel_id FROM codex_oauth_credential_channels ORDER BY api_format",
    )
    .fetch_all(&mut *connection)
    .await
    .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM upstream_credentials")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        4
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM upstream_credentials WHERE secret='same-test-secret'"
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        2
    );
    assert_eq!(sqlx::query_scalar::<_, i64>("SELECT count(*) FROM channels WHERE upstream_api_key IS NOT NULL OR upstream_auth_kind<>'none'")
        .fetch_one(&mut *connection).await.unwrap(), 0);
    let bindings: Vec<String> = sqlx::query_scalar("SELECT credential_id FROM channels WHERE id IN (SELECT channel_id FROM codex_oauth_credential_channels)")
        .fetch_all(&mut *connection).await.unwrap();
    assert_eq!(bindings, [codex.clone(), codex.clone()]);
    let state: (i64, String, String) = sqlx::query_as("SELECT refresh_generation,access_token,refresh_token FROM codex_oauth_credentials WHERE channel_id=?")
        .bind(&codex).fetch_one(&mut *connection).await.unwrap();
    assert_eq!(
        state,
        (7, "access-test-token".into(), "refresh-test-token".into())
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut *connection)
            .await
            .unwrap()
            .is_empty()
    );
    drop(connection);
    database.close().await;
}

#[tokio::test]
async fn invalid_disabled_credential_draft_blocks_the_whole_sqlite_upgrade_without_leaks() {
    let (_directory, database) = legacy_database().await;
    let id = uuid::Uuid::from_u128(20).to_string();
    let mut tx = database.begin_write().await.unwrap();
    sqlx::query("INSERT INTO channel_groups(id,name,api_format,enabled) VALUES(?,'draft','open_ai_chat_completions',0)")
        .bind(&id).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO channels(id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,upstream_api_key)
        VALUES(?,?,'open_ai_chat_completions','disabled draft','https://private.test?secret=test',0,'bearer','private-test-secret')")
        .bind(&id).bind(&id).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let error = database
        .migrate(IDENTITY_MIGRATIONS)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains(&id));
    assert!(!error.contains("private"));
    let mut connection = database.acquire_read().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT max(version) FROM _gateway_sqlite_migrations")
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_schema WHERE name='upstream_credentials'"
        )
        .fetch_one(&mut *connection)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT upstream_api_key FROM channels WHERE id=?")
            .bind(&id)
            .fetch_one(&mut *connection)
            .await
            .unwrap(),
        "private-test-secret"
    );
    drop(connection);
    let mut tx = database.begin_write().await.unwrap();
    sqlx::query(
        "UPDATE channels SET base_url='https://private.test',updated_at=ag_now() WHERE id=?",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(database.migrate(IDENTITY_MIGRATIONS).await.unwrap(), 1);
    database.close().await;
}
