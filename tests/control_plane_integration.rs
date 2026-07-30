use std::{
    collections::BTreeMap,
    env, io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    admission::AdmissionRuntime,
    application::{
        AutomaticDisableWorker, CODEX_ORIGINATOR, ChannelModelDiscoveryService,
        CodexConnectorService, ConsoleAuthService, ControlPlaneCoordinator, ModelSyncService,
        NoopRequestLogSink, ProxyService, ProxyTestService, QueueRequestLogSink,
        RecordingRequestLogSink, RequestLogSink, SystemMetricsService, UpstreamConnectorRegistry,
        codex_user_agent, hash_console_password,
    },
    domain::{
        ApiFormat, ApiKeyPermission, AutomaticDisableTrigger, ConnectorKind, RequestBilling,
        RequestLogEvent, RequestLogOutcome, RequestLogSource, RequestPriceSnapshot,
        RequestProtocol, RequestUsage,
    },
    http::console::{self, ConsoleState},
    models_dev::ModelsDevClient,
    persistence::{
        AuthRepository, ChannelGroupInput, CodexCredentialBatchInput,
        CodexCredentialBatchOperation, CodexCredentialBatchTarget, CodexCredentialCreate,
        CodexCredentialExportInput, CodexCredentialUpdateInput, CodexQuotaUpdate,
        CodexTokenRefreshUpdate, ControlPlaneMutation, ControlPlaneRepository, MIGRATOR,
        ProxyCreateInput, RequestLogBatchInsertOutcome, RequestLogInsertOutcome,
        RequestLogRepository, RequestLogSettlementOutcome, SystemAutomaticDisableSettingsInput,
        SystemPassiveHealthSettingsInput, SystemSessionAffinityKeySourceInput,
        SystemSessionAffinityRuleInput, SystemSessionAffinitySettingsInput, SystemSettingsInput,
        SystemUpstreamSettingsInput,
    },
    routing::{self, PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{
        AuthConfig, ModelsSyncConfig, RequestLoggingConfig, RuntimeConfig, compile_control_plane,
        compile_runtime_config,
    },
    upstream::UpstreamClientRegistry,
    workers::{ControlPlaneReloader, DurableRequestLogWorker, RequestLogWorker},
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{
        State,
        ws::{Message as UpstreamWebSocketMessage, WebSocket, WebSocketUpgrade},
    },
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{ACCEPT_ENCODING, AUTHORIZATION},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt, stream};
use http_body_util::BodyExt;
use reqwest::Url;
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as ClientWebSocketMessage, client::IntoClientRequest},
};
use tower::ServiceExt;
use uuid::Uuid;

const DEFAULT_ADMIN_URL: &str = "postgres://ai_gateway:ai_gateway@127.0.0.1:5432/postgres";
const PASSWORD_FILE_ADMIN_URL: &str = "postgres://ai_gateway@127.0.0.1:5432/postgres";

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
    let mut url = Url::parse(PASSWORD_FILE_ADMIN_URL).expect("default admin URL must be valid");
    url.set_password(Some(&password))
        .expect("PostgreSQL URL must accept a password");
    url.to_string()
}

fn system_settings() -> SystemSettingsInput {
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

#[tokio::test]
async fn request_log_tables_use_production_autovacuum_settings() {
    let database = TestDatabase::new().await;
    let rows = sqlx::query_as::<_, (String, Option<Vec<String>>)>(
        "SELECT relname,reloptions
         FROM pg_class
         WHERE relname IN ('request_logs','request_log_ingest')
         ORDER BY relname",
    )
    .fetch_all(&database.pool)
    .await
    .unwrap();
    let options = rows
        .into_iter()
        .map(|(table, options)| (table, options.unwrap_or_default()))
        .collect::<BTreeMap<_, _>>();

    assert!(options["request_logs"].contains(&"autovacuum_vacuum_scale_factor=0.02".into()));
    assert!(options["request_logs"].contains(&"autovacuum_analyze_scale_factor=0.02".into()));
    assert!(options["request_log_ingest"].contains(&"autovacuum_vacuum_scale_factor=0.01".into()));
    assert!(
        options["request_log_ingest"]
            .contains(&"autovacuum_vacuum_insert_scale_factor=0.01".into())
    );
    database.cleanup().await;
}

#[tokio::test]
async fn automatic_disable_and_scheduled_recovery_publish_channel_availability() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query(
        "UPDATE channels
         SET auto_disable_allowed=true, test_model='upstream-v1'
         WHERE id=$1",
    )
    .bind(seed.channel)
    .execute(&database.pool)
    .await
    .unwrap();
    let mut settings = system_settings();
    settings.automatic_disable = SystemAutomaticDisableSettingsInput {
        enabled: true,
        error_status_codes: vec![429],
        error_message_keywords: vec!["quota exceeded".into()],
    };
    let repository = ControlPlaneRepository::new(database.pool.clone());
    sqlx::query("UPDATE system_settings SET value=$2 WHERE setting_key=$1")
        .bind("forwarding_policy")
        .bind(serde_json::to_value(settings).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );

    assert!(
        coordinator
            .automatically_disable_channel(seed.channel, AutomaticDisableTrigger::HttpStatus(429))
            .await
            .unwrap()
    );
    let disabled: (bool, Option<String>) =
        sqlx::query_as("SELECT auto_disabled,auto_disabled_reason FROM channels WHERE id=$1")
            .bind(seed.channel)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(disabled.0);
    assert!(
        disabled
            .1
            .as_deref()
            .is_some_and(|reason| reason.contains("429"))
    );
    let disabled_snapshot = runtime.snapshot();
    assert!(disabled_snapshot.channel(seed.channel).is_none());
    assert!(
        disabled_snapshot
            .probe_channels()
            .any(|channel| channel.id() == seed.channel)
    );

    assert!(
        coordinator
            .automatically_recover_channel(seed.channel)
            .await
            .unwrap()
    );
    let recovered: (bool, Option<String>) =
        sqlx::query_as("SELECT auto_disabled,auto_disabled_reason FROM channels WHERE id=$1")
            .bind(seed.channel)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert!(!recovered.0);
    assert!(recovered.1.is_none());
    assert!(runtime.snapshot().channel(seed.channel).is_some());
    let actions = sqlx::query_scalar::<_, String>(
        "SELECT string_agg(action, ',' ORDER BY occurred_at)
         FROM audit_logs
         WHERE object_type='channel' AND object_id=$1",
    )
    .bind(seed.channel)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(actions.contains("auto_disable"));
    assert!(actions.contains("auto_recover"));

    database.cleanup().await;
}

#[tokio::test]
async fn matching_proxy_status_asynchronously_auto_disables_an_opted_in_channel() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(upstream))
            .with_state(UpstreamState(Arc::new(Mutex::new(
                UpstreamMode::Immediate(StatusCode::TOO_MANY_REQUESTS),
            )))),
    )
    .await;
    sqlx::query(
        "UPDATE channels
         SET base_url=$2, auto_disable_allowed=true
         WHERE id=$1",
    )
    .bind(seed.channel)
    .bind(format!("http://{}", upstream.address))
    .execute(&database.pool)
    .await
    .unwrap();
    let mut settings = system_settings();
    settings.automatic_disable = SystemAutomaticDisableSettingsInput {
        enabled: true,
        error_status_codes: vec![429],
        error_message_keywords: vec![],
    };
    sqlx::query("UPDATE system_settings SET value=$2 WHERE setting_key=$1")
        .bind("forwarding_policy")
        .bind(serde_json::to_value(settings).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();

    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let routing = RoutingRuntime::new(PassiveHealthPolicy::default());
    let upstream_clients = Arc::new(UpstreamClientRegistry::new());
    let coordinator = ControlPlaneCoordinator::new_with_upstream_registry(
        repository,
        Arc::clone(&runtime),
        routing.clone(),
        Arc::clone(&upstream_clients),
    )
    .unwrap();
    let (automatic_disable, worker) = AutomaticDisableWorker::start(coordinator);
    let proxy = ProxyService::with_dependencies_and_registry_and_automation(
        Arc::clone(&runtime),
        1_048_576,
        upstream_clients,
        Arc::new(NoopRequestLogSink),
        routing,
        AdmissionRuntime::new(),
        Some(automatic_disable),
    )
    .unwrap();
    let app = ai_gateway::http::router(proxy);
    let request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("authorization", format!("Bearer {}", seed.secret))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(
                    &serde_json::json!({"model": seed.client_model, "stream": false}),
                )
                .unwrap(),
            ))
            .unwrap()
    };
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let disabled: bool = sqlx::query_scalar("SELECT auto_disabled FROM channels WHERE id=$1")
            .bind(seed.channel)
            .fetch_one(&database.pool)
            .await
            .unwrap();
        let removed_from_runtime = runtime.snapshot().channel(seed.channel).is_none();
        if disabled && removed_from_runtime {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "matching upstream status did not publish an auto-disabled channel"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let unavailable = app.oneshot(request()).await.unwrap();
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);

    worker.shutdown().await;
    drop(upstream);
    database.cleanup().await;
}

#[tokio::test]
async fn system_probe_identity_is_an_internal_active_administrator() {
    let database = TestDatabase::new().await;
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let first = repository.ensure_system_probe_identity().await.unwrap();
    let second = repository.ensure_system_probe_identity().await.unwrap();
    assert_eq!(first, second);

    let identity: (String, bool, bool, bool) = sqlx::query_as(
        "SELECT user_account.role,
                user_account.is_system,
                key.is_system,
                key.status='active' AS key_active
         FROM users AS user_account
         JOIN api_keys AS key ON key.user_id=user_account.id
         WHERE user_account.id=$1 AND key.id=$2",
    )
    .bind(first.user_id)
    .bind(first.api_key_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(identity.0, "admin");
    assert!(identity.1);
    assert!(identity.2);
    assert!(identity.3);

    let lists = repository.control_plane_lists().await.unwrap();
    assert!(lists.users.iter().all(|user| user.id != first.user_id));
    assert!(lists.api_keys.iter().all(|key| key.id != first.api_key_id));

    let now = Utc::now();
    let model_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO models
         (id,source_model_id,display_name,enabled,currency,price_unit_tokens,
          input_unit_price,cached_input_unit_price,cache_write_unit_price,
          output_unit_price,price_effective_at)
         VALUES ($1,'scheduled-test-model','Scheduled test model',true,'USD',1,1,0,0,1,$2)",
    )
    .bind(model_id)
    .bind(now)
    .execute(&database.pool)
    .await
    .unwrap();
    let cost = rust_decimal::Decimal::new(125, 2);
    let event = RequestLogEvent {
        id: Uuid::new_v4(),
        started_at: now,
        completed_at: now,
        user_id: first.user_id,
        api_key_id: first.api_key_id,
        request_source: RequestLogSource::ScheduledTest,
        api_format: ApiFormat::OpenAiChatCompletions,
        request_protocol: RequestProtocol::NonStream,
        client_model: "scheduled-test-model".into(),
        reasoning_effort: None,
        fast_mode: false,
        upstream_model: Some("scheduled-test-model".into()),
        model_rule_id: None,
        channel_group_id: None,
        channel_id: None,
        model_id: Some(model_id),
        outcome: RequestLogOutcome::Succeeded,
        response_status_code: Some(200),
        streamed: false,
        ttft_ms: Some(1),
        total_duration_ms: 1,
        billing: Some(RequestBilling {
            usage: Some(RequestUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 5,
                reasoning_tokens: 0,
            }),
            price: RequestPriceSnapshot {
                currency: "USD".into(),
                price_unit_tokens: 1,
                price_effective_at: now,
                input_unit_price: rust_decimal::Decimal::new(5, 2),
                cached_input_unit_price: rust_decimal::Decimal::ZERO,
                cache_write_unit_price: rust_decimal::Decimal::ZERO,
                output_unit_price: rust_decimal::Decimal::new(15, 2),
            },
            cost_amount: Some(cost),
            output_tokens_per_second: Some(rust_decimal::Decimal::ONE),
        }),
        error_code: None,
        error_summary: None,
    };
    assert_eq!(
        RequestLogRepository::new(database.pool.clone())
            .insert(&event)
            .await
            .unwrap(),
        RequestLogInsertOutcome::Inserted
    );
    let request_source: String =
        sqlx::query_scalar("SELECT request_source FROM request_logs WHERE id=$1")
            .bind(event.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(request_source, "scheduled_test");
    assert!(matches!(
        RequestLogRepository::new(database.pool.clone())
            .settle(event.id)
            .await
            .unwrap(),
        RequestLogSettlementOutcome::Settled { .. }
    ));
    let settled: (rust_decimal::Decimal, rust_decimal::Decimal, bool) = sqlx::query_as(
        "SELECT user_account.balance_amount,
                key.quota_used_amount,
                log.billed_at IS NOT NULL
         FROM users AS user_account
         JOIN api_keys AS key ON key.user_id=user_account.id
         JOIN request_logs AS log ON log.api_key_id=key.id
         WHERE user_account.id=$1 AND key.id=$2 AND log.id=$3",
    )
    .bind(first.user_id)
    .bind(first.api_key_id)
    .bind(event.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(settled.0, -cost);
    assert_eq!(settled.1, cost);
    assert!(settled.2);
    database.cleanup().await;
}

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
    UsageJson,
    SseError,
    HeaderDelay,
    OneChunkThenIdle,
    TwoChunks,
}

#[derive(Clone)]
struct UpstreamState(Arc<Mutex<UpstreamMode>>);

#[derive(Clone, Debug)]
struct CapturedCodexRequest {
    authorization: Option<String>,
    accept_encoding: Option<String>,
    account_id: Option<String>,
    originator: Option<String>,
    user_agent: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    client_request_id: Option<String>,
    body: serde_json::Value,
}

#[derive(Clone, Debug)]
struct CapturedCodexWebSocketHandshake {
    authorization: Option<String>,
    account_id: Option<String>,
    originator: Option<String>,
    user_agent: Option<String>,
    version: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    client_request_id: Option<String>,
    openai_beta: Option<String>,
    accept_encoding: Option<String>,
}

#[derive(Clone, Default)]
struct CodexUpstreamState {
    http_requests: Arc<Mutex<Vec<CapturedCodexRequest>>>,
    websocket_handshakes: Arc<Mutex<Vec<CapturedCodexWebSocketHandshake>>>,
    websocket_requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn codex_responses_upstream(
    State(state): State<CodexUpstreamState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let request_index = {
        let mut requests = state.http_requests.lock().unwrap();
        let request_index = requests.len();
        requests.push(CapturedCodexRequest {
            authorization: header("authorization"),
            accept_encoding: header("accept-encoding"),
            account_id: header("chatgpt-account-id"),
            originator: header("originator"),
            user_agent: header("user-agent"),
            session_id: header("session-id"),
            thread_id: header("thread-id"),
            client_request_id: header("x-client-request-id"),
            body: serde_json::from_slice(&body).unwrap(),
        });
        request_index
    };
    let terminal = Bytes::from_static(
        b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_codex\",\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":40,\"cache_write_tokens\":60},\"output_tokens\":10,\"output_tokens_details\":{\"reasoning_tokens\":5},\"total_tokens\":110}}}\n\n",
    );
    let response = Response::builder().status(StatusCode::OK).header(
        "content-type",
        if request_index == 0 {
            "application/octet-stream"
        } else {
            "text/event-stream"
        },
    );
    if request_index == 0 {
        let first = stream::once(async move { Ok::<Bytes, io::Error>(terminal) });
        let pending = stream::pending::<Result<Bytes, io::Error>>();
        response
            .body(Body::from_stream(first.chain(pending)))
            .unwrap()
    } else {
        response.body(Body::from(terminal)).unwrap()
    }
}

async fn codex_responses_websocket_upstream(
    websocket: WebSocketUpgrade,
    State(state): State<CodexUpstreamState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let header = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    state
        .websocket_handshakes
        .lock()
        .unwrap()
        .push(CapturedCodexWebSocketHandshake {
            authorization: header("authorization"),
            account_id: header("chatgpt-account-id"),
            originator: header("originator"),
            user_agent: header("user-agent"),
            version: header("version"),
            session_id: header("session-id"),
            thread_id: header("thread-id"),
            client_request_id: header("x-client-request-id"),
            openai_beta: header("openai-beta"),
            accept_encoding: header("accept-encoding"),
        });
    websocket.on_upgrade(move |socket| codex_websocket_connection(socket, state))
}

async fn codex_websocket_connection(mut websocket: WebSocket, state: CodexUpstreamState) {
    let mut previous_response_id = None::<String>;
    let mut response_number = 1_u32;
    while let Some(Ok(UpstreamWebSocketMessage::Text(text))) = websocket.next().await {
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        state.websocket_requests.lock().unwrap().push(body.clone());
        let requested_previous = body
            .get("previous_response_id")
            .and_then(serde_json::Value::as_str);
        if requested_previous.is_some() && requested_previous != previous_response_id.as_deref() {
            websocket
                .send(UpstreamWebSocketMessage::Text(
                    serde_json::json!({
                        "type": "error",
                        "status": 400,
                        "error": {
                            "type": "invalid_request_error",
                            "code": "previous_response_not_found",
                            "message": "previous response was not on this connection"
                        }
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            continue;
        }

        let response_id = format!("resp_codex_ws_{response_number}");
        response_number = response_number.saturating_add(1);
        websocket
            .send(UpstreamWebSocketMessage::Text(
                serde_json::json!({
                    "type": "response.created",
                    "response": {"id": response_id}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        websocket
            .send(UpstreamWebSocketMessage::Text(
                serde_json::json!({
                    "type": "response.completed",
                    "response": {
                        "id": response_id,
                        "usage": {
                            "input_tokens": 100,
                            "input_tokens_details": {
                                "cached_tokens": 40,
                                "cache_write_tokens": 60
                            },
                            "output_tokens": 10,
                            "output_tokens_details": {"reasoning_tokens": 5},
                            "total_tokens": 110
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        previous_response_id = Some(response_id);
    }
}

async fn codex_websocket_response_create(
    websocket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    body: serde_json::Value,
) -> Vec<serde_json::Value> {
    websocket
        .send(ClientWebSocketMessage::Text(body.to_string().into()))
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let message = timeout(Duration::from_secs(2), websocket.next())
            .await
            .expect("Codex WebSocket response timed out")
            .expect("Codex WebSocket closed before a terminal event")
            .expect("Codex WebSocket response failed");
        let ClientWebSocketMessage::Text(text) = message else {
            continue;
        };
        let event: serde_json::Value = serde_json::from_str(&text).unwrap();
        let terminal = matches!(
            event.get("type").and_then(serde_json::Value::as_str),
            Some("response.completed" | "response.failed" | "error")
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

async fn upstream(State(state): State<UpstreamState>) -> Response {
    let mode = { state.0.lock().unwrap().clone() };
    match mode {
        UpstreamMode::Immediate(status) => Response::builder()
            .status(status)
            .body(Body::from("upstream"))
            .unwrap(),
        UpstreamMode::UsageJson => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"id":"usage-result","usage":{"prompt_tokens":2,"completion_tokens":3}}"#,
            ))
            .unwrap(),
        UpstreamMode::SseError => Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(
                "data: {\"error\":{\"code\":\"provider_error\",\"message\":\"upstream quota exhausted\"}}\n\n",
            ))
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
                        "cache_write": 0.50,
                        "tiers": [{
                            "input": 2.50,
                            "output": 5.00,
                            "cache_read": 0.50,
                            "cache_write": 1.00,
                            "tier": {"type": "context", "size": 128000}
                        }]
                    },
                    "experimental": {
                        "modes": {
                            "fast": {
                                "cost": {
                                    "input": 2.50,
                                    "output": 5.00,
                                    "cache_read": 0.50,
                                    "cache_write": 1.00
                                },
                                "provider": {
                                    "body": {
                                        "service_tier": "priority"
                                    }
                                }
                            }
                        }
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
    request_protocol: String,
    client_model: String,
    reasoning_effort: Option<String>,
    fast_mode: bool,
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
    error_summary: Option<String>,
}

#[derive(FromRow)]
struct TerminalLogCount {
    api_key_id: Uuid,
    client_model: String,
    outcome: String,
    error_code: Option<String>,
    count: i64,
}

#[derive(FromRow)]
struct PersistedBilling {
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    cost_amount: Option<rust_decimal::Decimal>,
    currency: Option<String>,
}

#[derive(FromRow)]
struct SettlementFacts {
    balance_amount: rust_decimal::Decimal,
    quota_used_amount: rust_decimal::Decimal,
    billed_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct DurablePersistedLog {
    id: Uuid,
    client_model: String,
    upstream_model: Option<String>,
    billed_at: Option<DateTime<Utc>>,
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
        let rows = sqlx::query_as::<_, PersistedLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, request_protocol, client_model, reasoning_effort, fast_mode, upstream_model, model_rule_id, channel_group_id, channel_id, model_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, error_code, error_summary FROM request_logs WHERE api_key_id = $1 AND client_model = $2")
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
        let rows = sqlx::query_as::<_, PersistedLog>("SELECT started_at, completed_at, user_id, api_key_id, api_format::text AS api_format, request_protocol, client_model, reasoning_effort, fast_mode, upstream_model, model_rule_id, channel_group_id, channel_id, model_id, outcome, response_status_code, streamed, ttft_ms, total_duration_ms, error_code, error_summary FROM request_logs WHERE api_key_id = $1 AND client_model = $2 AND outcome = $3 AND error_code IS NOT DISTINCT FROM $4")
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
        let admin_url = env::var("TEST_DATABASE_ADMIN_URL").unwrap_or_else(|_| default_admin_url());
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
        ControlPlaneRepository::new(pool.clone())
            .ensure_system_settings(system_settings())
            .await
            .expect("system settings must initialize");

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
    email: String,
    password: String,
    client_model: String,
}

async fn seed(pool: &PgPool) -> Seed {
    let user = Uuid::new_v4();
    let seed = Seed {
        user,
        model: Uuid::new_v4(),
        group: Uuid::new_v4(),
        other_group: Uuid::new_v4(),
        channel: Uuid::new_v4(),
        proxy: Uuid::new_v4(),
        template: Uuid::new_v4(),
        key: Uuid::new_v4(),
        rule: Uuid::new_v4(),
        secret: format!("test-client-{}", Uuid::new_v4()),
        email: format!("test-user-{user}@example.test"),
        password: "test-password-with-enough-length".into(),
        client_model: format!("test-model-{}", Uuid::new_v4()),
    };
    let password_hash = hash_console_password(seed.password.clone()).await.unwrap();
    sqlx::query("INSERT INTO users (id, email, display_name, role, status, password_hash) VALUES ($1, $2, $3, 'admin', 'active', $4)")
        .bind(seed.user)
        .bind(&seed.email)
        .bind(format!("test-user-{}", seed.user))
        .bind(password_hash)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO models (id, source_model_id, display_name, enabled, currency, price_unit_tokens, input_unit_price, cached_input_unit_price, cache_write_unit_price, output_unit_price, price_effective_at) VALUES ($1, 'upstream-v1', 'test', true, 'USD', 1, 0, 0, 0, 0, now())")
        .bind(seed.model)
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
    sqlx::query("INSERT INTO model_rules (id, client_model, api_format, upstream_model_id, channel_ids, enabled) VALUES ($1, $2, 'open_ai_chat_completions', $3, ARRAY[$4]::uuid[], true)")
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
        request_source: ai_gateway::domain::RequestLogSource::Client,
        api_format: ApiFormat::OpenAiChatCompletions,
        request_protocol: RequestProtocol::Sse,
        client_model: seed.client_model.clone(),
        reasoning_effort: Some("high".into()),
        fast_mode: true,
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
        billing: Some(RequestBilling {
            usage: Some(RequestUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
                reasoning_tokens: 1,
            }),
            price: RequestPriceSnapshot {
                currency: "USD".into(),
                price_unit_tokens: 1_000_000,
                price_effective_at: now,
                input_unit_price: rust_decimal::Decimal::new(100, 2),
                cached_input_unit_price: rust_decimal::Decimal::new(20, 2),
                cache_write_unit_price: rust_decimal::Decimal::new(30, 2),
                output_unit_price: rust_decimal::Decimal::new(200, 2),
            },
            cost_amount: Some(rust_decimal::Decimal::new(999, 8)),
            output_tokens_per_second: Some(rust_decimal::Decimal::new(20, 2)),
        }),
        error_code: None,
        error_summary: None,
    }
}

fn business_codex_credential(
    channel_group_id: Uuid,
    label: &str,
    email: &str,
    user_id: &str,
) -> CodexCredentialCreate {
    let now = Utc::now();
    CodexCredentialCreate {
        channel_group_id,
        label: label.into(),
        enabled: true,
        proxy_id: None,
        weight: 100,
        quota_threshold_percent: 95,
        base_url: "https://chatgpt.com/backend-api/codex".into(),
        email: Some(email.into()),
        account_id: "business-workspace".into(),
        user_id: Some(user_id.into()),
        plan_type: Some("business".into()),
        is_fedramp: false,
        id_token: format!("{label}-id-token"),
        access_token: format!("{label}-access-token"),
        refresh_token: format!("{label}-refresh-token"),
        access_token_expires_at: Some(now + chrono::Duration::hours(1)),
        available_models: vec!["gpt-5-codex".into()],
        quota: None,
    }
}

#[derive(FromRow)]
struct DeletedCodexCredentialState {
    id_token: String,
    access_token: String,
    refresh_token: String,
    deleted_at: Option<DateTime<Utc>>,
    channel_name: String,
    proxy_id: Option<Uuid>,
    quota_allowed: Option<bool>,
    primary_used_percent: Option<i32>,
}

#[tokio::test]
async fn codex_business_credentials_are_unique_per_workspace_member() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,connector_kind,priority,selection_strategy,enabled) \
         VALUES ($1,'codex-business','open_ai_responses','codex_oauth',0,'weighted_random',true)",
    )
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        runtime,
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );

    let mut legacy_member_a =
        business_codex_credential(codex_group, "member-a", "member-a@example.test", "user-a");
    legacy_member_a.user_id = None;
    let member_a = coordinator
        .create_codex_credential(seed.user, legacy_member_a, None)
        .await
        .unwrap();
    let migrated_member_a = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(codex_group, "member-a", "member-a@example.test", "user-a"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(migrated_member_a.id, member_a.id);
    assert_eq!(migrated_member_a.action, "update");

    let member_b = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(codex_group, "member-b", "member-b@example.test", "user-b"),
            None,
        )
        .await
        .unwrap();
    let member_c = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(codex_group, "member-c", "member-a@example.test", "user-c"),
            None,
        )
        .await
        .unwrap();

    assert_ne!(member_a.id, member_b.id);
    assert_ne!(member_a.id, member_c.id);
    assert_eq!(member_a.action, "create");
    assert_eq!(member_b.action, "create");
    assert_eq!(member_c.action, "create");

    let reauthorized_a = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(
                codex_group,
                "member-a-new",
                "member-a-renamed@example.test",
                "user-a",
            ),
            None,
        )
        .await
        .unwrap();
    assert_eq!(reauthorized_a.id, member_a.id);
    assert_eq!(reauthorized_a.action, "update");

    let credentials = repository.codex_credentials(codex_group).await.unwrap();
    assert_eq!(credentials.len(), 3);
    assert_eq!(
        credentials
            .iter()
            .map(|credential| credential.user_id.as_deref().unwrap())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["user-a", "user-b", "user-c"])
    );

    database.cleanup().await;
}

#[tokio::test]
async fn codex_credentials_support_atomic_batch_state_changes_and_token_scrubbing_delete() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,connector_kind,priority,selection_strategy,enabled) \
         VALUES ($1,'codex-batch','open_ai_responses','codex_oauth',0,'weighted_random',true)",
    )
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        runtime,
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let delete_proxy = coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::CreateProxy(ProxyCreateInput {
                name: "codex-delete-egress".into(),
                proxy_url: "http://127.0.0.1:18080".into(),
                username: Some("delete-user".into()),
                password: Some("delete-password".into()),
                no_proxy_hosts: Vec::new(),
                enabled: true,
            }),
        )
        .await
        .unwrap();
    let mut member_a_input = business_codex_credential(
        codex_group,
        "delete-a",
        "delete-a@example.test",
        "delete-user-a",
    );
    member_a_input.proxy_id = Some(delete_proxy.id);
    member_a_input.quota = Some(CodexQuotaUpdate {
        allowed: true,
        limit_reached: false,
        primary_used_percent: Some(42),
        primary_window_seconds: Some(10_800),
        primary_reset_at: Some(Utc::now() + chrono::Duration::hours(1)),
        secondary_used_percent: None,
        secondary_window_seconds: None,
        secondary_reset_at: None,
        checked_at: Utc::now(),
    });
    let member_a = coordinator
        .create_codex_credential(seed.user, member_a_input, None)
        .await
        .unwrap();
    let member_b = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(
                codex_group,
                "delete-b",
                "delete-b@example.test",
                "delete-user-b",
            ),
            None,
        )
        .await
        .unwrap();
    let views = repository.codex_credentials(codex_group).await.unwrap();

    let disabled = coordinator
        .update_codex_credentials_batch(
            seed.user,
            codex_group,
            CodexCredentialBatchInput {
                items: views
                    .iter()
                    .map(|credential| CodexCredentialBatchTarget {
                        id: credential.id,
                        updated_at: credential.updated_at,
                    })
                    .collect(),
                operation: CodexCredentialBatchOperation::Disable,
            },
        )
        .await
        .unwrap();
    assert_eq!(disabled.updated_ids.len(), 2);
    assert!(
        repository
            .codex_credentials(codex_group)
            .await
            .unwrap()
            .iter()
            .all(|credential| !credential.enabled && credential.runtime_status == "disabled")
    );
    let current_views = repository.codex_credentials(codex_group).await.unwrap();
    let current_member_a = current_views
        .iter()
        .find(|credential| credential.id == member_a.id)
        .unwrap();
    let stale_member_b = views
        .iter()
        .find(|credential| credential.id == member_b.id)
        .unwrap();
    let stale = coordinator
        .update_codex_credentials_batch(
            seed.user,
            codex_group,
            CodexCredentialBatchInput {
                items: vec![
                    CodexCredentialBatchTarget {
                        id: current_member_a.id,
                        updated_at: current_member_a.updated_at,
                    },
                    CodexCredentialBatchTarget {
                        id: stale_member_b.id,
                        updated_at: stale_member_b.updated_at,
                    },
                ],
                operation: CodexCredentialBatchOperation::Enable,
            },
        )
        .await;
    assert!(matches!(
        stale,
        Err(ai_gateway::application::ControlPlaneError::Repository(
            ai_gateway::persistence::RepositoryError::Conflict
        ))
    ));
    assert!(
        repository
            .codex_credentials(codex_group)
            .await
            .unwrap()
            .iter()
            .all(|credential| !credential.enabled)
    );

    let member_a_view = repository
        .codex_credential_view(member_a.id)
        .await
        .unwrap()
        .unwrap();
    coordinator
        .delete_codex_credential(seed.user, member_a.id, member_a_view.updated_at)
        .await
        .unwrap();
    assert!(
        repository
            .codex_credential_view(member_a.id)
            .await
            .unwrap()
            .is_none()
    );
    let deleted = sqlx::query_as::<_, DeletedCodexCredentialState>(
        "SELECT c.id_token,c.access_token,c.refresh_token,c.deleted_at, \
                ch.name AS channel_name, \
                ch.proxy_id,c.quota_allowed,c.primary_used_percent \
         FROM codex_oauth_credentials c JOIN channels ch ON ch.id=c.channel_id \
         WHERE c.channel_id=$1",
    )
    .bind(member_a.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(deleted.id_token, "deleted");
    assert_eq!(deleted.access_token, "deleted");
    assert_eq!(deleted.refresh_token, "deleted");
    assert!(deleted.deleted_at.is_some());
    assert_eq!(
        deleted.channel_name,
        format!("deleted-codex-{}", member_a.id)
    );
    assert!(deleted.proxy_id.is_none());
    assert!(deleted.quota_allowed.is_none());
    assert!(deleted.primary_used_percent.is_none());
    coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::DeleteProxy {
                id: delete_proxy.id,
                expected_updated_at: delete_proxy.updated_at,
            },
        )
        .await
        .unwrap();

    let member_b_view = repository
        .codex_credential_view(member_b.id)
        .await
        .unwrap()
        .unwrap();
    coordinator
        .update_codex_credentials_batch(
            seed.user,
            codex_group,
            CodexCredentialBatchInput {
                items: vec![CodexCredentialBatchTarget {
                    id: member_b.id,
                    updated_at: member_b_view.updated_at,
                }],
                operation: CodexCredentialBatchOperation::Delete,
            },
        )
        .await
        .unwrap();
    assert!(
        repository
            .codex_credentials(codex_group)
            .await
            .unwrap()
            .is_empty()
    );

    let reimported = coordinator
        .create_codex_credential(
            seed.user,
            business_codex_credential(
                codex_group,
                "delete-a",
                "delete-a@example.test",
                "delete-user-a",
            ),
            None,
        )
        .await
        .unwrap();
    assert_ne!(reimported.id, member_a.id);

    database.cleanup().await;
}

#[tokio::test]
async fn codex_credentials_create_managed_channels_and_recompute_quota_state() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,connector_kind,priority,selection_strategy,enabled) \
         VALUES ($1,'codex-managed','open_ai_responses','codex_oauth',0,'weighted_random',true)",
    )
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let now = Utc::now();
    let created = coordinator
        .create_codex_credential(
            seed.user,
            CodexCredentialCreate {
                channel_group_id: codex_group,
                label: "plus-account".into(),
                enabled: true,
                proxy_id: None,
                weight: 100,
                quota_threshold_percent: 95,
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                email: Some("codex@example.test".into()),
                account_id: "account-123".into(),
                user_id: Some("user-123".into()),
                plan_type: Some("plus".into()),
                is_fedramp: false,
                id_token: "secret-id-token".into(),
                access_token: "secret-access-token".into(),
                refresh_token: "secret-refresh-token".into(),
                access_token_expires_at: Some(now + chrono::Duration::hours(1)),
                available_models: vec!["gpt-5-codex".into()],
                quota: Some(CodexQuotaUpdate {
                    allowed: true,
                    limit_reached: false,
                    primary_used_percent: Some(96),
                    primary_window_seconds: Some(10_800),
                    primary_reset_at: Some(now + chrono::Duration::hours(1)),
                    secondary_used_percent: None,
                    secondary_window_seconds: None,
                    secondary_reset_at: None,
                    checked_at: now,
                }),
            },
            None,
        )
        .await
        .unwrap();

    assert_eq!(created.object_type, "codex_oauth_credential");
    let audit = created.after_redacted.to_string();
    assert!(!audit.contains("secret-id-token"));
    assert!(!audit.contains("secret-access-token"));
    assert!(!audit.contains("secret-refresh-token"));
    let snapshot = runtime.snapshot();
    let channel = snapshot
        .channel(created.id)
        .expect("managed channel compiled");
    assert_eq!(channel.connector_kind(), ConnectorKind::CodexOauth);
    assert!(channel.supports_websocket());

    let before = repository
        .codex_credential_view(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.runtime_status, "draining");
    let updated = coordinator
        .update_codex_credential(
            seed.user,
            created.id,
            CodexCredentialUpdateInput {
                label: "plus-account".into(),
                enabled: true,
                proxy_id: None,
                weight: 50,
                quota_threshold_percent: 99,
            },
            before.updated_at,
        )
        .await
        .unwrap();
    assert_eq!(updated.action, "update");
    let after = repository
        .codex_credential_view(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.runtime_status, "active");
    assert_eq!(after.weight, 50);

    repository
        .mark_codex_credential_error(
            created.id,
            true,
            "codex_refresh_token_invalid",
            "The Codex refresh token requires a new login.",
        )
        .await
        .unwrap();
    let reauth = repository
        .codex_credential_view(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reauth.runtime_status, "unavailable");
    coordinator
        .update_codex_credential(
            seed.user,
            created.id,
            CodexCredentialUpdateInput {
                label: "plus-account".into(),
                enabled: true,
                proxy_id: None,
                weight: 75,
                quota_threshold_percent: 99,
            },
            reauth.updated_at,
        )
        .await
        .unwrap();
    repository
        .persist_codex_quota(
            created.id,
            CodexQuotaUpdate {
                allowed: true,
                limit_reached: false,
                primary_used_percent: Some(1),
                primary_window_seconds: Some(10_800),
                primary_reset_at: Some(now + chrono::Duration::hours(1)),
                secondary_used_percent: None,
                secondary_window_seconds: None,
                secondary_reset_at: None,
                checked_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    let still_reauth = repository
        .codex_credential_view(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still_reauth.runtime_status, "unavailable");
    assert_eq!(
        still_reauth.last_error_code.as_deref(),
        Some("codex_refresh_token_invalid")
    );

    let record = repository
        .codex_credential(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(record.reauth_required);
    let mut transaction = repository.begin_codex_refresh().await.unwrap();
    assert!(
        repository
            .persist_codex_token_refresh_transaction(
                &mut transaction,
                created.id,
                CodexTokenRefreshUpdate {
                    expected_generation: record.refresh_generation,
                    id_token: None,
                    access_token: Some("replacement-access-token".into()),
                    refresh_token: Some("replacement-refresh-token".into()),
                    email: None,
                    account_id: None,
                    user_id: None,
                    plan_type: None,
                    is_fedramp: None,
                    access_token_expires_at: Some(now + chrono::Duration::hours(2)),
                    refreshed_at: Utc::now(),
                },
            )
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();
    let recovered = repository
        .codex_credential(created.id)
        .await
        .unwrap()
        .unwrap();
    assert!(!recovered.reauth_required);
    assert_eq!(recovered.runtime_status, "active");
    assert!(recovered.last_error_code.is_none());

    let reauthorized = coordinator
        .create_codex_credential(
            seed.user,
            CodexCredentialCreate {
                channel_group_id: codex_group,
                label: "plus-reauthorized".into(),
                enabled: true,
                proxy_id: None,
                weight: 80,
                quota_threshold_percent: 98,
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                email: Some("codex-updated@example.test".into()),
                account_id: "account-123".into(),
                user_id: Some("user-123".into()),
                plan_type: Some("pro".into()),
                is_fedramp: false,
                id_token: "reauthorized-id-token".into(),
                access_token: "reauthorized-access-token".into(),
                refresh_token: "reauthorized-refresh-token".into(),
                access_token_expires_at: Some(now + chrono::Duration::hours(3)),
                available_models: vec!["gpt-5-codex".into(), "gpt-5.1-codex".into()],
                quota: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(reauthorized.id, created.id);
    assert_eq!(reauthorized.action, "update");
    let reauthorized_audit = reauthorized.after_redacted.to_string();
    assert!(!reauthorized_audit.contains("reauthorized-id-token"));
    assert!(!reauthorized_audit.contains("reauthorized-access-token"));
    assert!(!reauthorized_audit.contains("reauthorized-refresh-token"));
    let reauthorized_record = repository
        .codex_credential(created.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reauthorized_record.label, "plus-reauthorized");
    assert_eq!(
        reauthorized_record.access_token,
        "reauthorized-access-token"
    );
    assert_eq!(reauthorized_record.weight, 80);
    assert_eq!(
        reauthorized_record.available_models,
        vec!["gpt-5-codex", "gpt-5.1-codex"]
    );

    let group_updated_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM channel_groups WHERE id=$1")
            .bind(codex_group)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let connector_change = coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::UpdateGroup {
                id: codex_group,
                input: ChannelGroupInput {
                    name: "codex-managed".into(),
                    api_format: "open_ai_responses".into(),
                    connector_kind: "openai_compatible".into(),
                    priority: 0,
                    selection_strategy: "weighted_random".into(),
                    enabled: true,
                },
                expected_updated_at: group_updated_at,
            },
        )
        .await;
    assert!(connector_change.is_err());

    database.cleanup().await;
}

#[tokio::test]
async fn codex_credentials_export_secrets_and_protect_assigned_proxies_from_deletion() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,connector_kind,priority,selection_strategy,enabled) \
         VALUES ($1,'codex-portable','open_ai_responses','codex_oauth',0,'weighted_random',true)",
    )
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository.clone(),
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let proxy = coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::CreateProxy(ProxyCreateInput {
                name: "codex-egress".into(),
                proxy_url: "socks5h://127.0.0.1:1080".into(),
                username: Some("proxy-user".into()),
                password: Some("proxy-password".into()),
                no_proxy_hosts: Vec::new(),
                enabled: true,
            }),
        )
        .await
        .unwrap();
    let now = Utc::now();
    let credential = coordinator
        .create_codex_credential(
            seed.user,
            CodexCredentialCreate {
                channel_group_id: codex_group,
                label: "portable-account".into(),
                enabled: true,
                proxy_id: Some(proxy.id),
                weight: 70,
                quota_threshold_percent: 91,
                base_url: "https://chatgpt.com/backend-api/codex".into(),
                email: Some("portable@example.test".into()),
                account_id: "portable-account-id".into(),
                user_id: Some("portable-user-id".into()),
                plan_type: Some("plus".into()),
                is_fedramp: false,
                id_token: "portable-id-token".into(),
                access_token: "portable-access-token".into(),
                refresh_token: "portable-refresh-token".into(),
                access_token_expires_at: Some(now + chrono::Duration::hours(1)),
                available_models: vec!["gpt-5-codex".into()],
                quota: None,
            },
            None,
        )
        .await
        .unwrap();

    let exported = repository
        .export_codex_credentials(
            codex_group,
            CodexCredentialExportInput {
                credential_ids: vec![credential.id],
                include_proxies: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(exported.export_type, "ai-gateway-codex-credentials");
    assert_eq!(exported.channel_group_name, "codex-portable");
    assert_eq!(exported.credentials.len(), 1);
    assert_eq!(exported.credentials[0].id_token, "portable-id-token");
    assert_eq!(
        exported.credentials[0].access_token,
        "portable-access-token"
    );
    assert_eq!(
        exported.credentials[0].refresh_token,
        "portable-refresh-token"
    );
    assert_eq!(
        exported.credentials[0].user_id.as_deref(),
        Some("portable-user-id")
    );
    assert_eq!(exported.credentials[0].proxy_key, Some(proxy.id));
    assert_eq!(exported.proxies.len(), 1);
    assert_eq!(exported.proxies[0].username.as_deref(), Some("proxy-user"));
    assert_eq!(
        exported.proxies[0].password.as_deref(),
        Some("proxy-password")
    );

    let delete_in_use = coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::DeleteProxy {
                id: proxy.id,
                expected_updated_at: proxy.updated_at,
            },
        )
        .await;
    assert!(matches!(
        delete_in_use,
        Err(ai_gateway::application::ControlPlaneError::Repository(
            ai_gateway::persistence::RepositoryError::ProxyInUse
        ))
    ));

    let credential_view = repository
        .codex_credential_view(credential.id)
        .await
        .unwrap()
        .unwrap();
    coordinator
        .update_codex_credential(
            seed.user,
            credential.id,
            CodexCredentialUpdateInput {
                label: credential_view.label,
                enabled: credential_view.enabled,
                proxy_id: None,
                weight: credential_view.weight,
                quota_threshold_percent: credential_view.quota_threshold_percent,
            },
            credential_view.updated_at,
        )
        .await
        .unwrap();
    coordinator
        .mutate(
            seed.user,
            ControlPlaneMutation::DeleteProxy {
                id: proxy.id,
                expected_updated_at: proxy.updated_at,
            },
        )
        .await
        .unwrap();
    assert!(
        coordinator
            .lists()
            .await
            .unwrap()
            .proxies
            .iter()
            .all(|item| item.id != proxy.id)
    );

    database.cleanup().await;
}

#[tokio::test]
async fn codex_connector_forwards_responses_with_managed_credentials_and_headers() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let captured = CodexUpstreamState::default();
    let upstream = start_server(
        Router::new()
            .route(
                "/backend-api/codex/responses",
                get(codex_responses_websocket_upstream).post(codex_responses_upstream),
            )
            .with_state(captured.clone()),
    )
    .await;
    let codex_group = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO channel_groups \
         (id,name,api_format,connector_kind,priority,selection_strategy,enabled) \
         VALUES ($1,'codex-forwarding','open_ai_responses','codex_oauth',0,'weighted_random',true)",
    )
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    let mut settings = system_settings();
    settings.session_affinity = SystemSessionAffinitySettingsInput {
        enabled: true,
        max_entries: 100,
        default_ttl_seconds: 3_600,
        rules: vec![SystemSessionAffinityRuleInput {
            name: "codex-session".into(),
            enabled: true,
            api_formats: vec!["open_ai_responses".into()],
            model_regex: vec!["^codex-client-.*$".into()],
            key_sources: vec![SystemSessionAffinityKeySourceInput::RequestHeader {
                name: "session-id".into(),
            }],
            value_regex: None,
            ttl_seconds: Some(3_600),
        }],
    };
    settings.websocket.enabled = true;
    sqlx::query("UPDATE system_settings SET value=$2 WHERE setting_key=$1")
        .bind("forwarding_policy")
        .bind(serde_json::to_value(settings).unwrap())
        .execute(&database.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET websocket_enabled=true WHERE id=$1")
        .bind(seed.user)
        .execute(&database.pool)
        .await
        .unwrap();

    let repository = ControlPlaneRepository::new(database.pool.clone());
    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let routing = RoutingRuntime::new(PassiveHealthPolicy::default());
    let upstream_clients = Arc::new(UpstreamClientRegistry::new());
    let coordinator = ControlPlaneCoordinator::new_with_upstream_registry(
        repository.clone(),
        Arc::clone(&runtime),
        routing.clone(),
        Arc::clone(&upstream_clients),
    )
    .unwrap();
    let now = Utc::now();
    let credential = coordinator
        .create_codex_credential(
            seed.user,
            CodexCredentialCreate {
                channel_group_id: codex_group,
                label: "forwarding-account".into(),
                enabled: true,
                proxy_id: None,
                weight: 100,
                quota_threshold_percent: 95,
                base_url: format!("http://{}/backend-api/codex", upstream.address),
                email: Some("codex@example.test".into()),
                account_id: "account-123".into(),
                user_id: Some("user-123".into()),
                plan_type: Some("plus".into()),
                is_fedramp: false,
                id_token: "id-token".into(),
                access_token: "access-token".into(),
                refresh_token: "refresh-token".into(),
                access_token_expires_at: Some(now + chrono::Duration::hours(1)),
                available_models: vec!["upstream-v1".into()],
                quota: Some(CodexQuotaUpdate {
                    allowed: true,
                    limit_reached: false,
                    primary_used_percent: Some(10),
                    primary_window_seconds: Some(10_800),
                    primary_reset_at: Some(now + chrono::Duration::hours(1)),
                    secondary_used_percent: None,
                    secondary_window_seconds: None,
                    secondary_reset_at: None,
                    checked_at: now,
                }),
            },
            None,
        )
        .await
        .unwrap();
    let client_model = format!("codex-client-{}", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO model_rules \
         (id,client_model,api_format,upstream_model_id,channel_group_ids,enabled) \
         VALUES ($1,$2,'open_ai_responses',$3,ARRAY[$4]::uuid[],true)",
    )
    .bind(Uuid::new_v4())
    .bind(&client_model)
    .bind(seed.model)
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE api_keys SET \
         allowed_api_formats=ARRAY['open_ai_chat_completions','open_ai_responses']::api_format[], \
         allowed_group_ids=ARRAY[$2,$3]::uuid[] \
         WHERE id=$1",
    )
    .bind(seed.key)
    .bind(seed.group)
    .bind(codex_group)
    .execute(&database.pool)
    .await
    .unwrap();
    coordinator.reload().await.unwrap();

    let codex_connector = CodexConnectorService::new(
        repository.clone(),
        coordinator,
        Arc::clone(&runtime),
        Arc::clone(&upstream_clients),
    )
    .await
    .unwrap();
    let connectors = UpstreamConnectorRegistry::default().with_codex(codex_connector.clone());
    let logs = RecordingRequestLogSink::default();
    let proxy = ProxyService::with_dependencies_and_registry(
        runtime,
        1_048_576,
        upstream_clients,
        Arc::new(logs.clone()),
        routing,
        AdmissionRuntime::new(),
    )
    .unwrap()
    .with_connector_registry(connectors);
    let app = ai_gateway::http::router(proxy);
    let request = |session_id: &'static str, body: serde_json::Value| {
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("authorization", format!("Bearer {}", seed.secret))
            .header("content-type", "application/json")
            .header("accept-encoding", "gzip, br")
            .header("session-id", session_id)
            .header("thread-id", "thread-456")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
    };

    let response = app
        .clone()
        .oneshot(request(
            "session-123",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": true,
                "store": true,
                "input": "hello"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut response_body = response.into_body();
    let terminal_frame = response_body
        .frame()
        .await
        .expect("Codex terminal event was not forwarded")
        .unwrap()
        .into_data()
        .expect("Codex terminal frame must contain data");
    assert!(String::from_utf8_lossy(&terminal_frame).contains("response.completed"));
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, RequestLogOutcome::Succeeded);
    assert_eq!(events[0].error_code, None);
    assert_eq!(
        events[0].billing.as_ref().unwrap().usage,
        Some(RequestUsage {
            input_tokens: 100,
            cached_input_tokens: 40,
            cache_write_tokens: 60,
            output_tokens: 10,
            reasoning_tokens: 5,
        })
    );
    drop(response_body);

    let requests = captured.http_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    let forwarded = &requests[0];
    assert_eq!(
        forwarded.authorization.as_deref(),
        Some("Bearer access-token")
    );
    assert_eq!(forwarded.accept_encoding.as_deref(), Some("identity"));
    assert_eq!(forwarded.account_id.as_deref(), Some("account-123"));
    assert_eq!(forwarded.originator.as_deref(), Some(CODEX_ORIGINATOR));
    assert_eq!(
        forwarded.user_agent.as_deref(),
        Some(codex_user_agent().as_str())
    );
    assert_eq!(forwarded.session_id.as_deref(), Some("session-123"));
    assert_eq!(forwarded.thread_id.as_deref(), Some("thread-456"));
    assert_eq!(forwarded.client_request_id.as_deref(), Some("thread-456"));
    assert_eq!(forwarded.body["model"], "upstream-v1");
    assert_eq!(forwarded.body["stream"], true);
    assert_eq!(forwarded.body["store"], false);
    assert_eq!(
        runtime_channel_connector(&database.pool, credential.id).await,
        "codex_oauth"
    );

    let non_streaming = app
        .clone()
        .oneshot(request(
            "session-123",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": false,
                "input": "hello"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(non_streaming.status(), StatusCode::BAD_REQUEST);
    let non_streaming_body = non_streaming
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert!(String::from_utf8_lossy(&non_streaming_body).contains("codex_streaming_required"));

    let previous = app
        .clone()
        .oneshot(request(
            "session-123",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": true,
                "previous_response_id": "resp_previous",
                "input": "hello"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(previous.status(), StatusCode::BAD_REQUEST);
    let previous_body = previous.into_body().collect().await.unwrap().to_bytes();
    assert!(
        String::from_utf8_lossy(&previous_body).contains("codex_previous_response_unsupported")
    );
    assert_eq!(captured.http_requests.lock().unwrap().len(), 1);

    let gateway = start_server(app.clone()).await;
    let websocket_request = || {
        let mut request = format!("ws://{}/v1/responses", gateway.address)
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", seed.secret)).unwrap(),
        );
        request
            .headers_mut()
            .insert("session-id", HeaderValue::from_static("session-123"));
        request
            .headers_mut()
            .insert("thread-id", HeaderValue::from_static("thread-456"));
        request
            .headers_mut()
            .insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br"));
        request
    };
    let (mut websocket, response) = connect_async(websocket_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    let first_websocket_events = codex_websocket_response_create(
        &mut websocket,
        serde_json::json!({
            "type": "response.create",
            "model": client_model.clone(),
            "stream": false,
            "store": true,
            "generate": false,
            "input": [{"type": "message", "role": "user", "content": []}]
        }),
    )
    .await;
    assert_eq!(
        first_websocket_events
            .last()
            .and_then(|event| event["response"]["id"].as_str()),
        Some("resp_codex_ws_1")
    );
    let second_websocket_events = codex_websocket_response_create(
        &mut websocket,
        serde_json::json!({
            "type": "response.create",
            "model": client_model.clone(),
            "stream": true,
            "store": true,
            "previous_response_id": "resp_codex_ws_1",
            "input": [{"type": "message", "role": "user", "content": []}]
        }),
    )
    .await;
    assert_eq!(
        second_websocket_events
            .last()
            .and_then(|event| event["response"]["id"].as_str()),
        Some("resp_codex_ws_2")
    );
    websocket.close(None).await.unwrap();
    let _ = timeout(Duration::from_secs(1), websocket.next()).await;

    let (mut reconnected_websocket, response) = connect_async(websocket_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let reconnected_events = codex_websocket_response_create(
        &mut reconnected_websocket,
        serde_json::json!({
            "type": "response.create",
            "model": client_model.clone(),
            "stream": true,
            "store": true,
            "previous_response_id": "resp_codex_ws_2",
            "input": [{"type": "message", "role": "user", "content": []}]
        }),
    )
    .await;
    assert_eq!(
        reconnected_events
            .last()
            .and_then(|event| event["response"]["id"].as_str()),
        Some("resp_codex_ws_3")
    );
    reconnected_websocket.close(None).await.unwrap();
    let _ = timeout(Duration::from_secs(1), reconnected_websocket.next()).await;

    let websocket_handshakes = captured.websocket_handshakes.lock().unwrap().clone();
    assert_eq!(websocket_handshakes.len(), 1);
    let handshake = &websocket_handshakes[0];
    assert_eq!(
        handshake.authorization.as_deref(),
        Some("Bearer access-token")
    );
    assert_eq!(handshake.account_id.as_deref(), Some("account-123"));
    assert_eq!(handshake.originator.as_deref(), Some(CODEX_ORIGINATOR));
    assert_eq!(
        handshake.user_agent.as_deref(),
        Some(codex_user_agent().as_str())
    );
    assert_eq!(
        handshake.version.as_deref(),
        codex_user_agent()
            .split_once('/')
            .map(|(_, version)| version)
    );
    assert_eq!(handshake.session_id.as_deref(), Some("session-123"));
    assert_eq!(handshake.thread_id.as_deref(), Some("thread-456"));
    assert_eq!(handshake.client_request_id.as_deref(), Some("thread-456"));
    assert_eq!(
        handshake.openai_beta.as_deref(),
        Some("responses_websockets=2026-02-06")
    );
    assert_eq!(handshake.accept_encoding, None);

    let websocket_requests = captured.websocket_requests.lock().unwrap().clone();
    assert_eq!(websocket_requests.len(), 3);
    assert_eq!(websocket_requests[0]["type"], "response.create");
    assert_eq!(websocket_requests[0]["model"], "upstream-v1");
    assert_eq!(websocket_requests[0]["stream"], true);
    assert_eq!(websocket_requests[0]["store"], false);
    assert_eq!(websocket_requests[0]["generate"], false);
    assert_eq!(
        websocket_requests[1]["previous_response_id"],
        "resp_codex_ws_1"
    );
    assert_eq!(
        websocket_requests[2]["previous_response_id"],
        "resp_codex_ws_2"
    );
    assert_eq!(
        logs.events()
            .iter()
            .filter(|event| event.request_protocol == RequestProtocol::WebSocket)
            .count(),
        3
    );

    repository
        .persist_codex_quota(
            credential.id,
            CodexQuotaUpdate {
                allowed: true,
                limit_reached: false,
                primary_used_percent: Some(96),
                primary_window_seconds: Some(10_800),
                primary_reset_at: Some(Utc::now() + chrono::Duration::hours(1)),
                secondary_used_percent: None,
                secondary_window_seconds: None,
                secondary_reset_at: None,
                checked_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    codex_connector.run_maintenance().await.unwrap();

    let sticky = app
        .clone()
        .oneshot(request(
            "session-123",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": true,
                "input": "continue"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(sticky.status(), StatusCode::OK);
    let _ = sticky.into_body().collect().await.unwrap();

    let new_session = app
        .clone()
        .oneshot(request(
            "session-other",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": true,
                "input": "new"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(new_session.status(), StatusCode::SERVICE_UNAVAILABLE);
    let new_session_body = new_session.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&new_session_body).contains("codex_credential_draining"));
    assert_eq!(captured.http_requests.lock().unwrap().len(), 2);

    repository
        .persist_codex_quota(
            credential.id,
            CodexQuotaUpdate {
                allowed: false,
                limit_reached: true,
                primary_used_percent: Some(100),
                primary_window_seconds: Some(10_800),
                primary_reset_at: Some(Utc::now() + chrono::Duration::hours(1)),
                secondary_used_percent: None,
                secondary_window_seconds: None,
                secondary_reset_at: None,
                checked_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    codex_connector.run_maintenance().await.unwrap();

    for _ in 0..2 {
        let sticky_unavailable = app
            .clone()
            .oneshot(request(
                "session-123",
                serde_json::json!({
                    "model": client_model.clone(),
                    "stream": true,
                    "input": "continue"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(sticky_unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        let sticky_body = sticky_unavailable
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert!(
            String::from_utf8_lossy(&sticky_body).contains("codex_sticky_credential_unavailable")
        );
    }
    assert_eq!(captured.http_requests.lock().unwrap().len(), 2);

    let view = codex_connector
        .credential(credential.id)
        .await
        .unwrap()
        .unwrap();
    codex_connector
        .update_credential(
            seed.user,
            credential.id,
            CodexCredentialUpdateInput {
                label: view.label,
                enabled: false,
                proxy_id: view.proxy_id,
                weight: view.weight,
                quota_threshold_percent: view.quota_threshold_percent,
            },
            view.updated_at,
        )
        .await
        .unwrap();
    let sticky_disabled = app
        .clone()
        .oneshot(request(
            "session-123",
            serde_json::json!({
                "model": client_model.clone(),
                "stream": true,
                "input": "continue"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(sticky_disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
    let sticky_disabled_body = sticky_disabled
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    assert!(
        String::from_utf8_lossy(&sticky_disabled_body)
            .contains("codex_sticky_credential_unavailable")
    );

    let new_disabled = app
        .clone()
        .oneshot(request(
            "session-disabled",
            serde_json::json!({
                "model": client_model,
                "stream": true,
                "input": "new"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(new_disabled.status(), StatusCode::SERVICE_UNAVAILABLE);
    let new_disabled_body = new_disabled.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&new_disabled_body).contains("codex_credential_disabled"));
    let channel_enabled: bool = sqlx::query_scalar("SELECT enabled FROM channels WHERE id=$1")
        .bind(credential.id)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(
        channel_enabled,
        "the provider runtime, not generic routing, owns managed credential enablement"
    );
    assert_eq!(captured.http_requests.lock().unwrap().len(), 2);

    drop(upstream);
    database.cleanup().await;
}

async fn runtime_channel_connector(pool: &PgPool, channel_id: Uuid) -> String {
    sqlx::query_scalar(
        "SELECT g.connector_kind \
         FROM channels c JOIN channel_groups g ON g.id=c.channel_group_id \
         WHERE c.id=$1",
    )
    .bind(channel_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn usd_is_the_only_persisted_billing_currency() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;

    let user_currency_column_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1
             FROM information_schema.columns
             WHERE table_schema = current_schema()
               AND table_name = 'users'
               AND column_name = 'currency'
         )",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(!user_currency_column_exists);

    assert!(
        sqlx::query("UPDATE models SET currency='EUR' WHERE id=$1")
            .bind(seed.model)
            .execute(&database.pool)
            .await
            .is_err(),
        "model prices must remain USD"
    );

    let mut non_usd_log = request_log_event(&seed, RequestLogOutcome::Succeeded);
    non_usd_log.billing.as_mut().unwrap().price.currency = "EUR".into();
    assert!(
        RequestLogRepository::new(database.pool.clone())
            .insert(&non_usd_log)
            .await
            .is_err(),
        "request price snapshots must remain USD"
    );

    database.cleanup().await;
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
    let persisted_billing: PersistedBilling = sqlx::query_as(
        "SELECT input_tokens,cached_input_tokens,cache_write_tokens,output_tokens,reasoning_tokens,cost_amount,currency FROM request_logs WHERE id=$1",
    )
    .bind(event.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(persisted_billing.input_tokens, Some(10));
    assert_eq!(persisted_billing.cached_input_tokens, Some(2));
    assert_eq!(persisted_billing.cache_write_tokens, Some(1));
    assert_eq!(persisted_billing.output_tokens, Some(4));
    assert_eq!(persisted_billing.reasoning_tokens, Some(1));
    assert_eq!(
        persisted_billing.cost_amount,
        Some(rust_decimal::Decimal::new(999, 8))
    );
    assert_eq!(persisted_billing.currency.as_deref(), Some("USD"));
    let persisted_modes: (Option<String>, bool) =
        sqlx::query_as("SELECT reasoning_effort,fast_mode FROM request_logs WHERE id=$1")
            .bind(event.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(persisted_modes.0.as_deref(), Some("high"));
    assert!(persisted_modes.1);

    let mut conflicting = event.clone();
    conflicting.error_code = Some("different_terminal_fact".into());
    assert!(matches!(
        repository.insert(&conflicting).await,
        Err(ai_gateway::persistence::RepositoryError::DuplicateConflict { .. })
    ));
    let mut conflicting_summary = event.clone();
    conflicting_summary.error_summary = Some("different upstream detail".into());
    assert!(matches!(
        repository.insert(&conflicting_summary).await,
        Err(ai_gateway::persistence::RepositoryError::DuplicateConflict { .. })
    ));
    let mut conflicting_fast_mode = event.clone();
    conflicting_fast_mode.fast_mode = false;
    assert!(matches!(
        repository.insert(&conflicting_fast_mode).await,
        Err(ai_gateway::persistence::RepositoryError::DuplicateConflict { .. })
    ));
    let mut conflicting_reasoning = event.clone();
    conflicting_reasoning
        .billing
        .as_mut()
        .unwrap()
        .usage
        .as_mut()
        .unwrap()
        .reasoning_tokens = 0;
    assert!(matches!(
        repository.insert(&conflicting_reasoning).await,
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

#[tokio::test]
async fn request_log_batch_insert_isolates_duplicates_and_invalid_statuses() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let first = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let exact_duplicate = first.clone();
    let mut conflicting_duplicate = first.clone();
    conflicting_duplicate.error_code = Some("conflicting_batch_fact".into());
    let second = request_log_event(&seed, RequestLogOutcome::Failed);
    let mut invalid_status = request_log_event(&seed, RequestLogOutcome::Failed);
    invalid_status.response_status_code = Some(99);

    let outcomes = repository
        .insert_batch(&[
            first.clone(),
            exact_duplicate,
            conflicting_duplicate,
            second.clone(),
            invalid_status,
        ])
        .await
        .unwrap();
    assert_eq!(outcomes[0].outcome, RequestLogBatchInsertOutcome::Inserted);
    assert_eq!(
        outcomes[1].outcome,
        RequestLogBatchInsertOutcome::ExactDuplicate
    );
    assert_eq!(
        outcomes[2].outcome,
        RequestLogBatchInsertOutcome::DuplicateConflict
    );
    assert_eq!(outcomes[3].outcome, RequestLogBatchInsertOutcome::Inserted);
    assert_eq!(
        outcomes[4].outcome,
        RequestLogBatchInsertOutcome::InvalidResponseStatus { status: 99 }
    );
    let persisted: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM request_logs ORDER BY id")
        .fetch_all(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        persisted
            .into_iter()
            .collect::<std::collections::HashSet<_>>(),
        std::collections::HashSet::from([first.id, second.id])
    );

    database.cleanup().await;
}

#[tokio::test]
async fn durable_request_log_pipeline_replays_spool_after_an_ingress_outage() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let spool_directory =
        env::temp_dir().join(format!("ai-gateway-durable-log-test-{}", Uuid::new_v4()));
    let mut config = RequestLoggingConfig {
        queue_capacity: 1,
        spool_directory: spool_directory.clone(),
        spool_compaction_threshold_bytes: 1,
        metrics_interval_seconds: 3_600,
        shutdown_drain_seconds: 1,
        settlement_interval_milliseconds: 10,
        ..RequestLoggingConfig::default()
    };

    let mut ingress_lock = database.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE request_log_ingest IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *ingress_lock)
        .await
        .unwrap();
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), &config)
            .await
            .unwrap();
    let mut events = Vec::new();
    for _ in 0..5 {
        let mut event = request_log_event(&seed, RequestLogOutcome::Succeeded);
        event.client_model = "copy\tbackslash\\雪".into();
        event.upstream_model = Some("upstream\tbackslash\\雪".into());
        sink.try_record(event.clone());
        events.push(event);
    }
    drop(sink);
    worker.shutdown().await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM request_logs")
            .fetch_one(&mut *ingress_lock)
            .await
            .unwrap(),
        0
    );
    ingress_lock.rollback().await.unwrap();

    config.shutdown_drain_seconds = 5;
    let (sink, worker) =
        DurableRequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), &config)
            .await
            .unwrap();
    drop(sink);
    let ids = events.iter().map(|event| event.id).collect::<Vec<_>>();
    let expected_rows = i64::try_from(ids.len()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let settled_rows: i64 = sqlx::query_scalar(
                "SELECT count(*)::bigint
                 FROM request_logs
                 WHERE id = ANY($1)
                   AND billed_at IS NOT NULL",
            )
            .bind(&ids)
            .fetch_one(&database.pool)
            .await
            .unwrap();
            if settled_rows == expected_rows {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("replayed request logs did not settle before the timeout");
    worker.shutdown().await;

    let persisted: Vec<DurablePersistedLog> = sqlx::query_as(
        "SELECT id,client_model,upstream_model,billed_at
         FROM request_logs
         WHERE id = ANY($1)
         ORDER BY id",
    )
    .bind(&ids)
    .fetch_all(&database.pool)
    .await
    .unwrap();
    assert_eq!(persisted.len(), events.len());
    assert!(
        persisted
            .iter()
            .all(|row| ids.contains(&row.id) && row.client_model == "copy\tbackslash\\雪")
    );
    assert!(
        persisted
            .iter()
            .all(|row| row.upstream_model.as_deref() == Some("upstream\tbackslash\\雪"))
    );
    assert!(persisted.iter().all(|row| row.billed_at.is_some()));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*)::bigint FROM request_log_ingest")
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        0
    );

    std::fs::remove_dir_all(&spool_directory).unwrap();
    database.cleanup().await;
}

#[tokio::test]
async fn settlement_claim_is_concurrent_idempotent_and_allows_soft_quota_overdraft() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let event = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let cost = event.billing.as_ref().unwrap().cost_amount.unwrap();
    sqlx::query("UPDATE api_keys SET quota_limit_amount = $1 WHERE id = $2")
        .bind(rust_decimal::Decimal::new(1, 8))
        .bind(seed.key)
        .execute(&database.pool)
        .await
        .unwrap();
    repository.insert(&event).await.unwrap();

    let first = repository.clone();
    let second = repository.clone();
    let (first, second) = tokio::join!(first.settle(event.id), second.settle(event.id));
    let outcomes = [first.unwrap(), second.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RequestLogSettlementOutcome::AlreadyBilled))
            .count(),
        1
    );

    let facts: SettlementFacts = sqlx::query_as(
        "SELECT u.balance_amount, k.quota_used_amount, log.billed_at
         FROM request_logs AS log
         JOIN users AS u ON u.id = log.user_id
         JOIN api_keys AS k ON k.id = log.api_key_id
         WHERE log.id = $1",
    )
    .bind(event.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(facts.balance_amount, -cost);
    assert_eq!(facts.quota_used_amount, cost);
    assert!(facts.quota_used_amount > rust_decimal::Decimal::new(1, 8));
    assert!(facts.billed_at.is_some());

    assert_eq!(
        repository.settle(event.id).await.unwrap(),
        RequestLogSettlementOutcome::AlreadyBilled
    );
    let after_retry: SettlementFacts = sqlx::query_as(
        "SELECT u.balance_amount, k.quota_used_amount, log.billed_at
         FROM request_logs AS log
         JOIN users AS u ON u.id = log.user_id
         JOIN api_keys AS k ON k.id = log.api_key_id
         WHERE log.id = $1",
    )
    .bind(event.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(after_retry.balance_amount, facts.balance_amount);
    assert_eq!(after_retry.quota_used_amount, facts.quota_used_amount);
    assert_eq!(after_retry.billed_at, facts.billed_at);

    let mut zero_cost = request_log_event(&seed, RequestLogOutcome::Failed);
    zero_cost.billing.as_mut().unwrap().cost_amount = Some(rust_decimal::Decimal::ZERO);
    repository.insert(&zero_cost).await.unwrap();
    assert!(matches!(
        repository.settle(zero_cost.id).await.unwrap(),
        RequestLogSettlementOutcome::Settled { .. }
    ));
    let zero_cost_facts: SettlementFacts = sqlx::query_as(
        "SELECT u.balance_amount, k.quota_used_amount, log.billed_at
         FROM request_logs AS log
         JOIN users AS u ON u.id = log.user_id
         JOIN api_keys AS k ON k.id = log.api_key_id
         WHERE log.id = $1",
    )
    .bind(zero_cost.id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(zero_cost_facts.balance_amount, facts.balance_amount);
    assert_eq!(zero_cost_facts.quota_used_amount, facts.quota_used_amount);
    assert!(zero_cost_facts.billed_at.is_some());
    database.cleanup().await;
}

#[tokio::test]
async fn batch_settlement_aggregates_account_updates_and_deduplicates_ids() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let first = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let second = request_log_event(&seed, RequestLogOutcome::Failed);
    let cost = first.billing.as_ref().unwrap().cost_amount.unwrap();
    repository
        .insert_batch(&[first.clone(), second.clone()])
        .await
        .unwrap();

    let outcomes = repository
        .settle_batch(&[first.id, second.id, first.id])
        .await
        .unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .all(|(_, outcome)| matches!(outcome, RequestLogSettlementOutcome::Settled { .. }))
    );
    let facts: (rust_decimal::Decimal, rust_decimal::Decimal, i64) = sqlx::query_as(
        "SELECT user_account.balance_amount,
                key.quota_used_amount,
                count(log.billed_at)::bigint
         FROM users AS user_account
         JOIN api_keys AS key ON key.user_id = user_account.id
         JOIN request_logs AS log ON log.api_key_id = key.id
         WHERE user_account.id = $1 AND key.id = $2
         GROUP BY user_account.balance_amount,key.quota_used_amount",
    )
    .bind(seed.user)
    .bind(seed.key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(facts.0, -(cost + cost));
    assert_eq!(facts.1, cost + cost);
    assert_eq!(facts.2, 2);

    let retried = repository
        .settle_batch(&[first.id, second.id])
        .await
        .unwrap();
    assert!(
        retried
            .iter()
            .all(|(_, outcome)| matches!(outcome, RequestLogSettlementOutcome::AlreadyBilled))
    );
    let unchanged: (rust_decimal::Decimal, rust_decimal::Decimal) = sqlx::query_as(
        "SELECT user_account.balance_amount,key.quota_used_amount
         FROM users AS user_account
         JOIN api_keys AS key ON key.user_id = user_account.id
         WHERE user_account.id = $1 AND key.id = $2",
    )
    .bind(seed.user)
    .bind(seed.key)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(unchanged, (facts.0, facts.1));

    database.cleanup().await;
}

#[tokio::test]
async fn batch_settlement_classifies_ineligible_rows_independently() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());
    let billable = request_log_event(&seed, RequestLogOutcome::Succeeded);
    let mut not_billable = request_log_event(&seed, RequestLogOutcome::Failed);
    not_billable.billing = None;

    let other_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,display_name,role,status)
         VALUES ($1,$2,'user','active')",
    )
    .bind(other_user)
    .bind(format!("batch-mismatch-user-{other_user}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let mut mismatched = request_log_event(&seed, RequestLogOutcome::Failed);
    mismatched.user_id = other_user;
    repository
        .insert_batch(&[billable.clone(), not_billable.clone(), mismatched.clone()])
        .await
        .unwrap();
    let missing = Uuid::new_v4();

    let outcomes = repository
        .settle_batch(&[billable.id, not_billable.id, mismatched.id, missing])
        .await
        .unwrap()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    assert!(matches!(
        outcomes.get(&billable.id),
        Some(RequestLogSettlementOutcome::Settled { .. })
    ));
    assert_eq!(
        outcomes.get(&not_billable.id),
        Some(&RequestLogSettlementOutcome::NotBillable)
    );
    assert_eq!(
        outcomes.get(&mismatched.id),
        Some(&RequestLogSettlementOutcome::AccountMismatch)
    );
    assert_eq!(
        outcomes.get(&missing),
        Some(&RequestLogSettlementOutcome::NotFound)
    );

    let unbilled: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint
         FROM request_logs
         WHERE id = ANY($1) AND billed_at IS NULL",
    )
    .bind(vec![not_billable.id, mismatched.id])
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(unbilled, 2);

    database.cleanup().await;
}

#[tokio::test]
async fn settlement_leaves_account_mismatch_unbilled_and_worker_recovers_durable_logs() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let repository = RequestLogRepository::new(database.pool.clone());

    let other_user = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id,display_name,role,status) \
         VALUES ($1,$2,'user','active')",
    )
    .bind(other_user)
    .bind(format!("settlement-other-user-{other_user}"))
    .execute(&database.pool)
    .await
    .unwrap();
    let mut mismatched = request_log_event(&seed, RequestLogOutcome::Succeeded);
    mismatched.user_id = other_user;
    repository.insert(&mismatched).await.unwrap();
    assert_eq!(
        repository.settle(mismatched.id).await.unwrap(),
        RequestLogSettlementOutcome::AccountMismatch
    );
    let mismatch_billed: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT billed_at FROM request_logs WHERE id = $1")
            .bind(mismatched.id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(mismatch_billed, None);

    let recoverable = request_log_event(&seed, RequestLogOutcome::Cancelled);
    repository.insert(&recoverable).await.unwrap();
    let (_sink, worker) = RequestLogWorker::start(repository, 4);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let billed: Option<DateTime<Utc>> =
            sqlx::query_scalar("SELECT billed_at FROM request_logs WHERE id = $1")
                .bind(recoverable.id)
                .fetch_one(&database.pool)
                .await
                .unwrap();
        if billed.is_some() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "settlement worker did not recover a durable unbilled request log"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    worker.shutdown().await;
    database.cleanup().await;
}

#[tokio::test]
async fn settled_usage_updates_the_live_soft_quota_before_snapshot_reload() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let state = UpstreamState(Arc::new(Mutex::new(UpstreamMode::UsageJson)));
    let upstream_server = start_server(
        Router::new()
            .route("/v1/chat/completions", post(upstream))
            .with_state(state),
    )
    .await;
    sqlx::query(
        "UPDATE models
         SET price_unit_tokens = 1,
             input_unit_price = 1,
             cached_input_unit_price = 0,
             cache_write_unit_price = 0,
             output_unit_price = 1
         WHERE id = $1",
    )
    .bind(seed.model)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE api_keys
         SET quota_limit_amount = 1,
             quota_used_amount = 0
         WHERE id = $1",
    )
    .bind(seed.key)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE channels SET base_url = $1 WHERE id = $2")
        .bind(format!("http://{}", upstream_server.address))
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();

    let runtime = Arc::new(RuntimeConfig::new(
        compile_runtime_config(
            ControlPlaneRepository::new(database.pool.clone())
                .load_runtime()
                .await
                .unwrap(),
        )
        .unwrap(),
    ));
    let admission = AdmissionRuntime::new();
    let (sink, worker) = RequestLogWorker::start_with_admission(
        RequestLogRepository::new(database.pool.clone()),
        32,
        admission.clone(),
    );
    let proxy = ProxyService::with_dependencies(
        runtime,
        1_048_576,
        Arc::new(sink),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        admission,
    )
    .unwrap();
    let gateway = start_server(ai_gateway::http::router(proxy)).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let request = || {
        client
            .post(format!("http://{}/v1/chat/completions", gateway.address))
            .header("authorization", format!("Bearer {}", seed.secret))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(
                    &serde_json::json!({ "model": seed.client_model, "stream": false }),
                )
                .unwrap(),
            )
    };

    let first = request().send().await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert!(first.bytes().await.unwrap().starts_with(b"{"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let quota_used_amount: rust_decimal::Decimal =
            sqlx::query_scalar("SELECT quota_used_amount FROM api_keys WHERE id = $1")
                .bind(seed.key)
                .fetch_one(&database.pool)
                .await
                .unwrap();
        if quota_used_amount == rust_decimal::Decimal::new(5, 0) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "usage settlement did not update the API-key quota"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let second = request().send().await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    let error: serde_json::Value = serde_json::from_slice(&second.bytes().await.unwrap()).unwrap();
    assert_eq!(error["error"]["code"], "insufficient_quota");

    worker.shutdown().await;
    drop(gateway);
    drop(upstream_server);
    database.cleanup().await;
}

const TEST_PASSWORD: &str = "test-password-with-enough-length";
const TEST_ED25519_PRIVATE_KEY: &[u8] = br#"-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIMrLMWiLkvZoPg8iIZRZC0qNdQQPyJV5dCAWdo0l6YBu
-----END PRIVATE KEY-----
"#;
const TEST_ED25519_PUBLIC_KEY: &[u8] = br#"-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAQvs1EKtSBUS0aGjOVZhD2kqVMSiXHugcTiZTZyZxWiQ=
-----END PUBLIC KEY-----
"#;

#[derive(Clone)]
struct ConsoleTestApp {
    router: Router,
    access_token: String,
}

fn test_auth_config() -> AuthConfig {
    AuthConfig {
        issuer: "test-ai-gateway".into(),
        audience: "test-console".into(),
        access_token_ttl_seconds: 900,
        refresh_token_ttl_seconds: 3_600,
        key_id: "test-key".into(),
        signing_key_path: std::path::PathBuf::from("unused-test-private.pem"),
        verification_key_path: std::path::PathBuf::from("unused-test-public.pem"),
    }
}

async fn admin_app(pool: PgPool, actor: Uuid) -> (ConsoleTestApp, Arc<RuntimeConfig>) {
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
) -> (ConsoleTestApp, Arc<RuntimeConfig>) {
    let repository = ControlPlaneRepository::new(pool.clone());
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
    let model_sync = ModelSyncService::new(coordinator.clone(), models_dev, 100);
    let auth = ConsoleAuthService::from_pem(
        AuthRepository::new(pool.clone()),
        &test_auth_config(),
        TEST_ED25519_PRIVATE_KEY,
        TEST_ED25519_PUBLIC_KEY,
    )
    .unwrap();
    let session = auth
        .login(
            format!("test-user-{actor}@example.test"),
            TEST_PASSWORD.into(),
        )
        .await
        .unwrap();
    (
        ConsoleTestApp {
            router: console::router(ConsoleState {
                coordinator,
                codex_connector,
                channel_models: ChannelModelDiscoveryService::new(
                    Arc::clone(&runtime),
                    Arc::clone(&upstream_clients),
                ),
                proxy_tests: ProxyTestService::new(
                    repository,
                    Arc::clone(&runtime),
                    upstream_clients,
                ),
                model_sync,
                auth,
                request_logs: RequestLogRepository::new(pool.clone()),
                system_metrics: SystemMetricsService::new(pool, 5),
                console_body_bytes: 1_048_576,
                auth_body_bytes: 16_384,
                allowed_origins: vec![],
            }),
            access_token: session.access_token,
        },
        runtime,
    )
}

async fn admin_request(
    app: ConsoleTestApp,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    admin_request_with_headers(app, method, path, body, &[]).await
}

async fn activate_invitation(
    app: &ConsoleTestApp,
    invitation_token: &str,
) -> axum::response::Response {
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/v1/auth/activate-invitation")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "invitation_token": invitation_token,
                "password": TEST_PASSWORD,
            }))
            .unwrap(),
        ))
        .unwrap();
    app.router.clone().oneshot(request).await.unwrap()
}

async fn admin_request_with_headers(
    app: ConsoleTestApp,
    method: &str,
    path: &str,
    body: serde_json::Value,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {}", app.access_token))
        .header("content-type", "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    app.router.oneshot(request).await.unwrap()
}

#[tokio::test]
async fn admin_key_create_publishes_immediately_and_audits_redacted() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let response = admin_request(
        app,
        "POST",
        "/console/v1/api-keys",
        serde_json::json!({
            "user_id": seed.user,
            "name": format!("managed-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "allowed_channel_ids": [],
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
        "/console/v1/users",
        serde_json::json!({
            "email": format!("managed-user-{}@example.test", Uuid::new_v4()),
            "display_name": format!("managed-user-{}", Uuid::new_v4()),
            "role": "user",
            "initial_balance_amount": "15.25",
            "default_api_key_policy_id": null
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let user_id = Uuid::parse_str(created["user_id"].as_str().unwrap()).unwrap();
    let invitation_token = created["invitation_token"].as_str().unwrap();
    let path = format!("/console/v1/users/{user_id}");
    let pending = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(pending.status(), StatusCode::OK);
    let pending_etag = pending.headers()["etag"].to_str().unwrap().to_owned();
    let pending: serde_json::Value =
        serde_json::from_slice(&pending.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(pending["status"], "invited");
    assert_eq!(
        pending["balance_amount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::new(1_525, 2)
    );
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PATCH",
            &path,
            serde_json::json!({"status": "active"}),
            &[("if-match", &pending_etag)]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PATCH",
            &path,
            serde_json::json!({"display_name": "Managed invitee renamed"}),
            &[("if-match", &pending_etag)]
        )
        .await
        .status(),
        StatusCode::OK
    );
    let pending = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(pending.status(), StatusCode::OK);
    let pending: serde_json::Value =
        serde_json::from_slice(&pending.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(pending["status"], "invited");
    assert_eq!(pending["display_name"], "Managed invitee renamed");

    sqlx::query("UPDATE users SET status='disabled' WHERE id=$1")
        .bind(user_id)
        .execute(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        activate_invitation(&app, invitation_token).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let replacement = admin_request(
        app.clone(),
        "POST",
        &format!("/console/v1/users/{user_id}/invitation"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(replacement.status(), StatusCode::CREATED);
    let replacement: serde_json::Value =
        serde_json::from_slice(&replacement.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let replacement_token = replacement["invitation_token"].as_str().unwrap();
    assert_eq!(
        activate_invitation(&app, invitation_token).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let recovered = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered: serde_json::Value =
        serde_json::from_slice(&recovered.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(recovered["status"], "invited");
    assert_eq!(recovered["can_reissue_invitation"], true);
    assert_eq!(
        activate_invitation(&app, replacement_token).await.status(),
        StatusCode::OK
    );
    let users = admin_request(
        app.clone(),
        "GET",
        "/console/v1/users",
        serde_json::json!({}),
    )
    .await;
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
        "/console/v1/api-keys",
        serde_json::json!({
            "user_id": user_id,
            "name": format!("managed-key-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "allowed_channel_ids": [],
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

    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        detail["balance_amount"]
            .as_str()
            .unwrap()
            .parse::<rust_decimal::Decimal>()
            .unwrap(),
        rust_decimal::Decimal::new(1_525, 2)
    );
    let status_update = serde_json::json!({"status": "suspended"});

    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PATCH",
            &path,
            status_update.clone(),
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
    assert_eq!(audit["after"]["balance_amount"].as_f64(), Some(15.25));

    assert_eq!(
        admin_request_with_headers(app, "PATCH", &path, status_update, &[("if-match", &etag)])
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
        "/console/v1/models",
        serde_json::json!({
            "source_model_id": source_model_id,
            "display_name": "Managed model",
            "provider_name": "test-provider",
            "enabled": true,
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
            "/console/v1/models",
            serde_json::json!({
                "source_model_id": format!("invalid-model-{}", Uuid::new_v4()),
                "display_name": "Invalid model",
                "enabled": true,
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
        "/console/v1/models",
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

    let path = format!("/console/v1/models/{model_id}");
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

    let seed_path = format!("/console/v1/models/{}", seed.model);
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
async fn models_dev_catalog_apply_is_explicit_and_updates_selected_existing_prices() {
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
        "/console/v1/catalog/models/sync/preview",
        serde_json::json!({"provider_ids":["provider-a"]}),
    )
    .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let preview: serde_json::Value =
        serde_json::from_slice(&preview.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(preview["models"].as_array().unwrap().len(), 1);
    assert_eq!(preview["models"][0]["provider_id"], "provider-a");
    assert_eq!(preview["models"][0]["model_id"], "catalog-model");
    assert_eq!(preview["models"][0]["action"], "import");
    assert_eq!(
        preview["models"][0]["advanced_billing"]["long_context_tiers"][0]["input_tokens_threshold"],
        128_000
    );
    assert_eq!(
        preview["models"][0]["advanced_billing"]["long_context_tiers"][0]["output_unit_price"],
        "5.0"
    );
    assert_eq!(
        preview["models"][0]["advanced_billing"]["request_multipliers"][0],
        serde_json::json!({
            "json_pointer": "/service_tier",
            "value": "priority",
            "multiplier": "2"
        })
    );
    assert_eq!(preview["excluded_missing_prices"], 1);
    assert!(preview["models"][0].get("source_payload").is_none());

    let removed_sync = admin_request(
        app.clone(),
        "POST",
        "/console/v1/models/sync",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(removed_sync.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM models WHERE source_model_id='catalog-model'",
        )
        .fetch_one(&database.pool)
        .await
        .unwrap(),
        0
    );

    let imported = admin_request(
        app.clone(),
        "POST",
        "/console/v1/catalog/models/import",
        serde_json::json!({
            "selections":[{"provider_id":"provider-a","model_id":"catalog-model"}]
        }),
    )
    .await;
    assert_eq!(imported.status(), StatusCode::OK);
    let imported: serde_json::Value =
        serde_json::from_slice(&imported.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(imported["model_count"], 1);
    assert_eq!(imported["imported_count"], 1);
    assert_eq!(imported["updated_count"], 0);

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
    let imported_billing: serde_json::Value = sqlx::query_scalar(
        "SELECT advanced_billing FROM models WHERE source_model_id='catalog-model'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(
        imported_billing["long_context_tiers"][0]["input_tokens_threshold"],
        128_000
    );
    assert_eq!(
        imported_billing["long_context_tiers"][0]["output_unit_price"],
        "5.0"
    );
    assert_eq!(
        imported_billing["request_multipliers"][0],
        serde_json::json!({
            "json_pointer": "/service_tier",
            "value": "priority",
            "multiplier": "2"
        })
    );

    let model_id: Uuid =
        sqlx::query_scalar("SELECT id FROM models WHERE source_model_id='catalog-model'")
            .fetch_one(&database.pool)
            .await
            .unwrap();
    let detail = admin_request(
        app.clone(),
        "GET",
        &format!("/console/v1/models/{model_id}"),
        serde_json::json!({}),
    )
    .await;
    let detail = serde_json::from_slice::<serde_json::Value>(
        &detail.into_body().collect().await.unwrap().to_bytes(),
    )
    .unwrap();
    assert!(detail.get("source_payload").is_none());
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT after_redacted FROM audit_logs WHERE object_id=$1 AND object_type='model' AND action='import'",
    )
    .bind(model_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert!(audit.get("source_payload").is_none());

    let preview = admin_request(
        app.clone(),
        "POST",
        "/console/v1/catalog/models/sync/preview",
        serde_json::json!({"provider_ids":["provider-a"]}),
    )
    .await;
    let preview: serde_json::Value =
        serde_json::from_slice(&preview.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(preview["models"][0]["action"], "price_update");

    sqlx::query(
        r#"UPDATE models
         SET display_name='locally named',
             provider_name='Local provider',
             enabled=false,
             input_unit_price=99,
             output_unit_price=99,
             advanced_billing=jsonb_set(
                 advanced_billing,
                 '{request_multipliers}',
                 '[{"json_pointer":"/reasoning/effort","value":"high","multiplier":"2"}]'::jsonb
             )
         WHERE id=$1"#,
    )
    .bind(model_id)
    .execute(&database.pool)
    .await
    .unwrap();
    let removed_sync = admin_request(
        app.clone(),
        "POST",
        "/console/v1/models/sync",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(removed_sync.status(), StatusCode::METHOD_NOT_ALLOWED);
    let prices: (rust_decimal::Decimal, rust_decimal::Decimal) =
        sqlx::query_as("SELECT input_unit_price,output_unit_price FROM models WHERE id=$1")
            .bind(model_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        prices,
        (
            rust_decimal::Decimal::new(99, 0),
            rust_decimal::Decimal::new(99, 0)
        )
    );

    let updated = admin_request(
        app.clone(),
        "POST",
        "/console/v1/catalog/models/import",
        serde_json::json!({
            "selections":[{"provider_id":"provider-a","model_id":"catalog-model"}]
        }),
    )
    .await;
    assert_eq!(updated.status(), StatusCode::OK);
    let updated: serde_json::Value =
        serde_json::from_slice(&updated.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(updated["model_count"], 1);
    assert_eq!(updated["imported_count"], 0);
    assert_eq!(updated["updated_count"], 1);
    let updated_model: (
        String,
        Option<String>,
        bool,
        rust_decimal::Decimal,
        rust_decimal::Decimal,
    ) = sqlx::query_as(
        "SELECT display_name,provider_name,enabled,input_unit_price,output_unit_price
         FROM models WHERE id=$1",
    )
    .bind(model_id)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(updated_model.0, "locally named");
    assert_eq!(updated_model.1.as_deref(), Some("Local provider"));
    assert!(!updated_model.2);
    assert_eq!(updated_model.3, rust_decimal::Decimal::new(125, 2));
    assert_eq!(updated_model.4, rust_decimal::Decimal::new(250, 2));
    let updated_billing: serde_json::Value =
        sqlx::query_scalar("SELECT advanced_billing FROM models WHERE id=$1")
            .bind(model_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        updated_billing["long_context_tiers"][0]["input_tokens_threshold"],
        128_000
    );
    assert_eq!(
        updated_billing["request_multipliers"][0]["json_pointer"],
        "/reasoning/effort"
    );
    assert_eq!(
        updated_billing["request_multipliers"][1],
        serde_json::json!({
            "json_pointer": "/service_tier",
            "value": "priority",
            "multiplier": "2"
        })
    );

    let missing_price_import = admin_request(
        app,
        "POST",
        "/console/v1/catalog/models/import",
        serde_json::json!({
            "selections":[{"provider_id":"provider-a","model_id":"missing-price"}]
        }),
    )
    .await;
    assert_eq!(
        missing_price_import.status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let price_sync_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_logs WHERE object_type='model' AND action='price_sync'",
    )
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(price_sync_audits, 1);
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
        "/console/v1/api-keys",
        serde_json::json!({
            "user_id": seed.user,
            "name": format!("policy-managed-{}", Uuid::new_v4()),
            "allowed_api_formats": ["open_ai_chat_completions"],
            "permissions": ["proxy"],
            "allowed_group_ids": [seed.group],
            "allowed_channel_ids": [],
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
        "/console/v1/api-keys",
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

    let path = format!("/console/v1/api-keys/{id}");
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["requests_per_minute"], 7);
    assert_eq!(detail["max_concurrent_requests"], 3);
    assert_eq!(detail["quota_limit_amount"], "125.50000000");
    assert_eq!(detail["quota_used_amount"], "0");

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
            "allowed_channel_ids": [],
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

    database.cleanup().await;
}

#[tokio::test]
async fn manual_channel_disable_publishes_an_unavailable_route() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let path = format!("/console/v1/routing/channels/{}", seed.channel);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let response = admin_request_with_headers(
        app,
        "PUT",
        &path,
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
            "enabled": false, "weight": 1,
            "upstream_auth_kind": "bearer",
            "available_models": ["upstream-v1"]
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM channels WHERE id=$1")
        .bind(seed.channel)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert!(!enabled);
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audit_after, audit_before + 1);
    let snapshot = runtime.snapshot();
    assert!(snapshot.channel(seed.channel).is_none());
    let rule = snapshot
        .model_rule(ApiFormat::OpenAiChatCompletions, &seed.client_model)
        .unwrap();
    assert!(rule.tiers().is_empty());
    assert_eq!(rule.unavailable_candidates()[0].channel_id(), seed.channel);
    let key = snapshot.authenticate(&seed.secret).unwrap();
    assert_eq!(
        snapshot.models_for(&key, ApiFormat::OpenAiChatCompletions),
        vec![Arc::from(seed.client_model.as_str())]
    );
    assert!(matches!(
        routing::select(
            &snapshot,
            &key,
            ApiFormat::OpenAiChatCompletions,
            &seed.client_model,
        ),
        ai_gateway::routing::SelectionResult::NoHealthyChannel { .. }
    ));
    database.cleanup().await;
}

#[tokio::test]
async fn adding_a_group_channel_for_another_model_preserves_existing_routes() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    sqlx::query(
        "UPDATE model_rules SET channel_group_ids=ARRAY[$1]::uuid[], channel_ids='{}' WHERE id=$2",
    )
    .bind(seed.group)
    .bind(seed.rule)
    .execute(&database.pool)
    .await
    .unwrap();
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let response = admin_request(
        app,
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": seed.group,
            "api_format": "open_ai_chat_completions",
            "name": format!("other-model-{}", Uuid::new_v4()),
            "base_url": "https://other-model.example.test",
            "enabled": true,
            "weight": 1,
            "upstream_auth_kind": "bearer",
            "upstream_api_key": "other-upstream-secret",
            "available_models": ["different-upstream"]
        }),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let created_channel: Uuid = serde_json::from_value(created["id"].clone()).unwrap();

    let snapshot = runtime.snapshot();
    assert!(snapshot.channel(created_channel).is_some());
    let rule = snapshot
        .model_rule(ApiFormat::OpenAiChatCompletions, &seed.client_model)
        .unwrap();
    assert_eq!(rule.tiers().len(), 1);
    assert_eq!(rule.tiers()[0].channel_ids(), &[seed.channel]);
    database.cleanup().await;
}

#[tokio::test]
async fn model_incompatible_direct_channel_rolls_back_database_audit_and_snapshot() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, runtime) = admin_app(database.pool.clone(), seed.user).await;
    let audit_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let path = format!("/console/v1/routing/channels/{}", seed.channel);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let response = admin_request_with_headers(
        app,
        "PUT",
        &path,
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
            "enabled": true, "weight": 1,
            "upstream_auth_kind": "bearer",
            "available_models": ["different-upstream"]
        }),
        &[("if-match", &etag)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), br#"{"error":"routing_dependency_invalid"}"#);
    let available_models: Vec<String> =
        sqlx::query_scalar("SELECT available_models FROM channels WHERE id=$1")
            .bind(seed.channel)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(available_models, vec!["upstream-v1"]);
    let audit_after: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(audit_before, audit_after);
    assert!(runtime.snapshot().channel(seed.channel).is_some());
    database.cleanup().await;
}

#[tokio::test]
async fn proxy_template_management_exposes_editable_documents_and_keeps_audits_redacted() {
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
        "/console/v1/network/proxies",
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
        "/console/v1/transforms/templates",
        serde_json::json!({
            "name": "managed-template",
            "description": "managed template",
            "document": template_document.clone(),
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
        "/console/v1/network/proxies",
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

    let proxy_path = format!("/console/v1/network/proxies/{proxy_id}");
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
            "proxy_url": "socks5h://managed-proxy.test:1080",
            "password": "updated-proxy-password",
            "no_proxy_hosts": ["internal.test", "metadata.test"],
            "enabled": true
        }),
        &[("if-match", &proxy_etag)],
    )
    .await;
    assert_eq!(updated_proxy.status(), StatusCode::OK);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT proxy_url FROM proxies WHERE id=$1")
            .bind(proxy_id)
            .fetch_one(&database.pool)
            .await
            .unwrap(),
        "socks5h://managed-proxy.test:1080"
    );

    let template_list = admin_request(
        app.clone(),
        "GET",
        "/console/v1/transforms/templates",
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

    let template_path = format!("/console/v1/transforms/templates/{template_id}");
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
    assert_eq!(template_detail["document"], template_document);
    assert!(template_detail.to_string().contains(template_value));
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &template_path,
            serde_json::json!({
                "name": "managed-template-updated",
                "description": "updated template",
                "document": template_document.clone(),
                "enabled": true
            }),
            &[("if-match", &template_etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );
    let template_detail =
        admin_request(app.clone(), "GET", &template_path, serde_json::json!({})).await;
    let template_etag = template_detail.headers()["etag"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &template_path,
            serde_json::json!({
                "name": "managed-template-metadata-only",
                "description": "metadata-only update",
                "enabled": true
            }),
            &[("if-match", &template_etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );
    let persisted_template_document: serde_json::Value =
        sqlx::query_scalar("SELECT document FROM config_templates WHERE id=$1")
            .bind(template_id)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(persisted_template_document, template_document);

    let channel_path = format!("/console/v1/routing/channels/{}", seed.channel);
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
    let channel_etag = channel_read.headers()["etag"].to_str().unwrap().to_owned();
    let channel_read: serde_json::Value =
        serde_json::from_slice(&channel_read.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(
        channel_read["override_document"],
        valid_channel["override_document"]
    );
    assert_eq!(channel_read["upstream_api_key"], "upstream-secret");
    assert!(channel_read.to_string().contains(channel_value));
    let mut metadata_only_channel = valid_channel.clone();
    metadata_only_channel
        .as_object_mut()
        .unwrap()
        .remove("override_document");
    metadata_only_channel["name"] = serde_json::json!("managed-channel-metadata-only");
    assert_eq!(
        admin_request_with_headers(
            app.clone(),
            "PUT",
            &channel_path,
            metadata_only_channel,
            &[("if-match", &channel_etag)],
        )
        .await
        .status(),
        StatusCode::OK
    );
    let persisted_channel_document: serde_json::Value =
        sqlx::query_scalar("SELECT override_document FROM channels WHERE id=$1")
            .bind(seed.channel)
            .fetch_one(&database.pool)
            .await
            .unwrap();
    assert_eq!(
        persisted_channel_document,
        valid_channel["override_document"]
    );
    let published = runtime.snapshot();
    assert_eq!(
        published
            .channel(seed.channel)
            .unwrap()
            .upstream_policy()
            .effective_transforms()
            .request_headers()
            .operations()
            .len(),
        2
    );

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
            "/console/v1/network/proxies",
            serde_json::json!({"name":"invalid-proxy", "proxy_url":"ftp://invalid.test", "password":"invalid-proxy-password", "enabled":true}),
            "invalid-proxy-password",
        ),
        (
            "/console/v1/transforms/templates",
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
        &format!("/console/v1/api-keys/{}/revoke", seed.key),
        serde_json::json!({"reason":"incident"}),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::OK);
    let updated = admin_request(
        app,
        "PUT",
        &format!("/console/v1/api-keys/{}", seed.key),
        serde_json::json!({
            "name":"test", "status":"active", "allowed_api_formats":["open_ai_chat_completions"],
            "permissions":["proxy","models.read"], "allowed_group_ids":[seed.group],
            "allowed_channel_ids":[], "expires_at":null
        }),
    )
    .await;
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
        &format!("/console/v1/api-keys/{}/revoke", seed.key),
        serde_json::json!({"reason": "x".repeat(501)}),
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), br#"{"error":"Console operation rejected"}"#);
    let status: String = sqlx::query_scalar("SELECT status FROM api_keys WHERE id=$1")
        .bind(seed.key)
        .fetch_one(&database.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    database.cleanup().await;
}

#[tokio::test]
async fn management_channel_credentials_are_visible_kept_replaced_and_cleared_safely() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let path = format!("/console/v1/routing/channels/{}", seed.channel);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(detail.headers()["cache-control"], "no-store");
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["upstream_credential_configured"], true);
    assert_eq!(detail["upstream_api_key"], "upstream-secret");

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
async fn channel_documents_are_rejected_and_never_escape_audit_allowlists() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let path = format!("/console/v1/routing/channels/{}", seed.channel);
    let audits_before: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_logs")
        .fetch_one(&database.pool)
        .await
        .unwrap();
    let rejected_create = admin_request(
        app.clone(),
        "POST",
        "/console/v1/routing/channels",
        serde_json::json!({
            "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
            "name": format!("rejected-channel-{}", Uuid::new_v4()),
            "base_url": "https://example.test", "enabled": true, "weight": 1,
            "upstream_auth_kind": "bearer", "upstream_api_key": "upstream-secret",
            "available_models": ["upstream-v1"],
            "override_document": {"headers": {"Authorization": "rejected-create-secret"}}
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
    sqlx::query("UPDATE channels SET override_document=$1 WHERE id=$2")
        .bind(&persisted_override)
        .bind(seed.channel)
        .execute(&database.pool)
        .await
        .unwrap();

    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let detail: serde_json::Value =
        serde_json::from_slice(&detail.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(detail["override_document"], persisted_override);

    let valid_update = serde_json::json!({
        "channel_group_id": seed.group, "api_format": "open_ai_chat_completions",
        "name": format!("test-channel-{}", seed.channel), "base_url": "https://example.test",
        "enabled": true, "weight": 1, "upstream_auth_kind": "bearer",
        "available_models": ["upstream-v1"], "upstream_api_key": "upstream-secret",
        "override_document": {}
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
        "Authorization",
        "cookie",
        "body",
        "nested-authorization-secret",
        "nested-cookie-secret",
        "nested-body-secret",
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
        "override_document": {"headers": {"Authorization": "rejected-secret"}}
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
    let path = format!("/console/v1/api-keys/{}", seed.key);
    let detail = admin_request(app.clone(), "GET", &path, serde_json::json!({})).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let etag = detail.headers()["etag"].to_str().unwrap().to_owned();
    let input = serde_json::json!({"name":"updated", "status":"active", "allowed_api_formats":["open_ai_chat_completions"], "permissions":["proxy"], "allowed_group_ids":[seed.group], "allowed_channel_ids":[], "expires_at":null});
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
    let proxy = ProxyService::new(Arc::new(RuntimeConfig::new(snapshot)), 1_048_576).unwrap();
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
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
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
    sqlx::query("UPDATE models SET source_model_id = 'upstream-v2' WHERE id = $1")
        .bind(seed.model)
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
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
    ));
    let coordinator = ControlPlaneCoordinator::new(
        repository,
        Arc::clone(&runtime),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
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
        compile_runtime_config(repository.load_runtime().await.unwrap()).unwrap(),
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
    sqlx::query(
        "UPDATE api_keys \
         SET allowed_group_ids = '{}', allowed_channel_ids = ARRAY[$1]::uuid[] \
         WHERE id = $2",
    )
    .bind(seed.channel)
    .bind(seed.key)
    .execute(&database.pool)
    .await
    .unwrap();
    let channel_snapshot = compile_control_plane(
        ControlPlaneRepository::new(database.pool.clone())
            .load()
            .await
            .unwrap(),
    )
    .unwrap();
    let channel_key = channel_snapshot.authenticate(&seed.secret).unwrap();
    assert!(
        !routing::select(
            &channel_snapshot,
            &channel_key,
            ApiFormat::OpenAiChatCompletions,
            &seed.client_model,
        )
        .is_none()
    );
    assert_eq!(
        channel_snapshot.models_for(&channel_key, ApiFormat::OpenAiChatCompletions),
        vec![Arc::from(seed.client_model.as_str())]
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
async fn admission_controls_and_overlapping_group_targets_are_compiled() {
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
    assert!(compile_control_plane(records).is_ok());
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
        compile_runtime_config(
            ControlPlaneRepository::new(database.pool.clone())
                .load_runtime()
                .await
                .unwrap(),
        )
        .unwrap(),
    ));
    let (sink, worker): (QueueRequestLogSink, RequestLogWorker) =
        RequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), 32);
    let proxy = ProxyService::with_log_sink(runtime, 1_048_576, Arc::new(sink)).unwrap();
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
                serde_json::to_vec(&serde_json::json!({
                    "model": model,
                    "stream": stream,
                    "reasoning_effort": "high",
                    "service_tier": "priority"
                }))
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
    assert_eq!(success.reasoning_effort.as_deref(), Some("high"));
    assert!(success.fast_mode);
    assert_eq!(success.upstream_model.as_deref(), Some("upstream-v1"));
    assert_eq!(success.model_rule_id, Some(seed.rule));
    assert_eq!(success.channel_group_id, Some(seed.group));
    assert_eq!(success.channel_id, Some(seed.channel));
    assert_eq!(success.model_id, Some(seed.model));
    assert_eq!(success.response_status_code, Some(200));
    assert_eq!(success.request_protocol, "sse");
    assert!(success.streamed);
    assert!(success.ttft_ms.is_some());
    assert_eq!(success.error_code, None);
    assert_eq!(success.error_summary, None);

    *state.0.lock().unwrap() = UpstreamMode::SseError;
    let response = request(&seed.secret, &seed.client_model, true)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .bytes()
            .await
            .unwrap()
            .windows(b"provider_error".len())
            .any(|window| window == b"provider_error")
    );
    let sse_error = wait_for_terminal_log(
        &database.pool,
        seed.key,
        &seed.client_model,
        "failed",
        Some("provider_error"),
    )
    .await;
    assert_eq!(sse_error.response_status_code, Some(200));
    assert_eq!(sse_error.request_protocol, "sse");
    assert_eq!(
        sse_error.error_summary.as_deref(),
        Some("upstream quota exhausted")
    );

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
    assert_eq!(upstream_failure.request_protocol, "non_stream");

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
    assert_eq!(rejected.reasoning_effort.as_deref(), Some("high"));
    assert!(rejected.fast_mode);
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
    assert_eq!(total, 8);
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
                Some("provider_error".into()),
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
        compile_runtime_config(
            ControlPlaneRepository::new(database.pool.clone())
                .load_runtime()
                .await
                .unwrap(),
        )
        .unwrap(),
    ));
    let (sink, worker): (QueueRequestLogSink, RequestLogWorker) =
        RequestLogWorker::start(RequestLogRepository::new(database.pool.clone()), 2);
    // Keeping this clone alive verifies shutdown closes acceptance before draining.
    let producer_clone = sink.clone();
    let proxy = ProxyService::with_log_sink(runtime, 1_048_576, Arc::new(sink)).unwrap();
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

    const TOTAL_REQUESTS: i64 = 33;
    for _ in 1..TOTAL_REQUESTS {
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
    assert!(
        (3..TOTAL_REQUESTS).contains(&total),
        "accepted and in-flight batches must drain while overflow events drop; persisted {total}"
    );
    let accepted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM request_logs WHERE api_key_id = $1 AND client_model = $2 AND outcome = 'succeeded'",
    )
    .bind(seed.key)
    .bind(&seed.client_model)
    .fetch_one(&database.pool)
    .await
    .unwrap();
    assert_eq!(accepted, total);
    drop(gateway);
    drop(upstream_server);
    database.cleanup().await;
}

#[tokio::test]
async fn console_members_are_limited_to_their_own_keys_and_logs() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (admin_app, _) = admin_app(database.pool.clone(), seed.user).await;

    let invitation = admin_request(
        admin_app.clone(),
        "POST",
        "/console/v1/users",
        serde_json::json!({
            "email": format!("member-{}@example.test", Uuid::new_v4()),
            "display_name": "Member",
            "role": "user",
            "default_api_key_policy_id": null
        }),
    )
    .await;
    assert_eq!(invitation.status(), StatusCode::CREATED);
    let invitation: serde_json::Value =
        serde_json::from_slice(&invitation.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let member_id = Uuid::parse_str(invitation["user_id"].as_str().unwrap()).unwrap();
    let invitation_token = invitation["invitation_token"].as_str().unwrap();
    let activation = activate_invitation(&admin_app, invitation_token).await;
    assert_eq!(activation.status(), StatusCode::OK);
    let activation: serde_json::Value =
        serde_json::from_slice(&activation.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    let member_app = ConsoleTestApp {
        router: admin_app.router.clone(),
        access_token: activation["access_token"].as_str().unwrap().to_owned(),
    };

    let policy_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO api_key_policies
         (id,name,allowed_group_ids,allowed_channel_ids,enabled)
         VALUES ($1,$2,ARRAY[$3]::uuid[],'{}',true)",
    )
    .bind(policy_id)
    .bind(format!("member-policy-{policy_id}"))
    .bind(seed.group)
    .execute(&database.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET default_api_key_policy_id=$2 WHERE id=$1")
        .bind(member_id)
        .bind(policy_id)
        .execute(&database.pool)
        .await
        .unwrap();

    assert_eq!(
        admin_request(
            member_app.clone(),
            "GET",
            "/console/v1/users",
            serde_json::json!({}),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );

    let created = admin_request(
        member_app.clone(),
        "POST",
        "/console/v1/me/api-keys",
        serde_json::json!({
            "name":"member-key",
            "expires_at":null,
            "allowed_group_ids":[seed.group],
            "allowed_channel_ids":[],
            "requests_per_minute":null,
            "max_concurrent_requests":null,
            "quota_limit_amount":null
        }),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: serde_json::Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(created["secret"].as_str().is_some());

    assert_eq!(
        admin_request(
            member_app.clone(),
            "GET",
            &format!("/console/v1/me/api-keys/{}", seed.key),
            serde_json::json!({}),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        admin_request(
            member_app.clone(),
            "GET",
            "/console/v1/me/request-logs",
            serde_json::json!({}),
        )
        .await
        .status(),
        StatusCode::OK
    );

    sqlx::query("UPDATE users SET status='suspended',auth_version=auth_version+1 WHERE id=$1")
        .bind(member_id)
        .execute(&database.pool)
        .await
        .unwrap();
    assert_eq!(
        admin_request(member_app, "GET", "/console/v1/me", serde_json::json!({}),)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    database.cleanup().await;
}

#[tokio::test]
async fn console_refresh_tokens_rotate_and_replay_is_rejected() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let login = axum::http::Request::builder()
        .method("POST")
        .uri("/console/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "email": seed.email,
                "password": seed.password,
            }))
            .unwrap(),
        ))
        .unwrap();
    let login = app.router.clone().oneshot(login).await.unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let refresh_cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();

    let refresh_request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/console/v1/auth/refresh")
            .header("cookie", &refresh_cookie)
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.router
            .clone()
            .oneshot(refresh_request())
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.router
            .clone()
            .oneshot(refresh_request())
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    database.cleanup().await;
}

#[tokio::test]
async fn console_auth_body_limit_returns_payload_too_large() {
    let database = TestDatabase::new().await;
    let seed = seed(&database.pool).await;
    let (app, _) = admin_app(database.pool.clone(), seed.user).await;
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/v1/auth/login")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "email": seed.email,
                "password": "x".repeat(20_000),
            }))
            .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    database.cleanup().await;
}
