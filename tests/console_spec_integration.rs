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
    http::console::{self, ConsoleState},
    models_dev::ModelsDevClient,
    persistence::{
        AuthRepository, ControlPlaneRepository, DEFAULT_USER_GROUP_ID, MIGRATOR,
        RequestLogRepository, SystemPassiveHealthSettingsInput, SystemSettingsInput,
        SystemUpstreamSettingsInput,
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

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

impl TestDatabase {
    async fn new() -> Self {
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
        sqlx::query(&format!("CREATE DATABASE \"{name}\""))
            .execute(&admin)
            .await
            .expect("temp database creatable");
        database_url.set_path(&format!("/{name}"));
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(database_url.as_str())
            .await
            .expect("temp database connectable");
        MIGRATOR.run(&pool).await.expect("migrations apply");
        Self { pool, admin, name }
    }

    async fn cleanup(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE \"{}\" WITH (FORCE)", self.name))
            .execute(&self.admin)
            .await
            .expect("temp database removable");
        self.admin.close().await;
    }
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
        request_logs: RequestLogRepository::new(pool.clone()),
        system_metrics: SystemMetricsService::new(pool, 5),
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
    assert!(stored.settings.request_retry.enabled);
    assert_eq!(stored.settings.request_retry.max_retries, 1);
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

async fn request(
    app: &App,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    request_with_token(app, &app.access_token, method, path, body, headers).await
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

    let group_path = format!("/console/v1/user-groups/{target_group_id}");
    let group_detail = request(&app, "GET", &group_path, serde_json::json!({}), &[]).await;
    assert_eq!(group_detail.status(), StatusCode::OK);
    let group_etag = group_detail.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let delete_group = request(
        &app,
        "DELETE",
        &group_path,
        serde_json::json!({}),
        &[("if-match", &group_etag)],
    )
    .await;
    assert_eq!(delete_group.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(delete_group).await["error"], "user_group_in_use");

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
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(update.status(), StatusCode::OK);

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
    let options = ControlPlaneRepository::new(database.pool.clone())
        .own_api_key_options(invited_user_id)
        .await
        .unwrap();
    assert_eq!(options.policy_id, policy_id);

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
    assert_eq!(overridden.policy_id, override_policy_id);

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
    assert_eq!(inherited_again.policy_id, policy_id);

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

/// Deleting a user anonymizes the retained owner row and revokes every live
/// credential instead of cascading away request-log/audit ownership.
#[tokio::test]
async fn user_delete_anonymizes_and_revokes_credentials() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let user_id = Uuid::new_v4();
    let api_key_id = Uuid::new_v4();
    let session_id = Uuid::new_v4();
    let invitation_id = Uuid::new_v4();
    let email = format!("delete-{user_id}@example.test");
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
    .bind(format!("sk-delete-{}", Uuid::new_v4().simple()))
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
    let key_status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(api_key_id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(key_status, "revoked");
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

/// A mutable admin resource (`channel-groups`) returns an `ETag` on GET and
/// requires `If-Match` on PUT; a stale `If-Match` yields `409` with an error
/// body. Channel groups are chosen because updating them does not change the
/// actor's `auth_version` (unlike `UpdateUser`), so the issued JWT stays valid
/// across the two PUTs.
#[tokio::test]
async fn etag_if_match_optimistic_concurrency_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let create = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "spec-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let group_id = body_json(create).await["id"].as_str().unwrap().to_owned();
    let path = format!("/console/v1/routing/channel-groups/{group_id}");

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
    update["name"] = serde_json::json!("spec-group-renamed");
    for field in ["id", "updated_at"] {
        update.as_object_mut().unwrap().remove(field);
    }

    let ok = request(&app, "PUT", &path, update.clone(), &[("if-match", &etag)]).await;
    assert_eq!(ok.status(), StatusCode::OK);
    let ok_body = body_json(ok).await;
    assert!(
        ok_body["correlation_id"].is_string(),
        "mutation correlation"
    );

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
async fn codex_oauth_flow_contract_uses_pkce_and_actor_scoped_completion() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let create_group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "spec-codex",
            "api_format": "open_ai_responses",
            "connector_kind": "codex_oauth",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(create_group.status(), StatusCode::CREATED);
    let group_id = body_json(create_group).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let start = request(
        &app,
        "POST",
        &format!("/console/v1/providers/codex-oauth/channel-groups/{group_id}/oauth/flows"),
        serde_json::json!({
            "label": "spec-account",
            "weight": 100,
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

/// The admin-only load endpoint exposes the current instance's resource,
/// runtime, queue, backlog, and database-pool pressure shape.
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
    input["upstream"]["stream_idle_timeout_seconds"] = serde_json::json!(8);
    input["request_retry"] = serde_json::json!({
        "enabled": false,
        "max_retries": 4,
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
    assert!(!published.request_retry().enabled());
    assert_eq!(published.request_retry().max_retries(), 4);
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

/// A model rule chooses exactly one upstream model record. Its forwarded
/// model identifier and its price snapshot come from that same record; the
/// API no longer accepts a separate priced-model/upstream-model pair.
#[tokio::test]
async fn model_rule_uses_its_upstream_model_as_the_price_source() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let effective_at = chrono::Utc::now().to_rfc3339();
    let model = request(
        &app,
        "POST",
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": "spec-upstream-model",
            "display_name": "Spec upstream model",
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

    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "spec-rule-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": "spec-rule-channel",
            "base_url": "https://upstream.example.test",
            "enabled": true,
            "weight": 1,
            "upstream_auth_kind": "none",
            "available_models": ["spec-upstream-model"],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = body_json(channel).await["id"].as_str().unwrap().to_owned();

    let rule = request(
        &app,
        "POST",
        "/console/v1/routing/model-rules",
        serde_json::json!({
            "client_model": "spec-client-model",
            "api_format": "open_ai_chat_completions",
            "upstream_model_id": model_id,
            "channel_ids": [channel_id],
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(rule.status(), StatusCode::CREATED);
    let rule_id = body_json(rule).await["id"].as_str().unwrap().to_owned();

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/model-rules/{rule_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = body_json(detail).await;
    assert_eq!(detail["upstream_model_id"], model_id);
    assert_eq!(detail["upstream_model"], "spec-upstream-model");
    assert!(detail.get("model_id").is_none());
    database.cleanup().await;
}

/// Model prices may define immutable advanced billing policy: input-price
/// tiers and request-body JSON Pointer multipliers are model-level facts, not
/// channel transforms. Invalid policy is rejected before it can be persisted.
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
    database.cleanup().await;
}

/// API key creation uses the `sk-` prefix and the same value remains
/// retrievable from authorized list/detail endpoints.
#[tokio::test]
async fn api_key_create_returns_retrievable_prefixed_secret() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "spec-key-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();
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
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "editable-detail-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();

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
    let override_document = serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "request_headers": {"set": {"x-channel-source": "spec-test"}}
    });
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": "editable-detail-channel",
            "base_url": "https://editable-detail.example.test",
            "enabled": true,
            "weight": 1,
            "billing_multiplier": "1.5",
            "config_template_id": template_id,
            "override_document": override_document,
            "upstream_auth_kind": "bearer",
            "upstream_api_key": upstream_api_key,
            "available_models": ["editable-detail-model"],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = body_json(channel).await["id"].as_str().unwrap().to_owned();

    let channel_list = request(
        &app,
        "GET",
        "/console/v1/routing/channels",
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
    assert!(channel_list_item.get("override_document").is_none());
    assert!(channel_list_item.get("upstream_api_key").is_none());
    assert_eq!(channel_list_item["billing_multiplier"], "1.500000000000");

    let channel_detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/channels/{channel_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(channel_detail.status(), StatusCode::OK);
    assert!(channel_detail.headers().get(header::ETAG).is_some());
    let channel_detail = body_json(channel_detail).await;
    assert_eq!(channel_detail["override_document"], override_document);
    assert_eq!(channel_detail["upstream_api_key"], upstream_api_key);
    assert_eq!(channel_detail["billing_multiplier"], "1.500000000000");

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
async fn channel_websocket_support_is_responses_only_and_defaults_to_opt_in() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let responses_group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "websocket-responses-group",
            "api_format": "open_ai_responses",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    let responses_group_id = body_json(responses_group).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let responses_channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": responses_group_id,
            "api_format": "open_ai_responses",
            "name": "websocket-capable",
            "base_url": "https://responses-websocket.example.test",
            "enabled": true,
            "supports_websocket": true,
            "weight": 1,
            "upstream_auth_kind": "none",
        }),
        &[],
    )
    .await;
    assert_eq!(responses_channel.status(), StatusCode::CREATED);
    let responses_channel_id = body_json(responses_channel).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/channels/{responses_channel_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(body_json(detail).await["supports_websocket"], true);

    let chat_group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "websocket-chat-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    let chat_group_id = body_json(chat_group).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let invalid = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": chat_group_id,
            "api_format": "open_ai_chat_completions",
            "name": "invalid-websocket-chat",
            "base_url": "https://chat-websocket.example.test",
            "enabled": true,
            "supports_websocket": true,
            "weight": 1,
            "upstream_auth_kind": "none",
        }),
        &[],
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);

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
            "upstream_auth_kind": "bearer",
            "upstream_api_key": "spec-model-discovery-secret"
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
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "batch-channel-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();

    let mut channel_ids = Vec::new();
    for suffix in ["a", "b"] {
        let channel = request(
            &app,
            "POST",
            "/console/v1/routing/channels",
            serde_json::json!({
                "channel_group_id": group_id,
                "api_format": "open_ai_chat_completions",
                "name": format!("batch-channel-{suffix}"),
                "base_url": format!("https://batch-{suffix}.example.test"),
                "enabled": true,
                "weight": 1,
                "upstream_auth_kind": "none",
                "available_models": [],
            }),
            &[],
        )
        .await;
        assert_eq!(channel.status(), StatusCode::CREATED);
        channel_ids.push(body_json(channel).await["id"].as_str().unwrap().to_owned());
    }

    let before = body_json(
        request(
            &app,
            "GET",
            "/console/v1/routing/channels",
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

    let updated = request(
        &app,
        "POST",
        "/console/v1/routing/channels/batch",
        serde_json::json!({
            "items": before_items,
            "changes": {
                "status_statistics_enabled": true,
                "auto_disable_allowed": true,
                "weight": 7,
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
            "/console/v1/routing/channels",
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
        assert_eq!(channel["weight"], 7);
        assert_eq!(channel["billing_multiplier"], "2.500000000000");
        assert_eq!(channel["status_statistics_enabled"], true);
        assert_eq!(channel["auto_disable_allowed"], true);
        let compiled = app
            .runtime
            .snapshot()
            .channel(Uuid::parse_str(id).unwrap())
            .unwrap();
        assert_eq!(compiled.weight(), 7);
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
        "/console/v1/routing/channels/batch",
        serde_json::json!({
            "items": [
                current_items[0].clone(),
                {
                    "id": channel_ids[1],
                    "updated_at": stale_second_version
                }
            ],
            "changes": {"weight": 9}
        }),
        &[],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let persisted_weights: Vec<i32> =
        sqlx::query_scalar("SELECT weight FROM channels WHERE id = ANY($1) ORDER BY id")
            .bind(
                channel_ids
                    .iter()
                    .map(|id| Uuid::parse_str(id).unwrap())
                    .collect::<Vec<_>>(),
            )
            .fetch_all(&database.pool)
            .await
            .unwrap();
    assert_eq!(persisted_weights, vec![7, 7]);
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
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "manual-recovery-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": "manual-recovery-channel",
            "base_url": "https://manual-recovery.example.test",
            "enabled": true,
            "weight": 1,
            "upstream_auth_kind": "none",
            "available_models": [],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = Uuid::parse_str(body_json(channel).await["id"].as_str().unwrap()).unwrap();

    sqlx::query(
        "UPDATE channels
         SET auto_disabled=true, auto_disabled_reason='test automatic disable'
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
            "/console/v1/routing/channels",
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
    let updated_at = channel["updated_at"].clone();

    let recovered = request(
        &app,
        "POST",
        &format!("/console/v1/routing/channels/{channel_id}/recover"),
        serde_json::json!({ "updated_at": updated_at }),
        &[],
    )
    .await;
    assert_eq!(recovered.status(), StatusCode::OK);

    let state: (bool, Option<String>) =
        sqlx::query_as("SELECT auto_disabled,auto_disabled_reason FROM channels WHERE id=$1")
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
        &format!("/console/v1/routing/channels/{channel_id}/recover"),
        serde_json::json!({ "updated_at": updated_at }),
        &[],
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    database.cleanup().await;
}

#[tokio::test]
async fn api_key_policy_only_stores_selectable_targets() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "policy-target-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": "policy-target-channel",
            "base_url": "https://upstream.example.test",
            "enabled": true,
            "weight": 1,
            "upstream_auth_kind": "none",
            "available_models": [],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = body_json(channel).await["id"].as_str().unwrap().to_owned();

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
    let group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "self-key-group",
            "api_format": "open_ai_chat_completions",
            "priority": 1,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);
    let group_id = body_json(group).await["id"].as_str().unwrap().to_owned();
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": "self-key-channel",
            "base_url": "https://upstream.example.test",
            "enabled": true,
            "weight": 1,
            "upstream_auth_kind": "none",
            "available_models": [],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = body_json(channel).await["id"].as_str().unwrap().to_owned();
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

    let policy_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_key_policies \
         (id,name,allowed_group_ids,allowed_channel_ids,enabled) \
         VALUES ($1,$2,ARRAY[$3]::uuid[],'{}',false)",
    )
    .bind(policy_id)
    .bind(format!("spec-policy-{policy_id}"))
    .bind(Uuid::parse_str(&group_id).unwrap())
    .execute(&database.pool)
    .await
    .unwrap();
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
    assert_eq!(options["channels"][0]["id"], channel_id);

    let other_group = request(
        &app,
        "POST",
        "/console/v1/routing/channel-groups",
        serde_json::json!({
            "name": "self-key-other-group",
            "api_format": "open_ai_chat_completions",
            "priority": 2,
            "selection_strategy": "weighted_random",
            "enabled": true,
        }),
        &[],
    )
    .await;
    assert_eq!(other_group.status(), StatusCode::CREATED);
    let other_group_id = body_json(other_group).await["id"]
        .as_str()
        .unwrap()
        .to_owned();
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
             (id,started_at,completed_at,user_id,api_key_id,api_format,client_model,upstream_model,outcome,streamed,total_duration_ms,input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,error_code,error_summary) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions',$5,$6,$7,false,1,12,2,1,5,1,$8,$9)",
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
    assert_eq!(body[0]["request_protocol"], "non_stream");
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
    assert_eq!(detail["request_protocol"], "non_stream");
    assert_eq!(detail["reasoning_tokens"], 1);
    assert_eq!(detail["error_code"], "provider_error");
    assert_eq!(detail["error_summary"], "upstream quota exhausted");

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
    database.cleanup().await;
}

#[tokio::test]
async fn statistics_endpoints_aggregate_channel_health_and_costs() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let group_id = Uuid::new_v4();
    let api_key_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,priority,selection_strategy,enabled) \
         VALUES ($1,$2,'open_ai_chat_completions',1,'weighted_random',true)",
    )
    .bind(group_id)
    .bind(format!("statistics-group-{group_id}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let channel = request(
        &app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": group_id,
            "api_format": "open_ai_chat_completions",
            "name": format!("statistics-channel-{group_id}"),
            "base_url": "https://statistics.example.test",
            "enabled": true,
            "status_statistics_enabled": true,
            "weight": 1,
            "upstream_auth_kind": "none",
            "available_models": ["statistics-model"],
        }),
        &[],
    )
    .await;
    assert_eq!(channel.status(), StatusCode::CREATED);
    let channel_id = Uuid::parse_str(body_json(channel).await["id"].as_str().unwrap()).unwrap();
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
            Some(rust_decimal::Decimal::new(5, 2)),
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
            None,
        ),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,client_model, \
              upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
              streamed,ttft_ms,total_duration_ms,output_tokens_per_second,input_tokens, \
              cached_input_tokens,cache_write_tokens,output_tokens,currency,price_unit_tokens, \
              price_effective_at,input_unit_price,cached_input_unit_price, \
              cache_write_unit_price,output_unit_price,cost_amount) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','statistics-client-model', \
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
         (id,started_at,completed_at,user_id,api_key_id,request_source,api_format,client_model, \
          upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
          streamed,ttft_ms,total_duration_ms,input_tokens,cached_input_tokens, \
          cache_write_tokens,output_tokens,currency,price_unit_tokens,price_effective_at, \
          input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price, \
          cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'scheduled_test','open_ai_chat_completions', \
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

    let channel_detail = request(
        &app,
        "GET",
        &format!("/console/v1/routing/channels/{channel_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(channel_detail.status(), StatusCode::OK);
    assert_eq!(
        body_json(channel_detail).await["status_statistics_enabled"],
        true
    );

    let status = request(
        &app,
        "GET",
        "/console/v1/statistics/channel-status?window=24h",
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
    assert_eq!(status["channels"][0]["id"], channel_id.to_string());
    assert!(status["channels"][0]["models"][0]["history"].is_array());

    let range_start = (started_at - chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let range_end = (started_at + chrono::Duration::hours(1))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={}&api_key_id={api_key_id}",
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
    assert_eq!(costs["summary"]["priced_request_count"], 2);
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
    assert!((amount - 0.30).abs() < f64::EPSILON);
    assert_eq!(costs["models"][0]["model"], "statistics-model");
    assert_eq!(costs["models"][0]["input_tokens"], 110);
    assert_eq!(costs["models"][0]["cached_input_tokens"], 22);
    assert_eq!(costs["models"][0]["cache_write_tokens"], 6);
    assert_eq!(costs["models"][0]["output_tokens"], 50);
    assert_eq!(costs["models"][0]["success_rate"], 0.5);
    assert_eq!(costs["channels"][0]["id"], channel_id.to_string());
    assert_eq!(
        costs["channels"][0]["channel_group_name"],
        format!("statistics-group-{group_id}")
    );
    assert_eq!(
        costs["channels"][0]["name"],
        format!("statistics-channel-{group_id}")
    );
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
         (id,started_at,completed_at,user_id,api_key_id,api_format,client_model, \
          upstream_model,channel_group_id,channel_id,outcome,response_status_code, \
          streamed,ttft_ms,total_duration_ms,output_tokens_per_second,input_tokens, \
          cached_input_tokens,cache_write_tokens,output_tokens,currency,price_unit_tokens, \
          price_effective_at,input_unit_price,cached_input_unit_price, \
          cache_write_unit_price,output_unit_price,cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','statistics-user-client-model', \
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
    assert_eq!(
        admin_logs[0]["channel_group_name"],
        format!("statistics-group-{group_id}")
    );
    assert_eq!(admin_logs[0]["channel_id"], channel_id.to_string());
    assert_eq!(
        admin_logs[0]["channel_name"],
        format!("statistics-channel-{group_id}")
    );
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
    assert_eq!(
        user_logs[0]["channel_group_name"],
        format!("statistics-group-{group_id}")
    );
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
        "/console/v1/statistics/channel-status?window=24h",
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
    assert_eq!(user_channel_filter.status(), StatusCode::FORBIDDEN);

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
    assert_eq!(other_user_costs.status(), StatusCode::FORBIDDEN);

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

    let admin_user_costs = request(
        &app,
        "GET",
        &format!(
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&user_id={regular_user_id}"
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
            "/console/v1/statistics/costs?started_after={range_start}&started_before={range_end}&granularity=hour&channel_id={channel_id}"
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
    assert!((leaderboard_total - 1.30).abs() < f64::EPSILON);
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
             (id,started_at,completed_at,user_id,api_key_id,api_format,client_model, \
              outcome,response_status_code,streamed,input_tokens,output_tokens,currency, \
              price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price, \
              cache_write_unit_price,output_unit_price,cost_amount) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','leaderboard-model', \
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
    repository
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
         (id,started_at,completed_at,user_id,api_key_id,api_format,client_model, \
          outcome,response_status_code,streamed,input_tokens,output_tokens,currency, \
          price_unit_tokens,price_effective_at,input_unit_price,cached_input_unit_price, \
          cache_write_unit_price,output_unit_price,cost_amount) \
         VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions','leaderboard-model', \
                 'succeeded',200,false,10,5,'USD',1000000,$2,1,0,0,1,1)",
    )
    .bind(Uuid::new_v4())
    .bind(after_midnight)
    .bind(app.user_id)
    .bind(api_key_id)
    .execute(&database.pool)
    .await
    .unwrap();

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
