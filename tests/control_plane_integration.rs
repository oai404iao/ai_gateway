use std::{env, sync::Arc};

use ai_gateway::{
    domain::{ApiFormat, ApiKeyPermission},
    persistence::{ControlPlaneRepository, MIGRATOR},
    routing,
    runtime_config::{RuntimeConfig, compile_control_plane},
    workers::ControlPlaneReloader,
};
use reqwest::Url;
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
        let admin_url =
            env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_owned());
        let mut database_url =
            Url::parse(&admin_url).expect("TEST_DATABASE_ADMIN_URL must be a valid PostgreSQL URL");
        assert_ne!(
            database_url.path().trim_matches('/'),
            "ai_gateway",
            "TEST_DATABASE_ADMIN_URL must not target the ai_gateway application database"
        );

        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("configured PostgreSQL administrator database must be available");
        let name = format!("ai_gateway_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .expect("temporary integration-test database must be creatable");
        database_url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url.as_str())
            .await
            .expect("temporary integration-test database must be connectable");
        MIGRATOR
            .run(&pool)
            .await
            .expect("migrations must apply to the temporary database");

        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .execute(&self.admin)
            .await
            .expect("temporary integration-test database must be removable");
        self.admin.close().await;
    }
}

struct Seed {
    user: Uuid,
    model: Uuid,
    group: Uuid,
    other_group: Uuid,
    channel: Uuid,
    key: Uuid,
    rule: Uuid,
    secret: String,
    client_model: String,
}

async fn seed(pool: &PgPool) -> Seed {
    let seed = Seed {
        user: Uuid::new_v4(),
        model: Uuid::new_v4(),
        group: Uuid::new_v4(),
        other_group: Uuid::new_v4(),
        channel: Uuid::new_v4(),
        key: Uuid::new_v4(),
        rule: Uuid::new_v4(),
        secret: format!("test-client-{}", Uuid::new_v4()),
        client_model: format!("test-model-{}", Uuid::new_v4()),
    };
    sqlx::query("INSERT INTO users (id, name, status, currency) VALUES ($1, $2, 'active', 'USD')")
        .bind(seed.user)
        .bind(format!("test-user-{}", seed.user))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO models (id, source_model_id, display_name, enabled, currency, price_unit_tokens, input_unit_price, cached_input_unit_price, cache_write_unit_price, output_unit_price, price_effective_at) VALUES ($1, $2, 'test', true, 'USD', 1, 0, 0, 0, 0, now())")
        .bind(seed.model)
        .bind(format!("test-model-{}", seed.model))
        .execute(pool)
        .await
        .unwrap();
    for (id, name) in [(seed.group, "route"), (seed.other_group, "other")] {
        sqlx::query("INSERT INTO channel_groups (id, name, api_format, priority, selection_strategy, enabled) VALUES ($1, $2, 'open_ai_chat_completions', 0, 'weighted_random', true)")
            .bind(id)
            .bind(format!("test-group-{name}-{id}"))
            .execute(pool)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO channels (id, channel_group_id, api_format, name, base_url, enabled, weight, upstream_auth_kind, upstream_api_key, available_models) VALUES ($1, $2, 'open_ai_chat_completions', $3, 'https://example.test', true, 1, 'bearer', 'upstream-secret', ARRAY['upstream-v1']::text[])")
        .bind(seed.channel)
        .bind(seed.group)
        .bind(format!("test-channel-{}", seed.channel))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_keys (id, user_id, name, secret_value, status, allowed_api_formats, permissions, allowed_group_ids) VALUES ($1, $2, 'test', $3, 'active', ARRAY['open_ai_chat_completions']::api_format[], ARRAY['proxy', 'models.read'], ARRAY[$4]::uuid[])")
        .bind(seed.key)
        .bind(seed.user)
        .bind(&seed.secret)
        .bind(seed.group)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO model_rules (id, client_model, api_format, model_id, upstream_model, channel_ids, enabled) VALUES ($1, $2, 'open_ai_chat_completions', $3, 'upstream-v1', ARRAY[$4]::uuid[], true)")
        .bind(seed.rule)
        .bind(&seed.client_model)
        .bind(seed.model)
        .bind(seed.channel)
        .execute(pool)
        .await
        .unwrap();
    seed
}

#[tokio::test]
async fn repository_migrates_compiles_seeded_snapshot_and_authenticates() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    let snapshot = compile_control_plane(records).unwrap();
    let key = snapshot.authenticate(&seed.secret).unwrap();
    assert!(key.permits(ApiFormat::OpenAiChatCompletions, ApiKeyPermission::Proxy));
    assert_eq!(
        snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, &seed.client_model)
            .unwrap()
            .upstream_model(),
        "upstream-v1"
    );
    assert!(!format!("{snapshot:?}").contains("upstream-secret"));
    database.cleanup().await;
}

#[tokio::test]
async fn dangling_enabled_route_is_rejected() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE model_rules SET channel_ids = ARRAY[$1]::uuid[] WHERE id = $2")
        .bind(Uuid::new_v4())
        .bind(seed.rule)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_err());
    database.cleanup().await;
}

#[tokio::test]
async fn cross_format_enabled_route_is_rejected() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let group = Uuid::new_v4();
    let channel = Uuid::new_v4();
    sqlx::query("INSERT INTO channel_groups (id, name, api_format, priority, selection_strategy, enabled) VALUES ($1, $2, 'open_ai_responses', 0, 'weighted_random', true)")
        .bind(group)
        .bind(format!("responses-group-{group}"))
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO channels (id, channel_group_id, api_format, name, base_url, enabled, weight, upstream_auth_kind, upstream_api_key, available_models) VALUES ($1, $2, 'open_ai_responses', $3, 'https://example.test', true, 1, 'bearer', 'upstream-secret', ARRAY['upstream-v1']::text[])")
        .bind(channel)
        .bind(group)
        .bind(format!("responses-channel-{channel}"))
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE model_rules SET channel_ids = ARRAY[$1]::uuid[] WHERE id = $2")
        .bind(channel)
        .bind(seed.rule)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_err());
    database.cleanup().await;
}

#[tokio::test]
async fn reloader_replaces_atomically_retains_old_arcs_and_rolls_back_failures() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(repository.load().await.unwrap()).unwrap(),
    ));
    let reloader = ControlPlaneReloader::new(repository, Arc::clone(&runtime));
    let old = runtime.snapshot();
    sqlx::query(
        "UPDATE channels SET available_models = ARRAY['upstream-v2']::text[] WHERE id = $1",
    )
    .bind(seed.channel)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE model_rules SET upstream_model = 'upstream-v2' WHERE id = $1")
        .bind(seed.rule)
        .execute(&database.pool)
        .await
        .unwrap();
    let first = reloader.reload();
    let second = reloader.reload();
    let (first, second) = tokio::join!(first, second);
    first.unwrap();
    second.unwrap();
    let replaced = runtime.snapshot();
    assert!(!Arc::ptr_eq(&old, &replaced));
    assert_eq!(
        old.model_rule(ApiFormat::OpenAiChatCompletions, &seed.client_model)
            .unwrap()
            .upstream_model(),
        "upstream-v1"
    );
    assert!(old.authenticate(&seed.secret).is_some());
    assert_eq!(
        replaced
            .model_rule(ApiFormat::OpenAiChatCompletions, &seed.client_model)
            .unwrap()
            .upstream_model(),
        "upstream-v2"
    );
    sqlx::query("UPDATE model_rules SET channel_ids = ARRAY[$1]::uuid[] WHERE id = $2")
        .bind(Uuid::new_v4())
        .bind(seed.rule)
        .execute(&database.pool)
        .await
        .unwrap();
    assert!(reloader.reload().await.is_err());
    assert!(Arc::ptr_eq(&replaced, &runtime.snapshot()));
    database.cleanup().await;
}

#[tokio::test]
async fn suspending_a_user_publishes_a_snapshot_that_revokes_their_keys() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(repository.load().await.unwrap()).unwrap(),
    ));
    let reloader = ControlPlaneReloader::new(repository, Arc::clone(&runtime));
    let old = runtime.snapshot();
    sqlx::query("UPDATE users SET status = 'suspended' WHERE id = $1")
        .bind(seed.user)
        .execute(&database.pool)
        .await
        .unwrap();
    reloader.reload().await.unwrap();
    assert!(old.authenticate(&seed.secret).is_some());
    assert!(runtime.snapshot().authenticate(&seed.secret).is_none());
    database.cleanup().await;
}

#[tokio::test]
async fn group_authorization_and_expired_keys_are_not_usable() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE api_keys SET allowed_group_ids = ARRAY[$1]::uuid[] WHERE id = $2")
        .bind(seed.other_group)
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let snapshot = compile_control_plane(
        ControlPlaneRepository::new(database.pool.clone())
            .load()
            .await
            .unwrap(),
    )
    .unwrap();
    let key = snapshot.authenticate(&seed.secret).unwrap();
    assert!(
        routing::select(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            &seed.client_model,
        )
        .is_none()
    );
    assert!(
        snapshot
            .models_for(&key, ApiFormat::OpenAiChatCompletions)
            .is_empty()
    );
    sqlx::query("UPDATE api_keys SET created_at = now() - interval '2 minutes', expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let expired = compile_control_plane(
        ControlPlaneRepository::new(database.pool.clone())
            .load()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(expired.authenticate(&seed.secret).is_none());
    database.cleanup().await;
}

#[tokio::test]
async fn admission_controls_and_invalid_channel_targets_are_rejected_before_stage_five() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE api_keys SET requests_per_minute = 1 WHERE id = $1")
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_err());
    sqlx::query("UPDATE api_keys SET status = 'disabled' WHERE id = $1")
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_ok());
    sqlx::query("UPDATE api_keys SET status = 'active', requests_per_minute = NULL WHERE id = $1")
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE channels SET available_models = ARRAY[]::text[] WHERE id = $1")
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_err());
    sqlx::query(
        "UPDATE channels SET available_models = ARRAY['upstream-v1']::text[] WHERE id = $1",
    )
    .bind(seed.channel)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE model_rules SET channel_group_ids = ARRAY[$1]::uuid[] WHERE id = $2")
        .bind(seed.group)
        .bind(seed.rule)
        .execute(&database.pool)
        .await
        .unwrap();
    let records = ControlPlaneRepository::new(database.pool.clone())
        .load()
        .await
        .unwrap();
    assert!(compile_control_plane(records).is_err());
    database.cleanup().await;
}
