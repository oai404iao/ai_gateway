use std::{
    convert::Infallible,
    future::pending,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{
        AutomaticDisableSettings, PassiveHealthSettings, RequestRetrySettings,
        ScheduledTestingSettings, SessionAffinitySettings, SystemRuntimeSettings,
        UpstreamTimeoutDefaults,
    },
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane_with_system_settings},
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::Response,
    routing::post,
};
use futures_util::{StreamExt, stream};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};
use uuid::Uuid;

const CLIENT_KEY: &str = "passive-health-client-key";
const WAIT: Duration = Duration::from_secs(3);

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

struct ProxyFixture {
    service: ProxyService,
    runtime: Arc<RuntimeConfig>,
    channel_ids: Vec<Uuid>,
    group_ids: Vec<Uuid>,
    logs: RecordingRequestLogSink,
}

fn proxy_fixture(
    upstream_urls: &[String],
    priorities: &[i32],
    allowed_indices: &[usize],
    upstream: UpstreamConfig,
    routing: RoutingRuntime,
) -> ProxyFixture {
    proxy_fixture_with_retry(
        upstream_urls,
        priorities,
        allowed_indices,
        upstream,
        routing,
        RequestRetrySettings::default(),
    )
}

fn proxy_fixture_with_retry(
    upstream_urls: &[String],
    priorities: &[i32],
    allowed_indices: &[usize],
    upstream: UpstreamConfig,
    routing: RoutingRuntime,
    request_retry: RequestRetrySettings,
) -> ProxyFixture {
    assert_eq!(upstream_urls.len(), priorities.len());
    let group_ids = (0..upstream_urls.len())
        .map(|_| Uuid::new_v4())
        .collect::<Vec<_>>();
    let channel_ids = (0..upstream_urls.len())
        .map(|_| Uuid::new_v4())
        .collect::<Vec<_>>();
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            secret_value: CLIENT_KEY.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec!["open_ai_chat_completions".into()],
            permissions: vec!["proxy".into()],
            allowed_group_ids: allowed_indices
                .iter()
                .map(|index| group_ids[*index])
                .collect(),
            allowed_channel_ids: vec![],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }],
        groups: group_ids
            .iter()
            .zip(priorities)
            .map(|(id, priority)| ChannelGroupRecord {
                id: *id,
                name: id.to_string(),
                api_format: "open_ai_chat_completions".into(),
                priority: *priority,
                selection_strategy: "weighted_random".into(),
                enabled: true,
            })
            .collect(),
        channels: channel_ids
            .iter()
            .zip(group_ids.iter())
            .zip(upstream_urls)
            .map(|((id, group_id), base_url)| ChannelRecord {
                id: *id,
                channel_group_id: *group_id,
                api_format: "open_ai_chat_completions".into(),
                name: id.to_string(),
                base_url: base_url.clone(),
                enabled: true,
                auto_disabled: false,
                auto_disable_allowed: false,
                weight: 1,
                proxy_id: None,
                config_template_id: None,
                override_document: serde_json::json!({}),
                connect_timeout_ms: None,
                response_header_timeout_ms: None,
                stream_idle_timeout_ms: None,
                upstream_auth_kind: "none".into(),
                upstream_auth_header_name: None,
                upstream_api_key: None,
                available_models: vec!["model".into()],
                test_model: None,
            })
            .collect(),
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: "model".into(),
            api_format: "open_ai_chat_completions".into(),
            upstream_model_id: Uuid::new_v4(),
            upstream_model_enabled: true,
            upstream_model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Default::default(),
            cached_input_unit_price: Default::default(),
            cache_write_unit_price: Default::default(),
            output_unit_price: Default::default(),
            upstream_model: "model".into(),
            channel_group_ids: group_ids.clone(),
            channel_ids: vec![],
            enabled: true,
        }],
        proxies: vec![],
        templates: vec![],
    };
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::new_with_all(
                UpstreamTimeoutDefaults::new(
                    Duration::from_secs(upstream.connect_timeout_seconds),
                    Duration::from_secs(upstream.response_header_timeout_seconds),
                    Duration::from_secs(upstream.stream_idle_timeout_seconds),
                ),
                request_retry,
                PassiveHealthSettings::default(),
                AutomaticDisableSettings::default(),
                ScheduledTestingSettings::default(),
                SessionAffinitySettings::default(),
            ),
        )
        .unwrap(),
    ));
    let logs = RecordingRequestLogSink::default();
    let service = ProxyService::with_log_sink_and_routing(
        Arc::clone(&runtime),
        1_048_576,
        Arc::new(logs.clone()),
        routing,
    )
    .unwrap();
    ProxyFixture {
        service,
        runtime,
        channel_ids,
        group_ids,
        logs,
    }
}

fn upstream_config(headers: u64, idle: u64) -> UpstreamConfig {
    UpstreamConfig {
        connect_timeout_seconds: 1,
        response_header_timeout_seconds: headers,
        stream_idle_timeout_seconds: idle,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

fn request(client: &reqwest::Client, address: SocketAddr) -> reqwest::RequestBuilder {
    client
        .post(format!("http://{address}/v1/chat/completions"))
        .header("authorization", format!("Bearer {CLIENT_KEY}"))
        .header("content-type", "application/json")
        .body(r#"{"model":"model"}"#)
}

#[derive(Clone)]
struct HeaderHangState {
    attempts: Arc<AtomicUsize>,
    accepted: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn hang_before_headers(State(state): State<HeaderHangState>) -> Response {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    if let Some(sender) = state.accepted.lock().unwrap().take() {
        let _ = sender.send(());
    }
    pending().await
}

#[tokio::test]
async fn header_timeout_makes_one_attempt_and_remains_neutral_for_ordinary_channels() {
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let attempts = Arc::new(AtomicUsize::new(0));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hang_before_headers))
            .with_state(HeaderHangState {
                attempts: Arc::clone(&attempts),
                accepted: Arc::new(Mutex::new(Some(accepted_tx))),
            }),
    )
    .await;
    let fixture = proxy_fixture(
        &[format!("http://{}", upstream.address)],
        &[0],
        &[0],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy {
            connection_failure_threshold: 1,
            cooldown: Duration::from_secs(30),
        }),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    timeout(WAIT, accepted_rx).await.unwrap().unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn header_timeout_retries_the_lower_priority_channel() {
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let preferred_attempts = Arc::new(AtomicUsize::new(0));
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let preferred = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hang_before_headers))
            .with_state(HeaderHangState {
                attempts: Arc::clone(&preferred_attempts),
                accepted: Arc::new(Mutex::new(Some(accepted_tx))),
            }),
    )
    .await;
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture(
        &[
            format!("http://{}", preferred.address),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[0, 1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    timeout(WAIT, accepted_rx).await.unwrap().unwrap();
    assert_eq!(preferred_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn disabled_retry_returns_the_first_header_timeout() {
    let preferred_attempts = Arc::new(AtomicUsize::new(0));
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let preferred = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hang_before_headers))
            .with_state(HeaderHangState {
                attempts: Arc::clone(&preferred_attempts),
                accepted: Arc::new(Mutex::new(None)),
            }),
    )
    .await;
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture_with_retry(
        &[
            format!("http://{}", preferred.address),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[0, 1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        RequestRetrySettings::new(false, 1),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(preferred_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn connection_failure_retries_the_lower_priority_channel() {
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable_address = unused.local_addr().unwrap();
    drop(unused);
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture(
        &[
            format!("http://{unavailable_address}"),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[0, 1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn successful_failover_logs_one_terminal_event_for_the_final_channel() {
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable_address = unused.local_addr().unwrap();
    drop(unused);
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(fallback_attempts),
    )
    .await;
    let fixture = proxy_fixture(
        &[
            format!("http://{unavailable_address}"),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[0, 1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let final_channel_id = fixture.channel_ids[1];
    let logs = fixture.logs.clone();
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();

    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].channel_id, Some(final_channel_id));
    assert_eq!(
        events[0].outcome,
        ai_gateway::domain::RequestLogOutcome::Succeeded
    );
}

#[tokio::test]
async fn connect_timeout_retries_the_lower_priority_channel() {
    let hanging_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hanging_address = hanging_listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let hanging_task = tokio::spawn(async move {
        let (socket, _) = hanging_listener.accept().await.unwrap();
        let _ = accepted_tx.send(());
        let _socket = socket;
        pending::<()>().await;
    });
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture(
        &[
            format!("https://{hanging_address}"),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[0, 1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    timeout(WAIT, accepted_rx).await.unwrap().unwrap();
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 1);
    hanging_task.abort();
}

#[tokio::test]
async fn max_retries_excludes_the_initial_channel() {
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    drop(first);
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_address = second.local_addr().unwrap();
    drop(second);
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture_with_retry(
        &[
            format!("http://{first_address}"),
            format!("http://{second_address}"),
            format!("http://{}", fallback.address),
        ],
        &[0, 1, 2],
        &[0, 1, 2],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        RequestRetrySettings::new(true, 1),
    );
    let gateway = start_server(http::router(fixture.service)).await;

    let response = timeout(WAIT, request(&client(), gateway.address).send())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn connection_failures_trip_breaker_without_a_third_upstream_contact() {
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = unused.local_addr().unwrap();
    drop(unused);
    let fixture = proxy_fixture(
        &[format!("http://{address}")],
        &[0],
        &[0],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy {
            connection_failure_threshold: 2,
            cooldown: Duration::from_secs(30),
        }),
    );
    let gateway = start_server(http::router(fixture.service)).await;
    let client = client();

    for _ in 0..2 {
        let response = timeout(WAIT, request(&client, gateway.address).send())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
    let listener = TcpListener::bind(address).await.unwrap();
    let response = request(&client, gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err()
    );
}

#[derive(Clone)]
struct IdleBodyState {
    attempts: Arc<AtomicUsize>,
    body_dropped: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

struct DropSignal(Option<oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

async fn headers_then_idle(State(state): State<IdleBodyState>) -> Response {
    state.attempts.fetch_add(1, Ordering::SeqCst);
    let signal = DropSignal(state.body_dropped.lock().unwrap().take());
    let first = stream::once(async { Ok::<Bytes, Infallible>(Bytes::from_static(b"first")) });
    let rest = stream::unfold(signal, |signal| async move {
        pending::<()>().await;
        drop(signal);
        None::<(Result<Bytes, Infallible>, DropSignal)>
    });
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from_stream(first.chain(rest)))
        .unwrap()
}

#[tokio::test]
async fn headers_then_idle_body_does_not_trip_health_and_releases_in_flight() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let (dropped_tx, dropped_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(headers_then_idle))
            .with_state(IdleBodyState {
                attempts: Arc::clone(&attempts),
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let routing = RoutingRuntime::new(PassiveHealthPolicy {
        connection_failure_threshold: 1,
        cooldown: Duration::from_secs(30),
    });
    let fixture = proxy_fixture(
        &[format!("http://{}", upstream.address)],
        &[0],
        &[0],
        upstream_config(2, 1),
        routing.clone(),
    );
    let channel = fixture
        .runtime
        .snapshot()
        .channel(fixture.channel_ids[0])
        .unwrap();
    let gateway = start_server(http::router(fixture.service)).await;
    let mut response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(
        response.chunk().await.unwrap().unwrap(),
        Bytes::from_static(b"first")
    );
    let _ = timeout(WAIT, response.chunk()).await.unwrap();
    timeout(WAIT, dropped_rx).await.unwrap().unwrap();
    assert_eq!(routing.health(&channel).in_flight, 0);

    let response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn client_cancellation_releases_the_channel_lease() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let (dropped_tx, dropped_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(headers_then_idle))
            .with_state(IdleBodyState {
                attempts,
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let routing = RoutingRuntime::new(PassiveHealthPolicy::default());
    let fixture = proxy_fixture(
        &[format!("http://{}", upstream.address)],
        &[0],
        &[0],
        upstream_config(2, 5),
        routing.clone(),
    );
    let channel = fixture
        .runtime
        .snapshot()
        .channel(fixture.channel_ids[0])
        .unwrap();
    let gateway = start_server(http::router(fixture.service)).await;

    let mut response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(
        response.chunk().await.unwrap().unwrap(),
        Bytes::from_static(b"first")
    );
    drop(response);
    timeout(WAIT, dropped_rx).await.unwrap().unwrap();
    assert_eq!(routing.health(&channel).in_flight, 0);
    assert_eq!(routing.health(&channel).consecutive_connection_failures, 0);
}

async fn count_ok(State(attempts): State<Arc<AtomicUsize>>) -> Response {
    attempts.fetch_add(1, Ordering::SeqCst);
    Response::new(Body::from("ok"))
}

#[tokio::test]
async fn authorization_falls_back_to_a_permitted_lower_priority_channel() {
    let preferred_attempts = Arc::new(AtomicUsize::new(0));
    let fallback_attempts = Arc::new(AtomicUsize::new(0));
    let preferred = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&preferred_attempts)),
    )
    .await;
    let fallback = start_server(
        Router::new()
            .route("/v1/chat/completions", post(count_ok))
            .with_state(Arc::clone(&fallback_attempts)),
    )
    .await;
    let fixture = proxy_fixture(
        &[
            format!("http://{}", preferred.address),
            format!("http://{}", fallback.address),
        ],
        &[0, 1],
        &[1],
        upstream_config(2, 2),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
    );
    assert_eq!(fixture.group_ids.len(), 2);
    let gateway = start_server(http::router(fixture.service)).await;

    let response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(preferred_attempts.load(Ordering::SeqCst), 0);
    assert_eq!(fallback_attempts.load(Ordering::SeqCst), 1);
}
