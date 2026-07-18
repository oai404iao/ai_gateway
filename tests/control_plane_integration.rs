use std::{
    collections::BTreeMap,
    env, io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    application::{
        ControlPlaneCoordinator, ModelSyncService, ProxyService, QueueRequestLogSink,
        RequestLogSink,
    },
    domain::{ApiFormat, ApiKeyPermission, RequestLogEvent, RequestLogOutcome},
    http::admin::{self, AdminState},
    models_dev::ModelsDevClient,
    persistence::{
        ControlPlaneRepository, MIGRATOR, RequestLogInsertOutcome, RequestLogRepository,
    },
    routing::{self, PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{ModelsSyncConfig, RuntimeConfig, UpstreamConfig, compile_control_plane},
    workers::{ControlPlaneReloader, RequestLogWorker},
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::Response,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
use http_body_util::BodyExt;
use reqwest::Url;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;
use uuid::Uuid;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";

struct TestDatabase {
    pool: PgPool,
    admin: PgPool,
    name: String,
}

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_server(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer { address, task }
}

#[derive(Clone)]
enum UpstreamMode {
    Immediate(StatusCode),
    HeaderDelay,
    OneChunkThenIdle,
    TwoChunks,
}

#[derive(Clone)]
struct UpstreamState(Arc<Mutex<UpstreamMode>>);

async fn upstream(State(state): State<UpstreamState>) -> Response {
    let mode = { state.0.lock().unwrap().clone() };
    match mode {
        UpstreamMode::Immediate(status) => Response::builder()
            .status(status)
            .body(Body::from("upstream"))
            .unwrap(),
        UpstreamMode::HeaderDelay => {
            tokio::time::sleep(Duration::from_secs(2)).await;
            Response::new(Body::from("late"))
        }
        UpstreamMode::OneChunkThenIdle => Response::new(Body::from_stream(
            stream::once(async { Ok::<Bytes, io::Error>(Bytes::from_static(b"first")) })
                .chain(stream::pending()),
        )),
        UpstreamMode::TwoChunks => Response::new(Body::from_stream(stream::iter(vec![
            Ok::<Bytes, io::Error>(Bytes::from_static(b"first")),
            Ok::<Bytes, io::Error>(Bytes::from_static(b"second")),
        ]))),
    }
}

async fn models_dev_catalog() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "provider-a": {
            "id": "provider-a",
            "name": "Provider A",
            "models": {
                "catalog-model": {
                    "id": "catalog-model",
                    "name": "Catalog Model",
                    "cost": {
                        "input": 1.25,
                        "output": 2.50,
                        "cache_read": 0.25,
                        "cache_write": 0.50
                    },
                    "metadata": {"safe": true}
                },
                "missing-price": {
                    "id": "missing-price",
                    "name": "Missing Price",
                    "cost": {"input": 1.0}
                }
            }
        }
    }))
}

#[derive(FromRow)]
struct PersistedLog {
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    user_id: Uuid,
    api_key_id: Uuid,
    api_format: String,
    client_model: String,
    upstream_model: Option<String>,
    model_rule_id: Option<Uuid>,
    channel_group_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    model_id: Option<Uuid>,
    outcome: String,
    response_status_code: Option<i16>,
    streamed: bool,
    ttft_ms: Option<i32>,
    total_duration_ms: Option<i32>,
    error_code: Option<String>,
}

#[derive(FromRow)]
struct TerminalLogCount {
    api_key_id: Uuid,
    client_model: String,
    outcome: String,
    error_code: Option<String>,
    count: i64,
}

fn assert_log_timing(log: &PersistedLog) {
    assert!(log.completed_at >= log.started_at);
    let total_duration_ms = log
        .total_duration_ms
        .expect("proxy terminal logs must record a total duration");
    assert!(total_duration_ms >= 0);
    if let Some(ttft_ms) = log.ttft_ms {
        assert!(ttft_ms >= 0);
        assert!(ttft_ms <= total_duration_ms);
    }
}

async fn wait_for_log(pool: &PgPool, api_key_id: Uuid, client_model: &str) -> PersistedLog {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let rows = sqlx::query_as::<_, PersistedLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, model_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, error_code FROM request_logs WHERE api_key_id = $1 AND client_model = $2")
            .bind(api_key_id)
            .bind(client_model)
            .fetch_all(pool)
            .await
            .unwrap();
        if rows.len() == 1 {
            let log = rows.into_iter().next().unwrap();
            assert_log_timing(&log);
            return log;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "request log was not persisted exactly once"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_terminal_log(
    pool: &PgPool,
    api_key_id: Uuid,
    client_model: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> PersistedLog {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let rows = sqlx::query_as::<_, PersistedLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, client_model, upstream_model, model_rule_id, channel_group_id, channel_id, model_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, error_code FROM request_logs WHERE api_key_id = $1 AND client_model = $2 AND outcome = $3 AND error_code IS NOT DISTINCT FROM $4")
            .bind(api_key_id)
            .bind(client_model)
            .bind(outcome)
            .bind(error_code)
            .fetch_all(pool)
            .await
            .unwrap();
        if rows.len() == 1 {
            let log = rows.into_iter().next().unwrap();
            assert_log_timing(&log);
            return log;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "terminal request log was not persisted exactly once"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_blocked_request_log_insert(pool: &PgPool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE relation = 'request_logs'::regclass AND mode = 'RowExclusiveLock' AND NOT granted)",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        if blocked {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "request log worker did not reach the database lock"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
    proxy: Uuid,
    template: Uuid,
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
        proxy: Uuid::new_v4(),
        template: Uuid::new_v4(),
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
    sqlx::query("INSERT INTO proxies (id, name, proxy_url, username, password, no_proxy_hosts, enabled) VALUES ($1, $2, 'https://seed-proxy.test', 'seed-proxy-user', 'seed-proxy-password', ARRAY['seed.internal']::text[], true)")
        .bind(seed.proxy)
        .bind(format!("seed-proxy-{}", seed.proxy))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO config_templates (id, name, description, document, enabled) VALUES ($1, $2, 'seed template', $3, true)")
        .bind(seed.template)
        .bind(format!("seed-template-{}", seed.template))
        .bind(serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-seed-template": "seed-default"}}
        }))
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

fn request_log_event(seed: &Seed, outcome: RequestLogOutcome) -> RequestLogEvent {
    let now = Utc::now();
    RequestLogEvent {
        id: Uuid::new_v4(),
        started_at: now,
        completed_at: now,
        user_id: seed.user,
        api_key_id: seed.key,
        api_format: ApiFormat::OpenAiChatCompletions,
        client_model: seed.client_model.clone(),
        upstream_model: Some("upstream-v1".into()),
        model_rule_id: Some(seed.rule),
        channel_group_id: Some(seed.group),
        channel_id: Some(seed.channel),
        model_id: Some(seed.model),
        outcome,
        response_status_code: Some(200),
        streamed: true,
        ttft_ms: Some(1),
        total_duration_ms: 2,
        error_code: None,
    }
}

#[tokio::test]
async fn request_log_insert_is_idempotent_and_worker_continues_after_failure() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);

    assert_eq!(
        repository.insert(&event).await.unwrap(),
        RequestLogInsertOutcome::Inserted
    );
    assert_eq!(
        repository.insert(&event).await.unwrap(),
        RequestLogInsertOutcome::ExactDuplicate
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs WHERE id = $1")
        .bind(event.id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    let mut conflicting = event.clone();
    conflicting.error_code = Some("different_terminal_fact");
    assert!(matches!(
        repository.insert(&conflicting).await,
        Err(ai_gateway::persistence::RepositoryError::DuplicateConflict { .. })
    ));
    let mut invalid_status = request_log_event(&seed, RequestLogOutcome::Failed);
    invalid_status.response_status_code = Some(99);
    assert!(matches!(
        repository.insert(&invalid_status).await,
        Err(ai_gateway::persistence::RepositoryError::InvalidResponseStatus { status: 99 })
    ));

    let (sink, worker) = RequestLogWorker::start(repository, 2);
    let mut invalid = request_log_event(&seed, RequestLogOutcome::Failed);
    invalid.user_id = Uuid::new_v4();
    sink.try_record(invalid);
    let valid = request_log_event(&seed, RequestLogOutcome::Cancelled);
    let valid_id = valid.id;
    sink.try_record(valid);
    for _ in 0..20 {
        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs WHERE id = $1")
            .bind(valid_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
        if persisted == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs WHERE id = $1")
        .bind(valid_id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(persisted, 1);
    // Keep a producer clone alive: shutdown must close acceptance and drain
    // without waiting for it to be dropped.
    worker.shutdown().await;
    database.cleanup().await;
}

const ADMIN_TOKEN: &str = "stage-four-admin-token-with-at-least-32-characters";

fn upstream_defaults() -> UpstreamConfig {
    UpstreamConfig {
        connect_timeout_seconds: 1,
        response_header_timeout_seconds: 2,
        stream_idle_timeout_seconds: 3,
    }
}

async fn admin_app(pool: PgPool, actor: Uuid) -> (Router, Arc<RuntimeConfig>) {
    admin_app_with_models_dev(
        pool,
        actor,
        ModelsDevClient::new(&ModelsSyncConfig::default()).unwrap(),
    )
    .await
}

async fn admin_app_with_models_dev(
    pool: PgPool,
    actor: Uuid,
    models_dev: ModelsDevClient,
) -> (Router, Arc<RuntimeConfig>) {
    let repository = ControlPlaneRepository::new(pool);
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(repository.load().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        upstream_defaults(),
    );
    let model_sync = ModelSyncService::new(coordinator.clone(), models_dev, 100);
    (
        admin::router(AdminState {
            coordinator,
            model_sync,
            actor_user_id: actor,
            verifier: ai_gateway::domain::AdminTokenVerifier::from_token(ADMIN_TOKEN),
        }),
        runtime,
    )
}

async fn admin_request(
    app: Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    admin_request_with_headers(app, method, path, body, &[]).await
}

async fn admin_request_with_headers(
    app: Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {ADMIN_TOKEN}"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.oneshot(request).await.unwrap()
}

#[tokio::test]
async fn admin_key_create_publishes_immediately_and_audits_redacted() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let response = admin_request(
        app,
        "POST",
        "/admin/v1/api-keys",
        serde_json::json!({
            "user_id": seed.user,
            "name": format!("managed-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "expires_at": null
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let secret = created["secret"].as_str().unwrap();
    assert!(runtime.snapshot().authenticate(secret).is_some());
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE action = 'create' AND object_type = 'api_key'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(audit.get("secret_value").is_none());
    assert!(!audit.to_string().contains(secret));
    database.cleanup().await;
}

#[tokio::test]
async fn managed_users_are_versioned_audited_and_immediately_revoke_their_keys() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;

    let created = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/users",
        serde_json::json!({
            "name": format!("managed-user-{}", Uuid::new_v4()),
            "status": "active",
            "currency": "USD"
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let user_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let users = admin_request(app.clone(), "GET", "/admin/v1/users", serde_json::json!({})).await;
    assert_eq!(users.status(), StatusCode::OK);
    let users: serde_json::Value =
        serde_json::from_slice(&users.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        users
            .as_array()
            .unwrap()
            .iter()
            .any(|user| user["id"] == user_id.to_string())
    );

    let key = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/api-keys",
        serde_json::json!({
            "user_id": user_id,
            "name": format!("managed-key-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "expires_at": null
        }),
    )
    .await;
    assert_eq!(key.status(), StatusCode::CREATED);
    let key: serde_json::Value =
        serde_json::from_slice(&key.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let secret = key["secret"].as_str().unwrap().to_owned();
    let old_snapshot = runtime.snapshot();
    assert!(old_snapshot.authenticate(&secret).is_some());

    let path = format!("/admin/v1/users/{user_id}");
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["balance_amount"], "0");
    detail["status"] = serde_json::json!("suspended");
    for field in ["id", "balance_amount", "created_at", "updated_at"] {
        detail.as_object_mut().unwrap().remove(field);
    }

    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &path,
            detail.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert!(old_snapshot.authenticate(&secret).is_some());
    assert!(runtime.snapshot().authenticate(&secret).is_none());

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('before', before_redacted, 'after', after_redacted) FROM audit_logs WHERE object_id=$1 AND object_type='user' AND action='update' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["before"]["status"], "active");
    assert_eq!(audit["after"]["status"], "suspended");
    assert_eq!(audit["after"]["balance_amount"].as_f64(), Some(0.0));

    assert_eq!(
        admin_request_with_headers(app, "PUT", &path, detail, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    database.cleanup().await;
}

#[tokio::test]
async fn managed_models_are_versioned_and_invalid_disable_rolls_back() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let source_model_id = format!("managed-model-{}", Uuid::new_v4());
    let created = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/models",
        serde_json::json!({
            "source_model_id": source_model_id,
            "display_name": "Managed model",
            "provider_name": "test-provider",
            "enabled": true,
            "currency": "USD",
            "price_unit_tokens": 1000000,
            "input_unit_price": "1.25",
            "cached_input_unit_price": "0.25",
            "cache_write_unit_price": "0.50",
            "output_unit_price": "2.50",
            "price_effective_at": "2026-07-18T00:00:00Z",
            "source_payload": {"source": "test"}
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let model_id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let audits_before_invalid: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE object_type='model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        admin_request(
            app.clone(),
            "POST",
            "/admin/v1/models",
            serde_json::json!({
                "source_model_id": format!("invalid-model-{}", Uuid::new_v4()),
                "display_name": "Invalid model",
                "enabled": true,
                "currency": "USD",
                "price_unit_tokens": 1,
                "input_unit_price": 0,
                "cached_input_unit_price": 0,
                "cache_write_unit_price": 0,
                "output_unit_price": 0,
                "price_effective_at": "2026-07-18T00:00:00Z",
                "source_payload": []
            })
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let audits_after_invalid: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE object_type='model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(audits_after_invalid, audits_before_invalid);

    let list = admin_request(
        app.clone(),
        "GET",
        "/admin/v1/models",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == model_id.to_string())
    );

    let path = format!("/admin/v1/models/{model_id}");
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut update: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(update.get("source_payload").is_none());
    update["display_name"] = serde_json::json!("Updated managed model");
    for field in ["id", "last_synced_at", "created_at", "updated_at"] {
        update.as_object_mut().unwrap().remove(field);
    }
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &path,
            update.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        admin_request_with_headers(app.clone(), "PUT", &path, update, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_id=$1 AND object_type='model' AND action='update' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(model_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["display_name"], "Updated managed model");
    assert!(audit.get("source_payload").is_none());
    let source_payload: serde_json::Value =
        sqlx::query_scalar("SELECT source_payload FROM models WHERE id=$1")
            .bind(model_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(source_payload, serde_json::json!({"source": "test"}));

    let seed_path = format!("/admin/v1/models/{}", seed.model);
    let seed_detail = admin_request(app.clone(), "GET", &seed_path, serde_json::json!({})).await;
    let seed_etag = seed_detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut disable: serde_json::Value =
        serde_json::from_slice(&seed_detail.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    disable["enabled"] = serde_json::json!(false);
    for field in ["id", "last_synced_at", "created_at", "updated_at"] {
        disable.as_object_mut().unwrap().remove(field);
    }
    let published = runtime.snapshot();
    let audit_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE object_type='model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        admin_request_with_headers(app, "PUT", &seed_path, disable, &[("if-match", &seed_etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM models WHERE id=$1")
        .bind(seed.model)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(enabled);
    let audit_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_logs WHERE object_type='model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(audit_after, audit_before);
    assert!(Arc::ptr_eq(&published, &runtime.snapshot()));
    database.cleanup().await;
}

#[tokio::test]
async fn models_dev_preview_and_selected_sync_upsert_prices_without_leaking_metadata() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let catalog_server =
        start_server(Router::new().route("/api.json", get(models_dev_catalog))).await;
    let models_dev = ModelsDevClient::new(&ModelsSyncConfig {
        api_url: format!("http://{}/api.json", catalog_server.address),
        request_timeout_seconds: 1,
        max_response_bytes: 1_024 * 1_024,
        max_model_metadata_bytes: 1_024,
        max_selections: 10,
    })
    .unwrap();
    let (app, _) = admin_app_with_models_dev(database.pool.clone(), seed.user, models_dev).await;

    let preview = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/models/sync/preview",
        serde_json::json!({"provider_ids":["provider-a"]}),
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview: serde_json::Value =
        serde_json::from_slice(&preview.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(preview["models"].as_array().unwrap().len(), 1);
    assert_eq!(preview["models"][0]["provider_id"], "provider-a");
    assert_eq!(preview["models"][0]["model_id"], "catalog-model");
    assert_eq!(preview["excluded_missing_prices"], 1);
    assert!(preview["models"][0].get("source_payload").is_none());

    let sync = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/models/sync",
        serde_json::json!({
            "selections":[{"provider_id":"provider-a","model_id":"catalog-model"}]
        }),
    )
    .await;
    assert_eq!(sync.status(), StatusCode::OK);
    let sync: serde_json::Value =
        serde_json::from_slice(&sync.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(sync["model_count"], 1);

    let persisted: (
        String,
        Option<String>,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
        i64,
    ) = sqlx::query_as(
        "SELECT source_model_id,provider_name,input_unit_price,cached_input_unit_price,cache_write_unit_price,output_unit_price,price_unit_tokens FROM models WHERE source_model_id='catalog-model'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(persisted.0, "catalog-model");
    assert_eq!(persisted.1.as_deref(), Some("Provider A"));
    assert_eq!(persisted.2, rust_decimal::Decimal::new(125, 2));
    assert_eq!(persisted.3, rust_decimal::Decimal::new(25, 2));
    assert_eq!(persisted.4, rust_decimal::Decimal::new(50, 2));
    assert_eq!(persisted.5, rust_decimal::Decimal::new(250, 2));
    assert_eq!(persisted.6, 1_000_000);
    let source_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT source_payload FROM models WHERE source_model_id='catalog-model'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(source_payload["source"], "models.dev");
    assert_eq!(source_payload["provider_id"], "provider-a");

    let model_id: Uuid =
        sqlx::query_scalar("SELECT id FROM models WHERE source_model_id='catalog-model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let detail = admin_request(
        app.clone(),
        "GET",
        &format!("/admin/v1/models/{model_id}"),
        serde_json::json!({}),
    )
    .await;
    let detail = serde_json::from_slice::<serde_json::Value>(
        &detail.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    assert!(detail.get("source_payload").is_none());
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_id=$1 AND object_type='model' AND action='sync'",
    )
    .bind(model_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(audit.get("source_payload").is_none());

    let audits_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE object_type='model' AND action='sync'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    let rejected = admin_request(
        app,
        "POST",
        "/admin/v1/models/sync",
        serde_json::json!({
            "selections":[{"provider_id":"provider-a","model_id":"missing-price"}]
        }),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let audits_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE object_type='model' AND action='sync'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audits_after, audits_before);
    database.cleanup().await;
}

#[tokio::test]
async fn admin_api_key_policies_persist_publish_and_are_audited_without_secrets() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let created = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/api-keys",
        serde_json::json!({
            "user_id": seed.user,
            "name": format!("policy-managed-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "expires_at": null,
            "requests_per_minute": 7,
            "max_concurrent_requests": 3,
            "quota_limit_amount": 125.50
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    let secret = created["secret"].as_str().unwrap().to_owned();

    let persisted: (Option<i32>, Option<i32>, Option<rust_decimal::Decimal>) = sqlx::query_as(
        "SELECT requests_per_minute, max_concurrent_requests, quota_limit_amount FROM api_keys WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        persisted,
        (
            Some(7),
            Some(3),
            Some(rust_decimal::Decimal::new(12_550, 2)),
        )
    );
    let compiled = runtime.snapshot().authenticate(&secret).unwrap();
    assert_eq!(compiled.requests_per_minute(), Some(7));
    assert_eq!(compiled.max_concurrent_requests(), Some(3));
    assert_eq!(
        compiled.quota_limit_amount(),
        Some(rust_decimal::Decimal::new(12_550, 2))
    );
    assert!(!compiled.quota_exhausted());

    let list = admin_request(
        app.clone(),
        "GET",
        "/admin/v1/api-keys",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let listed = list
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id.to_string())
        .unwrap();
    assert_eq!(listed["requests_per_minute"], 7);
    assert_eq!(listed["max_concurrent_requests"], 3);
    assert_eq!(listed["quota_limit_amount"], "125.50000000");
    assert_eq!(listed["quota_used_amount"], "0");
    assert!(listed["tokens_per_minute"].is_null());

    let path = format!("/admin/v1/api-keys/{id}");
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["requests_per_minute"], 7);
    assert_eq!(detail["max_concurrent_requests"], 3);
    assert_eq!(detail["quota_limit_amount"], "125.50000000");
    assert_eq!(detail["quota_used_amount"], "0");
    assert!(detail["tokens_per_minute"].is_null());

    let updated = admin_request_with_headers(
        app.clone(),
        "PUT",
        &path,
        serde_json::json!({
            "name": detail["name"].clone(),
            "status": "active",
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "expires_at": null,
            "requests_per_minute": 9,
            "max_concurrent_requests": 4,
            "quota_limit_amount": 200.00
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let persisted: (Option<i32>, Option<i32>, Option<rust_decimal::Decimal>) = sqlx::query_as(
        "SELECT requests_per_minute, max_concurrent_requests, quota_limit_amount FROM api_keys WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        persisted,
        (
            Some(9),
            Some(4),
            Some(rust_decimal::Decimal::new(20_000, 2)),
        )
    );
    let compiled = runtime.snapshot().authenticate(&secret).unwrap();
    assert_eq!(compiled.requests_per_minute(), Some(9));
    assert_eq!(compiled.max_concurrent_requests(), Some(4));
    assert_eq!(
        compiled.quota_limit_amount(),
        Some(rust_decimal::Decimal::new(20_000, 2))
    );

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('before', before_redacted, 'after', after_redacted) FROM audit_logs WHERE object_id=$1 AND action='update' AND object_type='api_key'",
    )
    .bind(id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["before"]["requests_per_minute"], 7);
    assert_eq!(audit["before"]["max_concurrent_requests"], 3);
    assert_eq!(audit["before"]["quota_limit_amount"], 125.50000000);
    assert_eq!(audit["after"]["requests_per_minute"], 9);
    assert_eq!(audit["after"]["max_concurrent_requests"], 4);
    assert_eq!(audit["after"]["quota_limit_amount"], 200.00000000);
    assert!(audit["before"].get("secret_value").is_none());
    assert!(audit["after"].get("secret_value").is_none());
    assert!(!audit.to_string().contains(&secret));

    let token_edit = admin_request(
        app,
        "POST",
        "/admin/v1/api-keys",
        serde_json::json!({
            "user_id": seed.user,
            "name": format!("token-edit-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "expires_at": null,
            "requests_per_minute": 1,
            "max_concurrent_requests": 1,
            "quota_limit_amount": 1,
            "tokens_per_minute": 1
        }),
    )
    .await;
    assert_eq!(token_edit.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

#[tokio::test]
async fn invalid_admin_mutation_rolls_back_database_audit_and_snapshot() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let response = admin_request(
        app,
        "PUT",
        &format!("/admin/v1/channels/{}", seed.channel),
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
            "enabled": false, "auto_disabled": false, "weight": 1,
            "upstream_auth_kind": "bearer", "upstream_api_key": "upstream-secret",
            "available_models": ["upstream-v1"]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM channels WHERE id=$1")
        .bind(seed.channel)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(enabled);
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audit_before, audit_after);
    assert!(runtime.snapshot().channel(seed.channel).is_some());
    database.cleanup().await;
}

#[tokio::test]
async fn proxy_template_management_is_redacted_and_publishes_or_rolls_back_atomically() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let proxy_password = "created-proxy-password";
    let proxy_username = "created-proxy-user";
    let template_value = "template-document-private-value";
    let channel_value = "channel-override-private-value";

    let created_proxy = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/proxies",
        serde_json::json!({
            "name": "managed-proxy",
            "proxy_url": "https://managed-proxy.test:8443",
            "username": proxy_username,
            "password": proxy_password,
            "no_proxy_hosts": ["internal.test"],
            "enabled": true
        }),
    )
    .await;
    assert_eq!(created_proxy.status(), StatusCode::CREATED);
    let created_proxy: serde_json::Value = serde_json::from_slice(
        &created_proxy
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let proxy_id: Uuid = serde_json::from_value(created_proxy["id"].clone()).unwrap();

    let template_document = serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "request_headers": {"set": {"x-template": template_value}}
    });
    let created_template = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/config-templates",
        serde_json::json!({
            "name": "managed-template",
            "description": "managed template",
            "document": template_document,
            "enabled": true
        }),
    )
    .await;
    assert_eq!(created_template.status(), StatusCode::CREATED);
    let created_template: serde_json::Value = serde_json::from_slice(
        &created_template
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    let template_id: Uuid = serde_json::from_value(created_template["id"].clone()).unwrap();

    let proxy_list = admin_request(
        app.clone(),
        "GET",
        "/admin/v1/proxies",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(proxy_list.status(), StatusCode::OK);
    let proxy_list: serde_json::Value =
        serde_json::from_slice(&proxy_list.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert!(
        proxy_list
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == proxy_id.to_string())
    );
    assert!(!proxy_list.to_string().contains(proxy_password));
    assert!(!proxy_list.to_string().contains(proxy_username));

    let proxy_path = format!("/admin/v1/proxies/{proxy_id}");
    let proxy_detail = admin_request(app.clone(), "GET", &proxy_path, serde_json::json!({})).await;
    assert_eq!(proxy_detail.status(), StatusCode::OK);
    let proxy_etag = proxy_detail.headers()["etag"].to_str().unwrap().to_owned();
    let proxy_detail: serde_json::Value =
        serde_json::from_slice(&proxy_detail.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(proxy_detail["credential_configured"], true);
    assert!(proxy_detail.get("username").is_none());
    assert!(proxy_detail.get("password").is_none());
    assert!(!proxy_detail.to_string().contains(proxy_password));

    let updated_proxy = admin_request_with_headers(
        app.clone(),
        "PUT",
        &proxy_path,
        serde_json::json!({
            "name": "managed-proxy-updated",
            "proxy_url": "https://managed-proxy.test:9443",
            "password": "updated-proxy-password",
            "no_proxy_hosts": ["internal.test", "metadata.test"],
            "enabled": true
        }),
        &[("if-match", &proxy_etag)],
    )
    .await;
    assert_eq!(updated_proxy.status(), StatusCode::OK);

    let template_list = admin_request(
        app.clone(),
        "GET",
        "/admin/v1/config-templates",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(template_list.status(), StatusCode::OK);
    let template_list: serde_json::Value = serde_json::from_slice(
        &template_list
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert!(
        template_list
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == template_id.to_string())
    );
    assert!(!template_list.to_string().contains(template_value));

    let template_path = format!("/admin/v1/config-templates/{template_id}");
    let template_detail =
        admin_request(app.clone(), "GET", &template_path, serde_json::json!({})).await;
    assert_eq!(template_detail.status(), StatusCode::OK);
    let template_etag = template_detail.headers()["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    let template_detail: serde_json::Value = serde_json::from_slice(
        &template_detail
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert!(template_detail.get("document").is_none());
    assert!(!template_detail.to_string().contains(template_value));
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &template_path,
            serde_json::json!({
                "name": "managed-template-updated",
                "description": "updated template",
                "document": template_document,
                "enabled": true
            }),
            &[("if-match", &template_etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );

    let channel_path = format!("/admin/v1/channels/{}", seed.channel);
    let channel_detail =
        admin_request(app.clone(), "GET", &channel_path, serde_json::json!({})).await;
    let channel_etag = channel_detail.headers()["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    let valid_channel = serde_json::json!({
        "channel_group_id": seed.group,
        "api_format": "open_ai_chat_completions",
        "name": format!("test-channel-{}", seed.channel),
        "base_url": "https://example.test",
        "enabled": true,
        "weight": 1,
        "proxy_id": proxy_id,
        "config_template_id": template_id,
        "override_document": {
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-channel": channel_value}}
        },
        "connect_timeout_ms": 11,
        "response_header_timeout_ms": 22,
        "stream_idle_timeout_ms": 33,
        "upstream_auth_kind": "bearer",
        "available_models": ["upstream-v1"]
    });
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &channel_path,
            valid_channel.clone(),
            &[("if-match", &channel_etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );
    let published = runtime.snapshot();
    let channel = published.channel(seed.channel).unwrap();
    let policy = channel.upstream_policy();
    assert_eq!(policy.proxy().unwrap().id(), proxy_id);
    assert_eq!(policy.template().unwrap().id(), template_id);
    assert_eq!(policy.timeouts().connect(), Some(Duration::from_millis(11)));
    assert_eq!(
        policy.timeouts().response_header(),
        Some(Duration::from_millis(22))
    );
    assert_eq!(
        policy.timeouts().stream_idle(),
        Some(Duration::from_millis(33))
    );
    assert_eq!(
        policy
            .effective_transforms()
            .request_headers()
            .operations()
            .len(),
        2
    );
    let channel_read =
        admin_request(app.clone(), "GET", &channel_path, serde_json::json!({})).await;
    assert_eq!(channel_read.status(), StatusCode::OK);
    let channel_read: serde_json::Value =
        serde_json::from_slice(&channel_read.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert!(channel_read.get("override_document").is_none());
    assert!(!channel_read.to_string().contains(channel_value));

    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_agg(jsonb_build_object('before', before_redacted, 'after', after_redacted)) FROM audit_logs WHERE object_id = ANY($1)",
    )
    .bind(vec![proxy_id, template_id, seed.channel])
    .fetch_one(&database.pool)
    .await
    .unwrap();
    let audit = audit.to_string();
    for secret in [
        proxy_password,
        proxy_username,
        template_value,
        channel_value,
    ] {
        assert!(!audit.contains(secret), "audit leaked {secret}");
    }
    assert!(!audit.contains("password"));
    assert!(!audit.contains("username"));
    assert!(!audit.contains("document"));
    assert!(!audit.contains("override_document"));

    let current = admin_request(app.clone(), "GET", &channel_path, serde_json::json!({})).await;
    let stable_etag = current.headers()["etag"].to_str().unwrap().to_owned();
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let proxy_count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM proxies")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let template_count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM config_templates")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let invalid_channel_inputs = [
        serde_json::json!({"proxy_id": Uuid::new_v4()}),
        serde_json::json!({"connect_timeout_ms": 0}),
        serde_json::json!({"response_header_timeout_ms": 10}),
    ];
    for changes in invalid_channel_inputs {
        let mut invalid = valid_channel.clone();
        for (key, value) in changes.as_object().unwrap() {
            invalid[key] = value.clone();
        }
        let response = admin_request_with_headers(
            app.clone(),
            "PUT",
            &channel_path,
            invalid,
            &[("if-match", &stable_etag)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            !std::str::from_utf8(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap()
                .contains(channel_value)
        );
    }
    for (path, body, secret) in [
        (
            "/admin/v1/proxies",
            serde_json::json!({"name":"invalid-proxy", "proxy_url":"ftp://invalid.test", "password":"invalid-proxy-password", "enabled":true}),
            "invalid-proxy-password",
        ),
        (
            "/admin/v1/config-templates",
            serde_json::json!({"name":"invalid-template", "document":{"version":1,"api_format":"open_ai_chat_completions","unknown":"invalid-document-value"}, "enabled":true}),
            "invalid-document-value",
        ),
    ] {
        let response = admin_request(app.clone(), "POST", path, body).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(
            !std::str::from_utf8(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap()
                .contains(secret)
        );
    }
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audit_after, audit_before);
    assert_eq!(
        sqlx::query_scalar::<_, Option<i32>>(
            "SELECT response_header_timeout_ms FROM channels WHERE id=$1",
        )
        .bind(seed.channel)
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        Some(22)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM proxies")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        proxy_count_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM config_templates")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        template_count_before
    );
    assert!(Arc::ptr_eq(&published, &runtime.snapshot()));
    database.cleanup().await;
}

#[tokio::test]
async fn revoked_api_key_cannot_be_reactivated() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let revoked = admin_request(
        app.clone(),
        "POST",
        &format!("/admin/v1/api-keys/{}/revoke", seed.key),
        serde_json::json!({"reason":"incident"}),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    let updated = admin_request(app, "PUT", &format!("/admin/v1/api-keys/{}", seed.key), serde_json::json!({
        "name":"test", "status":"active", "allowed_api_formats":["open_ai_chat_completions"],
        "permissions":["proxy","models.read"], "allowed_group_ids":[seed.group], "expires_at":null
    })).await;
    assert_eq!(updated.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(seed.key)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "revoked");
    database.cleanup().await;
}

#[tokio::test]
async fn overly_long_revoke_reason_is_a_safe_unprocessable_response() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let response = admin_request(
        app,
        "POST",
        &format!("/admin/v1/api-keys/{}/revoke", seed.key),
        serde_json::json!({"reason": "x".repeat(501)}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        body.as_ref(),
        br#"{"error":"management operation rejected"}"#
    );
    let status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(seed.key)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    database.cleanup().await;
}

#[tokio::test]
async fn management_channel_credentials_are_redacted_kept_replaced_and_cleared_safely() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let path = format!("/admin/v1/channels/{}", seed.channel);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["upstream_credential_configured"], true);
    assert!(!detail.to_string().contains("upstream-secret"));

    let update = |credential: serde_json::Value| {
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
            "enabled": true, "weight": 1, "upstream_auth_kind": "bearer",
            "available_models": ["upstream-v1"], "upstream_api_key": credential
        })
    };
    let keep = admin_request_with_headers(
        app.clone(),
        "PUT",
        &path,
        update(serde_json::json!("replaced-secret")),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(keep.status(), StatusCode::OK);
    let secret: String = sqlx::query_scalar("SELECT upstream_api_key FROM channels WHERE id=$1")
        .bind(seed.channel)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(secret, "replaced-secret");
    let current = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = current.headers()["etag"].to_str().unwrap().to_owned();
    let keep = admin_request_with_headers(app.clone(), "PUT", &path, serde_json::json!({
        "channel_group_id": seed.group, "api_format": "open_ai_chat_completions", "name": format!("test-channel-{}", seed.channel),
        "base_url": "https://example.test", "enabled": true, "weight": 1, "upstream_auth_kind": "bearer", "available_models": ["upstream-v1"]
    }), &[("if-match", &etag)]).await;
    assert_eq!(keep.status(), StatusCode::OK);
    let secret: String = sqlx::query_scalar("SELECT upstream_api_key FROM channels WHERE id=$1")
        .bind(seed.channel)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(secret, "replaced-secret");
    let current = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = current.headers()["etag"].to_str().unwrap().to_owned();
    let invalid_clear = admin_request_with_headers(
        app,
        "PUT",
        &path,
        update(serde_json::Value::Null),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(invalid_clear.status(), StatusCode::UNPROCESSABLE_ENTITY);
    database.cleanup().await;
}

#[tokio::test]
async fn channel_documents_are_rejected_and_never_escape_admin_or_audit_allowlists() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let path = format!("/admin/v1/channels/{}", seed.channel);
    let audits_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let rejected_create = admin_request(
        app.clone(),
        "POST",
        "/admin/v1/channels",
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("rejected-channel-{}", Uuid::new_v4()),
            "base_url": "https://example.test", "enabled": true, "weight": 1,
            "upstream_auth_kind": "bearer", "upstream_api_key": "upstream-secret",
            "available_models": ["upstream-v1"],
            "override_document": {"headers": {"Authorization": "rejected-create-secret"}},
            "health_check": {"body": {"cookie": "rejected-create-cookie"}}
        }),
    )
    .await;
    assert_eq!(rejected_create.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let rejected_create = rejected_create
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let rejected_create = std::str::from_utf8(&rejected_create).unwrap();
    assert!(!rejected_create.contains("Authorization"));
    assert!(!rejected_create.contains("cookie"));
    assert!(!rejected_create.contains("body"));
    let audits_after_create: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audits_after_create, audits_before);
    let persisted_override = serde_json::json!({
        "headers": {"Authorization": "nested-authorization-secret", "cookie": "nested-cookie-secret"},
        "body": {"token": "nested-body-secret"}
    });
    let persisted_health = serde_json::json!({
        "request": {"Authorization": "health-authorization-secret", "cookie": "health-cookie-secret"},
        "body": {"token": "health-body-secret"}
    });
    sqlx::query("UPDATE channels SET override_document=$1, health_check=$2 WHERE id=$3")
        .bind(&persisted_override)
        .bind(&persisted_health)
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();

    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let rendered = detail.to_string();
    assert!(detail.get("override_document").is_none());
    assert!(detail.get("health_check").is_none());
    for forbidden in [
        "Authorization",
        "cookie",
        "body",
        "nested-authorization-secret",
        "nested-cookie-secret",
        "nested-body-secret",
        "health-authorization-secret",
        "health-cookie-secret",
        "health-body-secret",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "public response leaked {forbidden}"
        );
    }

    let valid_update = serde_json::json!({
        "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
        "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
        "enabled": true, "weight": 1, "upstream_auth_kind": "bearer",
        "available_models": ["upstream-v1"], "upstream_api_key": "upstream-secret"
    });
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &path,
            valid_update,
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('before', before_redacted, 'after', after_redacted) FROM audit_logs WHERE object_id=$1 AND object_type='channel' AND action='update' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(seed.channel)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    let audit = audit.to_string();
    for forbidden in [
        "override_document",
        "health_check",
        "Authorization",
        "cookie",
        "body",
        "nested-authorization-secret",
        "nested-cookie-secret",
        "nested-body-secret",
        "health-authorization-secret",
        "health-cookie-secret",
        "health-body-secret",
    ] {
        assert!(
            !audit.contains(forbidden),
            "audit snapshot leaked {forbidden}"
        );
    }

    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let rejected_update = serde_json::json!({
        "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
        "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
        "enabled": true, "weight": 1, "upstream_auth_kind": "bearer",
        "available_models": ["upstream-v1"], "upstream_api_key": "upstream-secret",
        "override_document": {"headers": {"Authorization": "rejected-secret"}},
        "health_check": {"body": {"cookie": "rejected-cookie"}}
    });
    let audits_before_update: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        admin_request_with_headers(app, "PUT", &path, rejected_update, &[("if-match", &etag)],)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let audits_after_update: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audits_after_update, audits_before_update);
    database.cleanup().await;
}

#[tokio::test]
async fn full_put_requires_current_etag_and_stale_update_does_not_audit() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let path = format!("/admin/v1/api-keys/{}", seed.key);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let input = serde_json::json!({"name":"updated", "status":"active", "allowed_api_formats":["open_ai_chat_completions"], "permissions":["proxy"], "allowed_group_ids":[seed.group], "expires_at":null});
    assert_eq!(
        admin_request(app.clone(), "PUT", &path, input.clone())
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &path,
            input.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE object_id=$1 AND action='update'",
    )
    .bind(seed.key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        admin_request_with_headers(app, "PUT", &path, input, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE object_id=$1 AND action='update'",
    )
    .bind(seed.key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(after, audits);
    database.cleanup().await;
}

#[tokio::test]
async fn disabled_legacy_key_tpm_is_visible_audited_and_cannot_be_activated() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE api_keys SET status='disabled', tokens_per_minute=42 WHERE id=$1")
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let list = admin_request(
        app.clone(),
        "GET",
        "/admin/v1/api-keys",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(list.status(), StatusCode::OK);
    let list: serde_json::Value =
        serde_json::from_slice(&list.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == seed.key.to_string())
            .unwrap()["tokens_per_minute"],
        42
    );

    let path = format!("/admin/v1/api-keys/{}", seed.key);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["tokens_per_minute"], 42);
    let input = serde_json::json!({
        "name":"test", "status":"disabled",
        "allowed_api_formats":["open_ai_chat_completions"], "permissions":["proxy","models.read"],
        "allowed_group_ids":[seed.group], "expires_at":null,
        "requests_per_minute":null, "max_concurrent_requests":null, "quota_limit_amount":null
    });
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &path,
            input.clone(),
            &[("if-match", &etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_id=$1 AND action='update' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(seed.key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(audit["tokens_per_minute"], 42);

    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let mut activation = input;
    activation["status"] = serde_json::json!("active");
    assert_eq!(
        admin_request_with_headers(app, "PUT", &path, activation, &[("if-match", &etag)])
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(seed.key)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "disabled");
    database.cleanup().await;
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
async fn migrated_soft_quota_allows_over_limit_usage_and_rejects_the_seeded_key() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE api_keys SET quota_limit_amount=$1, quota_used_amount=$2 WHERE id=$3")
        .bind(rust_decimal::Decimal::new(100, 2))
        .bind(rust_decimal::Decimal::new(101, 2))
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .expect("0003 must allow settled usage above the soft quota limit");

    let snapshot = compile_control_plane(
        ControlPlaneRepository::new(database.pool.clone())
            .load()
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        snapshot
            .authenticate(&seed.secret)
            .unwrap()
            .quota_exhausted()
    );
    let proxy = ProxyService::new(
        Arc::new(RuntimeConfig::new(snapshot)),
        1_048_576,
        &ai_gateway::runtime_config::UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    )
    .unwrap();
    let response = ai_gateway::http::router(proxy)
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("authorization", format!("Bearer {}", seed.secret))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({"model": seed.client_model})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], "insufficient_quota");
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
async fn disabled_rules_still_require_structurally_valid_targets() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query("UPDATE model_rules SET enabled=false, channel_ids=ARRAY[$1]::uuid[] WHERE id=$2")
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
    let reloader = ControlPlaneReloader::new(repository, Arc::clone(&runtime), upstream_defaults());
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
async fn invalid_effective_upstream_policy_preserves_reload_manual_reload_and_snapshot_state() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(repository.load().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        upstream_defaults(),
    );
    let published = runtime.snapshot();
    let audits_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE channels SET connect_timeout_ms = 2000, response_header_timeout_ms = 1000 WHERE id = $1",
    )
    .bind(seed.channel)
    .execute(&database.pool)
    .await
    .unwrap();

    assert!(coordinator.reload().await.is_err());
    assert!(coordinator.manual_reload(seed.user).await.is_err());

    let timeouts: (Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT connect_timeout_ms, response_header_timeout_ms FROM channels WHERE id = $1",
    )
    .bind(seed.channel)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(timeouts, (Some(2000), Some(1000)));
    let audits_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audits_after, audits_before);
    assert!(Arc::ptr_eq(&published, &runtime.snapshot()));
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
    let reloader = ControlPlaneReloader::new(repository, Arc::clone(&runtime), upstream_defaults());
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
async fn admission_controls_are_compiled_and_invalid_channel_targets_are_rejected() {
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

#[tokio::test]
async fn proxy_request_logs_reach_postgres_for_terminal_and_rejected_requests() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let state = UpstreamState(Arc::new(Mutex::new(UpstreamMode::Immediate(
        StatusCode::OK,
    ))));
    let upstream_server = start_server(
        Router::new()
            .route("/v1/chat/completions", post(upstream))
            .with_state(state.clone()),
    )
    .await;
    sqlx::query("UPDATE channels SET base_url = $1 WHERE id = $2")
        .bind(format!("http://{}", upstream_server.address))
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();

    let inaccessible_key = Uuid::new_v4();
    let inaccessible_secret = format!("inaccessible-{}", Uuid::new_v4());
    sqlx::query("INSERT INTO api_keys (id, user_id, name, secret_value, status, allowed_api_formats, permissions, allowed_group_ids) VALUES ($1, $2, 'inaccessible', $3, 'active', ARRAY['open_ai_chat_completions']::api_format[], ARRAY['proxy'], ARRAY[$4]::uuid[])")
        .bind(inaccessible_key)
        .bind(seed.user)
        .bind(&inaccessible_secret)
        .bind(seed.other_group)
        .execute(&database.pool)
        .await
        .unwrap();

    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(
            ControlPlaneRepository::new(database.pool.clone())
                .load()
                .await
                .unwrap(),
        )
        .unwrap(),
    ));
    let (sink, worker): (QueueRequestLogSink, RequestLogWorker) =
        RequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), 32);
    let proxy = ProxyService::with_log_sink(
        runtime,
        1_048_576,
        &ai_gateway::runtime_config::UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        Arc::new(sink),
    )
    .unwrap();
    let gateway = start_server(ai_gateway::http::router(proxy)).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
        .unwrap();
    let request = |key: &str, model: &str, stream: bool| {
        client
            .post(format!("http://{}/v1/chat/completions", gateway.address))
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "model": model, "stream": stream }))
                    .unwrap(),
            )
    };

    *state.0.lock().unwrap() = UpstreamMode::TwoChunks;
    let response = request(&seed.secret, &seed.client_model, true)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"firstsecond");
    let success = wait_for_log(&database.pool, seed.key, &seed.client_model).await;
    assert_eq!(success.outcome, "succeeded");
    assert_eq!(success.user_id, seed.user);
    assert_eq!(success.api_key_id, seed.key);
    assert_eq!(success.api_format, "open_ai_chat_completions");
    assert_eq!(success.client_model, seed.client_model);
    assert_eq!(success.upstream_model.as_deref(), Some("upstream-v1"));
    assert_eq!(success.model_rule_id, Some(seed.rule));
    assert_eq!(success.channel_group_id, Some(seed.group));
    assert_eq!(success.channel_id, Some(seed.channel));
    assert_eq!(success.model_id, Some(seed.model));
    assert_eq!(success.response_status_code, Some(200));
    assert!(success.streamed);
    assert!(success.ttft_ms.is_some());
    assert_eq!(success.error_code, None);

    *state.0.lock().unwrap() = UpstreamMode::Immediate(StatusCode::TOO_MANY_REQUESTS);
    let response = request(&seed.secret, &seed.client_model, false)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"upstream");
    let upstream_failure = wait_for_terminal_log(
        &database.pool,
        seed.key,
        &seed.client_model,
        "failed",
        Some("upstream_http_error"),
    )
    .await;
    assert_eq!(upstream_failure.response_status_code, Some(429));

    *state.0.lock().unwrap() = UpstreamMode::HeaderDelay;
    let response = request(&seed.secret, &seed.client_model, false)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let header_timeout = wait_for_terminal_log(
        &database.pool,
        seed.key,
        &seed.client_model,
        "failed",
        Some("response_header_timeout"),
    )
    .await;
    assert_eq!(header_timeout.outcome, "failed");
    assert_eq!(header_timeout.response_status_code, Some(504));
    assert_eq!(
        header_timeout.error_code.as_deref(),
        Some("response_header_timeout")
    );

    *state.0.lock().unwrap() = UpstreamMode::OneChunkThenIdle;
    let response = request(&seed.secret, &seed.client_model, false)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.bytes().await.is_err());
    let idle = wait_for_terminal_log(
        &database.pool,
        seed.key,
        &seed.client_model,
        "failed",
        Some("stream_idle_timeout"),
    )
    .await;
    assert_eq!(idle.response_status_code, Some(200));

    *state.0.lock().unwrap() = UpstreamMode::OneChunkThenIdle;
    let response = request(&seed.secret, &seed.client_model, false)
        .send()
        .await
        .unwrap();
    let mut chunks = response.bytes_stream();
    assert_eq!(chunks.next().await.unwrap().unwrap().as_ref(), b"first");
    drop(chunks);
    let cancelled = wait_for_terminal_log(
        &database.pool,
        seed.key,
        &seed.client_model,
        "cancelled",
        Some("client_cancelled"),
    )
    .await;
    assert_eq!(cancelled.response_status_code, Some(200));

    let unknown = format!("unknown-{}", Uuid::new_v4());
    let response = request(&seed.secret, &unknown, false).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let rejected = wait_for_log(&database.pool, seed.key, &unknown).await;
    assert_eq!(rejected.outcome, "rejected");
    assert_eq!(rejected.response_status_code, Some(404));
    assert_eq!(rejected.upstream_model, None);
    assert_eq!(rejected.model_rule_id, None);
    assert_eq!(rejected.channel_id, None);

    let response = request(&inaccessible_secret, &seed.client_model, false)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let inaccessible = wait_for_log(&database.pool, inaccessible_key, &seed.client_model).await;
    assert_eq!(inaccessible.outcome, "rejected");
    assert_eq!(inaccessible.api_key_id, inaccessible_key);
    assert_eq!(inaccessible.channel_group_id, None);

    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .post(format!("http://{}/v1/chat/completions", gateway.address))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({"model": seed.client_model})).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&seed.secret, " ", false)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(&seed.secret, &"x".repeat(301), false)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(before, after);

    worker.shutdown().await;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(total, 7);
    let grouped = sqlx::query_as::<_, TerminalLogCount>(
        "SELECT api_key_id, client_model, outcome, error_code, count(*) AS count FROM request_logs GROUP BY api_key_id, client_model, outcome, error_code",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap()
    .into_iter()
    .map(|row| {
        (
            (
                row.api_key_id,
                row.client_model,
                row.outcome,
                row.error_code,
            ),
            row.count,
        )
    })
    .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        (
            (
                seed.key,
                seed.client_model.clone(),
                "succeeded".into(),
                None,
            ),
            1,
        ),
        (
            (
                seed.key,
                seed.client_model.clone(),
                "failed".into(),
                Some("upstream_http_error".into()),
            ),
            1,
        ),
        (
            (
                seed.key,
                seed.client_model.clone(),
                "failed".into(),
                Some("response_header_timeout".into()),
            ),
            1,
        ),
        (
            (
                seed.key,
                seed.client_model.clone(),
                "failed".into(),
                Some("stream_idle_timeout".into()),
            ),
            1,
        ),
        (
            (
                seed.key,
                seed.client_model.clone(),
                "cancelled".into(),
                Some("client_cancelled".into()),
            ),
            1,
        ),
        (
            (
                seed.key,
                unknown,
                "rejected".into(),
                Some("model_not_found".into()),
            ),
            1,
        ),
        (
            (
                inaccessible_key,
                seed.client_model.clone(),
                "rejected".into(),
                Some("model_not_found".into()),
            ),
            1,
        ),
    ]);
    assert_eq!(grouped, expected);
    drop(gateway);
    drop(upstream_server);
    database.cleanup().await;
}

#[tokio::test]
async fn saturated_request_log_queue_does_not_delay_proxy_responses_and_drains_accepted_events() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let state = UpstreamState(Arc::new(Mutex::new(UpstreamMode::Immediate(
        StatusCode::OK,
    ))));
    let upstream_server = start_server(
        Router::new()
            .route("/v1/chat/completions", post(upstream))
            .with_state(state),
    )
    .await;
    sqlx::query("UPDATE channels SET base_url = $1 WHERE id = $2")
        .bind(format!("http://{}", upstream_server.address))
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(
            ControlPlaneRepository::new(database.pool.clone())
                .load()
                .await
                .unwrap(),
        )
        .unwrap(),
    ));
    let (sink, worker): (QueueRequestLogSink, RequestLogWorker) =
        RequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), 2);
    // Keeping this clone alive verifies shutdown closes acceptance before draining.
    let producer_clone = sink.clone();
    let proxy = ProxyService::with_log_sink(
        runtime,
        1_048_576,
        &ai_gateway::runtime_config::UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        Arc::new(sink),
    )
    .unwrap();
    let gateway = start_server(ai_gateway::http::router(proxy)).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let send_request = || async {
        let response = client
            .post(format!("http://{}/v1/chat/completions", gateway.address))
            .header("authorization", format!("Bearer {}", seed.secret))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "model": seed.client_model })).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"upstream");
    };

    let mut table_lock = database.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE request_logs IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *table_lock)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), send_request())
        .await
        .expect("first proxy response should not wait for request-log persistence");
    wait_for_blocked_request_log_insert(&database.pool).await;

    for _ in 0..3 {
        tokio::time::timeout(Duration::from_secs(2), send_request())
            .await
            .expect("proxy response should not wait for a saturated request-log queue");
    }

    table_lock.rollback().await.unwrap();
    worker.shutdown().await;
    drop(producer_clone);
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM request_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        total, 3,
        "three accepted events must drain; the overflow event drops"
    );
    let accepted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM request_logs WHERE api_key_id = $1 AND client_model = $2 AND outcome = 'succeeded'",
    )
    .bind(seed.key)
    .bind(&seed.client_model)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(accepted, 3);
    drop(gateway);
    drop(upstream_server);
    database.cleanup().await;
}
