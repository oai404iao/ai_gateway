//! Console API OpenAPI-spec consistency tests.
//!
//! These tests verify that the live Console HTTP implementation matches the
//! authoritative spec in `docs/openapi/console-v1.yaml` for the request and
//! response shapes the SPA depends on: the auth/session flow, error body
//! shape `{"error": ...}`, ETag/`If-Match` optimistic concurrency (success
//! then `409` on a stale tag), retrievable masked-by-default API key values,
//! and `limit` clamping on log endpoints.
//!
//! They follow the same PostgreSQL integration-test convention as
//! `tests/control_plane_integration.rs`: `TestDatabase::new()` creates a
//! throwaway database and `docker compose up -d` must provide PostgreSQL.

use std::sync::Arc;

use ai_gateway::{
    application::{
        AuthError, ConsoleAuthService, ControlPlaneCoordinator, ModelSyncService,
        hash_console_password,
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
    let mut update = body_json(detail).await;
    update["display_name"] = serde_json::json!("Updated display name");
    for field in ["id", "created_at", "updated_at"] {
        update.as_object_mut().unwrap().remove(field);
    }

    let display_name_update = request(&app, "PUT", &path, update, &[("if-match", &etag)]).await;
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
    let mut update = body_json(detail).await;
    update["role"] = serde_json::json!("user");
    for field in ["id", "created_at", "updated_at"] {
        update.as_object_mut().unwrap().remove(field);
    }

    let role_update = request(&app, "PUT", &path, update, &[("if-match", &etag)]).await;
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
    let mut update = body_json(detail).await;
    update["balance_amount"] = serde_json::json!("42.75");
    for field in ["id", "created_at", "updated_at"] {
        update.as_object_mut().unwrap().remove(field);
    }
    let response = request(&app, "PUT", &path, update, &[("if-match", &etag)]).await;
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

/// API key creation uses the `sk-` prefix and the same value remains
/// retrievable from authorized list/detail endpoints.
#[tokio::test]
async fn api_key_create_returns_retrievable_prefixed_secret() {
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

/// Self-service key creation reports actionable policy precondition codes.
#[tokio::test]
async fn self_api_key_create_reports_policy_preconditions() {
    let database = TestDatabase::new().await;
    let app = app(database.pool.clone()).await;

    let missing = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({"name": "missing-policy"}),
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
         (id,name,allowed_api_formats,permissions,max_active_keys,enabled) \
         VALUES ($1,$2,ARRAY['open_ai_chat_completions']::api_format[],ARRAY['proxy'],1,false)",
    )
    .bind(policy_id)
    .bind(format!("spec-policy-{policy_id}"))
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
        serde_json::json!({"name": "disabled-policy"}),
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
    let expiry = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let created = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({"name": "first-key", "expires_at": expiry}),
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
    assert_eq!(body_json(detail).await["secret"], secret);

    let limited = request(
        &app,
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({"name": "second-key"}),
        &[],
    )
    .await;
    assert_eq!(limited.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(limited).await,
        serde_json::json!({"error": "api_key_limit_reached"})
    );
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
    for (id, client_model, outcome) in [
        (matching_log_id, "filter-model", "succeeded"),
        (Uuid::new_v4(), "other-model", "failed"),
    ] {
        sqlx::query(
            "INSERT INTO request_logs \
             (id,started_at,completed_at,user_id,api_key_id,api_format,client_model,upstream_model,outcome,streamed,total_duration_ms) \
             VALUES ($1,$2,$2,$3,$4,'open_ai_chat_completions',$5,$6,$7,false,1)",
        )
        .bind(id)
        .bind(now)
        .bind(app.user_id)
        .bind(api_key_id)
        .bind(client_model)
        .bind("filter-upstream")
        .bind(outcome)
        .execute(&database.pool)
        .await
        .unwrap();
    }

    let response = request(
        &app,
        "GET",
        "/console/v1/request-logs?model=filter-model&api_format=open_ai_chat_completions&outcome=succeeded&billed=false&limit=25",
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], matching_log_id.to_string());

    let detail = request(
        &app,
        "GET",
        &format!("/console/v1/request-logs/{matching_log_id}"),
        serde_json::json!({}),
        &[],
    )
    .await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(body_json(detail).await["id"], matching_log_id.to_string());

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
    for (outcome, status, ttft_ms, tps, input_tokens, output_tokens, cost) in [
        (
            "succeeded",
            200_i16,
            Some(500_i32),
            Some(rust_decimal::Decimal::new(200, 1)),
            100_i64,
            50_i64,
            Some(rust_decimal::Decimal::new(25, 2)),
        ),
        (
            "failed",
            500_i16,
            None,
            None,
            10_i64,
            0_i64,
            Some(rust_decimal::Decimal::new(5, 2)),
        ),
        ("cancelled", 200_i16, None, None, 0_i64, 0_i64, None),
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
                     'statistics-model',$5,$6,$7,$8,false,$9,1000,$10,$11,0,0,$12, \
                     'USD',1000000,$2,1,0,0,1,$13)",
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
        .bind(output_tokens)
        .bind(cost)
        .execute(&database.pool)
        .await
        .unwrap();
    }

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
    let amount = costs["summary"]["cost_amount"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((amount - 0.30).abs() < f64::EPSILON);
    assert_eq!(costs["models"][0]["model"], "statistics-model");
    assert_eq!(costs["models"][0]["success_rate"], 0.5);
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
