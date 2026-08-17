#![cfg(feature = "sqlite-backend")]

use std::{str::FromStr, time::Duration};

use ai_gateway::{
    domain::ApiFormat,
    persistence::{
        DEFAULT_USER_GROUP_ID, RepositoryError, SQLITE_MIGRATOR, SqliteDecimal,
        SqliteRuntimeConfigRepository,
    },
    runtime_config::compile_runtime_config,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteSynchronous},
    types::Json,
};
use uuid::Uuid;

const USER_ID: Uuid = Uuid::from_u128(0x301);
const MODEL_ID: Uuid = Uuid::from_u128(0x302);
const GROUP_ID: Uuid = Uuid::from_u128(0x303);
const PROXY_ID: Uuid = Uuid::from_u128(0x304);
const TEMPLATE_ID: Uuid = Uuid::from_u128(0x305);
const CHANNEL_ID: Uuid = Uuid::from_u128(0x306);
const RULE_ID: Uuid = Uuid::from_u128(0x307);
const API_KEY_ID: Uuid = Uuid::from_u128(0x308);
const SYSTEM_API_KEY_ID: Uuid = Uuid::from_u128(0x309);
const MCP_SERVER_ID: Uuid = Uuid::from_u128(0x30a);
const DELETED_MCP_SERVER_ID: Uuid = Uuid::from_u128(0x30b);

fn decimal(value: &str) -> SqliteDecimal {
    SqliteDecimal::from(Decimal::from_str_exact(value).unwrap())
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn system_settings() -> Value {
    json!({
        "api_hosts": ["https://gateway.example.test"],
        "upstream": {
            "connect_timeout_seconds": 2,
            "response_header_timeout_seconds": 30,
            "images_response_header_timeout_seconds": 300,
            "standalone_web_search_response_header_timeout_seconds": 120,
            "stream_idle_timeout_seconds": 45
        },
        "request_retry": {
            "enabled": true,
            "max_retries": 2
        },
        "passive_health": {
            "connection_failure_threshold": 3,
            "cooldown_seconds": 30
        },
        "automatic_disable": {
            "enabled": true,
            "error_status_codes": [401, 429],
            "error_message_keywords": ["quota"]
        },
        "scheduled_testing": {
            "mode": "failure_only",
            "auto_recover": true,
            "interval_minutes": 10,
            "prompt": "reply '1'"
        },
        "session_affinity": {
            "enabled": false,
            "max_entries": 1000,
            "default_ttl_seconds": 600,
            "rules": []
        },
        "websocket": {
            "enabled": true,
            "max_idle_connections": 32,
            "idle_timeout_seconds": 120,
            "max_connection_age_seconds": 600
        },
        "codex": {
            "workspace_path": "/workspace",
            "git_remote_url": "https://github.com/example/runtime.git"
        },
        "mcp": {
            "enabled": false,
            "allowed_origins": [],
            "allow_legacy_2025_11_25": false,
            "request_body_bytes": 1048576,
            "image_request_body_bytes": 10485760,
            "search_result_bytes": 1048576,
            "image_result_bytes": 10485760
        }
    })
}

async fn migrated_pool() -> SqlitePool {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5))
        .synchronous(SqliteSynchronous::Full);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    SQLITE_MIGRATOR.run(&pool).await.unwrap();
    pool
}

async fn seed_runtime_snapshot(pool: &SqlitePool) {
    let price_effective_at = timestamp("2026-01-02T03:04:05.678Z");
    let expires_at = timestamp("9999-01-02T03:04:05.678Z");
    let system_settings_updated_at = timestamp("2026-02-03T04:05:06.789Z");
    let mut transaction = pool.begin().await.unwrap();

    sqlx::query("UPDATE user_groups SET filter_fast_mode=1 WHERE id=?1")
        .bind(DEFAULT_USER_GROUP_ID)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users
         (id,display_name,status,email,role,password_hash,user_group_id,websocket_enabled)
         VALUES (?1,?2,'active',?3,'user',?4,?5,1)",
    )
    .bind(USER_ID)
    .bind("SQLite runtime user")
    .bind("sqlite-runtime@example.test")
    .bind("not-used-by-runtime-reader")
    .bind(DEFAULT_USER_GROUP_ID)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO models
         (id,source_model_id,display_name,enabled,currency,price_unit_tokens,
          input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,
          price_effective_at,advanced_billing)
         VALUES (?1,?2,?3,1,'USD',1000000,?4,?5,?6,?7,?8,?9)",
    )
    .bind(MODEL_ID)
    .bind("sqlite-upstream-model")
    .bind("SQLite upstream model")
    .bind(decimal("1.125"))
    .bind(decimal("0.25"))
    .bind(decimal("0.5"))
    .bind(decimal("2.75"))
    .bind(price_effective_at)
    .bind(Json(json!({
        "long_context_tiers": [],
        "request_multipliers": []
    })))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channel_groups
         (id,name,api_format,priority,selection_strategy,enabled,connector_kind,
          request_compression)
         VALUES (?1,?2,'open_ai_responses',7,'weighted_round_robin',1,
                 'openai_compatible','zstd')",
    )
    .bind(GROUP_ID)
    .bind("SQLite runtime group")
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO proxies
         (id,name,proxy_url,username,password,no_proxy_hosts,enabled)
         VALUES (?1,?2,?3,?4,?5,?6,1)",
    )
    .bind(PROXY_ID)
    .bind("SQLite runtime proxy")
    .bind("https://proxy.example.test:8443")
    .bind("proxy-user")
    .bind("proxy-password")
    .bind(Json(vec![
        "internal.example.test".to_owned(),
        "metadata.example.test".to_owned(),
    ]))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO config_templates
         (id,name,description,document,enabled)
         VALUES (?1,?2,?3,?4,1)",
    )
    .bind(TEMPLATE_ID)
    .bind("SQLite runtime template")
    .bind("Exercises JSON decoding")
    .bind(Json(json!({
        "version": 1,
        "api_format": "open_ai_responses",
        "request_headers": {
            "set": {
                "x-template": "sqlite"
            }
        }
    })))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO channels
         (id,channel_group_id,api_format,name,base_url,enabled,auto_disabled,weight,
          proxy_id,config_template_id,override_document,connect_timeout_ms,
          response_header_timeout_ms,stream_idle_timeout_ms,upstream_auth_kind,
          upstream_auth_header_name,upstream_api_key,available_models,auto_disable_allowed,
          test_model,billing_multiplier,supports_websocket,supports_standalone_web_search)
         VALUES (?1,?2,'open_ai_responses',?3,?4,1,0,11,?5,?6,?7,1500,8000,20000,
                 'header','x-upstream-key',?8,?9,1,?10,?11,1,1)",
    )
    .bind(CHANNEL_ID)
    .bind(GROUP_ID)
    .bind("SQLite runtime channel")
    .bind("https://upstream.example.test/v1")
    .bind(PROXY_ID)
    .bind(TEMPLATE_ID)
    .bind(Json(json!({})))
    .bind("upstream-secret")
    .bind(Json(vec!["sqlite-upstream-model".to_owned()]))
    .bind("sqlite-upstream-model")
    .bind(decimal("1.25"))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO model_rules
         (id,client_model,api_format,upstream_model_id,channel_group_ids,channel_ids,enabled,
          description)
         VALUES (?1,?2,'open_ai_responses',?3,?4,?5,1,?6)",
    )
    .bind(RULE_ID)
    .bind("sqlite-client-model")
    .bind(MODEL_ID)
    .bind(Json(vec![GROUP_ID]))
    .bind(Json(Vec::<Uuid>::new()))
    .bind("SQLite runtime rule")
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,expires_at,allowed_api_formats,permissions,
          allowed_group_ids,allowed_channel_ids,requests_per_minute,max_concurrent_requests,
          quota_limit_amount,quota_used_amount)
         VALUES (?1,?2,?3,?4,'active',?5,?6,?7,?8,?9,123,17,?10,?11)",
    )
    .bind(API_KEY_ID)
    .bind(USER_ID)
    .bind("SQLite runtime key")
    .bind("sk-sqlite-runtime")
    .bind(expires_at)
    .bind(Json(vec!["open_ai_responses".to_owned()]))
    .bind(Json(vec!["proxy".to_owned(), "models.read".to_owned()]))
    .bind(Json(vec![GROUP_ID]))
    .bind(Json(vec![CHANNEL_ID]))
    .bind(decimal("100.5"))
    .bind(decimal("4.25"))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions,is_system)
         VALUES (?1,?2,?3,?4,'active',?5,?6,1)",
    )
    .bind(SYSTEM_API_KEY_ID)
    .bind(USER_ID)
    .bind("SQLite internal key")
    .bind("sk-sqlite-system")
    .bind(Json(vec!["open_ai_responses".to_owned()]))
    .bind(Json(vec!["proxy".to_owned()]))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mcp_servers
         (id,slug,kind,name,description,model_rule_id,settings_version,settings,enabled)
         VALUES (?1,?2,'web_search',?3,?4,?5,1,?6,1)",
    )
    .bind(MCP_SERVER_ID)
    .bind("runtime-search")
    .bind("SQLite runtime search")
    .bind("Exercises MCP JSON decoding")
    .bind(RULE_ID)
    .bind(Json(json!({
        "allowed_domains": ["docs.example.test"]
    })))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO mcp_servers
         (id,slug,kind,name,model_rule_id,settings,enabled,deleted_at)
         VALUES (?1,?2,'web_search',?3,?4,?5,0,?6)",
    )
    .bind(DELETED_MCP_SERVER_ID)
    .bind("deleted-search")
    .bind("Deleted SQLite search")
    .bind(RULE_ID)
    .bind(Json(json!({})))
    .bind(timestamp("2026-02-03T04:05:07.000Z"))
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO system_settings (setting_key,value,updated_at)
         VALUES ('forwarding_policy',?1,?2)",
    )
    .bind(Json(system_settings()))
    .bind(system_settings_updated_at)
    .execute(&mut *transaction)
    .await
    .unwrap();

    transaction.commit().await.unwrap();
}

#[tokio::test]
async fn sqlite_runtime_repository_decodes_and_compiles_a_complete_snapshot() {
    let pool = migrated_pool().await;
    seed_runtime_snapshot(&pool).await;
    let repository = SqliteRuntimeConfigRepository::new(pool);
    let records = repository.load_runtime().await.unwrap();

    assert_eq!(records.control_plane.api_keys.len(), 1);
    let api_key = &records.control_plane.api_keys[0];
    assert_eq!(api_key.id, API_KEY_ID);
    assert_eq!(api_key.user_id, USER_ID);
    assert_eq!(api_key.user_status, "active");
    assert!(api_key.user_websocket_enabled);
    assert!(api_key.user_filter_fast_mode);
    assert_eq!(api_key.secret_value, "sk-sqlite-runtime");
    assert_eq!(
        api_key.expires_at,
        Some(timestamp("9999-01-02T03:04:05.678Z"))
    );
    assert_eq!(
        api_key.allowed_api_formats,
        ["open_ai_responses".to_owned()]
    );
    assert_eq!(
        api_key.permissions,
        ["proxy".to_owned(), "models.read".to_owned()]
    );
    assert_eq!(api_key.allowed_group_ids, [GROUP_ID]);
    assert_eq!(api_key.allowed_channel_ids, [CHANNEL_ID]);
    assert_eq!(api_key.requests_per_minute, Some(123));
    assert_eq!(api_key.max_concurrent_requests, Some(17));
    assert_eq!(
        api_key.quota_limit_amount,
        Some(Decimal::from_str("100.5").unwrap())
    );
    assert_eq!(
        api_key.quota_used_amount,
        Decimal::from_str("4.25").unwrap()
    );

    assert_eq!(records.control_plane.models.len(), 1);
    let model = &records.control_plane.models[0];
    assert_eq!(model.id, MODEL_ID);
    assert_eq!(model.source_model_id, "sqlite-upstream-model");
    assert_eq!(model.price_unit_tokens, 1_000_000);
    assert_eq!(model.input_unit_price, Decimal::from_str("1.125").unwrap());
    assert_eq!(
        model.cached_input_unit_price,
        Decimal::from_str("0.25").unwrap()
    );
    assert_eq!(
        model.cache_write_unit_price,
        Decimal::from_str("0.5").unwrap()
    );
    assert_eq!(model.output_unit_price, Decimal::from_str("2.75").unwrap());
    assert_eq!(
        model.price_effective_at,
        timestamp("2026-01-02T03:04:05.678Z")
    );
    assert_eq!(
        model.advanced_billing,
        json!({
            "long_context_tiers": [],
            "request_multipliers": []
        })
    );

    assert_eq!(records.control_plane.model_rules.len(), 1);
    let rule = &records.control_plane.model_rules[0];
    assert_eq!(rule.id, RULE_ID);
    assert_eq!(rule.client_model, "sqlite-client-model");
    assert_eq!(rule.upstream_model_id, MODEL_ID);
    assert!(rule.upstream_model_enabled);
    assert_eq!(rule.upstream_model, "sqlite-upstream-model");
    assert_eq!(rule.channel_group_ids, [GROUP_ID]);
    assert!(rule.channel_ids.is_empty());

    assert_eq!(records.control_plane.groups.len(), 1);
    let group = &records.control_plane.groups[0];
    assert_eq!(group.id, GROUP_ID);
    assert_eq!(group.request_compression, "zstd");
    assert_eq!(group.priority, 7);
    assert_eq!(group.selection_strategy, "weighted_round_robin");

    assert_eq!(records.control_plane.channels.len(), 1);
    let channel = &records.control_plane.channels[0];
    assert_eq!(channel.id, CHANNEL_ID);
    assert_eq!(channel.proxy_id, Some(PROXY_ID));
    assert_eq!(channel.config_template_id, Some(TEMPLATE_ID));
    assert_eq!(
        channel.billing_multiplier,
        Decimal::from_str("1.25").unwrap()
    );
    assert!(channel.supports_websocket);
    assert!(channel.supports_standalone_web_search);
    assert!(channel.auto_disable_allowed);
    assert_eq!(channel.upstream_auth_kind, "header");
    assert_eq!(
        channel.upstream_auth_header_name.as_deref(),
        Some("x-upstream-key")
    );
    assert_eq!(channel.upstream_api_key.as_deref(), Some("upstream-secret"));
    assert_eq!(channel.connect_timeout_ms, Some(1500));
    assert_eq!(channel.response_header_timeout_ms, Some(8000));
    assert_eq!(channel.stream_idle_timeout_ms, Some(20000));
    assert_eq!(
        channel.available_models,
        ["sqlite-upstream-model".to_owned()]
    );

    assert_eq!(records.control_plane.proxies.len(), 1);
    let proxy = &records.control_plane.proxies[0];
    assert_eq!(proxy.id, PROXY_ID);
    assert_eq!(proxy.username.as_deref(), Some("proxy-user"));
    assert_eq!(proxy.password.as_deref(), Some("proxy-password"));
    assert_eq!(
        proxy.no_proxy_hosts,
        [
            "internal.example.test".to_owned(),
            "metadata.example.test".to_owned()
        ]
    );

    assert_eq!(records.control_plane.templates.len(), 1);
    let template = &records.control_plane.templates[0];
    assert_eq!(template.id, TEMPLATE_ID);
    assert_eq!(template.document["version"], 1);
    assert_eq!(
        template.document["request_headers"]["set"]["x-template"],
        "sqlite"
    );

    assert_eq!(records.control_plane.mcp_servers.len(), 1);
    let mcp_server = &records.control_plane.mcp_servers[0];
    assert_eq!(mcp_server.id, MCP_SERVER_ID);
    assert_eq!(mcp_server.slug, "runtime-search");
    assert_eq!(mcp_server.settings_version, 1);
    assert_eq!(
        mcp_server.settings["allowed_domains"],
        json!(["docs.example.test"])
    );

    assert_eq!(records.system_settings.value, system_settings());
    assert_eq!(
        records.system_settings.updated_at,
        timestamp("2026-02-03T04:05:06.789Z")
    );

    let snapshot = compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap();
    let compiled_key = snapshot.authenticate("sk-sqlite-runtime").unwrap();
    assert!(
        snapshot
            .model_rule(ApiFormat::OpenAiResponses, "sqlite-client-model")
            .is_some()
    );
    assert!(snapshot.channel(CHANNEL_ID).is_some());
    assert!(snapshot.mcp_server("runtime-search").is_some());
    let visible_models = snapshot.models_for(&compiled_key, ApiFormat::OpenAiResponses);
    assert_eq!(
        visible_models
            .iter()
            .map(|model| model.as_ref())
            .collect::<Vec<_>>(),
        ["sqlite-client-model"]
    );
}

#[tokio::test]
async fn sqlite_runtime_repository_requires_the_singleton_system_settings_row() {
    let pool = migrated_pool().await;
    let repository = SqliteRuntimeConfigRepository::new(pool);

    let control_plane = repository.load().await.unwrap();
    assert!(control_plane.api_keys.is_empty());
    assert!(control_plane.models.is_empty());
    assert!(control_plane.model_rules.is_empty());
    assert!(control_plane.groups.is_empty());
    assert!(control_plane.channels.is_empty());
    assert!(control_plane.proxies.is_empty());
    assert!(control_plane.templates.is_empty());
    assert!(control_plane.mcp_servers.is_empty());

    let error = repository.load_runtime().await.unwrap_err();
    assert!(matches!(error, RepositoryError::NotFound));
}
