//! Console API OpenAPI-spec consistency tests.
//!
//! These tests verify that the live Console HTTP implementation matches the
//! authoritative spec in `docs/openapi/console-v1.yaml` for the request and
//! response shapes the SPA depends on: the auth/session flow, error body
//! shape `{"error": ...}`, ETag/`If-Match` optimistic concurrency (success
//! then `409` on a stale tag), retrievable masked-by-default API key values,
//! proxy-draft IP diagnostics, and `limit` clamping on log endpoints.
//!
//! They follow the same PostgreSQL integration-test convention as
//! `tests/control_plane_integration.rs`: `TestDatabase::new()` creates a
//! throwaway database and `docker compose up -d` must provide PostgreSQL.

use std::sync::Arc;

use ai_gateway::{
    application::{
        AuthError, ChannelModelDiscoveryService, CodexConnectorService, ConsoleAuthService,
        ControlPlaneCoordinator, ModelSyncService, ProxyTestService, SystemMetricsService,
        hash_console_password,
    },
    domain::ApiFormat,
    http::console::{self, ConsoleState},
    models_dev::ModelsDevClient,
    persistence::{
        AuthRepository, ControlPlaneRepository, DEFAULT_USER_GROUP_ID, MIGRATOR,
        RequestLogRepository, SystemPassiveHealthSettingsInput, SystemSettingsInput,
        SystemUpstreamSettingsInput, run_migrations,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{AuthConfig, ModelsSyncConfig, RuntimeConfig, compile_runtime_config},
    upstream::UpstreamClientRegistry,
};
use axum::{
    Json, Router,
    body::Body,
    http::{HeaderMap, StatusCode, header},
    routing::{any, get},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use http_body_util::BodyExt;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::net::TcpListener;
use tower::ServiceExt;
use uuid::Uuid;

#[path = "support/metering.rs"]
mod metering_fixtures;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";
const PASSWORD_FILE_ADMIN_URL: &str = "postgres://ai_gateway@127.0.0.1:5432/postgres";
const TEST_PASSWORD: &str = "test-password-with-enough-length";
const TEST_ED25519_PRIVATE_KEY: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIMrLMWiLkvZoPg8iIZRZC0qNdQQPyJV5dCAWdo0l6YBu
-----END PRIVATE KEY-----
"#;
const TEST_ED25519_PUBLIC_KEY: &[u8] = br#"-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAQvs1EKtSBUS0aGjOVZhD2kqVMSiXHugcTiZTZyZxWiQ=
-----END PUBLIC KEY-----
"#;

#[path = "support/upstream_credentials.rs"]
mod upstream_credentials;

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
        Self::with_migrator(&MIGRATOR).await
    }

    async fn with_migrator(migrator: &sqlx::migrate::Migrator) -> Self {
        let admin_url =
            std::env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| default_admin_url());
        let mut database_url = reqwest::Url::parse(&admin_url).expect("admin URL valid");
        assert_ne!(
            database_url.path().trim_matches('/'),
            "ai_gateway",
            "TEST_DATABASE_ADMIN_URL must not target the ai_gateway database"
        );
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("PostgreSQL admin database available");
        let name = format!("ai_gateway_spec_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
            .execute(&admin)
            .await
            .expect("temp database creatable");
        database_url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url.as_str())
            .await
            .expect("temp database connectable");
        if std::ptr::eq(migrator, &MIGRATOR) {
            ai_gateway::persistence::run_migrations(&pool)
                .await
                .expect("startup migrations apply");
        } else {
            migrator
                .run(&pool)
                .await
                .expect("historical migrations apply");
        }
        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE \"{}\" WITH (FORCE)",
            self.name
        )))
        .execute(&self.admin)
        .await
        .expect("temp database removable");
        self.admin.close().await;
    }
}

#[tokio::test]
async fn retired_adapter_migration_preserves_usage_and_removes_control_plane_state() {
    let mut previous = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations"))
        .await
        .unwrap();
    previous.migrations = previous
        .iter()
        .filter(|migration| migration.version < 53)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    let database = TestDatabase::with_migrator(&previous).await;
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status) \
        VALUES ($1,'legacy-sharing-fixture@example.test','Legacy fixture','admin','active')",
    )
    .bind(user_id)
    .execute(&database.pool)
    .await
    .unwrap();
    ControlPlaneRepository::new(database.pool.clone())
        .ensure_system_settings(bootstrap_system_settings())
        .await
        .unwrap();
    let key = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,'legacy usage','legacy-test-secret','active', \
         ARRAY['open_ai_responses']::api_format[],ARRAY['proxy'])",
    )
    .bind(key)
    .bind(user_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let log = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation, \
          client_model,outcome,streamed,total_duration_ms,request_source,cost_amount) \
         VALUES ($1,now(),now(),$2,$3,'open_ai_responses','standalone_web_search', \
                 'legacy-search','succeeded',false,100,'mcp',0.25)",
    )
    .bind(log)
    .bind(user_id)
    .bind(key)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE system_settings SET value=value || '{\"mcp\":{\"enabled\":true}}'::jsonb \
         WHERE setting_key='forwarding_policy'",
    )
    .execute(&database.pool)
    .await
    .unwrap();

    run_migrations(&database.pool).await.unwrap();
    let app = app(database.pool.clone()).await;
    let state: (bool, bool, bool) = sqlx::query_as(
        "SELECT to_regclass('mcp_servers') IS NULL, to_regtype('mcp_server_kind') IS NULL, \
         NOT (SELECT value ? 'mcp' FROM system_settings WHERE setting_key='forwarding_policy')",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state, (true, true, true));
    let preserved: (String, String) =
        sqlx::query_as("SELECT request_source,cost_amount::text FROM request_logs WHERE id=$1")
            .bind(log)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(preserved.0, "client");
    assert_eq!(preserved.1.parse::<f64>().unwrap(), 0.25);
    let settings = ControlPlaneRepository::new(database.pool.clone())
        .system_settings()
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(settings.settings).unwrap(),
        serde_json::to_value(bootstrap_system_settings()).unwrap()
    );
    for path in ["/console/v1/mcp-servers", "/console/v1/mcp-servers/retired"] {
        let response = request(&app, "GET", path, serde_json::json!({}), &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    database.cleanup().await;
}

fn default_admin_url() -> String {
    let Ok(mut password) = std::fs::read_to_string("./config/postgres-password") else {
        return DEFAULT_ADMIN_URL.into();
    };
    while matches!(password.as_bytes().last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    if password.is_empty() {
        return DEFAULT_ADMIN_URL.into();
    }
    let mut url =
        reqwest::Url::parse(PASSWORD_FILE_ADMIN_URL).expect("default admin URL must be valid");
    url.set_password(Some(&password))
        .expect("PostgreSQL URL must accept a password");
    url.to_string()
}

fn auth_config() -> AuthConfig {
    AuthConfig {
        issuer: "test-ai-gateway".into(),
        audience: "test-console".into(),
        access_token_ttl_seconds: 900,
        refresh_token_ttl_seconds: 3_600,
        key_id: "test-key".into(),
        signing_key_path: "unused-test-private.pem".into(),
        verification_key_path: "unused-test-public.pem".into(),
    }
}

fn bootstrap_system_settings() -> SystemSettingsInput {
    SystemSettingsInput {
        api_hosts: Vec::new(),
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 300,
            standalone_web_search_response_header_timeout_seconds: 300,
            stream_idle_timeout_seconds: 3,
        },
        request_retry: Default::default(),
        passive_health: SystemPassiveHealthSettingsInput {
            connection_failure_threshold: 3,
            cooldown_seconds: 30,
        },
        automatic_disable: Default::default(),
        scheduled_testing: Default::default(),
        session_affinity: Default::default(),
        websocket: Default::default(),
        codex: Default::default(),
    }
}

struct App {
    router: axum::Router,
    access_token: String,
    user_id: Uuid,
    runtime: Arc<RuntimeConfig>,
    auth: ConsoleAuthService,
}

async fn app(pool: PgPool) -> App {
    app_with_proxy_test_endpoint(pool, None).await
}

async fn app_with_proxy_test_endpoint(
    pool: PgPool,
    proxy_test_endpoint: Option<reqwest::Url>,
) -> App {
    let user_id = Uuid::new_v4();
    let password_hash = hash_console_password(TEST_PASSWORD.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, email, display_name, role, status, password_hash) \
         VALUES ($1, $2, $3, 'admin', 'active', $4)",
    )
    .bind(user_id)
    .bind(format!("spec-user-{user_id}@example.test"))
    .bind(format!("spec-{user_id}"))
    .bind(password_hash)
    .execute(&pool)
    .await
    .unwrap();

    let repository = ControlPlaneRepository::new(pool.clone());
    repository
        .ensure_system_settings(bootstrap_system_settings())
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let upstream_clients = Arc::new(UpstreamClientRegistry::new());
    let codex_connector = CodexConnectorService::new(
        repository.clone(),
        coordinator.clone(),
        Arc::clone(&runtime),
        Arc::clone(&upstream_clients),
    )
    .await
    .unwrap();
    let model_sync = ModelSyncService::new(
        coordinator.clone(),
        ModelsDevClient::new(&ModelsSyncConfig::default()).unwrap(),
        100,
    );
    let auth = ConsoleAuthService::from_pem(
        AuthRepository::new(pool.clone()),
        &auth_config(),
        TEST_ED25519_PRIVATE_KEY,
        TEST_ED25519_PUBLIC_KEY,
    )
    .unwrap();
    let email = format!("spec-user-{user_id}@example.test");
    let session = auth
        .login_with_user_agent(email, TEST_PASSWORD.into(), Some("Spec Browser/1.0".into()))
        .await
        .unwrap();
    // Sanity: the freshly issued token must round-trip through the same
    // authenticator before we hand it to HTTP. This surfaces key/claim
    // mismatches as a clear panic instead of a downstream 401.
    auth.authenticate_access_token(&session.access_token)
        .await
        .expect("issued access token must authenticate");
    let proxy_tests = match proxy_test_endpoint {
        Some(endpoint) => ProxyTestService::new_with_endpoint(
            repository.clone(),
            Arc::clone(&runtime),
            Arc::clone(&upstream_clients),
            endpoint,
        ),
        None => ProxyTestService::new(
            repository.clone(),
            Arc::clone(&runtime),
            Arc::clone(&upstream_clients),
        ),
    };
    let router = console::router(ConsoleState {
        coordinator,
        codex_connector,
        channel_models: ChannelModelDiscoveryService::new(
            Arc::clone(&runtime),
            Arc::clone(&upstream_clients),
        ),
        proxy_tests,
        model_sync,
        auth: auth.clone(),
        request_logs: RequestLogRepository::new(pool.clone()).queries(),
        system_metrics: SystemMetricsService::new(pool.into(), 5),
        console_body_bytes: 1_048_576,
        auth_body_bytes: 16_384,
        allowed_origins: vec![],
    });
    App {
        router,
        access_token: session.access_token,
        user_id,
        runtime,
        auth,
    }
}

struct TestTopology {
    group: Uuid,
    access: Uuid,
    channel: Uuid,
    capability: Uuid,
}

fn codex_fixture_input(group: Uuid, label: &str) -> ai_gateway::persistence::CodexCredentialCreate {
    ai_gateway::persistence::CodexCredentialCreate {
        channel_group_id: group,
        label: label.into(),
        enabled: true,
        proxy_id: None,
        quota_threshold_percent: 95,
        base_url: "https://chatgpt.com/backend-api/codex".into(),
        email: Some(format!("{label}@example.test")),
        account_id: Some(format!("{label}-account")),
        user_id: Some(format!("{label}-user")),
        plan_type: Some("plus".into()),
        is_fedramp: false,
        id_token: "id-token".into(),
        access_token: "access-token".into(),
        refresh_token: "refresh-token".into(),
        access_token_expires_at: None,
        available_models: vec!["gpt-5-codex".into()],
        quota: None,
    }
}

async fn create_test_codex_credential(
    pool: &PgPool,
    app: &App,
    input: ai_gateway::persistence::CodexCredentialCreate,
) -> Uuid {
    let coordinator = ControlPlaneCoordinator::new(
        ControlPlaneRepository::new(pool.clone()),
        Arc::clone(&app.runtime),
        ai_gateway::routing::RoutingRuntime::new(
            ai_gateway::routing::PassiveHealthPolicy::default(),
        ),
    );
    coordinator
        .create_codex_credential(app.user_id, input, None)
        .await
        .unwrap()
        .id
}

async fn codex_capability_id(pool: &PgPool, credential: Uuid, operation: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM channel_capabilities WHERE channel_id=$1 AND operation=$2 AND deleted_at IS NULL")
        .bind(credential).bind(operation).fetch_one(pool).await.unwrap()
}

async fn create_test_pricing_model(app: &App) -> Uuid {
    create_resource(
        app,
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": format!("priced-{}", Uuid::new_v4()),
            "display_name": "Spec pricing model", "enabled": true,
            "price_unit_tokens": 1000000, "input_unit_price": "1",
            "cached_input_unit_price": "0", "cache_write_unit_price": "0", "output_unit_price": "2",
            "price_effective_at": "2026-01-01T00:00:00Z"
        }),
    )
    .await
}

fn capability_input(channel: Uuid, operation: &str) -> serde_json::Value {
    serde_json::json!({
        "channel_id": channel,
        "settings": {
            "operation": operation, "enabled": true,
            "available_models": ["wire-v1"], "request_compression": "default",
            "auto_disable_allowed": false, "test_model": null, "test_pricing_model_id": null
        },
        "config_template_id": null, "override_document": {},
        "billing_multiplier": "1", "status_statistics_enabled": false
    })
}

async fn create_resource(app: &App, path: &str, input: serde_json::Value) -> Uuid {
    let response = request(app, "POST", path, input, &[]).await;
    let status = response.status();
    let body = body_json(response).await;
    assert_eq!(status, StatusCode::CREATED, "{path}: {body}");
    serde_json::from_value(body["id"].clone()).unwrap()
}

async fn seed_test_topology(app: &App, operation: &str) -> TestTopology {
    let group = create_resource(
        app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": format!("spec-{}", Uuid::new_v4()), "enabled": true
        }),
    )
    .await;
    let access = create_resource(
        app,
        "/console/v1/routing/accesses",
        serde_json::json!({
            "name": "Spec access", "connector_kind": "general",
            "base_url": "https://upstream.example.test", "enabled": true
        }),
    )
    .await;
    let channel = create_resource(
        app,
        "/console/v1/routing/logical-channels",
        serde_json::json!({
            "group_id": group, "access_id": access, "credential_id": null,
            "name": "Spec channel", "enabled": true
        }),
    )
    .await;
    let capability = create_resource(
        app,
        "/console/v1/routing/capabilities",
        capability_input(channel, operation),
    )
    .await;
    TestTopology {
        group,
        access,
        channel,
        capability,
    }
}

async fn create_test_operation_rule(app: &App, capability: Uuid, operation: &str) -> (Uuid, Uuid) {
    let model = create_test_pricing_model(app).await;
    let profile = create_resource(
        app,
        "/console/v1/routing/profiles",
        serde_json::json!({"model_id": model}),
    )
    .await;
    let rule = create_resource(app, "/console/v1/routing/operation-rules", serde_json::json!({
        "model_routing_profile_id": profile, "operation": operation, "enabled": true,
        "routing_tiers": [{
            "priority": 0, "selection_strategy": "weighted_random",
            "candidates": [{"capability_id": capability, "upstream_model": "wire-v1", "weight": 1}]
        }]
    })).await;
    (profile, rule)
}

async fn proxy_test_ip_api(
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<serde_json::Value>), StatusCode> {
    let expected = format!(
        "Basic {}",
        BASE64_STANDARD.encode("spec-proxy-user:spec-proxy-pass")
    );
    if headers
        .get("proxy-authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return Err(StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }
    let mut response_headers = HeaderMap::new();
    response_headers.insert("x-rl", "44".parse().unwrap());
    response_headers.insert("x-ttl", "60".parse().unwrap());
    Ok((
        response_headers,
        Json(serde_json::json!({
            "status": "success",
            "continent": "North America",
            "continentCode": "NA",
            "country": "United States",
            "countryCode": "US",
            "region": "CA",
            "regionName": "California",
            "city": "Los Angeles",
            "district": "",
            "zip": "90001",
            "lat": 34.0522,
            "lon": -118.2437,
            "timezone": "America/Los_Angeles",
            "offset": -25200,
            "currency": "USD",
            "isp": "Spec ISP",
            "org": "Spec Organization",
            "as": "AS64500 Spec",
            "asname": "SPEC",
            "mobile": false,
            "proxy": true,
            "hosting": false,
            "query": "203.0.113.10"
        })),
    ))
}

#[tokio::test]
async fn proxy_test_uses_saved_credentials_and_returns_ip_metadata() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy_task = tokio::spawn(async move {
        axum::serve(listener, Router::new().fallback(any(proxy_test_ip_api)))
            .await
            .unwrap();
    });
    let database = TestDatabase::new().await;
    let app = app_with_proxy_test_endpoint(
        database.pool.clone(),
        Some(reqwest::Url::parse("http://ip-api.test/json/").unwrap()),
    )
    .await;
    let proxy_url = format!("http://{proxy_address}");
    let created = request(
        &app,
        "POST",
        "/console/v1/network/proxies",
        serde_json::json!({
            "name": "spec-proxy-test",
            "proxy_url": proxy_url,
            "username": "spec-proxy-user",
            "password": "spec-proxy-pass",
            "no_proxy_hosts": ["ip-api.test"],
            "enabled": false
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let proxy_id = body_json(created).await["id"].as_str().unwrap().to_owned();

    let tested = request(
        &app,
        "POST",
        "/console/v1/network/proxies/test",
        serde_json::json!({
            "proxy_id": proxy_id,
            "proxy_url": proxy_url
        }),
        &[],
    )
    .await;
    assert_eq!(tested.status(), StatusCode::OK);
    let tested = body_json(tested).await;
    assert_eq!(tested["ip"], "203.0.113.10");
    assert_eq!(tested["country_code"], "US");
    assert_eq!(tested["region_name"], "California");
    assert_eq!(tested["isp"], "Spec ISP");
    assert_eq!(tested["proxy"], true);
    assert_eq!(tested["rate_limit_remaining"], 44);
    assert!(tested["latency_ms"].is_u64());

    let changed_endpoint = request(
        &app,
        "POST",
        "/console/v1/network/proxies/test",
        serde_json::json!({
            "proxy_id": proxy_id,
            "proxy_url": "http://127.0.0.1:9"
        }),
        &[],
    )
    .await;
    assert_eq!(changed_endpoint.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(changed_endpoint).await["error"],
        "proxy_test_credentials_required"
    );

    proxy_task.abort();
    database.cleanup().await;
}

#[tokio::test]
async fn system_settings_bootstrap_initializes_once_without_overwriting_database_values() {
    let database = TestDatabase::new().await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let first = bootstrap_system_settings();
    let replacement = SystemSettingsInput {
        api_hosts: Vec::new(),
        upstream: SystemUpstreamSettingsInput {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 10,
            images_response_header_timeout_seconds: 300,
            standalone_web_search_response_header_timeout_seconds: 300,
            stream_idle_timeout_seconds: 15,
        },
        request_retry: Default::default(),
        passive_health: SystemPassiveHealthSettingsInput {
            connection_failure_threshold: 6,
            cooldown_seconds: 60,
        },
        automatic_disable: Default::default(),
        scheduled_testing: Default::default(),
        session_affinity: Default::default(),
        websocket: Default::default(),
        codex: Default::default(),
    };

    repository
        .ensure_system_settings(first.clone())
        .await
        .unwrap();
    repository
        .ensure_system_settings(replacement)
        .await
        .unwrap();

    let stored = repository.system_settings().await.unwrap();
    assert_eq!(stored.settings.upstream.connect_timeout_seconds, 1);
    assert_eq!(
        stored
            .settings
            .upstream
            .images_response_header_timeout_seconds,
        300
    );
    assert!(stored.settings.request_retry.enabled);
    assert_eq!(stored.settings.request_retry.max_retries, 1);
    assert!(
        stored
            .settings
            .request_retry
            .retryable_status_codes
            .is_empty()
    );
    assert_eq!(
        stored.settings.passive_health.connection_failure_threshold,
        3
    );
    let initializations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs \
         WHERE actor_type='system' AND action='initialize' AND object_type='system_settings'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(initializations, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn system_settings_bootstrap_backfills_late_sections_once_for_upgraded_databases() {
    let database = TestDatabase::new().await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    repository
        .ensure_system_settings(bootstrap_system_settings())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE system_settings SET value=value-'codex' WHERE setting_key='forwarding_policy'",
    )
    .execute(&database.pool)
    .await
    .unwrap();

    let mut bootstrap = bootstrap_system_settings();
    bootstrap.codex.workspace_path = "/synthetic/project".into();
    bootstrap.codex.git_remote_url = "https://github.com/example/synthetic-project".into();
    bootstrap.codex.originator = "codex_gateway".into();
    bootstrap.codex.client_version = "9.8.7".into();
    bootstrap.codex.user_agent = "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway".into();
    repository
        .ensure_system_settings(bootstrap.clone())
        .await
        .unwrap();

    let stored = repository.system_settings().await.unwrap();
    assert_eq!(stored.settings.codex.workspace_path, "/synthetic/project");
    assert_eq!(
        stored.settings.codex.git_remote_url,
        "https://github.com/example/synthetic-project"
    );
    assert_eq!(stored.settings.codex.originator, "codex_gateway");
    assert_eq!(stored.settings.codex.client_version, "9.8.7");
    assert_eq!(
        stored.settings.codex.user_agent,
        "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway"
    );

    sqlx::query(
        "UPDATE system_settings \
         SET value=jsonb_set( \
             value, \
             '{codex}', \
             (value->'codex')-'originator'-'client_version'-'user_agent', \
             false \
         ) \
         WHERE setting_key='forwarding_policy'",
    )
    .execute(&database.pool)
    .await
    .unwrap();
    repository
        .ensure_system_settings(bootstrap.clone())
        .await
        .unwrap();
    let stored = repository.system_settings().await.unwrap();
    assert_eq!(stored.settings.codex.originator, "codex_gateway");
    assert_eq!(stored.settings.codex.client_version, "9.8.7");
    assert_eq!(
        stored.settings.codex.user_agent,
        "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway"
    );

    let mut replacement = bootstrap_system_settings();
    replacement.codex.workspace_path = "/replacement".into();
    replacement.codex.git_remote_url = "https://github.com/example/replacement".into();
    replacement.codex.originator = "replacement-originator".into();
    replacement.codex.client_version = "1.2.3".into();
    replacement.codex.user_agent = "replacement/1.2.3".into();
    repository
        .ensure_system_settings(replacement)
        .await
        .unwrap();

    let stored = repository.system_settings().await.unwrap();
    assert_eq!(stored.settings.codex.workspace_path, "/synthetic/project");
    assert_eq!(
        stored.settings.codex.git_remote_url,
        "https://github.com/example/synthetic-project"
    );
    assert_eq!(stored.settings.codex.originator, "codex_gateway");
    assert_eq!(stored.settings.codex.client_version, "9.8.7");
    assert_eq!(
        stored.settings.codex.user_agent,
        "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway"
    );
    database.cleanup().await;
}

#[tokio::test]
async fn session_affinity_cache_endpoint_reports_and_clears_current_process_state() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let report = request(
        &app,
        "GET",
        "/console/v1/system/session-affinity/cache",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(report.status(), StatusCode::OK);
    let report = body_json(report).await;
    assert_eq!(report["enabled"], false);
    assert_eq!(report["total_entries"], 0);
    assert!(report["rules"].is_array());

    let cleared = request(
        &app,
        "DELETE",
        "/console/v1/system/session-affinity/cache",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(cleared.status(), StatusCode::OK);
    let cleared = body_json(cleared).await;
    assert_eq!(cleared["cleared_entries"], 0);
    assert_eq!(cleared["cache"]["total_entries"], 0);

    let missing_rule = request(
        &app,
        "DELETE",
        "/console/v1/system/session-affinity/cache?rule_name=missing",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(missing_rule.status(), StatusCode::NOT_FOUND);

    database.cleanup().await;
}

#[tokio::test]
async fn emergency_admin_password_reset_revokes_existing_sessions() {
    let database = TestDatabase::new().await;
    let user_id = Uuid::new_v4();
    let email = format!("reset-admin-{user_id}@example.test");
    let old_password = "old-password-with-enough-length";
    let new_password = "new-password-with-enough-length";
    let old_hash = hash_console_password(old_password.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash) \
         VALUES ($1,$2,$3,'admin','active',$4)",
    )
    .bind(user_id)
    .bind(&email)
    .bind("Emergency reset admin")
    .bind(old_hash)
    .execute(&database.pool)
    .await
    .unwrap();

    let auth = ConsoleAuthService::from_pem(
        AuthRepository::new(database.pool.clone()),
        &auth_config(),
        TEST_ED25519_PRIVATE_KEY,
        TEST_ED25519_PUBLIC_KEY,
    )
    .unwrap();
    let old_session = auth
        .login(email.clone(), old_password.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE users SET password_change_required=true,temporary_password_issued_at=now(), \
         temporary_password_expires_at=now()+interval '1 day' WHERE id=$1",
    )
    .bind(user_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let new_hash = hash_console_password(new_password.to_owned())
        .await
        .unwrap();
    assert!(
        AuthRepository::new(database.pool.clone())
            .reset_active_admin_password(&email, &new_hash)
            .await
            .unwrap()
    );

    assert!(matches!(
        auth.authenticate_access_token(&old_session.access_token)
            .await,
        Err(AuthError::InvalidToken)
    ));
    assert!(matches!(
        auth.login(email.clone(), old_password.to_owned()).await,
        Err(AuthError::InvalidCredentials)
    ));
    auth.login(email, new_password.to_owned())
        .await
        .expect("the replacement password must work");
    let reset_state: (bool, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT password_change_required,temporary_password_expires_at \
         FROM users WHERE id=$1",
    )
    .bind(user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!reset_state.0);
    assert!(reset_state.1.is_none());

    let audit_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM audit_logs \
         WHERE actor_type='system' AND action='reset_password' AND object_id=$1",
    )
    .bind(user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn administrator_temporary_password_forces_and_completes_password_change() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let target_user_id = Uuid::new_v4();
    let target_email = format!("temporary-password-{target_user_id}@example.test");
    let old_password = "old-password-with-enough-length";
    let new_password = "new-permanent-password-value";
    let old_hash = hash_console_password(old_password.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash) \
         VALUES ($1,$2,'Temporary Password Target','user','active',$3)",
    )
    .bind(target_user_id)
    .bind(&target_email)
    .bind(old_hash)
    .execute(&database.pool)
    .await
    .unwrap();
    let api_key_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,'temporary-password-key',$3,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[], \
                 ARRAY['proxy','models.read'])",
    )
    .bind(api_key_id)
    .bind(target_user_id)
    .bind(format!("sk-temporary-password-{api_key_id}"))
    .execute(&database.pool)
    .await
    .unwrap();

    let old_session = app
        .auth
        .login(target_email.clone(), old_password.to_owned())
        .await
        .unwrap();
    let self_reset = request(
        &app,
        "POST",
        &format!("/console/v1/users/{}/temporary-password", app.user_id),
        serde_json::json!({"current_password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(self_reset.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(self_reset).await["error"], "cannot_reset_self");

    let wrong_reauthentication = request(
        &app,
        "POST",
        &format!("/console/v1/users/{target_user_id}/temporary-password"),
        serde_json::json!({"current_password":"wrong-administrator-password"}),
        &[],
    )
    .await;
    assert_eq!(wrong_reauthentication.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(wrong_reauthentication).await["error"],
        "reauthentication_failed"
    );

    let first_issue = request(
        &app,
        "POST",
        &format!("/console/v1/users/{target_user_id}/temporary-password"),
        serde_json::json!({"current_password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(first_issue.status(), StatusCode::CREATED);
    let first_issue = body_json(first_issue).await;
    let first_temporary_password = first_issue["temporary_password"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(first_temporary_password.starts_with("AGW-"));
    assert_eq!(first_issue["user_id"], target_user_id.to_string());
    let first_expiry = chrono::DateTime::parse_from_rfc3339(
        first_issue["expires_at"]
            .as_str()
            .expect("expiry is a string"),
    )
    .unwrap()
    .with_timezone(&chrono::Utc);
    let remaining = first_expiry.signed_duration_since(chrono::Utc::now());
    assert!(remaining > chrono::Duration::hours(23));
    assert!(remaining <= chrono::Duration::hours(24));

    assert!(matches!(
        app.auth
            .authenticate_access_token(&old_session.access_token)
            .await,
        Err(AuthError::InvalidToken)
    ));
    assert!(matches!(
        app.auth
            .login(target_email.clone(), old_password.to_owned())
            .await,
        Err(AuthError::InvalidCredentials)
    ));

    let replacement = request(
        &app,
        "POST",
        &format!("/console/v1/users/{target_user_id}/temporary-password"),
        serde_json::json!({"current_password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(replacement.status(), StatusCode::CREATED);
    let replacement = body_json(replacement).await;
    let temporary_password = replacement["temporary_password"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(temporary_password, first_temporary_password);

    let superseded_login = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/login",
        serde_json::json!({
            "email": &target_email,
            "password": first_temporary_password,
        }),
    )
    .await;
    assert_eq!(superseded_login.status(), StatusCode::UNAUTHORIZED);

    let temporary_login = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/login",
        serde_json::json!({
            "email": &target_email,
            "password": &temporary_password,
        }),
    )
    .await;
    assert_eq!(temporary_login.status(), StatusCode::OK);
    let refresh_token = temporary_login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_owned();
    let temporary_login = body_json(temporary_login).await;
    assert_eq!(temporary_login["user"]["password_change_required"], true);
    assert!(temporary_login["user"]["temporary_password_expires_at"].is_string());
    let temporary_access_token = temporary_login["access_token"].as_str().unwrap();
    let refreshed = app.auth.refresh(&refresh_token).await.unwrap();
    assert!(refreshed.user.password_change_required);
    assert!(refreshed.user.temporary_password_expires_at.is_some());
    let session_purpose: String = sqlx::query_scalar(
        "SELECT purpose FROM user_sessions \
         WHERE user_id=$1 AND revoked_at IS NULL ORDER BY created_at DESC LIMIT 1",
    )
    .bind(target_user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(session_purpose, "password_change");

    let blocked = request_with_token(
        &app,
        temporary_access_token,
        "GET",
        "/console/v1/me",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(blocked).await["error"],
        "password_change_required"
    );

    let same_password = request_with_token(
        &app,
        temporary_access_token,
        "POST",
        "/console/v1/auth/complete-password-reset",
        serde_json::json!({"new_password":&temporary_password}),
        &[],
    )
    .await;
    assert_eq!(same_password.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(same_password).await["error"],
        "new_password_matches_temporary"
    );

    let completed = request_with_token(
        &app,
        temporary_access_token,
        "POST",
        "/console/v1/auth/complete-password-reset",
        serde_json::json!({"new_password":new_password}),
        &[("user-agent", "Completed Reset Browser/1.0")],
    )
    .await;
    assert_eq!(completed.status(), StatusCode::OK);
    assert!(completed.headers().contains_key(header::SET_COOKIE));
    let completed = body_json(completed).await;
    assert_eq!(completed["user"]["password_change_required"], false);
    assert!(completed["user"]["temporary_password_expires_at"].is_null());
    let normal_access_token = completed["access_token"].as_str().unwrap();
    let profile = request_with_token(
        &app,
        normal_access_token,
        "GET",
        "/console/v1/me",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(profile.status(), StatusCode::OK);

    assert!(matches!(
        app.auth
            .login(target_email.clone(), temporary_password.clone())
            .await,
        Err(AuthError::InvalidCredentials)
    ));
    app.auth
        .login(target_email, new_password.to_owned())
        .await
        .expect("the permanent password must work");

    let api_key_status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(api_key_id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(api_key_status, "active");
    let reset_state: (bool, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT password_change_required,temporary_password_expires_at \
         FROM users WHERE id=$1",
    )
    .bind(target_user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!reset_state.0);
    assert!(reset_state.1.is_none());

    let audit_payload: String = sqlx::query_scalar(
        "SELECT string_agg(COALESCE(before_redacted,'{}')::text || \
                           COALESCE(after_redacted,'{}')::text,'') \
         FROM audit_logs \
         WHERE object_id=$1 AND action IN \
               ('issue_temporary_password','complete_password_reset')",
    )
    .bind(target_user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!audit_payload.contains(&temporary_password));
    assert!(!audit_payload.contains("password_hash"));
    let issue_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs \
         WHERE object_id=$1 AND action='issue_temporary_password'",
    )
    .bind(target_user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(issue_audits, 2);

    database.cleanup().await;
}

#[tokio::test]
async fn expired_temporary_password_rejects_login_refresh_and_completion() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let target_user_id = Uuid::new_v4();
    let target_email = format!("expired-temporary-password-{target_user_id}@example.test");
    let old_hash = hash_console_password("expired-old-password-value".to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash) \
         VALUES ($1,$2,'Expired Temporary Password','user','active',$3)",
    )
    .bind(target_user_id)
    .bind(&target_email)
    .bind(old_hash)
    .execute(&database.pool)
    .await
    .unwrap();

    let issued = request(
        &app,
        "POST",
        &format!("/console/v1/users/{target_user_id}/temporary-password"),
        serde_json::json!({"current_password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(issued.status(), StatusCode::CREATED);
    let temporary_password = body_json(issued).await["temporary_password"]
        .as_str()
        .unwrap()
        .to_owned();
    let session = app
        .auth
        .login(target_email.clone(), temporary_password.clone())
        .await
        .unwrap();
    let principal = app
        .auth
        .authenticate_access_token(&session.access_token)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE users SET temporary_password_issued_at=now()-interval '2 days', \
         temporary_password_expires_at=now()-interval '1 day' WHERE id=$1",
    )
    .bind(target_user_id)
    .execute(&database.pool)
    .await
    .unwrap();

    assert!(matches!(
        app.auth.login(target_email, temporary_password).await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(matches!(
        app.auth
            .authenticate_access_token(&session.access_token)
            .await,
        Err(AuthError::InvalidToken)
    ));
    assert!(matches!(
        app.auth.refresh(&session.refresh_token).await,
        Err(AuthError::InvalidToken)
    ));
    assert!(matches!(
        app.auth
            .complete_temporary_password(
                principal,
                "expired-reset-replacement-password".to_owned(),
                None,
            )
            .await,
        Err(AuthError::InvalidToken)
    ));
    let password_change_required: bool =
        sqlx::query_scalar("SELECT password_change_required FROM users WHERE id=$1")
            .bind(target_user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(password_change_required);

    database.cleanup().await;
}

#[tokio::test]
async fn concurrent_temporary_password_completion_allows_only_one_winner() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let target_user_id = Uuid::new_v4();
    let target_email = format!("concurrent-temporary-password-{target_user_id}@example.test");
    let old_hash = hash_console_password("concurrent-old-password-value".to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash) \
         VALUES ($1,$2,'Concurrent Temporary Password','user','active',$3)",
    )
    .bind(target_user_id)
    .bind(&target_email)
    .bind(old_hash)
    .execute(&database.pool)
    .await
    .unwrap();

    let issued = request(
        &app,
        "POST",
        &format!("/console/v1/users/{target_user_id}/temporary-password"),
        serde_json::json!({"current_password":TEST_PASSWORD}),
        &[],
    )
    .await;
    assert_eq!(issued.status(), StatusCode::CREATED);
    let temporary_password = body_json(issued).await["temporary_password"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_session = app
        .auth
        .login(target_email.clone(), temporary_password.clone())
        .await
        .unwrap();
    let second_session = app
        .auth
        .login(target_email.clone(), temporary_password.clone())
        .await
        .unwrap();
    let first_principal = app
        .auth
        .authenticate_access_token(&first_session.access_token)
        .await
        .unwrap();
    let second_principal = app
        .auth
        .authenticate_access_token(&second_session.access_token)
        .await
        .unwrap();
    let first_password = "first-concurrent-permanent-password";
    let second_password = "second-concurrent-permanent-password";
    let first_auth = app.auth.clone();
    let second_auth = app.auth.clone();
    let (first_result, second_result) = tokio::join!(
        first_auth.complete_temporary_password(first_principal, first_password.to_owned(), None,),
        second_auth
            .complete_temporary_password(second_principal, second_password.to_owned(), None,),
    );

    let (winning_password, losing_password, losing_error) = match (first_result, second_result) {
        (Ok(_), Err(error)) => (first_password, second_password, error),
        (Err(error), Ok(_)) => (second_password, first_password, error),
        (Ok(_), Ok(_)) => panic!("both concurrent password completions succeeded"),
        (Err(first), Err(second)) => {
            panic!("both concurrent password completions failed: {first}; {second}")
        }
    };
    assert!(matches!(losing_error, AuthError::InvalidToken));
    app.auth
        .login(target_email.clone(), winning_password.to_owned())
        .await
        .expect("the winning permanent password must work");
    assert!(matches!(
        app.auth
            .login(target_email.clone(), losing_password.to_owned())
            .await,
        Err(AuthError::InvalidCredentials)
    ));
    assert!(matches!(
        app.auth.login(target_email, temporary_password).await,
        Err(AuthError::InvalidCredentials)
    ));

    database.cleanup().await;
}

async fn request(
    app: &App,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    request_with_token(app, &app.access_token, method, path, body, headers).await
}

#[tokio::test]
async fn upstream_access_contract_is_versioned_and_does_not_create_authority() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = "/console/v1/routing/accesses";
    let input = serde_json::json!({
        "name": "Spec access", "connector_kind": "general",
        "base_url": "https://spec-access.test/private-path", "enabled": false,
        "proxy_id": null, "connect_timeout_ms": null,
        "response_header_timeout_ms": 30000, "stream_idle_timeout_ms": null,
    });
    assert_eq!(
        request_with_token(&app, "", "GET", path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let created = request(&app, "POST", path, input.clone(), &[]).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_str().unwrap().to_owned();
    let detail_path = format!("{path}/{id}");
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let before = body_json(detail).await;
    assert_eq!(before["enabled"], false);
    assert!(before.get("credential_id").is_none());
    let mut changed = input.clone();
    changed["name"] = serde_json::json!("Renamed access");
    assert_eq!(
        request(
            &app,
            "PUT",
            &detail_path,
            changed.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "PUT", &detail_path, changed, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let after = body_json(detail).await;
    assert_ne!(before["revision"], after["revision"]);
    let mut invalid = input.clone();
    invalid["connector_kind"] = serde_json::json!("codex");
    assert_eq!(
        request(&app, "PUT", &detail_path, invalid, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = input;
    invalid["credential_id"] = serde_json::json!(Uuid::new_v4());
    assert_eq!(
        request(&app, "POST", path, invalid, &[]).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let authority: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM upstream_channels)+(SELECT count(*) FROM channel_capabilities)
              +(SELECT count(*) FROM model_capability_candidates)+(SELECT count(*) FROM api_key_capability_grants)",
    ).fetch_one(&database.pool).await.unwrap();
    assert_eq!(authority, 0);
    let audit: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_type='upstream_access'",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit.len(), 2);
    assert!(audit.iter().all(|event| event["base_url"] == "[REDACTED]"));
    database.cleanup().await;
}

#[tokio::test]
async fn canonical_topology_contract_has_versioned_immutable_capability_identity() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    for path in [
        "/console/v1/routing/channel-groups",
        "/console/v1/routing/channels",
        "/console/v1/routing/model-rules",
    ] {
        for method in ["GET", "POST", "PUT", "DELETE"] {
            assert_eq!(
                request(&app, method, path, serde_json::json!({}), &[])
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{method} {path}",
            );
            let detail = format!("{path}/{}", Uuid::new_v4());
            assert_eq!(
                request(&app, method, &detail, serde_json::json!({}), &[])
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{method} {detail}",
            );
        }
    }
    let model = request(&app, "POST", "/console/v1/models", serde_json::json!({
        "source_model_id": "canonical-profile-model", "display_name": "Canonical profile model",
        "enabled": true, "price_unit_tokens": 1000000, "input_unit_price": "0.1",
        "cached_input_unit_price": "0", "cache_write_unit_price": "0", "output_unit_price": "0.2",
        "price_effective_at": chrono::Utc::now().to_rfc3339()
    }), &[]).await;
    assert_eq!(model.status(), StatusCode::CREATED);
    let model_id = body_json(model).await["id"].as_str().unwrap().to_owned();
    let profile_input = serde_json::json!({"model_id": model_id});
    let profile = request(
        &app,
        "POST",
        "/console/v1/routing/profiles",
        profile_input.clone(),
        &[],
    )
    .await;
    assert_eq!(profile.status(), StatusCode::CREATED);
    let profile_id = body_json(profile).await["id"].as_str().unwrap().to_owned();
    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/profiles/{profile_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert!(detail.headers().contains_key("etag"));
    assert_eq!(body_json(detail).await["model_id"], model_id);
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/routing/profiles",
            profile_input,
            &[]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let profiles = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/profiles",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(profiles[0]["id"], profile_id);
    let graph = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/operation-rules",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(graph, serde_json::json!([]));
    let mut owners = Vec::new();
    for (path, input) in [
        (
            "/console/v1/routing/groups",
            serde_json::json!({"name": "Canonical group", "enabled": true}),
        ),
        (
            "/console/v1/routing/accesses",
            serde_json::json!({
                "name": "Canonical access", "enabled": true,
                "connector_kind": "general", "base_url": "https://canonical.test"
            }),
        ),
    ] {
        let response = request(&app, "POST", path, input, &[]).await;
        assert_eq!(response.status(), StatusCode::CREATED);
        owners.push(body_json(response).await["id"].as_str().unwrap().to_owned());
    }
    let input = serde_json::json!({
        "group_id": owners[0], "access_id": owners[1], "credential_id": null,
        "name": "Canonical channel", "enabled": true,
    });
    let response = request(
        &app,
        "POST",
        "/console/v1/routing/logical-channels",
        input,
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let channel = body_json(response).await["id"].as_str().unwrap().to_owned();
    let mut input = serde_json::json!({
        "channel_id": channel,
        "settings": {
            "operation": "images_generation",
            "enabled": false, "available_models": ["image-wire"],
            "request_compression": "default", "test_model": null,
            "test_pricing_model_id": null, "auto_disable_allowed": false
        }
    });
    let response = request(
        &app,
        "POST",
        "/console/v1/routing/capabilities",
        input.clone(),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let capability = body_json(response).await["id"].as_str().unwrap().to_owned();
    let path = format!("/console/v1/routing/capabilities/{capability}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    input["settings"]["enabled"] = serde_json::json!(true);
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    input["settings"]["operation"] = serde_json::json!("images_edit");
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let detail = body_json(request(&app, "GET", &path, serde_json::json!({}), &[]).await).await;
    let batch = serde_json::json!({
        "items": [{"id": capability, "updated_at": detail["updated_at"]}],
        "changes": {"enabled": false, "auto_disable_allowed": true, "billing_multiplier": "1.5"}
    });
    let mut invalid_batch = batch.clone();
    invalid_batch["items"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": Uuid::new_v4(), "updated_at": detail["updated_at"]
        }));
    let batch_path = "/console/v1/routing/capabilities/batch";
    assert_eq!(
        request(&app, "POST", batch_path, invalid_batch, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        body_json(request(&app, "GET", &path, serde_json::json!({}), &[]).await).await,
        detail
    );
    let updated = request(&app, "POST", batch_path, batch.clone(), &[]).await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body_json(updated).await;
    assert_eq!(updated["updated_ids"], serde_json::json!([capability]));
    assert!(updated["correlation_id"].is_string());
    assert_eq!(
        request(&app, "POST", batch_path, batch, &[]).await.status(),
        StatusCode::CONFLICT
    );
    let recover_path = format!("{path}/recover");
    assert_eq!(
        request(
            &app,
            "POST",
            &recover_path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    sqlx::query("UPDATE channel_capabilities SET auto_disabled=true,auto_disable_reason='HTTP 429',auto_disable_at=now() WHERE id=$1")
        .bind(capability.parse::<Uuid>().unwrap()).execute(&database.pool).await.unwrap();
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let disabled = body_json(detail).await;
    assert_eq!(disabled["settings"]["enabled"], false);
    assert_eq!(
        request(
            &app,
            "POST",
            &recover_path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let recovered = body_json(detail).await;
    assert_eq!(recovered["settings"]["enabled"], false);
    assert_eq!(recovered["auto_disabled"], false);
    assert!(recovered["auto_disable_reason"].is_null());
    assert!(recovered["auto_disable_at"].is_null());
    assert_ne!(recovered["revision"], disabled["revision"]);
    let authority: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM model_capability_candidates)
              +(SELECT count(*) FROM api_key_capability_grants)
              +(SELECT count(*) FROM api_key_policy_capability_grants)",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(authority, 0);
    let channel_path = format!("/console/v1/routing/logical-channels/{channel}");
    let detail = request(&app, "GET", &channel_path, serde_json::json!({}), &[]).await;
    let channel_etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &channel_path,
            serde_json::json!({}),
            &[("if-match", &channel_etag)]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &channel_path,
            serde_json::json!({}),
            &[("if-match", &channel_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    database.cleanup().await;
}

#[tokio::test]
async fn upstream_credential_contract_is_secret_safe_versioned_and_strict() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = "/console/v1/routing/upstream-credentials";
    let input = serde_json::json!({
        "name": "spec shared identity", "kind": "header", "header_name": "x-api-key",
        "secret": "spec-identity-secret", "enabled": true,
        "allowed_base_urls": ["HTTPS://EXAMPLE.TEST:443/"],
    });
    let created = request(&app, "POST", path, input.clone(), &[]).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_str().unwrap().to_owned();
    let detail_path = format!("{path}/{id}");
    let listed = body_json(request(&app, "GET", path, serde_json::json!({}), &[]).await).await;
    let listed = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == id)
        .unwrap();
    assert!(listed.get("secret").is_none());
    assert_eq!(
        listed["allowed_base_urls"],
        serde_json::json!(["https://example.test"])
    );
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail = body_json(detail).await;
    assert_eq!(detail["secret"], "spec-identity-secret");
    assert_eq!(detail["kind"], "header");
    assert_eq!(detail["provider_managed"], false);
    assert_eq!(detail["channel_ids"], serde_json::json!([]));
    let mut invalid = input.clone();
    invalid["secret"] = serde_json::Value::Null;
    assert_eq!(
        request(&app, "PUT", &detail_path, invalid, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = input.clone();
    invalid["header_name"] = serde_json::json!("x-forwarded-for");
    assert_eq!(
        request(&app, "POST", path, invalid, &[]).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut invalid = input.clone();
    invalid["allowed_base_urls"] = serde_json::json!(["https://secret@example.test"]);
    assert_eq!(
        request(&app, "POST", path, invalid, &[]).await.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut updated = input;
    updated["secret"] = serde_json::json!("spec-rotated-secret");
    assert_eq!(
        request(
            &app,
            "PUT",
            &detail_path,
            updated.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "PUT", &detail_path, updated, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &detail_path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", &detail_path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let id: Uuid = id.parse().unwrap();
    let tombstone: (Option<String>, bool, bool) = sqlx::query_as(
        "SELECT secret,enabled,deleted_at IS NOT NULL FROM upstream_credentials WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(tombstone, (None, false, true));
    database.cleanup().await;
}

async fn request_with_token(
    app: &App,
    access_token: &str,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {access_token}"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.router.clone().oneshot(request).await.unwrap()
}

async fn unauthenticated_request(
    app: &App,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.router.clone().oneshot(request).await.unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn sharing_contract_is_versioned_scoped_and_never_exposes_provider_identity() {
    use ai_gateway::persistence::CodexCredentialCreate;
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        app.runtime.clone(),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let channel_group = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "sharing-contract", "enabled": true
        }),
    )
    .await;
    let credential = coordinator
        .create_codex_credential(
            app.user_id,
            CodexCredentialCreate {
                channel_group_id: channel_group,
                label: "Private provider label".into(),
                enabled: true,
                proxy_id: None,
                quota_threshold_percent: 95,
                base_url: "https://example.test/codex".into(),
                email: Some("private@example.test".into()),
                account_id: None,
                user_id: Some(Uuid::new_v4().to_string()),
                plan_type: None,
                is_fedramp: false,
                id_token: Uuid::new_v4().to_string(),
                access_token: Uuid::new_v4().to_string(),
                refresh_token: Uuid::new_v4().to_string(),
                access_token_expires_at: None,
                available_models: vec!["sharing-contract-model".into()],
                quota: None,
            },
            None,
        )
        .await
        .unwrap();
    let mut input = serde_json::json!({
        "credential_id": credential.id,
        "name": "Shared development", "enabled": false, "seats": [null, null],
        "primary_limit_amount": "20", "secondary_limit_amount": "100",
        "request_reservation_amount": "0.10", "user_requests_per_minute": 30,
        "group_requests_per_minute": 60, "user_max_concurrent_requests": 1,
        "group_max_concurrent_requests": 2
    });
    let created = request(
        &app,
        "POST",
        "/console/v1/codex-sharing-groups",
        input.clone(),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    assert!(created["correlation_id"].is_string());
    let id = created["id"].as_str().unwrap();
    let path = format!("/console/v1/codex-sharing-groups/{id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let detail = body_json(detail).await;
    assert_eq!(
        detail["primary_limit_amount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::from(20)
    );
    assert_eq!(detail["seats"], serde_json::json!([null, null]));
    assert!(detail.get("provider_user_id").is_none());
    assert!(detail.get("access_token").is_none());
    assert!(
        body_json(
            request(
                &app,
                "GET",
                "/console/v1/me/codex-sharing",
                serde_json::json!({}),
                &[],
            )
            .await,
        )
        .await
        .is_null()
    );
    input["seats"] = serde_json::json!([app.user_id, null]);
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::OK
    );
    let fresh = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = fresh.headers()[header::ETAG].to_str().unwrap().to_owned();
    let own = body_json(
        request(
            &app,
            "GET",
            "/console/v1/me/codex-sharing",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(own["id"], id);
    assert_eq!(own["currency"], "USD");
    assert_eq!(own["usage"]["seat_number"], 1);
    assert_eq!(own["usage"]["available"], false);
    for private in ["credential_id", "seats", "email", "access_token"] {
        assert!(own.get(private).is_none());
    }
    let options = request(
        &app,
        "GET",
        "/console/v1/me/api-key-options",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(options.status(), StatusCode::OK);
    let options = body_json(options).await;
    assert!(options["policy_id"].is_null());
    assert_eq!(options["policy_enabled"], false);
    assert_eq!(options["groups"], serde_json::json!([]));
    assert_eq!(options["channels"], serde_json::json!([]));
    assert_eq!(
        options["sharing_credentials"][0]["credential_id"],
        credential.id.to_string()
    );
    assert_eq!(
        options["sharing_credentials"][0]["name"],
        "Shared development"
    );
    let sharing_channel_ids = options["sharing_credentials"][0]["channel_ids"].clone();
    let sharing_key = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name": "sharing-without-policy",
            "allowed_group_ids": [],
            "allowed_channel_ids": sharing_channel_ids.clone(),
            "requests_per_minute": null,
            "max_concurrent_requests": null,
            "quota_limit_amount": null
        }),
        &[],
    )
    .await;
    assert_eq!(sharing_key.status(), StatusCode::CREATED);
    let policy_id = create_resource(
        &app,
        "/console/v1/api-key-policies",
        serde_json::json!({
            "name": "sharing-must-be-explicit", "enabled": true,
            "allowed_group_ids": [channel_group], "allowed_channel_ids": []
        }),
    )
    .await;
    sqlx::query("UPDATE users SET default_api_key_policy_id=$2 WHERE id=$1")
        .bind(app.user_id)
        .bind(policy_id)
        .execute(&database.pool)
        .await
        .unwrap();
    let categorized = body_json(
        request(
            &app,
            "GET",
            "/console/v1/me/api-key-options",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(categorized["policy_id"], policy_id.to_string());
    assert_eq!(categorized["policy_enabled"], true);
    assert_eq!(categorized["groups"], serde_json::json!([]));
    assert_eq!(categorized["channels"], serde_json::json!([]));
    assert_eq!(
        categorized["sharing_credentials"].as_array().unwrap().len(),
        1
    );
    let implicit_group_key = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name": "implicit-sharing-group",
            "allowed_group_ids": [channel_group],
            "allowed_channel_ids": [],
            "requests_per_minute": null,
            "max_concurrent_requests": null,
            "quota_limit_amount": null
        }),
        &[],
    )
    .await;
    assert_eq!(
        implicit_group_key.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        body_json(implicit_group_key).await,
        serde_json::json!({"error": "api_key_target_not_allowed"})
    );
    let ordinary = seed_test_topology(&app, "chat_completion").await;
    let ordinary_group = ordinary.group;
    let ordinary_channel = ordinary.channel;
    let policy_path = format!("/console/v1/api-key-policies/{policy_id}");
    let policy = request(&app, "GET", &policy_path, serde_json::json!({}), &[]).await;
    let policy_etag = policy.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "PUT",
            &policy_path,
            serde_json::json!({
                "name": "sharing-must-be-explicit", "enabled": true,
                "allowed_group_ids": [channel_group, ordinary_group], "allowed_channel_ids": []
            }),
            &[("if-match", &policy_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let combined_options = body_json(
        request(
            &app,
            "GET",
            "/console/v1/me/api-key-options",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(combined_options["groups"].as_array().unwrap().len(), 1);
    assert_eq!(
        combined_options["groups"][0]["id"],
        ordinary_group.to_string()
    );
    assert_eq!(
        combined_options["channels"][0]["id"],
        ordinary_channel.to_string()
    );
    assert_eq!(
        combined_options["sharing_credentials"][0]["credential_id"],
        credential.id.to_string()
    );
    let combined_key = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name": "sharing-and-ordinary",
            "allowed_group_ids": [ordinary_group],
            "allowed_channel_ids": sharing_channel_ids.clone(),
            "requests_per_minute": null,
            "max_concurrent_requests": null,
            "quota_limit_amount": null
        }),
        &[],
    )
    .await;
    assert_eq!(combined_key.status(), StatusCode::CREATED);
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    input["primary_limit_amount"] = "24".into();
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let fresh = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = fresh.headers()[header::ETAG].to_str().unwrap().to_owned();
    input["enabled"] = true.into();
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    input["enabled"] = false.into();
    input["seats"] = serde_json::json!([app.user_id, app.user_id]);
    assert_eq!(
        request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let outsider = Uuid::new_v4();
    let email = format!("{outsider}@example.test");
    let outside_group = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups (id,name) VALUES ($1,'outside-sharing')")
        .bind(outside_group)
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash,user_group_id) \
        SELECT $1,$2,'Outside sharing','user','active',password_hash,$4 FROM users WHERE id=$3",
    )
    .bind(outsider)
    .bind(&email)
    .bind(app.user_id)
    .bind(outside_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let session = app
        .auth
        .login_with_user_agent(email, TEST_PASSWORD.into(), None)
        .await
        .unwrap();
    assert_eq!(
        request_with_token(
            &app,
            &session.access_token,
            "GET",
            "/console/v1/codex-sharing-groups",
            serde_json::json!({}),
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let forged = format!("/console/v1/me/codex-sharing?user_id={}", app.user_id);
    assert!(
        body_json(
            request_with_token(
                &app,
                &session.access_token,
                "GET",
                &forged,
                serde_json::json!({}),
                &[]
            )
            .await
        )
        .await
        .is_null()
    );
    sqlx::query("UPDATE users SET default_api_key_policy_id=$2 WHERE id=$1")
        .bind(outsider)
        .bind(policy_id)
        .execute(&database.pool)
        .await
        .unwrap();
    let unseated_key = request_with_token(
        &app,
        &session.access_token,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name": "unseated-sharing-key",
            "allowed_group_ids": [],
            "allowed_channel_ids": sharing_channel_ids,
            "requests_per_minute": null,
            "max_concurrent_requests": null,
            "quota_limit_amount": null
        }),
        &[],
    )
    .await;
    assert_eq!(unseated_key.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(unseated_key).await,
        serde_json::json!({"error": "api_key_target_not_allowed"})
    );
    let fresh = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = fresh.headers()[header::ETAG].to_str().unwrap().to_owned();
    input["seats"] = serde_json::json!([app.user_id, outsider]);
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::OK
    );
    let cross_group_own = body_json(
        request_with_token(
            &app,
            &session.access_token,
            "GET",
            "/console/v1/me/codex-sharing",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(cross_group_own["id"], id);
    assert_eq!(cross_group_own["usage"]["seat_number"], 2);
    let own_anonymous = unauthenticated_request(
        &app,
        "GET",
        "/console/v1/me/codex-sharing",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(own_anonymous.status(), StatusCode::UNAUTHORIZED);
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_type='codex_sharing_group' ORDER BY id LIMIT 1",
    ).fetch_one(&database.pool).await.unwrap();
    assert!(audit.get("seats").is_some());
    assert!(audit.get("provider_user_id").is_none());
    database.cleanup().await;
}

async fn upstream_models(headers: HeaderMap) -> Result<Json<serde_json::Value>, StatusCode> {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer spec-model-discovery-secret")
        || headers
            .get("x-model-discovery")
            .and_then(|value| value.to_str().ok())
            != Some("console-spec")
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(serde_json::json!({
        "object": "list",
        "data": [
            {"id": "model-z"},
            {"id": "model-a"},
            {"id": "model-z"}
        ]
    })))
}

/// `/auth/login` matches the spec: `LoginResponse` with token_type "Bearer"
/// and an embedded `ConsoleUser`.
#[tokio::test]
async fn login_response_shape_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let response = request(
        &app,
        "POST",
        "/console/v1/auth/login",
        serde_json::json!({
            "email": format!("spec-user-{}@example.test", app.user_id),
            "password": TEST_PASSWORD,
        }),
        &[("user-agent", "Spec Login Browser/2.0")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["token_type"], "Bearer");
    assert!(body["access_token"].is_string());
    assert!(body["expires_in"].is_number());
    assert_eq!(body["user"]["role"], "admin");
    assert!(body["user"]["id"].is_string());
    assert_eq!(body["user"]["password_change_required"], false);
    assert!(body["user"]["temporary_password_expires_at"].is_null());
    database.cleanup().await;
}

#[tokio::test]
async fn session_management_identifies_clients_and_revokes_selected_scopes() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let email = format!("spec-user-{}@example.test", app.user_id);

    let second_login = request(
        &app,
        "POST",
        "/console/v1/auth/login",
        serde_json::json!({
            "email": email,
            "password": TEST_PASSWORD,
        }),
        &[(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Firefox/128.0",
        )],
    )
    .await;
    assert_eq!(second_login.status(), StatusCode::OK);

    let expired_session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,user_agent,created_at,last_seen_at,expires_at) \
         VALUES ($1,$2,$3,'Expired Browser/1.0',now()-interval '2 days', \
                 now()-interval '2 days',now()-interval '1 day')",
    )
    .bind(expired_session_id)
    .bind(app.user_id)
    .bind(vec![3_u8; 32])
    .execute(&database.pool)
    .await
    .unwrap();
    let revoked_session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,user_agent,created_at,last_seen_at,expires_at,revoked_at) \
         VALUES ($1,$2,$3,'curl/8.7.1 (Linux)',now()-interval '3 days', \
                 now()-interval '3 days',now()+interval '7 days',now()-interval '2 days')",
    )
    .bind(revoked_session_id)
    .bind(app.user_id)
    .bind(vec![4_u8; 32])
    .execute(&database.pool)
    .await
    .unwrap();

    let sessions = request(
        &app,
        "GET",
        "/console/v1/me/sessions",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(sessions.status(), StatusCode::OK);
    let sessions = body_json(sessions).await;
    let sessions = sessions.as_array().unwrap();
    assert_eq!(sessions.len(), 4);
    assert_eq!(sessions[0]["is_current"], true);
    assert_eq!(sessions[0]["state"], "active");
    assert_eq!(sessions[0]["user_agent"], "Spec Browser/1.0");
    assert!(sessions[0]["last_seen_at"].is_string());
    let current_session_id = Uuid::parse_str(sessions[0]["id"].as_str().unwrap()).unwrap();
    let other_active = sessions
        .iter()
        .find(|session| {
            session["user_agent"]
                .as_str()
                .is_some_and(|value| value.contains("Firefox/128.0"))
        })
        .unwrap();
    assert_eq!(other_active["state"], "active");
    assert_eq!(other_active["is_current"], false);
    let other_active_id = Uuid::parse_str(other_active["id"].as_str().unwrap()).unwrap();
    let expired = sessions
        .iter()
        .find(|session| session["id"] == expired_session_id.to_string())
        .unwrap();
    assert_eq!(expired["state"], "expired");
    let revoked = sessions
        .iter()
        .find(|session| session["id"] == revoked_session_id.to_string())
        .unwrap();
    assert_eq!(revoked["state"], "revoked");

    let other_user_id = Uuid::new_v4();
    let other_user_session_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status) \
         VALUES ($1,$2,'Other session owner','user','active')",
    )
    .bind(other_user_id)
    .bind(format!("session-owner-{other_user_id}@example.test"))
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions (id,user_id,refresh_token_hash,expires_at) \
         VALUES ($1,$2,$3,now()+interval '1 day')",
    )
    .bind(other_user_session_id)
    .bind(other_user_id)
    .bind(vec![5_u8; 32])
    .execute(&database.pool)
    .await
    .unwrap();
    let cross_user_revoke = request(
        &app,
        "DELETE",
        &format!("/console/v1/me/sessions/{other_user_session_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(cross_user_revoke.status(), StatusCode::NOT_FOUND);

    let revoke_others = request(
        &app,
        "DELETE",
        "/console/v1/me/sessions",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(revoke_others.status(), StatusCode::NO_CONTENT);
    let revocation_state: (bool, bool) = sqlx::query_as(
        "SELECT \
           (SELECT revoked_at IS NOT NULL FROM user_sessions WHERE id=$1), \
           (SELECT revoked_at IS NOT NULL FROM user_sessions WHERE id=$2)",
    )
    .bind(current_session_id)
    .bind(other_active_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!revocation_state.0);
    assert!(revocation_state.1);
    let other_user_revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM user_sessions WHERE id=$1")
            .bind(other_user_session_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(!other_user_revoked);

    let revoke_current = request(
        &app,
        "DELETE",
        &format!("/console/v1/me/sessions/{current_session_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(revoke_current.status(), StatusCode::NO_CONTENT);
    assert!(
        revoke_current.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let rejected = request(&app, "GET", "/console/v1/me", serde_json::json!({}), &[]).await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

    database.cleanup().await;
}

#[tokio::test]
async fn reusable_invitation_code_registers_an_active_user_and_enforces_usage_limit() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let invitation_code = "TEAM-ACCESS-2026";
    let created = request(
        &app,
        "POST",
        "/console/v1/registration-invitation-codes",
        serde_json::json!({
            "name": "One seat",
            "invitation_code": invitation_code,
            "max_uses": 1,
            "expires_at": "2030-01-01T00:00:00Z",
            "enabled": true,
            "user_group_id": DEFAULT_USER_GROUP_ID,
            "initial_balance_amount": "25.50",
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    assert_eq!(created["invitation_code"], invitation_code);
    let code_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();

    let listed = request(
        &app,
        "GET",
        "/console/v1/registration-invitation-codes",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = body_json(listed).await;
    let listed_code = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|code| code["id"] == code_id.to_string())
        .unwrap();
    assert!(listed_code.get("invitation_code").is_none());
    assert!(listed_code.get("code_hash").is_none());
    assert_eq!(listed_code["used_count"], 0);
    assert_eq!(listed_code["max_uses"], 1);

    let email = format!("self-register-{code_id}@example.test");
    let registered = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/register",
        serde_json::json!({
            "invitation_code": invitation_code,
            "email": email,
            "display_name": "Self Registered",
            "password": TEST_PASSWORD,
        }),
    )
    .await;
    assert_eq!(registered.status(), StatusCode::OK);
    assert!(registered.headers().contains_key(header::SET_COOKIE));
    let registered = body_json(registered).await;
    assert_eq!(registered["token_type"], "Bearer");
    assert_eq!(registered["user"]["role"], "user");
    assert_eq!(registered["user"]["email"], email);

    let account: (String, String, rust_decimal::Decimal, Uuid) = sqlx::query_as(
        "SELECT role,status,balance_amount,user_group_id FROM users WHERE lower(email)=lower($1)",
    )
    .bind(&email)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(account.0, "user");
    assert_eq!(account.1, "active");
    assert_eq!(account.2, rust_decimal::Decimal::new(2_550, 2));
    assert_eq!(account.3, DEFAULT_USER_GROUP_ID);

    let usage: (i64, Option<chrono::DateTime<chrono::Utc>>) = sqlx::query_as(
        "SELECT used_count,last_used_at FROM registration_invitation_codes WHERE id=$1",
    )
    .bind(code_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(usage.0, 1);
    assert!(usage.1.is_some());

    let exhausted = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/register",
        serde_json::json!({
            "invitation_code": invitation_code,
            "email": format!("second-{code_id}@example.test"),
            "display_name": "Second User",
            "password": TEST_PASSWORD,
        }),
    )
    .await;
    assert_eq!(exhausted.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(exhausted).await["error"],
        "invalid_registration_code"
    );

    let leaked: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
           SELECT 1 FROM audit_logs \
           WHERE before_redacted::text LIKE '%' || $1 || '%' \
              OR after_redacted::text LIKE '%' || $1 || '%' \
         )",
    )
    .bind(invitation_code)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!leaked);
    database.cleanup().await;
}

#[tokio::test]
async fn registration_invitation_code_settings_are_versioned_and_adjustable() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let target_group_id = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups (id,name,description) VALUES ($1,$2,$3)")
        .bind(target_group_id)
        .bind(format!("registration-group-{target_group_id}"))
        .bind("Group selected by a reusable registration code")
        .execute(&database.pool)
        .await
        .unwrap();

    let invitation_code = "ADJUSTABLE-ACCESS-2026";
    let created = request(
        &app,
        "POST",
        "/console/v1/registration-invitation-codes",
        serde_json::json!({
            "name": "Adjustable",
            "invitation_code": invitation_code,
            "max_uses": null,
            "expires_at": null,
            "enabled": true,
            "user_group_id": DEFAULT_USER_GROUP_ID,
            "initial_balance_amount": "0",
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let code_id = body_json(created).await["id"].as_str().unwrap().to_owned();
    let path = format!("/console/v1/registration-invitation-codes/{code_id}");

    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let detail_body = body_json(detail).await;
    assert!(detail_body.get("invitation_code").is_none());

    let adjusted = request(
        &app,
        "PUT",
        &path,
        serde_json::json!({
            "name": "Adjusted",
            "max_uses": 3,
            "expires_at": "2031-06-01T12:00:00Z",
            "enabled": false,
            "user_group_id": target_group_id,
            "initial_balance_amount": "75.25",
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(adjusted.status(), StatusCode::OK);

    let stale = request(
        &app,
        "PUT",
        &path,
        serde_json::json!({
            "name": "Stale",
            "max_uses": null,
            "expires_at": null,
            "enabled": true,
            "user_group_id": DEFAULT_USER_GROUP_ID,
            "initial_balance_amount": "0",
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let fresh_etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let detail = body_json(detail).await;
    assert_eq!(detail["name"], "Adjusted");
    assert_eq!(detail["max_uses"], 3);
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["user_group_id"], target_group_id.to_string());
    assert_eq!(
        detail["initial_balance_amount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::new(7_525, 2)
    );

    let disabled = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/register",
        serde_json::json!({
            "invitation_code": invitation_code,
            "email": format!("disabled-{code_id}@example.test"),
            "display_name": "Disabled Code",
            "password": TEST_PASSWORD,
        }),
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let enabled = request(
        &app,
        "PUT",
        &path,
        serde_json::json!({
            "name": "Adjusted",
            "max_uses": 3,
            "expires_at": "2031-06-01T12:00:00Z",
            "enabled": true,
            "user_group_id": target_group_id,
            "initial_balance_amount": "75.25",
        }),
        &[("if-match", &fresh_etag)],
    )
    .await;
    assert_eq!(enabled.status(), StatusCode::OK);

    let email = format!("adjusted-{code_id}@example.test");
    let registered = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/register",
        serde_json::json!({
            "invitation_code": invitation_code,
            "email": email,
            "display_name": "Adjusted User",
            "password": TEST_PASSWORD,
        }),
    )
    .await;
    assert_eq!(registered.status(), StatusCode::OK);
    let assignment: (Uuid, rust_decimal::Decimal) =
        sqlx::query_as("SELECT user_group_id,balance_amount FROM users WHERE email=$1")
            .bind(&email)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(assignment.0, target_group_id);
    assert_eq!(assignment.1, rust_decimal::Decimal::new(7_525, 2));

    let duplicate = unauthenticated_request(
        &app,
        "POST",
        "/console/v1/auth/register",
        serde_json::json!({
            "invitation_code": invitation_code,
            "email": email,
            "display_name": "Duplicate Email",
            "password": TEST_PASSWORD,
        }),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(duplicate).await["error"],
        "registration_email_conflict"
    );
    let used_count: i64 =
        sqlx::query_scalar("SELECT used_count FROM registration_invitation_codes WHERE id=$1")
            .bind(Uuid::parse_str(&code_id).unwrap())
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(used_count, 1);
    database.cleanup().await;
}

#[tokio::test]
async fn user_group_delete_reassigns_members_and_disables_registration_codes() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group_name = format!("soft-delete-group-{}", Uuid::new_v4());
    let created = request(
        &app,
        "POST",
        "/console/v1/user-groups",
        serde_json::json!({
            "name": group_name,
            "description": "temporary assignment",
            "default_api_key_policy_id": null,
            "visible_codex_quota_group_ids": [],
            "filter_fast_mode": true,
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let group_id = Uuid::parse_str(body_json(created).await["id"].as_str().unwrap()).unwrap();

    let member_user = Uuid::new_v4();
    let member_admin = Uuid::new_v4();
    for (id, role) in [(member_user, "user"), (member_admin, "admin")] {
        sqlx::query(
            "INSERT INTO users \
             (id,email,display_name,role,status,user_group_id) \
             VALUES ($1,$2,$3,$4,'active',$5)",
        )
        .bind(id)
        .bind(format!("{role}-{id}@example.test"))
        .bind(format!("{role}-{id}"))
        .bind(role)
        .bind(group_id)
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let code = request(
        &app,
        "POST",
        "/console/v1/registration-invitation-codes",
        serde_json::json!({
            "name": format!("soft-delete-code-{group_id}"),
            "invitation_code": format!("SOFT-DELETE-{}", group_id.simple()),
            "max_uses": null,
            "expires_at": null,
            "enabled": true,
            "user_group_id": group_id,
            "initial_balance_amount": "0",
        }),
        &[],
    )
    .await;
    assert_eq!(code.status(), StatusCode::CREATED);
    let code_id = Uuid::parse_str(body_json(code).await["id"].as_str().unwrap()).unwrap();

    let path = format!("/console/v1/user-groups/{group_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(body_json(detail).await["member_count"], 2);

    let deleted = request(
        &app,
        "DELETE",
        &path,
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);

    let tombstone: (Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) =
        sqlx::query_as("SELECT deleted_at,deleted_by FROM user_groups WHERE id=$1")
            .bind(group_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(tombstone.0.is_some());
    assert_eq!(tombstone.1, Some(app.user_id));

    let assignments: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT id,user_group_id FROM users WHERE id=ANY($1) ORDER BY id")
            .bind(vec![member_user, member_admin])
            .fetch_all(&database.pool)
            .await
            .unwrap();
    for (id, assigned_group) in assignments {
        assert_eq!(
            assigned_group,
            if id == member_admin {
                ai_gateway::persistence::DEFAULT_ADMIN_GROUP_ID
            } else {
                DEFAULT_USER_GROUP_ID
            }
        );
    }
    let code_enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM registration_invitation_codes WHERE id=$1")
            .bind(code_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(!code_enabled);
    assert_eq!(
        request(&app, "GET", &path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let recreated = request(
        &app,
        "POST",
        "/console/v1/user-groups",
        serde_json::json!({
            "name": group_name,
            "description": null,
            "default_api_key_policy_id": null,
            "visible_codex_quota_group_ids": [],
            "filter_fast_mode": false,
        }),
        &[],
    )
    .await;
    assert_eq!(recreated.status(), StatusCode::CREATED);
    assert_ne!(
        body_json(recreated).await["id"],
        serde_json::Value::String(group_id.to_string())
    );

    database.cleanup().await;
}

/// Unauthenticated request to a protected endpoint returns the spec
/// `ErrorBody` shape `{"error": ...}`.
#[tokio::test]
async fn unauthorized_error_body_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/v1/me")
        .body(Body::empty())
        .unwrap();
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(response).await;
    assert!(body.get("error").is_some(), "error body must have 'error'");
    database.cleanup().await;
}

/// `GET /me` returns the `ConsoleProfile` shape with its USD balance encoded
/// as a decimal string and no per-user currency setting.
#[tokio::test]
async fn profile_shape_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let response = request(&app, "GET", "/console/v1/me", serde_json::json!({}), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["id"], app.user_id.to_string());
    assert!(body["balance_amount"].is_string(), "decimal is a string");
    assert!(body.get("currency").is_none());
    assert_eq!(body["role"], "admin");
    database.cleanup().await;
}

#[tokio::test]
async fn personal_websocket_setting_is_published_to_owned_api_keys() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let secret = format!("sk-user-settings-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
          allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,$3,$4,'active',
                 ARRAY['open_ai_responses']::api_format[],
                 ARRAY['proxy']::text[],'{}'::uuid[],'{}'::uuid[])",
    )
    .bind(Uuid::new_v4())
    .bind(app.user_id)
    .bind(format!("user-settings-{}", Uuid::new_v4()))
    .bind(&secret)
    .execute(&database.pool)
    .await
    .unwrap();

    let current = request(
        &app,
        "GET",
        "/console/v1/me/settings",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(body_json(current).await["websocket_enabled"], false);

    let updated = request(
        &app,
        "PUT",
        "/console/v1/me/settings",
        serde_json::json!({"websocket_enabled": true}),
        &[],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(body_json(updated).await["websocket_enabled"], true);

    let compiled = app
        .runtime
        .snapshot()
        .authenticate(&secret)
        .expect("newly loaded API key");
    assert!(compiled.websocket_enabled());

    database.cleanup().await;
}

#[tokio::test]
async fn administrator_can_manage_a_users_personal_websocket_setting() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let managed_user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status) \
         VALUES ($1,$2,'Managed user','user','active')",
    )
    .bind(managed_user_id)
    .bind(format!("managed-settings-{managed_user_id}@example.test"))
    .execute(&database.pool)
    .await
    .unwrap();
    let secret = format!("sk-admin-user-settings-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
          allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,$3,$4,'active',
                 ARRAY['open_ai_responses']::api_format[],
                 ARRAY['proxy']::text[],'{}'::uuid[],'{}'::uuid[])",
    )
    .bind(Uuid::new_v4())
    .bind(managed_user_id)
    .bind(format!("admin-user-settings-{}", Uuid::new_v4()))
    .bind(&secret)
    .execute(&database.pool)
    .await
    .unwrap();

    let path = format!("/console/v1/users/{managed_user_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(body_json(detail).await["websocket_enabled"], false);

    let updated = request(
        &app,
        "PATCH",
        &path,
        serde_json::json!({"websocket_enabled": true}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);

    let compiled = app
        .runtime
        .snapshot()
        .authenticate(&secret)
        .expect("newly loaded API key");
    assert!(compiled.websocket_enabled());
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs \
         WHERE object_type='user' AND object_id=$1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(managed_user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["websocket_enabled"], true);

    database.cleanup().await;
}

/// Currency is a system-wide USD invariant rather than a mutable Console
/// field, so legacy currency properties are rejected by request decoding.
#[tokio::test]
async fn currency_fields_are_not_console_settings() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let invite = request(
        &app,
        "POST",
        "/console/v1/users",
        serde_json::json!({
            "email": format!("currency-field-{}@example.test", Uuid::new_v4()),
            "display_name": "Currency field",
            "role": "user",
            "currency": "USD"
        }),
        &[],
    )
    .await;
    assert_eq!(invite.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let model = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": format!("currency-field-{}", Uuid::new_v4()),
            "display_name": "Currency field",
            "enabled": true,
            "currency": "USD",
            "price_unit_tokens": 1000000,
            "input_unit_price": "0",
            "cached_input_unit_price": "0",
            "cache_write_unit_price": "0",
            "output_unit_price": "0",
            "price_effective_at": chrono::Utc::now().to_rfc3339()
        }),
        &[],
    )
    .await;
    assert_eq!(model.status(), StatusCode::UNPROCESSABLE_ENTITY);

    database.cleanup().await;
}

#[tokio::test]
async fn removed_console_compatibility_paths_are_not_routed() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    for (method, path) in [
        ("GET", "/console/v1/channel-groups"),
        ("GET", "/console/v1/channels"),
        ("GET", "/console/v1/model-rules"),
        ("GET", "/console/v1/proxies"),
        ("GET", "/console/v1/config-templates"),
        ("POST", "/console/v1/models/sync/preview"),
        ("POST", "/console/v1/models/sync/import"),
        ("POST", "/console/v1/reload"),
        ("GET", "/console/v1/statistics/channel-status"),
    ] {
        let response = request(&app, method, path, serde_json::json!({}), &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
    }

    database.cleanup().await;
}

#[tokio::test]
async fn legacy_user_name_field_is_rejected() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = format!("/console/v1/users/{}", app.user_id);
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let mut update = body_json(detail).await;
    let display_name = update["display_name"].clone();
    for field in ["id", "created_at", "updated_at", "display_name"] {
        update.as_object_mut().unwrap().remove(field);
    }
    update["name"] = display_name;

    let response = request(&app, "PUT", &path, update, &[("if-match", &etag)]).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

#[tokio::test]
async fn obsolete_control_plane_columns_are_absent() {
    let database = TestDatabase::new().await;

    for (table, column) in [
        ("api_keys", "tokens_per_minute"),
        ("channels", "health_check"),
        ("channels", "status_statistics_enabled"),
    ] {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (\
             SELECT 1 FROM information_schema.columns \
             WHERE table_schema='public' AND table_name=$1 AND column_name=$2\
             )",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&database.pool)
        .await
        .unwrap();
        assert!(!exists, "{table}.{column} must be removed");
    }

    let capability_status_monitoring_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (\
         SELECT 1 FROM information_schema.columns \
         WHERE table_schema='public' \
           AND table_name='channel_capabilities' \
           AND column_name='status_statistics_enabled'\
         )",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(capability_status_monitoring_exists);
    for table in [
        "channel_groups",
        "channels",
        "model_rules",
        "model_rule_routing_tiers",
        "model_rule_routing_candidates",
        "codex_oauth_credential_channels",
    ] {
        let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&database.pool)
            .await
            .unwrap();
        assert!(!exists, "{table} must be retired");
    }

    database.cleanup().await;
}

/// Non-auth account edits preserve the current session, while role changes
/// still invalidate it immediately.
#[tokio::test]
async fn user_updates_only_revoke_sessions_for_auth_identity_changes() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = format!("/console/v1/users/{}", app.user_id);

    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let display_name_update = request(
        &app,
        "PATCH",
        &path,
        serde_json::json!({"display_name": "Updated display name"}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(display_name_update.status(), StatusCode::OK);
    let profile = request(&app, "GET", "/console/v1/me", serde_json::json!({}), &[]).await;
    assert_eq!(profile.status(), StatusCode::OK);
    assert_eq!(
        body_json(profile).await["display_name"],
        "Updated display name"
    );

    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let role_update = request(
        &app,
        "PATCH",
        &path,
        serde_json::json!({"role": "user"}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(role_update.status(), StatusCode::OK);
    let invalidated = request(&app, "GET", "/console/v1/me", serde_json::json!({}), &[]).await;
    assert_eq!(invalidated.status(), StatusCode::UNAUTHORIZED);
    database.cleanup().await;
}

/// Administrators can set an account's balance through the versioned user
/// resource, and the change is immediately visible in the user's profile.
#[tokio::test]
async fn administrator_can_manage_user_balance() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = format!("/console/v1/users/{}", app.user_id);
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let response = request(
        &app,
        "PATCH",
        &path,
        serde_json::json!({"balance_amount": "42.75"}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let balance: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT balance_amount FROM users WHERE id=$1")
            .bind(app.user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(balance, rust_decimal::Decimal::new(4_275, 2));
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs \
         WHERE object_type='user' AND object_id=$1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(app.user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["balance_amount"].as_f64(), Some(42.75));
    database.cleanup().await;
}

/// Built-in role groups are present after migration, newly invited users enter
/// the matching group, and the group's policy is inherited dynamically.
#[tokio::test]
async fn user_groups_supply_role_defaults_and_inherited_api_policy() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let policy_id = Uuid::new_v4();
    let override_policy_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_key_policies \
         (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
         VALUES ($1,$2,'{}','{}',true)",
    )
    .bind(policy_id)
    .bind(format!("group-policy-{policy_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_key_policies \
         (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
         VALUES ($1,$2,'{}','{}',true)",
    )
    .bind(override_policy_id)
    .bind(format!("override-policy-{override_policy_id}"))
    .execute(&database.pool)
    .await
    .unwrap();

    let groups = request(
        &app,
        "GET",
        "/console/v1/user-groups",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(groups.status(), StatusCode::OK);
    let groups = body_json(groups).await;
    let default_user_group = groups
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["system_role"] == "user")
        .unwrap();
    assert_eq!(default_user_group["id"], DEFAULT_USER_GROUP_ID.to_string());
    assert!(
        groups
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["system_role"] == "admin")
    );

    let group_path = format!("/console/v1/user-groups/{DEFAULT_USER_GROUP_ID}");
    let detail = request(&app, "GET", &group_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let group = body_json(detail).await;
    let update = request(
        &app,
        "PUT",
        &group_path,
        serde_json::json!({
            "name": group["name"],
            "description": group["description"],
            "default_api_key_policy_id": policy_id,
            "visible_codex_quota_group_ids": group["visible_codex_quota_group_ids"],
            "filter_fast_mode": true,
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let compiled: bool = sqlx::query_scalar("SELECT filter_fast_mode FROM user_groups WHERE id=$1")
        .bind(DEFAULT_USER_GROUP_ID)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(compiled);

    let email = format!("group-invite-{policy_id}@example.test");
    let invite = request(
        &app,
        "POST",
        "/console/v1/users",
        serde_json::json!({
            "email": email,
            "display_name": "Inherited policy user",
            "role": "user",
            "initial_balance_amount": "0",
            "default_api_key_policy_id": null,
        }),
        &[],
    )
    .await;
    assert_eq!(invite.status(), StatusCode::CREATED);
    let invited_user_id =
        Uuid::parse_str(body_json(invite).await["user_id"].as_str().unwrap()).unwrap();
    let assignment: (Uuid, Option<Uuid>) =
        sqlx::query_as("SELECT user_group_id,default_api_key_policy_id FROM users WHERE id=$1")
            .bind(invited_user_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(assignment, (DEFAULT_USER_GROUP_ID, None));

    sqlx::query("UPDATE users SET status='active' WHERE id=$1")
        .bind(invited_user_id)
        .execute(&database.pool)
        .await
        .unwrap();
    let inherited_secret = format!("sk-fast-filter-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions,
          allowed_group_ids,allowed_channel_ids)
         VALUES ($1,$2,$3,$4,'active',
                 ARRAY['open_ai_responses']::api_format[],
                 ARRAY['proxy']::text[],'{}'::uuid[],'{}'::uuid[])",
    )
    .bind(Uuid::new_v4())
    .bind(invited_user_id)
    .bind(format!("fast-filter-{}", Uuid::new_v4()))
    .bind(&inherited_secret)
    .execute(&database.pool)
    .await
    .unwrap();
    let runtime = compile_runtime_config(
        ControlPlaneRepository::new(database.pool.clone())
            .load_runtime()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        runtime
            .authenticate(&inherited_secret)
            .expect("group member API key compiles")
            .filter_fast_mode()
    );
    let options = ControlPlaneRepository::new(database.pool.clone())
        .own_api_key_options(invited_user_id)
        .await
        .unwrap();
    assert_eq!(options.policy_id, Some(policy_id));

    let user_path = format!("/console/v1/users/{invited_user_id}");
    let user_detail = request(&app, "GET", &user_path, serde_json::json!({}), &[]).await;
    let user_etag = user_detail.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let override_update = request(
        &app,
        "PATCH",
        &user_path,
        serde_json::json!({
            "default_api_key_policy_id": override_policy_id
        }),
        &[("if-match", &user_etag)],
    )
    .await;
    assert_eq!(override_update.status(), StatusCode::OK);
    let overridden = ControlPlaneRepository::new(database.pool.clone())
        .own_api_key_options(invited_user_id)
        .await
        .unwrap();
    assert_eq!(overridden.policy_id, Some(override_policy_id));

    let user_detail = request(&app, "GET", &user_path, serde_json::json!({}), &[]).await;
    let user_etag = user_detail.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let inherit_update = request(
        &app,
        "PATCH",
        &user_path,
        serde_json::json!({"default_api_key_policy_id": null}),
        &[("if-match", &user_etag)],
    )
    .await;
    assert_eq!(inherit_update.status(), StatusCode::OK);
    let inherited_again = ControlPlaneRepository::new(database.pool.clone())
        .own_api_key_options(invited_user_id)
        .await
        .unwrap();
    assert_eq!(inherited_again.policy_id, Some(policy_id));

    let default_group = request(&app, "GET", &group_path, serde_json::json!({}), &[]).await;
    let default_group_etag = default_group.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let protected = request(
        &app,
        "DELETE",
        &group_path,
        serde_json::json!({}),
        &[("if-match", &default_group_etag)],
    )
    .await;
    assert_eq!(protected.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(protected).await,
        serde_json::json!({"error": "protected_user_group"})
    );
    database.cleanup().await;
}

/// Administrators grant quota visibility through user groups, while the
/// owner-scoped API exposes only credential IDs, subscription tiers, and
/// quota-window data.
#[tokio::test]
async fn user_group_codex_quota_visibility_is_scoped_sanitized_and_read_only() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let user_group_id = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups (id,name) VALUES ($1,$2)")
        .bind(user_group_id)
        .bind(format!("quota-viewers-{user_group_id}"))
        .execute(&database.pool)
        .await
        .unwrap();
    let viewer_id = Uuid::new_v4();
    let viewer_email = format!("quota-viewer-{viewer_id}@example.test");
    let viewer_password = "quota-viewer-password-value";
    let viewer_password_hash = hash_console_password(viewer_password.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users \
         (id,email,display_name,role,status,password_hash,user_group_id) \
         VALUES ($1,$2,'Quota Viewer','user','active',$3,$4)",
    )
    .bind(viewer_id)
    .bind(&viewer_email)
    .bind(viewer_password_hash)
    .bind(user_group_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let viewer_session = app
        .auth
        .login(viewer_email, viewer_password.to_owned())
        .await
        .unwrap();

    let ordinary_group_id = seed_test_topology(&app, "responses").await.group;
    let visible_group_id = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "Visible quota group", "enabled": true
        }),
    )
    .await;
    let hidden_group_id = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "Hidden quota group", "enabled": true
        }),
    )
    .await;
    let mut credential_ids = Vec::new();
    for (group_id, label, plan_type, used) in [
        (visible_group_id, "Visible private label", "plus", 42),
        (hidden_group_id, "Hidden private label", "business", 81),
    ] {
        let mut input = codex_fixture_input(group_id, label);
        input.plan_type = Some(plan_type.into());
        input.id_token = "private-id-token".into();
        input.access_token = "private-access-token".into();
        input.refresh_token = "private-refresh-token".into();
        let id = create_test_codex_credential(&database.pool, &app, input).await;
        sqlx::query(
            "UPDATE codex_oauth_credentials SET runtime_status='active',quota_allowed=true,
            quota_limit_reached=false,primary_used_percent=$2,primary_window_seconds=10800,
            primary_reset_at='2026-08-03T15:00:00Z',secondary_used_percent=12,
            secondary_window_seconds=604800,secondary_reset_at='2026-08-10T12:00:00Z',
            quota_reset_credits_available=3,quota_checked_at='2026-08-03T12:00:00Z'
            WHERE channel_id=$1",
        )
        .bind(id)
        .bind(used)
        .execute(&database.pool)
        .await
        .unwrap();
        credential_ids.push(id);
    }
    let visible_credential_id = credential_ids[0];
    let hidden_credential_id = credential_ids[1];
    let period_started_at = chrono::DateTime::parse_from_rfc3339("2026-08-03T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          ended_at,reset_reason,initial_used_percent,last_used_percent, \
          first_observed_at,last_observed_at) \
         VALUES ($1,$2,'primary',10800,$3,$4,NULL,NULL,5,42,$3,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(visible_credential_id)
    .bind(period_started_at)
    .bind(period_started_at + chrono::Duration::hours(3))
    .bind(period_started_at + chrono::Duration::hours(2))
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          ended_at,reset_reason,initial_used_percent,last_used_percent, \
          first_observed_at,last_observed_at) \
         VALUES ($1,$2,'secondary',604800,$3,$4,NULL,NULL,2,12,$3,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(visible_credential_id)
    .bind(period_started_at - chrono::Duration::days(2))
    .bind(period_started_at + chrono::Duration::days(5))
    .bind(period_started_at + chrono::Duration::hours(2))
    .execute(&database.pool)
    .await
    .unwrap();
    let visible_images_channel_id =
        codex_capability_id(&database.pool, visible_credential_id, "images_generation").await;
    for (request_user_id, api_format, api_operation, channel_group_id, channel_id, logs) in [
        (
            viewer_id,
            "open_ai_responses",
            "responses",
            visible_group_id,
            visible_credential_id,
            vec![
                (period_started_at, rust_decimal::Decimal::new(125, 2)),
                (
                    period_started_at + chrono::Duration::hours(3),
                    rust_decimal::Decimal::from(5),
                ),
                (
                    period_started_at + chrono::Duration::days(5),
                    rust_decimal::Decimal::from(10),
                ),
            ],
        ),
        (
            app.user_id,
            "open_ai_images",
            "images_generation",
            visible_group_id,
            visible_images_channel_id,
            vec![(
                period_started_at + chrono::Duration::hours(1),
                rust_decimal::Decimal::new(75, 2),
            )],
        ),
    ] {
        let request_api_key_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO api_keys \
             (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
             VALUES ($1,$2,$3,$4,'active',ARRAY[$5::api_format],ARRAY['proxy'])",
        )
        .bind(request_api_key_id)
        .bind(request_user_id)
        .bind(format!("quota-cost-key-{request_api_key_id}"))
        .bind(format!("quota-cost-secret-{request_api_key_id}"))
        .bind(api_format)
        .execute(&database.pool)
        .await
        .unwrap();
        for (request_started_at, cost) in logs {
            sqlx::query(
                "INSERT INTO request_logs \
                 (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation, \
                  client_model,channel_group_id,channel_id,outcome,response_status_code, \
                  cost_amount) \
                 VALUES ($1,$2,$2,$3,$4,$5::api_format,$6,'quota-cost-model',$7,$8, \
                         'succeeded',200,$9)",
            )
            .bind(Uuid::new_v4())
            .bind(request_started_at)
            .bind(request_user_id)
            .bind(request_api_key_id)
            .bind(api_format)
            .bind(api_operation)
            .bind(channel_group_id)
            .bind(channel_id)
            .bind(cost)
            .execute(&database.pool)
            .await
            .unwrap();
        }
    }
    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let legacy_zero_started_at = period_started_at - chrono::Duration::minutes(30);
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          ended_at,reset_reason,initial_used_percent,last_used_percent, \
          first_observed_at,last_observed_at) \
         VALUES ($1,$2,'primary',10800,$3,$4,$5,'openai_official',0,0,$3,$5)",
    )
    .bind(Uuid::new_v4())
    .bind(visible_credential_id)
    .bind(legacy_zero_started_at)
    .bind(legacy_zero_started_at + chrono::Duration::hours(3))
    .bind(period_started_at - chrono::Duration::minutes(15))
    .execute(&database.pool)
    .await
    .unwrap();

    let group_path = format!("/console/v1/user-groups/{user_group_id}");
    let detail = request(&app, "GET", &group_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let group_etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let group = body_json(detail).await;
    assert_eq!(
        group["visible_codex_quota_group_ids"],
        serde_json::json!([])
    );

    for invalid_group_id in [ordinary_group_id, visible_images_channel_id] {
        let invalid = request(
            &app,
            "PUT",
            &group_path,
            serde_json::json!({
                "name": group["name"],
                "description": group["description"],
                "default_api_key_policy_id": null,
                "visible_codex_quota_group_ids": [invalid_group_id],
                "filter_fast_mode": group["filter_fast_mode"],
            }),
            &[("if-match", &group_etag)],
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    let denied_by_default = request_with_token(
        &app,
        &viewer_session.access_token,
        "GET",
        "/console/v1/me/codex-quotas",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(denied_by_default.status(), StatusCode::OK);
    assert_eq!(body_json(denied_by_default).await, serde_json::json!([]));

    let update = request(
        &app,
        "PUT",
        &group_path,
        serde_json::json!({
            "name": group["name"],
            "description": group["description"],
            "default_api_key_policy_id": null,
            "visible_codex_quota_group_ids": [visible_group_id],
            "filter_fast_mode": group["filter_fast_mode"],
        }),
        &[("if-match", &group_etag)],
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs \
         WHERE object_type='user_group' AND object_id=$1 \
         ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(user_group_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        audit["visible_codex_quota_group_ids"],
        serde_json::json!([visible_group_id])
    );

    let quotas = request_with_token(
        &app,
        &viewer_session.access_token,
        "GET",
        "/console/v1/me/codex-quotas",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(quotas.status(), StatusCode::OK);
    let quotas = body_json(quotas).await;
    let quotas = quotas.as_array().unwrap();
    assert_eq!(quotas.len(), 1);
    let quota = quotas[0].as_object().unwrap();
    assert_eq!(quota.len(), 13);
    assert_eq!(quota["id"], visible_credential_id.to_string());
    assert_eq!(quota["name"], visible_credential_id.to_string());
    assert_eq!(quota["channel_group_id"], visible_group_id.to_string());
    assert_eq!(quota["plan_type"], "plus");
    assert_eq!(quota["primary_used_percent"], 42);
    assert_eq!(quota["primary_window_cost_amount"], "2.00000000");
    assert_eq!(quota["secondary_used_percent"], 12);
    assert_eq!(quota["secondary_window_cost_amount"], "7.00000000");
    assert_eq!(quota["quota_checked_at"], "2026-08-03T12:00:00Z");
    for forbidden in [
        "label",
        "email",
        "account_id",
        "user_id",
        "runtime_status",
        "quota_reset_credits_available",
        "last_error_code",
        "proxy_id",
        "weight",
        "enabled",
    ] {
        assert!(!quota.contains_key(forbidden));
    }

    let history = request_with_token(
        &app,
        &viewer_session.access_token,
        "GET",
        &format!("/console/v1/me/codex-quotas/{visible_credential_id}/windows?limit=10"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history = body_json(history).await;
    assert_eq!(history["credential_id"], visible_credential_id.to_string());
    assert_eq!(history["name"], visible_credential_id.to_string());
    assert_eq!(history["channel_group_id"], visible_group_id.to_string());
    assert_eq!(history["plan_type"], "plus");
    let periods = history["periods"].as_array().unwrap();
    assert_eq!(periods.len(), 2);
    for period in periods {
        let period = period.as_object().unwrap();
        assert_eq!(period.len(), 11);
        assert!(!period.contains_key("id"));
        assert!(!period.contains_key("credential_id"));
    }
    assert_eq!(periods[0]["window_kind"], "primary");
    assert_eq!(periods[0]["last_used_percent"], 42);
    assert_eq!(periods[0]["cost_amount"], "2.00000000");
    assert_eq!(periods[1]["window_kind"], "secondary");
    assert_eq!(periods[1]["last_used_percent"], 12);
    assert_eq!(periods[1]["cost_amount"], "7.00000000");

    let hidden_history = request_with_token(
        &app,
        &viewer_session.access_token,
        "GET",
        &format!("/console/v1/me/codex-quotas/{hidden_credential_id}/windows"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(hidden_history.status(), StatusCode::NOT_FOUND);

    let write_attempt = request_with_token(
        &app,
        &viewer_session.access_token,
        "POST",
        "/console/v1/me/codex-quotas",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(write_attempt.status(), StatusCode::METHOD_NOT_ALLOWED);

    let admin_history = request_with_token(
        &app,
        &viewer_session.access_token,
        "GET",
        &format!(
            "/console/v1/providers/codex-oauth/credentials/{visible_credential_id}/quota/windows"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_history.status(), StatusCode::FORBIDDEN);

    database.cleanup().await;
}

/// Batch user updates are all-or-nothing, versioned, and support status,
/// balance adjustment, policy override, and group assignment together.
#[tokio::test]
async fn user_batch_updates_are_atomic_and_cover_supported_fields() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group_id = Uuid::new_v4();
    let policy_id = Uuid::new_v4();
    sqlx::query("INSERT INTO user_groups (id,name) VALUES ($1,$2)")
        .bind(group_id)
        .bind(format!("batch-group-{group_id}"))
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO api_key_policies \
         (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
         VALUES ($1,$2,'{}','{}',true)",
    )
    .bind(policy_id)
    .bind(format!("batch-policy-{policy_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let user_ids = [Uuid::new_v4(), Uuid::new_v4()];
    for (index, user_id) in user_ids.into_iter().enumerate() {
        sqlx::query(
            "INSERT INTO users \
             (id,email,display_name,role,status,balance_amount) \
             VALUES ($1,$2,$3,'user','active',$4)",
        )
        .bind(user_id)
        .bind(format!("batch-{user_id}@example.test"))
        .bind(format!("batch-user-{user_id}"))
        .bind(rust_decimal::Decimal::from(10 + index as i64 * 10))
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let users = request(&app, "GET", "/console/v1/users", serde_json::json!({}), &[]).await;
    let users = body_json(users).await;
    let versions = user_ids
        .iter()
        .map(|id| {
            users
                .as_array()
                .unwrap()
                .iter()
                .find(|user| user["id"] == id.to_string())
                .unwrap()["updated_at"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect::<Vec<_>>();
    let update = request(
        &app,
        "POST",
        "/console/v1/users/batch",
        serde_json::json!({
            "items": [
                {"id": user_ids[0], "updated_at": versions[0]},
                {"id": user_ids[1], "updated_at": versions[1]},
            ],
            "changes": {
                "status": "suspended",
                "balance": {"operation": "increase", "amount": "5"},
                "user_group_id": group_id,
                "default_api_key_policy_id": policy_id,
            }
        }),
        &[],
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);
    let update_body = body_json(update).await;
    assert_eq!(update_body["updated_ids"].as_array().unwrap().len(), 2);

    let rows: Vec<(Uuid, String, rust_decimal::Decimal, Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id,status,balance_amount,user_group_id,default_api_key_policy_id \
             FROM users WHERE id=ANY($1) ORDER BY id",
    )
    .bind(user_ids)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert!(rows.iter().all(|row| row.1 == "suspended"));
    let mut balances = rows.iter().map(|row| row.2).collect::<Vec<_>>();
    balances.sort();
    assert_eq!(
        balances,
        vec![
            rust_decimal::Decimal::from(15),
            rust_decimal::Decimal::from(25)
        ]
    );
    assert!(
        rows.iter()
            .all(|row| row.3 == group_id && row.4 == Some(policy_id))
    );

    let current_users = request(&app, "GET", "/console/v1/users", serde_json::json!({}), &[]).await;
    let current_users = body_json(current_users).await;
    let current_version = current_users
        .as_array()
        .unwrap()
        .iter()
        .find(|user| user["id"] == user_ids[1].to_string())
        .unwrap()["updated_at"]
        .as_str()
        .unwrap()
        .to_owned();
    let stale = request(
        &app,
        "POST",
        "/console/v1/users/batch",
        serde_json::json!({
            "items": [
                {"id": user_ids[1], "updated_at": current_version},
                {"id": user_ids[0], "updated_at": versions[0]},
            ],
            "changes": {
                "balance": {"operation": "set", "amount": "99"}
            }
        }),
        &[],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let unchanged: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT balance_amount FROM users WHERE id=$1")
            .bind(user_ids[1])
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(unchanged, rust_decimal::Decimal::from(25));
    database.cleanup().await;
}

#[tokio::test]
async fn api_key_delete_erases_secret_hides_tombstone_and_releases_name() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let key_id = Uuid::new_v4();
    let key_name = format!("soft-delete-key-{key_id}");
    let secret = format!("sk-soft-delete-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions, \
          allowed_group_ids,allowed_channel_ids) \
         VALUES ($1,$2,$3,$4,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[], \
                 ARRAY['proxy']::text[],'{}','{}')",
    )
    .bind(key_id)
    .bind(app.user_id)
    .bind(&key_name)
    .bind(&secret)
    .execute(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/system/reload",
            serde_json::json!({}),
            &[],
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(app.runtime.snapshot().authenticate(&secret).is_some());

    let path = format!("/console/v1/me/api-keys/{key_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let deleted = request(
        &app,
        "DELETE",
        &path,
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert!(app.runtime.snapshot().authenticate(&secret).is_none());
    assert_eq!(
        request(&app, "GET", &path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    let tombstone: (
        String,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT status,secret_value,deleted_at,deleted_by FROM api_keys WHERE id=$1",
    )
    .bind(key_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(tombstone.0, "revoked");
    assert_eq!(tombstone.1, format!("deleted-api-key-{key_id}"));
    assert!(tombstone.2.is_some());
    assert_eq!(tombstone.3, Some(app.user_id));

    let replacement_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions, \
          allowed_group_ids,allowed_channel_ids) \
         VALUES ($1,$2,$3,$4,'disabled', \
                 ARRAY['open_ai_chat_completions']::api_format[], \
                 ARRAY['proxy']::text[],'{}','{}')",
    )
    .bind(replacement_id)
    .bind(app.user_id)
    .bind(&key_name)
    .bind(format!("sk-replacement-{}", Uuid::new_v4().simple()))
    .execute(&database.pool)
    .await
    .expect("a deleted Key releases its owner-scoped name");

    let admin_path = format!("/console/v1/api-keys/{replacement_id}");
    let detail = request(&app, "GET", &admin_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "DELETE",
            &admin_path,
            serde_json::json!({}),
            &[("if-match", &etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );
    let delete_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs \
         WHERE object_type='api_key' AND action IN ('self_delete','delete') \
           AND object_id=ANY($1)",
    )
    .bind(vec![key_id, replacement_id])
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(delete_audits, 2);

    database.cleanup().await;
}

/// Deleting a user anonymizes the retained owner row and tombstones every Key
/// instead of cascading away request-log/audit ownership.
#[tokio::test]
async fn user_delete_anonymizes_and_tombstones_api_keys() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let user_id = Uuid::new_v4();
    let api_key_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let invitation_id = Uuid::new_v4();
    let email = format!("delete-{user_id}@example.test");
    let api_key_secret = format!("sk-delete-{}", Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO users \
         (id,email,display_name,role,status,password_hash,balance_amount) \
         VALUES ($1,$2,$3,'user','active','test-hash',10)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(format!("delete-user-{user_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions, \
          allowed_group_ids,allowed_channel_ids) \
         VALUES ($1,$2,'delete-key',$3,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[], \
                 ARRAY['proxy']::text[],'{}','{}')",
    )
    .bind(api_key_id)
    .bind(user_id)
    .bind(&api_key_secret)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_sessions \
         (id,user_id,refresh_token_hash,expires_at) \
         VALUES ($1,$2,$3,now()+interval '1 day')",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(vec![1_u8; 32])
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_invitations \
         (id,user_id,invited_by,token_hash,expires_at) \
         VALUES ($1,$2,$3,$4,now()+interval '1 day')",
    )
    .bind(invitation_id)
    .bind(user_id)
    .bind(app.user_id)
    .bind(vec![2_u8; 32])
    .execute(&database.pool)
    .await
    .unwrap();

    let path = format!("/console/v1/users/{user_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let deleted = request(
        &app,
        "DELETE",
        &path,
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);

    let hidden = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let retained: (
        Option<String>,
        String,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Uuid,
    ) = sqlx::query_as(
        "SELECT email,display_name,status,deleted_at,user_group_id \
             FROM users WHERE id=$1",
    )
    .bind(user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(retained.0.is_none());
    assert!(retained.1.starts_with("Deleted user "));
    assert_eq!(retained.2, "disabled");
    assert!(retained.3.is_some());
    assert_eq!(retained.4, DEFAULT_USER_GROUP_ID);
    let key_tombstone: (
        String,
        String,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<Uuid>,
    ) = sqlx::query_as(
        "SELECT status,secret_value,deleted_at,deleted_by FROM api_keys WHERE id=$1",
    )
    .bind(api_key_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(key_tombstone.0, "revoked");
    assert_eq!(key_tombstone.1, format!("deleted-api-key-{api_key_id}"));
    assert_ne!(key_tombstone.1, api_key_secret);
    assert!(key_tombstone.2.is_some());
    assert_eq!(key_tombstone.3, Some(app.user_id));
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/console/v1/api-keys/{api_key_id}"),
            serde_json::json!({}),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let session_revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM user_sessions WHERE id=$1")
            .bind(session_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(session_revoked);
    let invitation_revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM user_invitations WHERE id=$1")
            .bind(invitation_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(invitation_revoked);
    let delete_audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs \
         WHERE object_type='user' AND object_id=$1 AND action='delete'",
    )
    .bind(user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(delete_audit, 1);

    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status) \
         VALUES ($1,$2,$3,'user','active')",
    )
    .bind(Uuid::new_v4())
    .bind(email)
    .bind(format!("replacement-{user_id}"))
    .execute(&database.pool)
    .await
    .expect("anonymization releases the email");

    let self_path = format!("/console/v1/users/{}", app.user_id);
    let self_detail = request(&app, "GET", &self_path, serde_json::json!({}), &[]).await;
    let self_etag = self_detail.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let self_delete = request(
        &app,
        "DELETE",
        &self_path,
        serde_json::json!({}),
        &[("if-match", &self_etag)],
    )
    .await;
    assert_eq!(self_delete.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(self_delete).await,
        serde_json::json!({"error": "cannot_delete_self"})
    );
    database.cleanup().await;
}

#[tokio::test]
async fn model_delete_hides_routing_clears_probes_and_preserves_history() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let source_model_id = format!("soft-delete-model-{}", Uuid::new_v4().simple());
    let model = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": source_model_id,
            "display_name": "Soft delete model",
            "provider_name": "Example",
            "enabled": true,
            "price_unit_tokens": 1_000_000,
            "input_unit_price": "0.10",
            "cached_input_unit_price": "0.01",
            "cache_write_unit_price": "0.02",
            "output_unit_price": "0.20",
            "price_effective_at": "2026-01-01T00:00:00Z",
            "advanced_billing": {
                "long_context_tiers": [],
                "request_multipliers": [],
                "time_multipliers": []
            },
            "source_payload": {"origin": "soft-delete-test"}
        }),
        &[],
    )
    .await;
    assert_eq!(model.status(), StatusCode::CREATED);
    let model_id = Uuid::parse_str(body_json(model).await["id"].as_str().unwrap()).unwrap();

    let topology = seed_test_topology(&app, "chat_completion").await;
    let group_id = topology.group;
    let channel_id = topology.capability;
    let group_name: String = sqlx::query_scalar("SELECT name FROM routing_groups WHERE id=$1")
        .bind(group_id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let cap_path = format!("/console/v1/routing/capabilities/{channel_id}");
    let cap = request(&app, "GET", &cap_path, serde_json::json!({}), &[]).await;
    let cap_etag = cap.headers()[header::ETAG].to_str().unwrap().to_owned();
    let mut cap_input = capability_input(topology.channel, "chat_completion");
    cap_input["settings"]["available_models"] = serde_json::json!(["model-delete-wire"]);
    cap_input["settings"]["test_model"] = serde_json::json!("model-delete-wire");
    cap_input["settings"]["test_pricing_model_id"] = serde_json::json!(model_id);
    assert_eq!(
        request(
            &app,
            "PUT",
            &cap_path,
            cap_input,
            &[("if-match", &cap_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let profile_id = create_resource(
        &app,
        "/console/v1/routing/profiles",
        serde_json::json!({"model_id": model_id}),
    )
    .await;
    let protocol_id = create_resource(
        &app,
        "/console/v1/routing/operation-rules",
        serde_json::json!({
            "model_routing_profile_id": profile_id, "operation": "chat_completion",
            "enabled": true, "routing_tiers": [{
                "priority": 0, "selection_strategy": "weighted_random",
                "candidates": [{"capability_id": channel_id,
                    "upstream_model": "model-delete-wire", "weight": 100}]
            }]
        }),
    )
    .await;
    assert!(
        app.runtime
            .snapshot()
            .model_rule(ApiFormat::OpenAiChatCompletions, &source_model_id)
            .is_some()
    );

    let key_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions, \
          allowed_group_ids,allowed_channel_ids) \
         VALUES ($1,$2,'model delete history',$3,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'], \
                 ARRAY[$4]::uuid[],'{}')",
    )
    .bind(key_id)
    .bind(app.user_id)
    .bind(format!("model-delete-history-{key_id}"))
    .bind(group_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let historical_log_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation, \
          client_model,upstream_model,model_rule_id,channel_group_id,channel_id, \
          outcome,streamed,total_duration_ms,model_id) \
         VALUES ($1,now(),now(),$2,$3,'open_ai_chat_completions','chat_completions', \
                 $4,'model-delete-wire',$5,$6,$7,'succeeded',false,10,$8)",
    )
    .bind(historical_log_id)
    .bind(app.user_id)
    .bind(key_id)
    .bind(&source_model_id)
    .bind(protocol_id)
    .bind(group_id)
    .bind(channel_id)
    .bind(model_id)
    .execute(&database.pool)
    .await
    .unwrap();

    let model_path = format!("/console/v1/models/{model_id}");
    let model_detail = request(&app, "GET", &model_path, serde_json::json!({}), &[]).await;
    assert_eq!(model_detail.status(), StatusCode::OK);
    let model_etag = model_detail.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let deleted = request(
        &app,
        "DELETE",
        &model_path,
        serde_json::json!({}),
        &[("if-match", &model_etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);

    let tombstone: (bool, Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) =
        sqlx::query_as("SELECT enabled,deleted_at,deleted_by FROM models WHERE id=$1")
            .bind(model_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(!tombstone.0);
    assert!(tombstone.1.is_some());
    assert_eq!(tombstone.2, Some(app.user_id));
    let retained_rule: (bool, i64) = sqlx::query_as(
        "SELECT enabled,(SELECT count(*) FROM model_capability_tiers \
                         WHERE rule_id=model_operation_rules.id) \
         FROM model_operation_rules WHERE id=$1",
    )
    .bind(protocol_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(retained_rule, (false, 0));
    let probe: (Option<String>, Option<Uuid>) = sqlx::query_as(
        "SELECT test_model,test_pricing_model_id FROM channel_capabilities WHERE id=$1",
    )
    .bind(channel_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(probe, (None, None));
    assert!(
        app.runtime
            .snapshot()
            .model_rule(ApiFormat::OpenAiChatCompletions, &source_model_id)
            .is_none()
    );
    assert_eq!(
        request(&app, "GET", &model_path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/console/v1/routing/profiles/{profile_id}"),
            serde_json::json!({}),
            &[],
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let visible_models = body_json(
        request(
            &app,
            "GET",
            "/console/v1/models",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert!(
        visible_models
            .as_array()
            .unwrap()
            .iter()
            .all(|model| model["id"] != model_id.to_string())
    );
    assert!(
        ControlPlaneRepository::new(database.pool.clone())
            .model_source_ids()
            .await
            .unwrap()
            .iter()
            .all(|candidate| candidate != &source_model_id)
    );

    let historical_log = body_json(
        request(
            &app,
            "GET",
            &format!("/console/v1/request-logs/{historical_log_id}"),
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(historical_log["client_model"], source_model_id);
    assert_eq!(historical_log["channel_group_name"], group_name);
    assert_eq!(historical_log["channel_name"], "Spec channel");
    let historical_ids: (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT model_id,model_rule_id FROM request_logs WHERE id=$1")
            .bind(historical_log_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(historical_ids, (Some(model_id), Some(protocol_id)));
    let deletion_audit: (serde_json::Value, Option<String>) = sqlx::query_as(
        "SELECT after_redacted,reason FROM audit_logs \
         WHERE object_type='model' AND object_id=$1 AND action='delete'",
    )
    .bind(model_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(deletion_audit.0["deleted_at"].is_string());
    assert_eq!(deletion_audit.0["deleted_by"], app.user_id.to_string());
    assert!(
        deletion_audit
            .1
            .is_some_and(|reason| reason.contains("1 operation rules disabled"))
    );

    let deleted_probe_reference = sqlx::query(
        "UPDATE channel_capabilities \
         SET test_model='model-delete-wire',test_pricing_model_id=$2 \
         WHERE id=$1",
    )
    .bind(channel_id)
    .bind(model_id)
    .execute(&database.pool)
    .await
    .unwrap_err();
    assert_eq!(
        deleted_probe_reference
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23514")
    );
    let reenabled_rule = sqlx::query("UPDATE model_operation_rules SET enabled=true WHERE id=$1")
        .bind(protocol_id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        reenabled_rule
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23514")
    );

    let replacement = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": source_model_id,
            "display_name": "Replacement model",
            "provider_name": "Example",
            "enabled": false,
            "price_unit_tokens": 1_000_000,
            "input_unit_price": "0.11",
            "cached_input_unit_price": "0.01",
            "cache_write_unit_price": "0.02",
            "output_unit_price": "0.21",
            "price_effective_at": "2026-01-02T00:00:00Z",
            "advanced_billing": {
                "long_context_tiers": [],
                "request_multipliers": [],
                "time_multipliers": []
            },
            "source_payload": {}
        }),
        &[],
    )
    .await;
    assert_eq!(replacement.status(), StatusCode::CREATED);
    let replacement_id =
        Uuid::parse_str(body_json(replacement).await["id"].as_str().unwrap()).unwrap();
    assert_ne!(replacement_id, model_id);
    let matching_models: Vec<(Uuid, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id,deleted_at FROM models \
         WHERE source_model_id=$1 ORDER BY deleted_at NULLS FIRST",
    )
    .bind(&source_model_id)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(matching_models.len(), 2);
    assert_eq!(matching_models[0], (replacement_id, None));
    assert_eq!(matching_models[1].0, model_id);
    assert!(matching_models[1].1.is_some());

    let tombstone_update = sqlx::query("UPDATE models SET display_name='changed' WHERE id=$1")
        .bind(model_id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        tombstone_update
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23514")
    );
    let hard_delete = sqlx::query("DELETE FROM models WHERE id=$1")
        .bind(model_id)
        .execute(&database.pool)
        .await
        .unwrap_err();
    assert_eq!(
        hard_delete
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::code)
            .as_deref(),
        Some("23514")
    );

    database.cleanup().await;
}

/// A mutable admin resource returns an `ETag` on GET and
/// requires `If-Match` on PUT; a stale `If-Match` yields `409` with an error
/// body. Channel groups are chosen because updating them does not change the
/// actor's `auth_version` (unlike `UpdateUser`), so the issued JWT stays valid
/// across the two PUTs.
#[tokio::test]
async fn etag_if_match_optimistic_concurrency_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let removed_group_routing = request(
        &app,
        "POST",
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "legacy-spec-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(
        removed_group_routing.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let create = request(
        &app,
        "POST",
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "spec-group",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let group_id = body_json(create).await["id"].as_str().unwrap().to_owned();
    let path = format!("/console/v1/routing/groups/{group_id}");

    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail
        .headers()
        .get(header::ETAG)
        .expect("detail returns ETag per spec")
        .to_str()
        .unwrap()
        .to_owned();
    let mut update = body_json(detail).await;
    assert!(update.get("connector_pool_id").is_none());
    assert!(update.get("request_compression").is_none());
    assert!(update.get("status_statistics_enabled").is_none());
    assert!(update.get("priority").is_none());
    assert!(update.get("selection_strategy").is_none());
    update["name"] = serde_json::json!("spec-group-renamed");
    for field in ["id", "created_at", "updated_at", "deleted_at"] {
        update.as_object_mut().unwrap().remove(field);
    }

    let ok = request(&app, "PUT", &path, update.clone(), &[("if-match", &etag)]).await;
    assert_eq!(ok.status(), StatusCode::OK);
    let ok_body = body_json(ok).await;
    assert!(
        ok_body["correlation_id"].is_string(),
        "mutation correlation"
    );
    let renamed: String = sqlx::query_scalar("SELECT name FROM routing_groups WHERE id=$1")
        .bind(Uuid::parse_str(&group_id).unwrap())
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(renamed, "spec-group-renamed");

    let conflict = request(&app, "PUT", &path, update, &[("if-match", &etag)]).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let conflict_body = body_text(conflict).await;
    assert!(
        conflict_body.contains("\"error\""),
        "conflict body is an error body"
    );
    database.cleanup().await;
}

#[tokio::test]
async fn request_compression_is_restricted_to_responses_capabilities() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    for operation in [
        "chat_completion",
        "images_generation",
        "images_edit",
        "web_search",
        "responses",
        "responses-ws",
    ] {
        let topology = seed_test_topology(&app, operation).await;
        let path = format!("/console/v1/routing/capabilities/{}", topology.capability);
        let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
        let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
        let before = body_json(detail).await;
        let mut input = capability_input(topology.channel, operation);
        input["settings"]["request_compression"] = serde_json::json!("zstd");
        let response = request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)]).await;
        assert_eq!(
            response.status(),
            if operation == "responses" {
                StatusCode::OK
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            }
        );
        let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
        let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
        let after = body_json(detail).await;
        if operation == "responses" {
            assert_eq!(after["settings"]["request_compression"], "zstd");
            input["settings"]["transports"] = serde_json::json!(["websocket"]);
            assert_eq!(
                request(&app, "PUT", &path, input, &[("if-match", &etag)])
                    .await
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        } else {
            assert_eq!(before, after);
        }
    }
    database.cleanup().await;
}

#[tokio::test]
async fn sharing_only_mode_is_codex_scoped_versioned_and_pool_wide() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let path = "/console/v1/routing/groups";
    let ordinary = seed_test_topology(&app, "responses").await;
    let ordinary_path = format!("{path}/{}", ordinary.group);
    let detail = request(&app, "GET", &ordinary_path, serde_json::json!({}), &[]).await;
    let ordinary_etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let invalid = request(
        &app,
        "PUT",
        &ordinary_path,
        serde_json::json!({
            "name":"invalid-sharing-only", "enabled":true, "sharing_only":true
        }),
        &[("if-match", &ordinary_etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let created = request(
        &app,
        "POST",
        path,
        serde_json::json!({
            "name":"sharing-only-contract", "enabled":true, "sharing_only":true
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_str().unwrap().to_owned();
    let group_id = Uuid::parse_str(&id).unwrap();
    let credential = create_test_codex_credential(
        &database.pool,
        &app,
        codex_fixture_input(group_id, "sharing-only-member"),
    )
    .await;
    let images = codex_capability_id(&database.pool, credential, "images_generation").await;
    let images_enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM channel_capabilities WHERE id=$1")
            .bind(images)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(!images_enabled);
    let detail_path = format!("{path}/{id}");
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(body_json(detail).await["sharing_only"], true);
    let mut input = serde_json::json!({
        "name":"sharing-only-renamed", "enabled":true, "sharing_only":true
    });
    let saved = request(
        &app,
        "PUT",
        &detail_path,
        input.clone(),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let detail = request(&app, "GET", &detail_path, serde_json::json!({}), &[]).await;
    let next_etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(body_json(detail).await["sharing_only"], true);
    input["sharing_only"] = serde_json::json!(false);
    assert_eq!(
        request(
            &app,
            "PUT",
            &detail_path,
            input.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "PUT",
            &detail_path,
            input,
            &[("if-match", &next_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let images: (bool, bool) =
        sqlx::query_as("SELECT c.enabled,g.sharing_only FROM channel_capabilities c JOIN upstream_channels u ON u.id=c.channel_id JOIN routing_groups g ON g.id=u.group_id WHERE c.id=$1")
            .bind(images)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(images, (false, false));
    database.cleanup().await;
}

#[tokio::test]
async fn codex_oauth_flow_contract_uses_pkce_and_actor_scoped_completion() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let legacy_group = request(
        &app,
        "POST",
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "legacy-format-group", "api_format": "open_ai_images",
            "connector_kind": "codex", "enabled": false
        }),
        &[],
    )
    .await;
    assert_eq!(legacy_group.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let group_uuid = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "spec-codex", "enabled": true
        }),
    )
    .await;
    let group_id = group_uuid.to_string();
    let groups = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/groups",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(groups.as_array().unwrap().len(), 1);
    assert_eq!(groups[0]["id"], group_id);
    assert!(groups[0].get("connector_pool_id").is_none());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM channel_capabilities")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let group_audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_type='routing_group' AND object_id=$1 AND action='create'"
    ).bind(group_uuid).fetch_one(&database.pool).await.unwrap();
    assert_eq!(group_audit["name"], "spec-codex");

    let legacy_import = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials"),
        serde_json::json!({
            "label": "legacy-import",
            "weight": 100,
            "quota_threshold_percent": 95,
            "access_token": "legacy-access",
            "refresh_token": "legacy-refresh"
        }),
        &[],
    )
    .await;
    assert_eq!(legacy_import.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let legacy_start = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/oauth/flows"),
        serde_json::json!({
            "label": "legacy-spec-account",
            "weight": 100,
            "quota_threshold_percent": 95
        }),
        &[],
    )
    .await;
    assert_eq!(legacy_start.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let start = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/oauth/flows"),
        serde_json::json!({
            "label": "spec-account",
            "quota_threshold_percent": 95
        }),
        &[],
    )
    .await;
    assert_eq!(start.status(), StatusCode::CREATED);
    let start = body_json(start).await;
    let flow_id = start["flow_id"].as_str().unwrap();
    let authorization_url = reqwest::Url::parse(start["authorization_url"].as_str().unwrap())
        .expect("authorization URL is valid");
    let query = authorization_url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(authorization_url.host_str(), Some("auth.openai.com"));
    assert_eq!(
        query.get("redirect_uri").map(|value| value.as_ref()),
        Some("http://localhost:1455/auth/callback")
    );
    assert_eq!(
        query
            .get("code_challenge_method")
            .map(|value| value.as_ref()),
        Some("S256")
    );
    assert!(query.contains_key("state"));
    assert!(!query.contains_key("code_verifier"));

    let stored: (i32, bool) = sqlx::query_as(
        "SELECT octet_length(state_hash), length(code_verifier) >= 43 \
         FROM codex_oauth_flows WHERE id=$1",
    )
    .bind(Uuid::parse_str(flow_id).unwrap())
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored, (32, true));

    let list = request(
        &app,
        "GET",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(body_json(list).await, serde_json::json!([]));

    let missing_group = Uuid::new_v4();
    for (method, path) in [
        (
            "GET",
            format!("/console/v1/providers/codex-oauth/channel-groups/{missing_group}/credentials"),
        ),
        (
            "POST",
            format!("/console/v1/providers/codex-oauth/channel-groups/{missing_group}/oauth/flows"),
        ),
    ] {
        let response = request(
            &app,
            method,
            &path,
            serde_json::json!({
                "label": "unknown-group", "quota_threshold_percent": 95
            }),
            &[],
        )
        .await;
        if method == "GET" {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(body_json(response).await, serde_json::json!([]));
        } else {
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }
    }

    let mismatched = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/oauth/flows/{flow_id}/complete"),
        serde_json::json!({
            "callback_url":
                "http://localhost:1455/auth/callback?code=spec-code&state=wrong-state"
        }),
        &[],
    )
    .await;
    assert_eq!(mismatched.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(mismatched).await,
        serde_json::json!({"error": "codex_oauth_state_mismatch"})
    );

    database.cleanup().await;
}

#[tokio::test]
async fn codex_export_and_proxy_delete_contracts_preserve_secrets_and_references() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group_id = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "spec-portable", "enabled": true
        }),
    )
    .await;
    let assigned_proxy_id = Uuid::new_v4();
    let removable_proxy_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO proxies \
         (id,name,proxy_url,username,password,no_proxy_hosts,enabled) \
         VALUES \
         ($1,'assigned','socks5h://127.0.0.1:1080','proxy-user','proxy-password','{}',true), \
         ($2,'removable','http://127.0.0.1:8080',NULL,NULL,'{}',true)",
    )
    .bind(assigned_proxy_id)
    .bind(removable_proxy_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let mut input = codex_fixture_input(group_id, "spec-account");
    input.proxy_id = Some(assigned_proxy_id);
    input.email = Some("portable@example.test".into());
    input.account_id = None;
    input.user_id = Some("portable-user".into());
    input.plan_type = Some("free".into());
    input.id_token = "secret-id".into();
    input.access_token = "secret-access".into();
    input.refresh_token = "secret-refresh".into();
    let channel_id = create_test_codex_credential(&database.pool, &app, input).await;

    let exported = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials/export"),
        serde_json::json!({
            "credential_ids": [channel_id],
            "include_proxies": true
        }),
        &[],
    )
    .await;
    assert_eq!(exported.status(), StatusCode::OK);
    let exported = body_json(exported).await;
    assert_eq!(exported["type"], "ai-gateway-codex-credentials");
    assert_eq!(exported["version"], 2);
    assert_eq!(
        exported["credentials"][0]["account_id"],
        serde_json::Value::Null
    );
    assert_eq!(exported["credentials"][0]["user_id"], "portable-user");
    assert_eq!(exported["credentials"][0]["id_token"], "secret-id");
    assert_eq!(exported["credentials"][0]["access_token"], "secret-access");
    assert_eq!(
        exported["credentials"][0]["refresh_token"],
        "secret-refresh"
    );
    assert_eq!(
        exported["credentials"][0]["proxy_key"],
        assigned_proxy_id.to_string()
    );
    assert!(exported["credentials"][0].get("weight").is_none());
    assert_eq!(exported["proxies"][0]["password"], "proxy-password");

    let assigned = request(
        &app,
        "GET",
        &format!("/console/v1/network/proxies/{assigned_proxy_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    let assigned_etag = assigned
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let blocked = request(
        &app,
        "DELETE",
        &format!("/console/v1/network/proxies/{assigned_proxy_id}"),
        serde_json::json!({}),
        &[("if-match", &assigned_etag)],
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(blocked).await,
        serde_json::json!({"error": "proxy_in_use"})
    );

    let removable = request(
        &app,
        "GET",
        &format!("/console/v1/network/proxies/{removable_proxy_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    let removable_etag = removable
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let deleted = request(
        &app,
        "DELETE",
        &format!("/console/v1/network/proxies/{removable_proxy_id}"),
        serde_json::json!({}),
        &[("if-match", &removable_etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(
        body_json(deleted).await["id"],
        removable_proxy_id.to_string()
    );

    database.cleanup().await;
}

#[tokio::test]
async fn codex_business_batch_and_delete_contracts_are_versioned() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group_id = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "spec-business", "enabled": true
        }),
    )
    .await;
    let mut members = Vec::new();
    for (label, email, user_id) in [
        ("business-member-a", "member-a@example.test", "user-a"),
        ("business-member-b", "member-b@example.test", "user-b"),
    ] {
        let mut input = codex_fixture_input(group_id, label);
        input.email = Some(email.into());
        input.account_id = Some("business-workspace".into());
        input.user_id = Some(user_id.into());
        input.plan_type = Some("business".into());
        input.id_token = format!("{label}-id");
        input.access_token = format!("{label}-access");
        input.refresh_token = format!("{label}-refresh");
        members.push(create_test_codex_credential(&database.pool, &app, input).await);
    }
    let member_a = members[0];

    let list = request(
        &app,
        "GET",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list = body_json(list).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["account_id"], "business-workspace");
    assert_ne!(list[0]["user_id"], list[1]["user_id"]);
    assert!(list[0].get("weight").is_none());

    let items = list
        .as_array()
        .unwrap()
        .iter()
        .map(|credential| {
            serde_json::json!({
                "id": credential["id"],
                "updated_at": credential["updated_at"],
            })
        })
        .collect::<Vec<_>>();
    let disabled = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials/batch"),
        serde_json::json!({
            "items": items,
            "operation": "disable",
        }),
        &[],
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::OK);
    assert_eq!(
        body_json(disabled).await["updated_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/providers/codex-oauth/credentials/{member_a}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let detail_body = body_json(detail).await;
    assert_eq!(detail_body["enabled"], false);
    assert!(detail_body.get("weight").is_none());

    let legacy_update = request(
        &app,
        "PUT",
        &format!("/console/v1/providers/codex-oauth/credentials/{member_a}"),
        serde_json::json!({
            "label": "business-member-a",
            "enabled": false,
            "proxy_id": null,
            "weight": 100,
            "quota_threshold_percent": 95
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(legacy_update.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let deleted = request(
        &app,
        "DELETE",
        &format!("/console/v1/providers/codex-oauth/credentials/{member_a}"),
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(body_json(deleted).await["id"], member_a.to_string());

    let remaining = request(
        &app,
        "GET",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/credentials"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(body_json(remaining).await.as_array().unwrap().len(), 1);
    let scrubbed: (String, String, String, bool) = sqlx::query_as(
        "SELECT id_token,access_token,refresh_token,deleted_at IS NOT NULL \
         FROM codex_oauth_credentials WHERE channel_id=$1",
    )
    .bind(member_a)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        scrubbed,
        ("deleted".into(), "deleted".into(), "deleted".into(), true)
    );

    database.cleanup().await;
}

/// The admin-only load endpoint exposes the current instance's resource,
/// runtime, queue, request-body spool, backlog, and database-pool pressure
/// shape.
#[tokio::test]
async fn system_load_reports_current_instance_pressure_shape() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let response = request(
        &app,
        "GET",
        "/console/v1/system/load",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["sampled_at"].is_string());
    assert!(body["started_at"].is_string());
    assert!(body["uptime_seconds"].is_number());
    assert!(body["host"]["logical_cpu_count"].is_number());
    assert!(
        body["process"]["cpu_usage_percent"].is_number()
            || body["process"]["cpu_usage_percent"].is_null()
    );
    assert!(body["runtime"]["in_flight_requests"].is_number());
    assert!(body["queues"]["request_log_notifications"]["depth"].is_number());
    assert!(body["request_log"]["spool_pending_bytes"].is_number());
    assert!(body["websocket"]["active_downstream_sessions"].is_number());
    assert!(body["websocket"]["pool_hits_total"].is_number());
    assert!(body["image_body_spool"]["active_files"].is_number());
    assert!(body["image_body_spool"]["active_bytes"].is_number());
    assert!(
        body["image_body_spool"]["available_bytes"].is_number()
            || body["image_body_spool"]["available_bytes"].is_null()
    );
    assert!(body["image_body_spool"]["storage_failures_total"].is_number());
    assert!(body["database"]["control_plane"]["capacity"].is_number());
    database.cleanup().await;
}

/// The singleton system-settings resource follows the same ETag convention as
/// other mutable Console resources and publishes its database-backed policy.
#[tokio::test]
async fn system_settings_are_versioned_audited_and_updated_via_console() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let detail = request(
        &app,
        "GET",
        "/console/v1/system/settings",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail
        .headers()
        .get(header::ETAG)
        .expect("system settings include an ETag")
        .to_str()
        .unwrap()
        .to_owned();
    let mut input = body_json(detail).await;
    input["api_hosts"] = serde_json::json!(["https://gateway.example.test/v1"]);
    input["upstream"]["connect_timeout_seconds"] = serde_json::json!(2);
    input["upstream"]["response_header_timeout_seconds"] = serde_json::json!(5);
    input["upstream"]["images_response_header_timeout_seconds"] = serde_json::json!(180);
    input["upstream"]["stream_idle_timeout_seconds"] = serde_json::json!(8);
    input["request_retry"] = serde_json::json!({
        "enabled": false,
        "max_retries": 4,
        "retryable_status_codes": [429, 503],
    });
    input["passive_health"]["connection_failure_threshold"] = serde_json::json!(4);
    input["passive_health"]["cooldown_seconds"] = serde_json::json!(45);
    input["automatic_disable"] = serde_json::json!({
        "enabled": true,
        "error_status_codes": [429, 503],
        "error_message_keywords": ["quota exceeded", "insufficient balance"],
    });
    input["scheduled_testing"] = serde_json::json!({
        "mode": "failure_only",
        "auto_recover": false,
        "interval_minutes": 7,
        "prompt": "reply '1'",
    });
    input["session_affinity"] = serde_json::json!({
        "enabled": true,
        "max_entries": 1000,
        "default_ttl_seconds": 3600,
        "rules": [{
            "name": "codex",
            "enabled": true,
            "api_formats": ["open_ai_responses"],
            "model_regex": ["^gpt-.*$"],
            "key_sources": [{"type": "json_pointer", "pointer": "/prompt_cache_key"}],
            "value_regex": null,
            "ttl_seconds": null,
        }],
    });
    input["websocket"] = serde_json::json!({
        "enabled": true,
        "max_idle_connections": 64,
        "idle_timeout_seconds": 120,
        "max_connection_age_seconds": 3300,
    });
    input["codex"] = serde_json::json!({
        "workspace_path": "/synthetic/project",
        "git_remote_url": "https://github.com/example/synthetic-project",
        "originator": "codex_gateway",
        "client_version": "9.8.7",
        "user_agent": "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway",
    });
    input.as_object_mut().unwrap().remove("updated_at");

    let mut invalid_retry = input.clone();
    invalid_retry["request_retry"]["max_retries"] = serde_json::json!(11);
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_retry,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_retry_status = input.clone();
    invalid_retry_status["request_retry"]["retryable_status_codes"] = serde_json::json!([399]);
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_retry_status,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_images_timeout = input.clone();
    invalid_images_timeout["upstream"]["images_response_header_timeout_seconds"] =
        serde_json::json!(2);
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_images_timeout,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_websocket = input.clone();
    invalid_websocket["websocket"]["max_connection_age_seconds"] = serde_json::json!(120);
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_websocket,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_api_host = input.clone();
    invalid_api_host["api_hosts"] = serde_json::json!(["ftp://gateway.example.test"]);
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_api_host,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_codex_remote = input.clone();
    invalid_codex_remote["codex"]["git_remote_url"] =
        serde_json::json!("git@github.com:private/repo.git");
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_codex_remote,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut invalid_codex_identity = input.clone();
    invalid_codex_identity["codex"]["user_agent"] =
        serde_json::json!("codex_gateway/9.8.7\r\ninjected");
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        invalid_codex_identity,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut missing_codex_identity = input.clone();
    missing_codex_identity["codex"]
        .as_object_mut()
        .unwrap()
        .remove("client_version");
    let invalid = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        missing_codex_identity,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let updated = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        input.clone(),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert!(body_json(updated).await["correlation_id"].is_string());

    let api_hosts = request(
        &app,
        "GET",
        "/console/v1/me/api-hosts",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(api_hosts.status(), StatusCode::OK);
    assert_eq!(
        body_json(api_hosts).await,
        serde_json::json!({"api_hosts": ["https://gateway.example.test/v1"]})
    );

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT value FROM system_settings WHERE setting_key='forwarding_policy'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(stored, input);
    let snapshot = app.runtime.snapshot();
    let published = snapshot.system_settings();
    assert_eq!(
        published.upstream_timeouts().connect(),
        std::time::Duration::from_secs(2)
    );
    assert_eq!(
        published.upstream_timeouts().images_response_header(),
        std::time::Duration::from_secs(180)
    );
    assert!(!published.request_retry().enabled());
    assert_eq!(published.request_retry().max_retries(), 4);
    assert_eq!(
        published.request_retry().retryable_status_codes(),
        &[429, 503]
    );
    assert_eq!(published.passive_health().connection_failure_threshold(), 4);
    assert!(published.automatic_disable().enabled());
    assert!(published.automatic_disable().matches_status(429));
    assert_eq!(
        published.scheduled_testing().mode(),
        ai_gateway::domain::ScheduledTestingMode::FailureOnly
    );
    assert!(!published.scheduled_testing().auto_recover());
    assert_eq!(
        published.scheduled_testing().interval(),
        std::time::Duration::from_secs(7 * 60)
    );
    assert!(published.session_affinity().enabled());
    assert_eq!(published.session_affinity().max_entries(), 1_000);
    assert_eq!(published.session_affinity().rules()[0].name(), "codex");
    assert!(published.websocket().enabled());
    assert_eq!(published.websocket().max_idle_connections(), 64);
    assert_eq!(
        published.websocket().idle_timeout(),
        std::time::Duration::from_secs(120)
    );
    assert_eq!(published.codex().workspace_path(), "/synthetic/project");
    assert_eq!(
        published.codex().git_remote_url(),
        "https://github.com/example/synthetic-project"
    );
    assert_eq!(
        published.codex().outbound_identity().originator(),
        "codex_gateway"
    );
    assert_eq!(
        published.codex().outbound_identity().client_version(),
        "9.8.7"
    );
    assert_eq!(
        published.codex().outbound_identity().user_agent(),
        "codex_gateway/9.8.7 (Linux 6.8.0; x86_64) ai-gateway"
    );

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs \
         WHERE object_type='system_settings' ORDER BY occurred_at DESC LIMIT 1",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["value"], input);

    let conflict = request(
        &app,
        "PUT",
        "/console/v1/system/settings",
        input,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    database.cleanup().await;
}

/// A parent model rule owns one priced client identity. Its protocol children
/// have immutable formats and choose wire models from explicit candidates.
#[tokio::test]
async fn model_rule_hierarchy_separates_pricing_from_candidate_models() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let effective_at = chrono::Utc::now().to_rfc3339();
    let model = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": "spec-priced-client",
            "display_name": "Spec priced client",
            "enabled": true,
            "price_unit_tokens": 1000000,
            "input_unit_price": "0.1",
            "cached_input_unit_price": "0",
            "cache_write_unit_price": "0",
            "output_unit_price": "0.2",
            "price_effective_at": effective_at,
        }),
        &[],
    )
    .await;
    assert_eq!(model.status(), StatusCode::CREATED);
    let model_id = body_json(model).await["id"].as_str().unwrap().to_owned();

    let topology = seed_test_topology(&app, "chat_completion").await;
    let channel_id = topology.capability;
    let cap_path = format!("/console/v1/routing/capabilities/{channel_id}");
    let cap = request(&app, "GET", &cap_path, serde_json::json!({}), &[]).await;
    let cap_etag = cap.headers()[header::ETAG].to_str().unwrap().to_owned();
    let mut cap_input = capability_input(topology.channel, "chat_completion");
    cap_input["settings"]["available_models"] =
        serde_json::json!(["spec-wire-model", "spec-wire-fallback"]);
    assert_eq!(
        request(
            &app,
            "PUT",
            &cap_path,
            cap_input,
            &[("if-match", &cap_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let parent = request(
        &app,
        "POST",
        "/console/v1/routing/profiles",
        serde_json::json!({"model_id": model_id}),
        &[],
    )
    .await;
    assert_eq!(parent.status(), StatusCode::CREATED);
    let parent_id = body_json(parent).await["id"].as_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/routing/profiles",
            serde_json::json!({"model_id": model_id}),
            &[],
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let parent = request(
        &app,
        "GET",
        &format!("/console/v1/routing/profiles/{parent_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(parent.status(), StatusCode::OK);
    let parent = body_json(parent).await;
    assert_eq!(parent["model_id"], model_id);
    assert_eq!(parent["client_model"], "spec-priced-client");
    assert!(parent.get("protocol_rules").is_none());
    assert!(parent.get("api_format").is_none());
    assert!(parent.get("upstream_model").is_none());

    let protocol = request(
        &app,
        "POST",
        "/console/v1/routing/operation-rules",
        serde_json::json!({"model_routing_profile_id": parent_id, "operation": "chat_completion", "enabled": false, "routing_tiers": []}),
        &[],
    )
    .await;
    assert_eq!(protocol.status(), StatusCode::CREATED);
    let protocol_id = body_json(protocol).await["id"].as_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "POST",
            "/console/v1/routing/operation-rules",
            serde_json::json!({"model_routing_profile_id": parent_id, "operation": "chat_completion", "enabled": false, "routing_tiers": []}),
            &[],
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let responses_protocol = request(
        &app,
        "POST",
        "/console/v1/routing/operation-rules",
        serde_json::json!({"model_routing_profile_id": parent_id, "operation": "responses", "enabled": false, "routing_tiers": []}),
        &[],
    )
    .await;
    assert_eq!(responses_protocol.status(), StatusCode::CREATED);
    let parent_with_protocols = request(
        &app,
        "GET",
        "/console/v1/routing/operation-rules",
        serde_json::json!({}),
        &[],
    )
    .await;
    let parent_with_protocols = body_json(parent_with_protocols).await;
    assert_eq!(parent_with_protocols.as_array().unwrap().len(), 2);

    let protocol_path = format!("/console/v1/routing/operation-rules/{protocol_id}");
    let detail = request(&app, "GET", &protocol_path, serde_json::json!({}), &[]).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let detail = body_json(detail).await;
    assert_eq!(detail["model_routing_profile_id"], parent_id);
    assert_eq!(detail["operation"], "chat_completion");
    assert_eq!(detail["enabled"], false);
    assert_eq!(detail["routing_tiers"], serde_json::json!([]));

    let route = |upstream_model: &str| {
        serde_json::json!({
            "model_routing_profile_id": parent_id, "operation": "chat_completion",
            "routing_tiers": [
                {
                    "priority": 0,
                    "selection_strategy": "weighted_round_robin",
                    "candidates": [{
                        "capability_id": channel_id,
                        "upstream_model": upstream_model,
                        "weight": 7
                    }]
                },
                {
                    "priority": 3,
                    "selection_strategy": "weighted_round_robin",
                    "candidates": [{
                        "capability_id": channel_id,
                        "upstream_model": "spec-wire-fallback",
                        "weight": 5
                    }]
                }
            ],
            "enabled": true,
        })
    };
    let mut format_mutation = route("spec-wire-model");
    format_mutation["operation"] = serde_json::json!("responses");
    let immutable_format = request(
        &app,
        "PUT",
        &protocol_path,
        format_mutation,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(immutable_format.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut missing_operation = route("spec-wire-model");
    missing_operation
        .as_object_mut()
        .unwrap()
        .remove("operation");
    let missing_operation = request(
        &app,
        "PUT",
        &protocol_path,
        missing_operation,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(missing_operation.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let mut missing_channel_model = route("spec-wire-model");
    missing_channel_model["routing_tiers"][0]["candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("upstream_model");
    let missing_channel_model = request(
        &app,
        "PUT",
        &protocol_path,
        missing_channel_model,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(
        missing_channel_model.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_candidate = request(
        &app,
        "PUT",
        &protocol_path,
        serde_json::json!({
            "model_routing_profile_id": parent_id, "operation": "chat_completion",
            "routing_tiers": [{
                "priority": 0,
                "selection_strategy": "weighted_random",
                "candidates": [{
                    "capability_id": channel_id,
                    "upstream_model": "not-advertised",
                    "weight": 100
                }]
            }],
            "enabled": true
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid_candidate.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(invalid_candidate).await,
        serde_json::json!({"error": "routing_dependency_invalid"})
    );

    let invalid = request(
        &app,
        "PUT",
        &protocol_path,
        route("not-advertised"),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(invalid).await,
        serde_json::json!({"error": "routing_dependency_invalid"})
    );

    let updated_input = route("spec-wire-model");
    let updated = request(
        &app,
        "PUT",
        &protocol_path,
        updated_input.clone(),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let current = request(&app, "GET", &protocol_path, serde_json::json!({}), &[]).await;
    let current = body_json(current).await;
    assert_eq!(current["routing_tiers"], updated_input["routing_tiers"]);

    let stale = request(
        &app,
        "PUT",
        &protocol_path,
        updated_input,
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let audit: (serde_json::Value, serde_json::Value) = sqlx::query_as(
        "SELECT before_redacted,after_redacted \
         FROM audit_logs \
         WHERE object_type='model_operation_rule' AND object_id=$1 AND action='update'",
    )
    .bind(Uuid::parse_str(&protocol_id).unwrap())
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit.0["tiers"], serde_json::json!([]));
    assert_eq!(audit.0["candidates"], serde_json::json!([]));
    assert_eq!(audit.1["tiers"].as_array().unwrap().len(), 2);
    assert_eq!(audit.1["candidates"].as_array().unwrap().len(), 2);
    assert_eq!(audit.1["rule"]["enabled"], true);
    assert!(audit.1["candidates"].as_array().unwrap().iter().any(
        |candidate| candidate["capability_id"] == channel_id.to_string()
            && candidate["upstream_model"] == "spec-wire-model"
            && candidate["weight"] == 7
    ));

    database.cleanup().await;
}

#[tokio::test]
async fn capability_deletion_preserves_fixed_grants_and_requires_route_withdrawal() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let (profile, rule) =
        create_test_operation_rule(&app, topology.capability, "chat_completion").await;
    let key = create_resource(&app, "/console/v1/api-keys", serde_json::json!({
        "user_id": app.user_id, "name": "deletion-key", "allowed_api_formats": ["open_ai_chat_completions"],
        "permissions": ["proxy"], "allowed_group_ids": [topology.group], "allowed_channel_ids": []
    })).await;
    let policy = create_resource(
        &app,
        "/console/v1/api-key-policies",
        serde_json::json!({
            "name": "deletion-policy", "enabled": true, "allowed_group_ids": [],
            "allowed_channel_ids": [topology.channel]
        }),
    )
    .await;
    let path = format!("/console/v1/routing/capabilities/{}", topology.capability);
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let before = body_json(detail).await;
    assert_eq!(
        request(
            &app,
            "DELETE",
            &path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        body_json(request(&app, "GET", &path, serde_json::json!({}), &[]).await).await,
        before
    );
    let rule_path = format!("/console/v1/routing/operation-rules/{rule}");
    let detail = request(&app, "GET", &rule_path, serde_json::json!({}), &[]).await;
    let rule_etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(
        request(
            &app,
            "PUT",
            &rule_path,
            serde_json::json!({
                "model_routing_profile_id": profile, "operation": "chat_completion",
                "enabled": false, "routing_tiers": []
            }),
            &[("if-match", &rule_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &path,
            serde_json::json!({}),
            &[("if-match", "\"2020-01-01T00:00:00Z\"")]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(&app, "GET", &path, serde_json::json!({}), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &app,
            "DELETE",
            &path,
            serde_json::json!({}),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let key_grants: Vec<(Uuid, String, Uuid)> = sqlx::query_as("SELECT capability_id,origin_kind,origin_id FROM api_key_capability_grants WHERE api_key_id=$1")
        .bind(key).fetch_all(&database.pool).await.unwrap();
    assert_eq!(
        key_grants,
        [(topology.capability, "group".into(), topology.group)]
    );
    let policy_grants: Vec<(Uuid, String, Uuid)> = sqlx::query_as("SELECT capability_id,origin_kind,origin_id FROM api_key_policy_capability_grants WHERE policy_id=$1")
        .bind(policy).fetch_all(&database.pool).await.unwrap();
    assert_eq!(
        policy_grants,
        [(topology.capability, "channel".into(), topology.channel)]
    );
    let channel = request(
        &app,
        "GET",
        &format!("/console/v1/routing/logical-channels/{}", topology.channel),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::OK);
    assert!(
        app.runtime
            .snapshot()
            .channel(topology.capability)
            .is_none()
    );
    let label: String =
        sqlx::query_scalar("SELECT label FROM channel_identity_registry WHERE id=$1")
            .bind(topology.capability)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(label, "Spec channel");
    database.cleanup().await;
}

#[tokio::test]
async fn group_deletion_requires_explicit_child_retirement_and_preserves_history() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let group_path = format!("/console/v1/routing/groups/{}", topology.group);
    let channel_path = format!("/console/v1/routing/logical-channels/{}", topology.channel);
    let capability_path = format!("/console/v1/routing/capabilities/{}", topology.capability);
    let mut etags = Vec::new();
    for path in [&group_path, &channel_path, &capability_path] {
        let detail = request(&app, "GET", path, serde_json::json!({}), &[]).await;
        etags.push(detail.headers()["etag"].to_str().unwrap().to_owned());
    }
    for (path, etag) in [(&group_path, &etags[0]), (&channel_path, &etags[1])] {
        assert_eq!(
            request(
                &app,
                "DELETE",
                path,
                serde_json::json!({}),
                &[("if-match", etag)]
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let policy = create_resource(
        &app,
        "/console/v1/api-key-policies",
        serde_json::json!({
            "name": "retained-group-policy", "allowed_group_ids": [topology.group],
            "allowed_channel_ids": [topology.channel], "enabled": true
        }),
    )
    .await;
    for (path, etag) in [
        (&capability_path, &etags[2]),
        (&channel_path, &etags[1]),
        (&group_path, &etags[0]),
    ] {
        let response = request(
            &app,
            "DELETE",
            path,
            serde_json::json!({}),
            &[("if-match", etag)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(body_json(response).await["correlation_id"].is_string());
        assert_eq!(
            request(&app, "GET", path, serde_json::json!({}), &[])
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        request(
            &app,
            "GET",
            &format!("/console/v1/routing/accesses/{}", topology.access),
            serde_json::json!({}),
            &[]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let detail = body_json(
        request(
            &app,
            "GET",
            &format!("/console/v1/api-key-policies/{policy}"),
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(
        detail["allowed_group_ids"],
        serde_json::json!([topology.group])
    );
    assert_eq!(
        detail["allowed_channel_ids"],
        serde_json::json!([topology.channel])
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_key_policy_capability_grants WHERE policy_id=$1",
    )
    .bind(policy)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    let history: i64 =
        sqlx::query_scalar("SELECT count(*) FROM group_identity_registry WHERE id=$1")
            .bind(topology.group)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(history, 1);
    database.cleanup().await;
}

/// Model prices may define immutable advanced billing policy: input-price
/// tiers, request-body JSON Pointer multipliers, and recurring weekly UTC price
/// windows are model-level facts, not channel transforms. Invalid policy is
/// rejected before it can be persisted.
#[tokio::test]
async fn model_advanced_billing_is_returned_and_validated() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let advanced_billing = serde_json::json!({
        "long_context_tiers": [{
            "input_tokens_threshold": 128000,
            "input_unit_price": "0.3",
            "cached_input_unit_price": "0.15",
            "cache_write_unit_price": "0.6"
        }],
        "request_multipliers": [{
            "json_pointer": "/reasoning/effort",
            "value": "high",
            "multiplier": "2"
        }],
        "time_multipliers": [{
            "label": "Peak 1",
            "start_time": "01:00",
            "end_time": "04:00",
            "multiplier": "2"
        }, {
            "label": "Peak 2",
            "weekdays": ["monday", "tuesday", "wednesday", "thursday", "friday"],
            "start_time": "06:00",
            "end_time": "10:00",
            "multiplier": "2"
        }]
    });
    let created = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": "advanced-billing-model",
            "display_name": "Advanced billing model",
            "enabled": true,
            "price_unit_tokens": 1000000,
            "input_unit_price": "0.15",
            "cached_input_unit_price": "0.075",
            "cache_write_unit_price": "0.3",
            "output_unit_price": "0.6",
            "price_effective_at": chrono::Utc::now().to_rfc3339(),
            "advanced_billing": advanced_billing,
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_str().unwrap().to_owned();

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/models/{id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(
        body_json(detail).await["advanced_billing"],
        advanced_billing
    );

    let invalid = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": "invalid-advanced-billing-model",
            "display_name": "Invalid advanced billing model",
            "enabled": true,
            "price_unit_tokens": 1000000,
            "input_unit_price": "0",
            "cached_input_unit_price": "0",
            "cache_write_unit_price": "0",
            "output_unit_price": "0",
            "price_effective_at": chrono::Utc::now().to_rfc3339(),
            "advanced_billing": {
                "long_context_tiers": [{
                    "input_tokens_threshold": 0,
                    "input_unit_price": "0",
                    "cached_input_unit_price": "0",
                    "cache_write_unit_price": "0"
                }],
                "request_multipliers": []
            }
        }),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let overlapping = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": "overlapping-time-billing-model",
            "display_name": "Overlapping time billing model",
            "enabled": true,
            "price_unit_tokens": 1000000,
            "input_unit_price": "0",
            "cached_input_unit_price": "0",
            "cache_write_unit_price": "0",
            "output_unit_price": "0",
            "price_effective_at": chrono::Utc::now().to_rfc3339(),
            "advanced_billing": {
                "long_context_tiers": [],
                "request_multipliers": [],
                "time_multipliers": [{
                    "label": "Overnight",
                    "start_time": "22:00",
                    "end_time": "02:00",
                    "multiplier": "0.5"
                }, {
                    "label": "Overlap",
                    "start_time": "01:00",
                    "end_time": "03:00",
                    "multiplier": "2"
                }]
            }
        }),
        &[],
    )
    .await;
    assert_eq!(overlapping.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

/// API key creation uses the `sk-` prefix and the same value remains
/// retrievable from authorized list/detail endpoints.
#[tokio::test]
async fn api_key_create_returns_retrievable_prefixed_secret() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let group_id = topology.group.to_string();
    let create = request(
        &app,
        "POST",
        "/console/v1/api-keys",
        serde_json::json!({
            "user_id": app.user_id,
            "name": "spec-key",
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [group_id],
            "allowed_channel_ids": [],
        }),
        &[],
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let body = body_json(create).await;
    let secret = body["secret"].as_str().expect("secret present on create");
    assert!(secret.starts_with("sk-"));
    let id = body["id"].as_str().expect("id present").to_owned();

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/api-keys/{id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let has_etag = detail.headers().get(header::ETAG).is_some();
    let detail_body = body_json(detail).await;
    assert_eq!(detail_body["secret"], secret);
    assert!(has_etag, "detail returns an ETag");
    database.cleanup().await;
}

/// Channel and transform-template detail endpoints return the stored values
/// needed by the administrator edit forms, while list responses stay compact.
#[tokio::test]
async fn channel_and_template_details_return_stored_editable_values() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let mut invalid_create = capability_input(topology.channel, "responses");
    invalid_create["weight"] = serde_json::json!(1);

    let legacy_weight = request(
        &app,
        "POST",
        "/console/v1/routing/capabilities",
        invalid_create,
        &[],
    )
    .await;
    assert_eq!(legacy_weight.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let template_document = serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "request_headers": {"set": {"x-template-source": "spec-test"}}
    });
    let template = request(
        &app,
        "POST",
        "/console/v1/transforms/templates",
        serde_json::json!({
            "name": "editable-detail-template",
            "description": "detail contract",
            "document": template_document,
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(template.status(), StatusCode::CREATED);
    let template_id = body_json(template).await["id"].as_str().unwrap().to_owned();

    let upstream_api_key = "sk-upstream-detail-secret";
    let credential_id = Uuid::new_v4();
    upstream_credentials::insert(
        &database.pool,
        credential_id,
        "https://upstream.example.test",
        upstream_api_key,
    )
    .await;
    let logical_path = format!("/console/v1/routing/logical-channels/{}", topology.channel);
    let detail = request(&app, "GET", &logical_path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(request(&app, "PUT", &logical_path, serde_json::json!({
        "group_id": topology.group, "access_id": topology.access, "credential_id": credential_id,
        "name": "Bound channel", "enabled": true
    }), &[("if-match", &etag)]).await.status(), StatusCode::OK);
    let override_document = serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "request_headers": {"set": {"x-channel-source": "spec-test"}}
    });
    let channel_id = topology.capability.to_string();
    let path = format!("/console/v1/routing/capabilities/{channel_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut input = capability_input(topology.channel, "chat_completion");
    input["billing_multiplier"] = serde_json::json!("1.5");
    input["config_template_id"] = serde_json::json!(template_id);
    input["override_document"] = override_document.clone();
    input["settings"]["available_models"] = serde_json::json!(["editable-detail-model"]);
    let channel = request(&app, "PUT", &path, input.clone(), &[("if-match", &etag)]).await;
    assert_eq!(channel.status(), StatusCode::OK);

    let channel_list = request(
        &app,
        "GET",
        "/console/v1/routing/capabilities",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(channel_list.status(), StatusCode::OK);
    let channel_list = body_json(channel_list).await;
    let channel_list_item = channel_list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == channel_id)
        .unwrap();
    assert_eq!(channel_list_item["override_document"], override_document);
    assert!(channel_list_item.get("upstream_api_key").is_none());
    assert!(channel_list_item.get("weight").is_none());
    assert_eq!(channel_list_item["billing_multiplier"], "1.500000000000");

    let channel_detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    assert_eq!(channel_detail.status(), StatusCode::OK);
    let channel_etag = channel_detail
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let channel_detail = body_json(channel_detail).await;
    assert_eq!(channel_detail["override_document"], override_document);
    assert_eq!(
        channel_detail["channel_id"],
        serde_json::json!(topology.channel)
    );
    let logical =
        body_json(request(&app, "GET", &logical_path, serde_json::json!({}), &[]).await).await;
    assert_eq!(logical["credential_id"], serde_json::json!(credential_id));
    assert!(channel_detail.get("upstream_api_key").is_none());
    assert!(channel_detail.get("weight").is_none());
    assert_eq!(channel_detail["billing_multiplier"], "1.500000000000");

    input["weight"] = serde_json::json!(1);
    let legacy_weight_update =
        request(&app, "PUT", &path, input, &[("if-match", &channel_etag)]).await;
    assert_eq!(
        legacy_weight_update.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let template_list = request(
        &app,
        "GET",
        "/console/v1/transforms/templates",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(template_list.status(), StatusCode::OK);
    let template_list = body_json(template_list).await;
    let template_list_item = template_list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == template_id)
        .unwrap();
    assert!(template_list_item.get("document").is_none());

    let template_detail = request(
        &app,
        "GET",
        &format!("/console/v1/transforms/templates/{template_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(template_detail.status(), StatusCode::OK);
    assert!(template_detail.headers().get(header::ETAG).is_some());
    assert_eq!(
        body_json(template_detail).await["document"],
        template_document
    );

    database.cleanup().await;
}

#[tokio::test]
async fn responses_websocket_and_search_are_independent_capabilities() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "responses").await;
    let path = format!("/console/v1/routing/capabilities/{}", topology.capability);
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail = body_json(detail).await;
    assert!(detail["settings"].get("transports").is_none());
    let mut input = capability_input(topology.channel, "responses");
    input["settings"]["transports"] = serde_json::json!(["http_json", "http_sse", "websocket"]);
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let websocket = create_resource(
        &app,
        "/console/v1/routing/capabilities",
        capability_input(topology.channel, "responses-ws"),
    )
    .await;
    assert_ne!(websocket, topology.capability);
    let search = create_resource(
        &app,
        "/console/v1/routing/capabilities",
        capability_input(topology.channel, "web_search"),
    )
    .await;
    assert_ne!(search, topology.capability);
    let capabilities = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/capabilities",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(capabilities.as_array().unwrap().len(), 3);
    assert!(
        capabilities
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == search.to_string()
                && item["settings"]["operation"] == "web_search"
                && item["settings"].get("transports").is_none())
    );
    for fault in ["websocket", "legacy_search_flag"] {
        let mut input = capability_input(topology.channel, "chat_completion");
        if fault == "websocket" {
            input["settings"]["transports"] = serde_json::json!(["websocket"]);
        } else {
            input["supports_standalone_web_search"] = serde_json::json!(true);
        }
        assert_eq!(
            request(&app, "POST", "/console/v1/routing/capabilities", input, &[])
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM api_key_capability_grants")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(grants, 0);
    database.cleanup().await;
}

#[tokio::test]
async fn images_control_plane_rejects_scheduled_probes_and_sse_transforms() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "images_generation").await;
    let model = create_test_pricing_model(&app).await;
    let path = format!("/console/v1/routing/capabilities/{}", topology.capability);
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let before = body_json(detail).await;
    let mut input = capability_input(topology.channel, "images_generation");
    input["settings"]["test_model"] = serde_json::json!("wire-v1");
    input["settings"]["test_pricing_model_id"] = serde_json::json!(model);
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        body_json(request(&app, "GET", &path, serde_json::json!({}), &[]).await).await,
        before
    );
    let document = serde_json::json!({
        "version": 1, "api_format": "open_ai_images",
        "sse": [{"event": "image_generation.partial_image", "json": []}]
    });
    let response = request(
        &app,
        "POST",
        "/console/v1/transforms/templates",
        serde_json::json!({
            "name": "invalid-images-sse", "document": document, "enabled": true
        }),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let mut input = capability_input(topology.channel, "images_generation");
    input["override_document"] = document;
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    database.cleanup().await;
}

/// Draft channel model discovery is admin-only, does not persist changes, and
/// returns unique IDs from an OpenAI-compatible `GET /v1/models` response.
#[tokio::test]
async fn channel_model_discovery_uses_draft_network_and_auth_settings() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/v1/models", get(upstream_models)),
        )
        .await
        .unwrap();
    });
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let credential_id = Uuid::new_v4();
    upstream_credentials::insert(
        &database.pool,
        credential_id,
        &format!("http://{address}"),
        "spec-model-discovery-secret",
    )
    .await;

    let response = request(
        &app,
        "POST",
        "/console/v1/routing/channels/models/discover",
        serde_json::json!({
            "api_format": "open_ai_chat_completions",
            "base_url": format!("http://{address}"),
            "override_document": {
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {
                    "set": {"x-model-discovery": "console-spec"}
                }
            },
            "credential_id": credential_id
        }),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        body_json(response).await,
        serde_json::json!({"models": ["model-z", "model-a"]})
    );

    upstream.abort();
    database.cleanup().await;
}

#[tokio::test]
async fn channel_batch_updates_are_atomic_versioned_and_published_once() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let first = seed_test_topology(&app, "chat_completion").await;
    let second = seed_test_topology(&app, "chat_completion").await;
    let channel_ids = vec![first.capability.to_string(), second.capability.to_string()];

    let before = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/capabilities",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    let before_items = channel_ids
        .iter()
        .map(|id| {
            let channel = before
                .as_array()
                .unwrap()
                .iter()
                .find(|channel| channel["id"] == *id)
                .unwrap();
            serde_json::json!({
                "id": id,
                "updated_at": channel["updated_at"],
            })
        })
        .collect::<Vec<_>>();

    let legacy_weight = request(
        &app,
        "POST",
        "/console/v1/routing/capabilities/batch",
        serde_json::json!({
            "items": before_items.clone(),
            "changes": {"weight": 7}
        }),
        &[],
    )
    .await;
    assert_eq!(legacy_weight.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let updated = request(
        &app,
        "POST",
        "/console/v1/routing/capabilities/batch",
        serde_json::json!({
            "items": before_items,
            "changes": {
                "auto_disable_allowed": true,
                "billing_multiplier": "2.5"
            }
        }),
        &[],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated = body_json(updated).await;
    assert_eq!(updated["updated_ids"].as_array().unwrap().len(), 2);
    assert!(updated["correlation_id"].is_string());

    let current = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/capabilities",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    for id in &channel_ids {
        let channel = current
            .as_array()
            .unwrap()
            .iter()
            .find(|channel| channel["id"] == *id)
            .unwrap();
        assert!(channel.get("weight").is_none());
        assert_eq!(channel["billing_multiplier"], "2.500000000000");
        assert_eq!(channel["settings"]["auto_disable_allowed"], true);
        let compiled = app
            .runtime
            .snapshot()
            .channel(Uuid::parse_str(id).unwrap())
            .unwrap();
        assert_eq!(
            compiled.billing_multiplier(),
            rust_decimal::Decimal::new(25, 1)
        );
    }
    let audit_facts: (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT correlation_id) \
         FROM audit_logs WHERE action='batch_update' AND object_id = ANY($1)",
    )
    .bind(
        channel_ids
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>(),
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit_facts, (2, 1));

    let current_items = channel_ids
        .iter()
        .map(|id| {
            let channel = current
                .as_array()
                .unwrap()
                .iter()
                .find(|channel| channel["id"] == *id)
                .unwrap();
            serde_json::json!({
                "id": id,
                "updated_at": channel["updated_at"],
            })
        })
        .collect::<Vec<_>>();
    let stale_second_version = before
        .as_array()
        .unwrap()
        .iter()
        .find(|channel| channel["id"] == channel_ids[1])
        .unwrap()["updated_at"]
        .clone();
    let conflict = request(
        &app,
        "POST",
        "/console/v1/routing/capabilities/batch",
        serde_json::json!({
            "items": [
                current_items[0].clone(),
                {
                    "id": channel_ids[1],
                    "updated_at": stale_second_version
                }
            ],
            "changes": {"billing_multiplier": "9"}
        }),
        &[],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let persisted_multipliers: Vec<rust_decimal::Decimal> = sqlx::query_scalar(
        "SELECT billing_multiplier FROM channel_capabilities WHERE id = ANY($1) ORDER BY id",
    )
    .bind(
        channel_ids
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        persisted_multipliers,
        vec![
            rust_decimal::Decimal::new(25, 1),
            rust_decimal::Decimal::new(25, 1)
        ]
    );
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs \
         WHERE action='batch_update' AND object_id = ANY($1)",
    )
    .bind(
        channel_ids
            .iter()
            .map(|id| Uuid::parse_str(id).unwrap())
            .collect::<Vec<_>>(),
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 2);

    database.cleanup().await;
}

#[tokio::test]
async fn administrator_can_manually_recover_an_auto_disabled_channel() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let channel_id = topology.capability;

    sqlx::query(
        "UPDATE channel_capabilities
         SET auto_disabled=true, auto_disable_reason='test automatic disable'
         WHERE id=$1",
    )
    .bind(channel_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let channels = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/capabilities",
            serde_json::json!({}),
            &[],
        )
        .await,
    )
    .await;
    let channel = channels
        .as_array()
        .unwrap()
        .iter()
        .find(|channel| channel["id"] == channel_id.to_string())
        .unwrap();
    assert_eq!(channel["auto_disabled"], true);
    let etag = format!("\"{}\"", channel["updated_at"].as_str().unwrap());

    let recovered = request(
        &app,
        "POST",
        &format!("/console/v1/routing/capabilities/{channel_id}/recover"),
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(recovered.status(), StatusCode::OK);

    let state: (bool, Option<String>) = sqlx::query_as(
        "SELECT auto_disabled,auto_disable_reason FROM channel_capabilities WHERE id=$1",
    )
    .bind(channel_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(state, (false, None));
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs
         WHERE action='manual_recover' AND object_id=$1",
    )
    .bind(channel_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);

    let stale = request(
        &app,
        "POST",
        &format!("/console/v1/routing/capabilities/{channel_id}/recover"),
        serde_json::json!({}),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    database.cleanup().await;
}

#[tokio::test]
async fn api_key_policy_only_stores_selectable_targets() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let channel_id = topology.channel.to_string();
    let created = request(
        &app,
        "POST",
        "/console/v1/api-key-policies",
        serde_json::json!({
            "name": "channel-only-policy",
            "allowed_group_ids": [],
            "allowed_channel_ids": [channel_id],
            "enabled": true
        }),
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_str().unwrap().to_owned();
    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/api-key-policies/{id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = body_json(detail).await;
    assert_eq!(detail["allowed_group_ids"], serde_json::json!([]));
    assert_eq!(
        detail["allowed_channel_ids"],
        serde_json::json!([channel_id])
    );
    for removed in [
        "allowed_api_formats",
        "permissions",
        "requests_per_minute",
        "max_concurrent_requests",
        "quota_limit_amount",
        "max_active_keys",
    ] {
        assert!(
            detail.get(removed).is_none(),
            "{removed} is no longer a policy field"
        );
    }
    database.cleanup().await;
}

/// Self-service key creation reports actionable policy precondition codes.
#[tokio::test]
async fn self_api_key_create_reports_policy_preconditions() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let group_id = topology.group.to_string();
    let channel_id = topology.channel.to_string();
    let key_input = |name: &str| {
        serde_json::json!({
            "name": name,
            "allowed_group_ids": [group_id],
            "allowed_channel_ids": [],
            "requests_per_minute": 30,
            "max_concurrent_requests": 2,
            "quota_limit_amount": "5.00"
        })
    };

    let missing = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        key_input("missing-policy"),
        &[],
    )
    .await;
    assert_eq!(missing.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(missing).await,
        serde_json::json!({"error": "default_api_key_policy_required"})
    );

    let policy_id = create_resource(
        &app,
        "/console/v1/api-key-policies",
        serde_json::json!({
            "name": "spec-policy", "allowed_group_ids": [group_id],
            "allowed_channel_ids": [], "enabled": false
        }),
    )
    .await;
    sqlx::query("UPDATE users SET default_api_key_policy_id=$2 WHERE id=$1")
        .bind(app.user_id)
        .bind(policy_id)
        .execute(&database.pool)
        .await
        .unwrap();

    let disabled = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        key_input("disabled-policy"),
        &[],
    )
    .await;
    assert_eq!(disabled.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(disabled).await,
        serde_json::json!({"error": "default_api_key_policy_disabled"})
    );

    sqlx::query("UPDATE api_key_policies SET enabled=true WHERE id=$1")
        .bind(policy_id)
        .execute(&database.pool)
        .await
        .unwrap();
    let options = request(
        &app,
        "GET",
        "/console/v1/me/api-key-options",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(options.status(), StatusCode::OK);
    let options = body_json(options).await;
    assert_eq!(options["policy_id"], policy_id.to_string());
    assert_eq!(options["groups"][0]["id"], group_id);
    assert!(options["groups"][0].get("priority").is_none());
    assert_eq!(options["channels"][0]["id"], channel_id);
    assert_eq!(options["channels"][0]["channel_group_enabled"], true);

    let other_group_id = seed_test_topology(&app, "chat_completion")
        .await
        .group
        .to_string();
    let denied = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name": "outside-policy",
            "allowed_group_ids": [other_group_id],
            "allowed_channel_ids": [],
            "requests_per_minute": null,
            "max_concurrent_requests": null,
            "quota_limit_amount": null
        }),
        &[],
    )
    .await;
    assert_eq!(denied.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(denied).await,
        serde_json::json!({"error": "api_key_target_not_allowed"})
    );

    let expiry = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let created = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        {
            let mut input = key_input("first-key");
            input["expires_at"] = serde_json::json!(expiry.clone());
            input
        },
        &[],
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = body_json(created).await;
    let id = created["id"].as_str().expect("created key id");
    let secret = created["secret"].as_str().expect("created key secret");
    assert!(secret.starts_with("sk-"));

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/me/api-keys/{id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()[header::ETAG].to_str().unwrap().to_owned();
    let detail = body_json(detail).await;
    assert_eq!(detail["secret"], secret);
    assert_eq!(detail["allowed_group_ids"], serde_json::json!([group_id]));
    assert_eq!(detail["allowed_channel_ids"], serde_json::json!([]));
    assert_eq!(
        detail["allowed_api_formats"],
        serde_json::json!(["open_ai_chat_completions"])
    );
    assert_eq!(
        detail["permissions"],
        serde_json::json!(["proxy", "models.read"])
    );
    assert_eq!(detail["requests_per_minute"], 30);
    assert_eq!(detail["max_concurrent_requests"], 2);
    assert_eq!(detail["quota_limit_amount"], "5.00000000");

    let updated = request(
        &app,
        "PUT",
        &format!("/console/v1/me/api-keys/{id}"),
        serde_json::json!({
            "name": "first-key-updated",
            "status": "active",
            "expires_at": expiry,
            "allowed_group_ids": [],
            "allowed_channel_ids": [channel_id],
            "requests_per_minute": 40,
            "max_concurrent_requests": 3,
            "quota_limit_amount": "8.50"
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/me/api-keys/{id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = body_json(detail).await;
    assert_eq!(detail["allowed_group_ids"], serde_json::json!([]));
    assert_eq!(
        detail["allowed_channel_ids"],
        serde_json::json!([channel_id])
    );
    assert_eq!(detail["requests_per_minute"], 40);
    assert_eq!(detail["max_concurrent_requests"], 3);
    assert_eq!(detail["quota_limit_amount"], "8.50000000");

    let second = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        key_input("second-key"),
        &[],
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    database.cleanup().await;
}

/// `limit` query is honored on log endpoints (bounded result, non-error).
#[tokio::test]
async fn request_logs_limit_query_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let response = request(
        &app,
        "GET",
        "/console/v1/request-logs?limit=10",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap()),
        Some("no-store"),
        "API responses are no-store per spec"
    );
    let body = body_json(response).await;
    assert!(body.is_array(), "request logs endpoint returns an array");
    database.cleanup().await;
}

/// An out-of-range `limit` is clamped rather than rejected (the spec allows
/// 1..=100 and documents clamping).
#[tokio::test]
async fn request_logs_limit_is_clamped() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let response = request(
        &app,
        "GET",
        "/console/v1/request-logs?limit=99999",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body.is_array());
    database.cleanup().await;
}

/// Request-log filters are server-side, composable, and reject unsupported
/// enum values rather than silently widening an administrator's query.
#[tokio::test]
async fn request_log_filters_match_the_console_contract() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let api_key_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,$3,$4,'active',ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'])",
    )
    .bind(api_key_id)
    .bind(app.user_id)
    .bind("filter-test-key")
    .bind(format!("filter-secret-{api_key_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let now = chrono::Utc::now();
    let matching_log_id = Uuid::new_v4();
    for (id, client_model, outcome, error_code, error_summary) in [
        (
            matching_log_id,
            "filter-model",
            "failed",
            Some("provider_error"),
            Some("upstream quota exhausted"),
        ),
        (Uuid::new_v4(), "other-model", "succeeded", None, None),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,upstream_model,outcome,streamed,ttft_ms,total_duration_ms,output_tokens_per_second,reasoning_effort,fast_mode,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,cost_amount,error_code,error_summary,peak_pricing) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','chat_completions',$5,$6,$7,false,100,1000,5.5556,'high',true,12,2,1,5,1,CASE WHEN $7='failed' THEN 0 ELSE NULL END,$8,$9,$10)",
        )
        .bind(id)
        .bind(now)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(client_model)
        .bind("filter-upstream")
        .bind(outcome)
        .bind(error_code)
        .bind(error_summary)
        .bind(id == matching_log_id)
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let response = request(
        &app,
        "GET",
        "/console/v1/request-logs?model=filter-model&api_format=open_ai_chat_completions&outcome=failed&billed=false&limit=25",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], matching_log_id.to_string());
    assert_eq!(body[0]["user_name"], format!("spec-{}", app.user_id));
    assert_eq!(body[0]["api_operation"], "chat_completion");
    assert_eq!(body[0]["request_protocol"], "non_stream");
    assert_eq!(body[0]["reasoning_effort"], "high");
    assert_eq!(body[0]["fast_mode"], true);
    assert_eq!(body[0]["peak_pricing"], true);
    assert_eq!(body[0]["ttft_ms"], 100);
    assert_eq!(body[0]["total_duration_ms"], 1000);
    assert_eq!(body[0]["output_tokens_per_second"], "5.5556");
    assert_eq!(body[0]["input_tokens"], 12);
    assert_eq!(body[0]["cached_input_tokens"], 2);
    assert_eq!(body[0]["output_tokens"], 5);
    assert_eq!(body[0]["reasoning_tokens"], 1);
    assert_eq!(body[0]["error_code"], "provider_error");
    assert_eq!(body[0]["error_summary"], "upstream quota exhausted");

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/request-logs/{matching_log_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = body_json(detail).await;
    assert_eq!(detail["id"], matching_log_id.to_string());
    assert_eq!(detail["user_name"], format!("spec-{}", app.user_id));
    assert_eq!(detail["api_operation"], "chat_completion");
    assert_eq!(detail["request_protocol"], "non_stream");
    assert_eq!(detail["reasoning_effort"], "high");
    assert_eq!(detail["fast_mode"], true);
    assert_eq!(detail["peak_pricing"], true);
    assert_eq!(detail["output_tokens_per_second"], "5.5556");
    assert_eq!(detail["reasoning_tokens"], 1);
    assert_eq!(detail["error_code"], "provider_error");
    assert_eq!(detail["error_summary"], "upstream quota exhausted");

    let image_log_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,outcome) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_images','images_generation','gpt-image-2','succeeded')",
    )
    .bind(image_log_id)
    .bind(now)
    .bind(app.user_id)
    .bind(api_key_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let images = request(
        &app,
        "GET",
        "/console/v1/request-logs?api_format=open_ai_images",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(images.status(), StatusCode::OK);
    let images = body_json(images).await;
    assert_eq!(images.as_array().unwrap().len(), 1);
    assert_eq!(images[0]["id"], image_log_id.to_string());
    assert_eq!(images[0]["api_operation"], "images_generation");

    let standalone_search_log_id = Uuid::new_v4();
    for (id, operation) in [
        (Uuid::new_v4(), "responses"),
        (standalone_search_log_id, "web_search"),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,outcome) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_responses',$5,'operation-filter-model','succeeded')",
        )
        .bind(id)
        .bind(now)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(operation)
        .execute(&database.pool)
        .await
        .unwrap();
    }
    let operation_filtered = request(
        &app,
        "GET",
        "/console/v1/request-logs?api_operation=web_search",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(operation_filtered.status(), StatusCode::OK);
    let operation_filtered = body_json(operation_filtered).await;
    assert_eq!(operation_filtered.as_array().unwrap().len(), 1);
    assert_eq!(
        operation_filtered[0]["id"],
        standalone_search_log_id.to_string()
    );

    let mismatched_operation = sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model,outcome) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_images','responses','invalid-images-log','failed')",
    )
    .bind(Uuid::new_v4())
    .bind(now)
    .bind(app.user_id)
    .bind(api_key_id)
    .execute(&database.pool)
    .await;
    assert!(mismatched_operation.is_err());
    let legacy_log_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,client_model,outcome) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_responses','legacy-response','succeeded')",
    )
    .bind(legacy_log_id)
    .bind(now)
    .bind(app.user_id)
    .bind(api_key_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let inferred_operation: String =
        sqlx::query_scalar("SELECT api_operation FROM request_logs WHERE id=$1")
            .bind(legacy_log_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(inferred_operation, "responses");

    let missing = request(
        &app,
        "GET",
        &format!("/console/v1/request-logs/{}", Uuid::new_v4()),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let invalid = request(
        &app,
        "GET",
        "/console/v1/request-logs?api_format=not-a-format",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_operation = request(
        &app,
        "GET",
        "/console/v1/request-logs?api_operation=not-an-operation",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(invalid_operation.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

#[tokio::test]
async fn statistics_endpoints_aggregate_channel_group_status_and_costs() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let topology = seed_test_topology(&app, "chat_completion").await;
    let group_id = topology.group;
    let channel_id = topology.capability;
    let api_key_id = Uuid::new_v4();
    let misplaced_monitoring = request(
        &app,
        "POST",
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "misplaced-statistics",
            "enabled": true,
            "status_statistics_enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(
        misplaced_monitoring.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let path = format!("/console/v1/routing/capabilities/{channel_id}");
    let detail = request(&app, "GET", &path, serde_json::json!({}), &[]).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut input = capability_input(topology.channel, "chat_completion");
    input["settings"]["available_models"] = serde_json::json!(["statistics-model"]);
    input["status_statistics_enabled"] = serde_json::json!(true);
    assert_eq!(
        request(&app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::OK
    );
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,$3,$4,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'])",
    )
    .bind(api_key_id)
    .bind(app.user_id)
    .bind(format!("statistics-key-{api_key_id}"))
    .bind(format!("statistics-secret-{api_key_id}"))
    .execute(&database.pool)
    .await
    .unwrap();

    let started_at = chrono::Utc::now() - chrono::Duration::minutes(10);
    for (
        outcome,
        status,
        ttft_ms,
        tps,
        input_tokens,
        cached_input_tokens,
        cache_write_tokens,
        output_tokens,
        cost,
    ) in [
        (
            "succeeded",
            200_i16,
            Some(500_i32),
            Some(rust_decimal::Decimal::new(200, 1)),
            100_i64,
            20_i64,
            5_i64,
            50_i64,
            Some(rust_decimal::Decimal::new(25, 2)),
        ),
        (
            "failed",
            500_i16,
            None,
            None,
            10_i64,
            2_i64,
            1_i64,
            0_i64,
            Some(rust_decimal::Decimal::ZERO),
        ),
        (
            "cancelled",
            200_i16,
            None,
            None,
            0_i64,
            0_i64,
            0_i64,
            0_i64,
            Some(rust_decimal::Decimal::ZERO),
        ),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model, \
              upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
              streamed,ttft_ms,total_duration_ms,output_tokens_per_second,input_tokens, \
              cached_input_tokens,cache_write_tokens,output_tokens,currency,price_unit_tokens, \
              price_effective_at,input_unit_price,cached_input_unit_price, \
              cache_write_unit_price,output_unit_price,cost_amount) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','chat_completions','statistics-client-model', \
                     'statistics-model',$5,$6,$7,$8,false,$9,1000,$10,$11,$12,$13,$14, \
                     'USD',1000000,$2,1,0,0,1,$15)",
        )
        .bind(Uuid::new_v4())
        .bind(started_at)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(group_id)
        .bind(channel_id)
        .bind(outcome)
        .bind(status)
        .bind(ttft_ms)
        .bind(tps)
        .bind(input_tokens)
        .bind(cached_input_tokens)
        .bind(cache_write_tokens)
        .bind(output_tokens)
        .bind(cost)
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let scheduled_test_at = started_at - chrono::Duration::days(2);
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,api_operation,client_model, \
          upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
          streamed,ttft_ms,total_duration_ms,input_tokens,cached_input_tokens, \
          cache_write_tokens,output_tokens,currency,price_unit_tokens,price_effective_at, \
          input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
          cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'scheduled_test','open_ai_chat_completions','chat_completions', \
                 'statistics-scheduled-client-model','statistics-model',$5,$6,'succeeded',200, \
                 false,100,200,1,0,0,1,'USD',1000000,$2,1,0,0,1,0.01)",
    )
    .bind(Uuid::new_v4())
    .bind(scheduled_test_at)
    .bind(app.user_id)
    .bind(api_key_id)
    .bind(group_id)
    .bind(channel_id)
    .execute(&database.pool)
    .await
    .unwrap();

    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let channel_detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/capabilities/{channel_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(channel_detail.status(), StatusCode::OK);
    assert_eq!(
        body_json(channel_detail).await["status_statistics_enabled"],
        true
    );
    let group_detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/groups/{group_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(group_detail.status(), StatusCode::OK);
    let group_detail = body_json(group_detail).await;
    assert!(group_detail.get("status_statistics_enabled").is_none());

    let status = request(
        &app,
        "GET",
        "/console/v1/statistics/channel-group-status?window=24h",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    let status = body_json(status).await;
    assert_eq!(status["window"], "24h");
    assert_eq!(status["models"][0]["model"], "statistics-model");
    assert_eq!(status["models"][0]["request_count"], 3);
    assert_eq!(status["models"][0]["success_rate"], 0.5);
    assert_eq!(status["models"][0]["p90_ttft_ms"], 500.0);
    assert_eq!(status["models"][0]["p50_tps"], 20.0);
    assert_eq!(status["groups"][0]["id"], group_id.to_string());
    assert!(status["groups"][0]["models"][0]["history"].is_array());

    let range_start = (started_at - chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let range_end = (started_at + chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={}&api_key_id={api_key_id}",
            app.user_id
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(costs.status(), StatusCode::OK);
    let costs = body_json(costs).await;
    assert_eq!(costs["granularity"], "hour");
    assert_eq!(costs["summary"]["request_count"], 3);
    assert_eq!(costs["summary"]["priced_request_count"], 3);
    assert_eq!(costs["summary"]["total_tokens"], 160);
    assert_eq!(costs["summary"]["input_tokens"], 110);
    assert_eq!(costs["summary"]["cached_input_tokens"], 22);
    assert_eq!(costs["summary"]["cache_write_tokens"], 6);
    assert_eq!(costs["summary"]["output_tokens"], 50);
    let amount = costs["summary"]["cost_amount"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((amount - 0.25).abs() < f64::EPSILON);
    assert_eq!(costs["models"][0]["model"], "statistics-model");
    assert_eq!(costs["models"][0]["input_tokens"], 110);
    assert_eq!(costs["models"][0]["cached_input_tokens"], 22);
    assert_eq!(costs["models"][0]["cache_write_tokens"], 6);
    assert_eq!(costs["models"][0]["output_tokens"], 50);
    assert_eq!(costs["models"][0]["success_rate"], 0.5);
    assert_eq!(costs["channels"][0]["id"], channel_id.to_string());
    assert_eq!(
        costs["channels"][0]["channel_group_name"],
        group_detail["name"]
    );
    assert_eq!(costs["channels"][0]["name"], "Spec channel");
    assert_eq!(costs["channels"][0]["request_count"], 3);
    assert_eq!(costs["channels"][0]["success_rate"], 0.5);
    assert!(
        costs["buckets"].as_array().unwrap().len() >= 2,
        "the full selected timeline should include empty UTC buckets"
    );
    assert_eq!(
        costs["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|bucket| bucket["request_count"].as_i64().unwrap() > 0)
            .count(),
        1
    );

    let admin_own_costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&api_key_id={api_key_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_own_costs.status(), StatusCode::OK);
    let admin_own_costs = body_json(admin_own_costs).await;
    assert_eq!(admin_own_costs["summary"]["request_count"], 3);
    assert_eq!(admin_own_costs["channels"], serde_json::json!([]));

    let admin_usage = request(
        &app,
        "GET",
        "/console/v1/me/usage",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_usage.status(), StatusCode::OK);
    let admin_usage = body_json(admin_usage).await;
    assert_eq!(admin_usage["total_request_count"], 3);
    assert_eq!(admin_usage["active_day_count"], 1);
    assert_eq!(admin_usage["days"].as_array().unwrap().len(), 365);
    assert_eq!(
        admin_usage["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|day| day["request_count"].as_i64().unwrap())
            .sum::<i64>(),
        3
    );

    let regular_user_id = Uuid::new_v4();
    let regular_api_key_id = Uuid::new_v4();
    let regular_email = format!("statistics-user-{regular_user_id}@example.test");
    let regular_display_name = format!("statistics-user-{regular_user_id}");
    let regular_password_hash = hash_console_password(TEST_PASSWORD.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id,email,display_name,role,status,password_hash) \
         VALUES ($1,$2,$3,'user','active',$4)",
    )
    .bind(regular_user_id)
    .bind(&regular_email)
    .bind(&regular_display_name)
    .bind(regular_password_hash)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,$3,$4,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'])",
    )
    .bind(regular_api_key_id)
    .bind(regular_user_id)
    .bind(format!("statistics-user-key-{regular_api_key_id}"))
    .bind(format!("statistics-user-secret-{regular_api_key_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let regular_request_log_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model, \
          upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
          streamed,ttft_ms,total_duration_ms,output_tokens_per_second,input_tokens, \
          cached_input_tokens,cache_write_tokens,output_tokens,currency,price_unit_tokens, \
          price_effective_at,input_unit_price,cached_input_unit_price, \
          cache_write_unit_price,output_unit_price,cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','chat_completions','statistics-user-client-model', \
                 'statistics-user-model',$5,$6,'succeeded',200,false,250,500,25,40,0,0,10, \
                 'USD',1000000,$2,1,0,0,1,1)",
    )
    .bind(regular_request_log_id)
    .bind(started_at)
    .bind(regular_user_id)
    .bind(regular_api_key_id)
    .bind(group_id)
    .bind(channel_id)
    .execute(&database.pool)
    .await
    .unwrap();
    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let regular_session = app
        .auth
        .login(regular_email.clone(), TEST_PASSWORD.to_owned())
        .await
        .unwrap();

    let admin_logs = request(
        &app,
        "GET",
        &format!("/console/v1/request-logs?api_key_id={regular_api_key_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_logs.status(), StatusCode::OK);
    let admin_logs = body_json(admin_logs).await;
    assert_eq!(admin_logs[0]["channel_group_name"], group_detail["name"]);
    assert_eq!(admin_logs[0]["channel_id"], channel_id.to_string());
    assert_eq!(admin_logs[0]["channel_name"], "Spec channel");
    assert_eq!(admin_logs[0]["user_name"], regular_display_name);
    assert_eq!(admin_logs[0]["request_protocol"], "non_stream");

    let admin_own_logs = request(
        &app,
        "GET",
        &format!("/console/v1/me/request-logs?api_key_id={api_key_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_own_logs.status(), StatusCode::OK);
    let admin_own_logs = body_json(admin_own_logs).await;
    assert!(admin_own_logs[0]["user_name"].is_null());
    assert!(admin_own_logs[0]["channel_id"].is_null());
    assert!(admin_own_logs[0]["channel_name"].is_null());
    assert_eq!(admin_own_logs[0]["request_protocol"], "non_stream");
    let admin_own_log_id = admin_own_logs[0]["id"].as_str().unwrap().to_owned();
    let admin_own_log = request(
        &app,
        "GET",
        &format!("/console/v1/me/request-logs/{admin_own_log_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_own_log.status(), StatusCode::OK);
    let admin_own_log = body_json(admin_own_log).await;
    assert!(admin_own_log["user_name"].is_null());
    assert!(admin_own_log["channel_id"].is_null());
    assert!(admin_own_log["channel_name"].is_null());
    assert_eq!(admin_own_log["request_protocol"], "non_stream");

    let user_logs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/me/request-logs",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_logs.status(), StatusCode::OK);
    let user_logs = body_json(user_logs).await;
    assert_eq!(user_logs[0]["channel_group_name"], group_detail["name"]);
    assert!(user_logs[0]["user_name"].is_null());
    assert!(user_logs[0]["channel_id"].is_null());
    assert!(user_logs[0]["channel_name"].is_null());
    assert_eq!(user_logs[0]["request_protocol"], "non_stream");

    let user_log = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!("/console/v1/me/request-logs/{regular_request_log_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_log.status(), StatusCode::OK);
    let user_log = body_json(user_log).await;
    assert!(user_log["user_name"].is_null());
    assert!(user_log["channel_id"].is_null());
    assert!(user_log["channel_name"].is_null());
    assert_eq!(user_log["request_protocol"], "non_stream");

    let other_user_log = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!("/console/v1/me/request-logs/{admin_own_log_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(other_user_log.status(), StatusCode::NOT_FOUND);

    let global_logs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/request-logs",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(global_logs.status(), StatusCode::FORBIDDEN);

    let user_channel_status = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/statistics/channel-group-status?window=24h",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_channel_status.status(), StatusCode::OK);

    let user_usage = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/me/usage",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_usage.status(), StatusCode::OK);
    let user_usage = body_json(user_usage).await;
    assert_eq!(user_usage["total_request_count"], 1);
    assert_eq!(user_usage["active_day_count"], 1);
    assert_eq!(user_usage["days"].as_array().unwrap().len(), 365);

    let user_costs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_costs.status(), StatusCode::OK);
    let user_costs = body_json(user_costs).await;
    assert_eq!(user_costs["summary"]["request_count"], 1);
    let user_cost_amount = user_costs["summary"]["cost_amount"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((user_cost_amount - 1.0).abs() < f64::EPSILON);
    assert_eq!(user_costs["models"][0]["model"], "statistics-user-model");
    assert_eq!(user_costs["channels"], serde_json::json!([]));

    let user_channel_filter = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&channel_id={channel_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        user_channel_filter.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let other_user_costs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={}",
            app.user_id
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(other_user_costs.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let other_user_key_costs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&api_key_id={api_key_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(other_user_key_costs.status(), StatusCode::OK);
    assert_eq!(
        body_json(other_user_key_costs).await["summary"]["request_count"],
        0
    );

    let user_system_load = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/system/load",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_system_load.status(), StatusCode::FORBIDDEN);
    for (method, path) in [
        ("GET", "/console/v1/routing/upstream-credentials"),
        ("POST", "/console/v1/routing/upstream-credentials"),
        (
            "GET",
            "/console/v1/routing/upstream-credentials/00000000-0000-0000-0000-000000000001",
        ),
        (
            "PUT",
            "/console/v1/routing/upstream-credentials/00000000-0000-0000-0000-000000000001",
        ),
        (
            "DELETE",
            "/console/v1/routing/upstream-credentials/00000000-0000-0000-0000-000000000001",
        ),
    ] {
        assert_eq!(
            request_with_token(
                &app,
                &regular_session.access_token,
                method,
                path,
                serde_json::json!({}),
                &[]
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }

    let user_system_costs = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_system_costs.status(), StatusCode::FORBIDDEN);

    let admin_personal_user_filter = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={regular_user_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        admin_personal_user_filter.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let admin_user_costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={regular_user_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_user_costs.status(), StatusCode::OK);
    assert_eq!(
        body_json(admin_user_costs).await["summary"]["request_count"],
        1
    );

    let admin_channel_costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&channel_id={channel_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(admin_channel_costs.status(), StatusCode::OK);
    let admin_channel_costs = body_json(admin_channel_costs).await;
    assert_eq!(admin_channel_costs["summary"]["request_count"], 4);
    assert_eq!(admin_channel_costs["channels"].as_array().unwrap().len(), 1);

    RequestLogRepository::new(database.pool.clone())
        .queries()
        .metering()
        .refresh_spend_leaderboard_snapshots()
        .await
        .unwrap();

    let leaderboard = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day&limit=1",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(leaderboard.status(), StatusCode::OK);
    let leaderboard = body_json(leaderboard).await;
    assert_eq!(leaderboard["period"], "day");
    assert!(leaderboard["refreshed_at"].is_string());
    let leaderboard_total = leaderboard["total_cost_amount"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((leaderboard_total - 1.25).abs() < f64::EPSILON);
    let entries = leaderboard["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["rank"], 1);
    assert_eq!(entries[0]["user_id"], regular_user_id.to_string());
    assert_eq!(
        entries[0]["display_name"],
        format!("statistics-user-{regular_user_id}")
    );
    assert!(entries[0].get("email").is_none());
    assert_eq!(entries[0]["request_count"], 1);
    assert_eq!(entries[0]["priced_request_count"], 1);
    assert_eq!(entries[0]["total_tokens"], 50);
    let entry_cost_amount = entries[0]["cost_amount"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((entry_cost_amount - 1.0).abs() < f64::EPSILON);

    let user_leaderboard = request_with_token(
        &app,
        &regular_session.access_token,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(user_leaderboard.status(), StatusCode::OK);
    assert_eq!(
        body_json(user_leaderboard).await["total_cost_amount"],
        leaderboard["total_cost_amount"]
    );

    let invalid_leaderboard_limit = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?limit=0",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        invalid_leaderboard_limit.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_leaderboard_period = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=quarter",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        invalid_leaderboard_period.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_leaderboard_week = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=week&period_start=2026-07-21",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        invalid_leaderboard_week.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let codex_group_id = create_resource(
        &app,
        "/console/v1/routing/groups",
        serde_json::json!({
            "name": "statistics-codex", "enabled": true
        }),
    )
    .await;
    let codex_credential_id = create_test_codex_credential(
        &database.pool,
        &app,
        codex_fixture_input(codex_group_id, "statistics-codex"),
    )
    .await;
    let codex_images_channel = (
        codex_capability_id(&database.pool, codex_credential_id, "images_generation").await,
        codex_group_id,
    );
    let history_period_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO codex_quota_window_periods \
         (id,credential_id,window_kind,window_seconds,started_at,scheduled_reset_at, \
          ended_at,reset_reason,initial_used_percent,last_used_percent, \
          first_observed_at,last_observed_at) \
         VALUES ($1,$2,'primary',18000,$3,$4,$4,'openai_official',5,70,$3,$4)",
    )
    .bind(history_period_id)
    .bind(codex_credential_id)
    .bind(started_at)
    .bind(started_at + chrono::Duration::hours(5))
    .execute(&database.pool)
    .await
    .unwrap();
    let quota_history = request(
        &app,
        "GET",
        &format!(
            "/console/v1/providers/codex-oauth/credentials/{codex_credential_id}/quota/windows?limit=10"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(quota_history.status(), StatusCode::OK);
    let quota_history = body_json(quota_history).await;
    assert_eq!(
        quota_history["credential_id"],
        codex_credential_id.to_string()
    );
    assert_eq!(
        quota_history["periods"][0]["id"],
        history_period_id.to_string()
    );
    assert_eq!(
        quota_history["periods"][0]["reset_reason"],
        "openai_official"
    );
    let codex_started_at = started_at + chrono::Duration::hours(3);
    for (api_format, api_operation, group_id, channel_id) in [
        (
            "open_ai_responses",
            "responses",
            codex_group_id,
            codex_credential_id,
        ),
        (
            "open_ai_images",
            "images_generation",
            codex_images_channel.1,
            codex_images_channel.0,
        ),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation, \
              client_model,upstream_model,channel_group_id,channel_id,outcome, \
              response_status_code,streamed,input_tokens,output_tokens) \
             VALUES ($1,$2,$2,$3,$4,$5::api_format,$6, \
                     'statistics-codex-model','statistics-codex-model',$7,$8, \
                     'succeeded',200,false,10,5)",
        )
        .bind(Uuid::new_v4())
        .bind(codex_started_at)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(api_format)
        .bind(api_operation)
        .bind(group_id)
        .bind(channel_id)
        .execute(&database.pool)
        .await
        .unwrap();
    }
    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let codex_range_start = (codex_started_at - chrono::Duration::minutes(5))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let codex_range_end = (codex_started_at + chrono::Duration::minutes(5))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let codex_costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={codex_range_start}&started_before={codex_range_end}&granularity=hour&codex_credential_id={codex_credential_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(codex_costs.status(), StatusCode::OK);
    let codex_costs = body_json(codex_costs).await;
    assert_eq!(codex_costs["summary"]["request_count"], 2);
    assert_eq!(codex_costs["channels"].as_array().unwrap().len(), 2);

    let personal_codex_filter = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={codex_range_start}&started_before={codex_range_end}&granularity=hour&codex_credential_id={codex_credential_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        personal_codex_filter.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let conflicting_channel_filters = request(
        &app,
        "GET",
        &format!(
            "/console/v1/system/statistics/costs?started_after={codex_range_start}&started_before={codex_range_end}&granularity=hour&channel_id={codex_credential_id}&codex_credential_id={codex_credential_id}"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(
        conflicting_channel_filters.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let invalid_start = (started_at - chrono::Duration::days(32))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let invalid = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={invalid_start}&started_before={range_end}&granularity=hour"
        ),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

#[tokio::test]
async fn spend_leaderboard_uses_shanghai_periods_and_serves_snapshots() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let api_key_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_keys \
         (id,user_id,name,secret_value,status,allowed_api_formats,permissions) \
         VALUES ($1,$2,$3,$4,'active', \
                 ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'])",
    )
    .bind(api_key_id)
    .bind(app.user_id)
    .bind(format!("leaderboard-key-{api_key_id}"))
    .bind(format!("leaderboard-secret-{api_key_id}"))
    .execute(&database.pool)
    .await
    .unwrap();

    let before_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-01T15:59:59Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let after_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-01T16:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let sunday_before_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-04T15:59:59Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let monday_at_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-04T16:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let month_last_second = chrono::DateTime::parse_from_rfc3339("2026-01-31T15:59:59Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let next_month_at_midnight = chrono::DateTime::parse_from_rfc3339("2026-01-31T16:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for (started_at, cost) in [
        (before_midnight, rust_decimal::Decimal::new(25, 2)),
        (after_midnight, rust_decimal::Decimal::new(75, 2)),
        (sunday_before_midnight, rust_decimal::Decimal::new(50, 2)),
        (monday_at_midnight, rust_decimal::Decimal::new(200, 2)),
        (month_last_second, rust_decimal::Decimal::new(60, 2)),
        (next_month_at_midnight, rust_decimal::Decimal::new(300, 2)),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model, \
              outcome,response_status_code,streamed,input_tokens,output_tokens,currency, \
              price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price, \
              cache_write_unit_price,output_unit_price,cost_amount) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','chat_completions','leaderboard-model', \
                     'succeeded',200,false,10,5,'USD',1000000,$2,1,0,0,1,$5)",
        )
        .bind(Uuid::new_v4())
        .bind(started_at)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(cost)
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let repository = RequestLogRepository::new(database.pool.clone());
    metering_fixtures::copy_log_fixtures(&database.pool).await;
    repository
        .queries()
        .metering()
        .refresh_spend_leaderboard_snapshots()
        .await
        .unwrap();

    let first_day = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day&period_start=2026-01-01",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(first_day.status(), StatusCode::OK);
    let first_day = body_json(first_day).await;
    assert_eq!(first_day["period_start"], "2026-01-01");
    assert_eq!(first_day["period_end"], "2026-01-02");
    assert!(first_day["previous_period_start"].is_null());
    assert_eq!(first_day["next_period_start"], "2026-01-02");
    assert!(
        (first_day["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 0.25)
            .abs()
            < f64::EPSILON
    );

    let second_day = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day&period_start=2026-01-02",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(second_day.status(), StatusCode::OK);
    let second_day = body_json(second_day).await;
    assert_eq!(second_day["previous_period_start"], "2026-01-01");
    assert!(second_day["next_period_start"].is_string());
    assert!(
        (second_day["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 0.75)
            .abs()
            < f64::EPSILON
    );
    assert_eq!(second_day["entries"][0]["total_tokens"], 15);

    let week = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=week&period_start=2025-12-29",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(week.status(), StatusCode::OK);
    let week = body_json(week).await;
    assert_eq!(week["period_end"], "2026-01-05");
    assert!(
        (week["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 1.5)
            .abs()
            < f64::EPSILON
    );

    let next_week = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=week&period_start=2026-01-05",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(next_week.status(), StatusCode::OK);
    let next_week = body_json(next_week).await;
    assert_eq!(next_week["period_end"], "2026-01-12");
    assert!(
        (next_week["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 2.0)
            .abs()
            < f64::EPSILON
    );

    let month = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=month&period_start=2026-01-01",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(month.status(), StatusCode::OK);
    let month = body_json(month).await;
    assert_eq!(month["period_end"], "2026-02-01");
    assert!(
        (month["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 4.1)
            .abs()
            < f64::EPSILON
    );

    let next_month = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=month&period_start=2026-02-01",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(next_month.status(), StatusCode::OK);
    let next_month = body_json(next_month).await;
    assert_eq!(next_month["period_end"], "2026-03-01");
    assert!(
        (next_month["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 3.0)
            .abs()
            < f64::EPSILON
    );

    sqlx::query(
        "INSERT INTO request_logs \
         (id,started_at,completed_at,user_id,api_key_id,api_format,api_operation,client_model, \
          outcome,response_status_code,streamed,input_tokens,output_tokens,currency, \
          price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price, \
          cache_write_unit_price,output_unit_price,cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','chat_completions','leaderboard-model', \
                 'succeeded',200,false,10,5,'USD',1000000,$2,1,0,0,1,1)",
    )
    .bind(Uuid::new_v4())
    .bind(after_midnight)
    .bind(app.user_id)
    .bind(api_key_id)
    .execute(&database.pool)
    .await
    .unwrap();

    metering_fixtures::copy_log_fixtures(&database.pool).await;
    let stale_snapshot = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day&period_start=2026-01-02",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(stale_snapshot.status(), StatusCode::OK);
    let stale_snapshot = body_json(stale_snapshot).await;
    assert!(
        (stale_snapshot["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 0.75)
            .abs()
            < f64::EPSILON
    );

    repository
        .queries()
        .metering()
        .refresh_spend_leaderboard_snapshots()
        .await
        .unwrap();
    let refreshed_snapshot = request(
        &app,
        "GET",
        "/console/v1/statistics/spend-leaderboard?period=day&period_start=2026-01-02",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(refreshed_snapshot.status(), StatusCode::OK);
    let refreshed_snapshot = body_json(refreshed_snapshot).await;
    assert!(
        (refreshed_snapshot["total_cost_amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap()
            - 1.75)
            .abs()
            < f64::EPSILON
    );
    database.cleanup().await;
}
