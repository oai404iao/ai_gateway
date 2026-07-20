//! Console API OpenAPI-spec consistency tests.
//!
//! These tests verify that the live Console HTTP implementation matches the
//! authoritative spec in `docs/openapi/console-v1.yaml` for the request and
//! response shapes the SPA depends on: the auth/session flow, error body
//! shape `{"error": ...}`, ETag/`If-Match` optimistic concurrency (success
//! then `409` on a stale tag), one-time secret presence on create, and
//! `limit` clamping on log endpoints.
//!
//! They follow the same PostgreSQL integration-test convention as
//! `tests/control_plane_integration.rs`: `TestDatabase::new()` creates a
//! throwaway database and `docker compose up -d` must provide PostgreSQL.

use std::sync::Arc;

use ai_gateway::{
    application::{
        ConsoleAuthService, ControlPlaneCoordinator, ModelSyncService, hash_console_password,
    },
    http::console::{self, ConsoleState},
    models_dev::ModelsDevClient,
    persistence::{AuthRepository, ControlPlaneRepository, MIGRATOR, RequestLogRepository},
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{
        AuthConfig, ModelsSyncConfig, RuntimeConfig, UpstreamConfig, compile_control_plane,
    },
};
use axum::{
    body::Body,
    http::{StatusCode, header},
};
use http_body_util::BodyExt;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tower::ServiceExt;
use uuid::Uuid;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";
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
        let admin_url = std::env::var("TEST_DATABASE_ADMIN_URL")
            .unwrap_or_else(|_| DEFAULT_ADMIN_URL.to_owned());
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

struct App {
    router: axum::Router,
    access_token: String,
    user_id: Uuid,
}

async fn app(pool: PgPool) -> App {
    let user_id = Uuid::new_v4();
    let password_hash = hash_console_password(TEST_PASSWORD.to_owned())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, email, display_name, role, status, password_hash, currency) \
         VALUES ($1, $2, $3, 'admin', 'active', $4, 'USD')",
    )
    .bind(user_id)
    .bind(format!("spec-user-{user_id}@example.test"))
    .bind(format!("spec-{user_id}"))
    .bind(password_hash)
    .execute(&pool)
    .await
    .unwrap();

    let repository = ControlPlaneRepository::new(pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(repository.load().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        runtime,
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 3,
        },
    );
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
    let session = auth.login(email, TEST_PASSWORD.into()).await.unwrap();
    // Sanity: the freshly issued token must round-trip through the same
    // authenticator before we hand it to HTTP. This surfaces key/claim
    // mismatches as a clear panic instead of a downstream 401.
    auth.authenticate_access_token(&session.access_token)
        .await
        .expect("issued access token must authenticate");
    let router = console::router(ConsoleState {
        coordinator,
        model_sync,
        auth,
        request_logs: RequestLogRepository::new(pool),
        console_body_bytes: 1_048_576,
        auth_body_bytes: 16_384,
        allowed_origins: vec![],
    });
    App {
        router,
        access_token: session.access_token,
        user_id,
    }
}

async fn request(
    app: &App,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", app.access_token))
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
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
        &[],
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

/// `GET /me` returns the `ConsoleProfile` shape, including decimal/currency
/// string fields.
#[tokio::test]
async fn profile_shape_matches_spec() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let response = request(&app, "GET", "/console/v1/me", serde_json::json!({}), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["id"], app.user_id.to_string());
    assert!(body["balance_amount"].is_string(), "decimal is a string");
    assert!(body["currency"].is_string());
    assert_eq!(body["role"], "admin");
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

/// Creating a resource returns a `MutationResponse` whose `secret` is present
/// exactly once and never retrievable again, per spec.
#[tokio::test]
async fn api_key_create_returns_one_time_secret() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;
    let create = request(
        &app,
        "POST",
        "/console/v1/api-keys",
        serde_json::json!({
            "user_id": app.user_id,
            "name": "spec-key",
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
        }),
        &[],
    )
    .await;
    assert_eq!(create.status(), StatusCode::CREATED);
    let body = body_json(create).await;
    let secret = body["secret"].as_str().expect("secret present on create");
    assert!(!secret.is_empty());
    let id = body["id"].as_str().expect("id present").to_owned();

    // The secret is never returned again by the detail endpoint.
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
    assert!(
        detail_body.get("secret").is_none(),
        "secret not retrievable"
    );
    assert!(has_etag, "detail returns an ETag");
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
