use std::{
    convert::Infallible,
    future::pending,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{PassiveHealthSettings, SystemRuntimeSettings, UpstreamTimeoutDefaults},
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane_with_system_settings},
};
use async_compression::tokio::write::GzipEncoder;
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::post,
};
use futures_util::{StreamExt, stream};
use serde_json::Value;
use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};
use uuid::Uuid;

const CLIENT_KEY: &str = "client-key";
const OUTER_TIMEOUT: Duration = Duration::from_secs(4);
const CANCELLATION_TIMEOUT: Duration = Duration::from_secs(2);

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

fn proxy_service(
    upstream_url: &str,
    response_header_timeout: u64,
    stream_idle_timeout: u64,
) -> ProxyService {
    proxy_service_with_documents(
        upstream_url,
        response_header_timeout,
        stream_idle_timeout,
        None,
        serde_json::json!({}),
        RecordingRequestLogSink::default(),
    )
}

fn proxy_service_with_documents(
    upstream_url: &str,
    response_header_timeout: u64,
    stream_idle_timeout: u64,
    template: Option<Value>,
    override_document: Value,
    logs: RecordingRequestLogSink,
) -> ProxyService {
    proxy_service_with_network_policy(
        upstream_url,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: response_header_timeout,
            images_response_header_timeout_seconds: response_header_timeout,
            stream_idle_timeout_seconds: stream_idle_timeout,
        },
        (None, None, None),
        template,
        override_document,
        logs,
    )
}

fn proxy_service_with_network_policy(
    upstream_url: &str,
    upstream_config: UpstreamConfig,
    channel_timeouts: (Option<i32>, Option<i32>, Option<i32>),
    template: Option<Value>,
    override_document: Value,
    logs: RecordingRequestLogSink,
) -> ProxyService {
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let template_id = template.as_ref().map(|_| Uuid::new_v4());
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            user_websocket_enabled: false,
            secret_value: CLIENT_KEY.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec!["open_ai_chat_completions".into()],
            permissions: vec!["proxy".into()],
            allowed_group_ids: vec![group_id],
            allowed_channel_ids: vec![],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }],
        groups: vec![ChannelGroupRecord {
            id: group_id,
            name: "chat".into(),
            api_format: "open_ai_chat_completions".into(),
            connector_kind: "openai_compatible".into(),
            priority: 0,
            selection_strategy: "weighted_random".into(),
            enabled: true,
        }],
        channels: vec![ChannelRecord {
            id: channel_id,
            channel_group_id: group_id,
            api_format: "open_ai_chat_completions".into(),
            name: "chat".into(),
            base_url: upstream_url.into(),
            enabled: true,
            supports_websocket: false,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: rust_decimal::Decimal::ONE,
            proxy_id: None,
            config_template_id: template_id,
            override_document,
            connect_timeout_ms: channel_timeouts.0,
            response_header_timeout_ms: channel_timeouts.1,
            stream_idle_timeout_ms: channel_timeouts.2,
            upstream_auth_kind: "bearer".into(),
            upstream_auth_header_name: None,
            upstream_api_key: Some("upstream-key".into()),
            available_models: vec!["stream-model".into()],
            test_model: None,
        }],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: "stream-model".into(),
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
            advanced_billing: serde_json::json!({
                "long_context_tiers": [],
                "request_multipliers": [],
            }),
            upstream_model: "stream-model".into(),
            channel_group_ids: vec![],
            channel_ids: vec![channel_id],
            enabled: true,
        }],
        proxies: vec![],
        templates: template_id
            .zip(template)
            .map(|(id, document)| {
                vec![ConfigTemplateRecord {
                    id,
                    name: "stream-transform-template".into(),
                    description: None,
                    document,
                    enabled: true,
                }]
            })
            .unwrap_or_default(),
    };
    ProxyService::with_log_sink(
        Arc::new(RuntimeConfig::new(
            compile_control_plane_with_system_settings(
                records,
                SystemRuntimeSettings::new(
                    UpstreamTimeoutDefaults::new(
                        Duration::from_secs(upstream_config.connect_timeout_seconds),
                        Duration::from_secs(upstream_config.response_header_timeout_seconds),
                        Duration::from_secs(upstream_config.stream_idle_timeout_seconds),
                    ),
                    PassiveHealthSettings::default(),
                ),
            )
            .unwrap(),
        )),
        1_048_576,
        Arc::new(logs),
    )
    .unwrap()
}

fn active_chat_sse_document(patch: Value) -> Value {
    serde_json::json!({
        "version": 1,
        "api_format": "open_ai_chat_completions",
        "sse": [{"event": "chat.completion.chunk", "json": patch}]
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().build().unwrap()
}

fn request(client: &reqwest::Client, gateway: SocketAddr) -> reqwest::RequestBuilder {
    client
        .post(format!("http://{gateway}/v1/chat/completions"))
        .header("authorization", format!("Bearer {CLIENT_KEY}"))
        .header("content-type", "application/json")
        .body(r#"{"model":"stream-model","stream":true}"#)
}

fn sse_response(body: Body) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(body)
        .unwrap()
}

fn sse_response_with_headers(body: Body, headers: &[(&str, &str)]) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(body).unwrap()
}

struct DropSignal(Option<oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

fn take_signal(signal: &Arc<Mutex<Option<oneshot::Sender<()>>>>) -> DropSignal {
    DropSignal(signal.lock().unwrap().take())
}

#[derive(Clone)]
struct HeaderTimeoutState {
    accepted: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    cancelled: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn never_responds(State(state): State<HeaderTimeoutState>) -> Response {
    if let Some(sender) = state.accepted.lock().unwrap().take() {
        let _ = sender.send(());
    }
    let cancelled = take_signal(&state.cancelled);
    let response = pending::<Response>().await;
    drop(cancelled);
    response
}

#[tokio::test]
async fn response_header_timeout_returns_openai_error_and_cancels_upstream_request() {
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let (cancelled_tx, cancelled_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(never_responds))
            .with_state(HeaderTimeoutState {
                accepted: Arc::new(Mutex::new(Some(accepted_tx))),
                cancelled: Arc::new(Mutex::new(Some(cancelled_tx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service(
        &format!("http://{}", upstream.address),
        2,
        5,
    )))
    .await;

    let response = timeout(OUTER_TIMEOUT, request(&client(), gateway.address).send())
        .await
        .expect("gateway did not enforce the response-header timeout")
        .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let error_bytes = timeout(OUTER_TIMEOUT, response.bytes())
        .await
        .expect("gateway error response body did not finish")
        .unwrap();
    let body: Value = serde_json::from_slice(&error_bytes).unwrap();
    assert_eq!(body["error"]["code"], "response_header_timeout");
    timeout(OUTER_TIMEOUT, accepted_rx)
        .await
        .expect("upstream never received the routed request")
        .unwrap();
    timeout(CANCELLATION_TIMEOUT, cancelled_rx)
        .await
        .expect("header-timeout cancellation did not drop the upstream request")
        .unwrap();
}

#[tokio::test]
async fn channel_response_header_timeout_overrides_the_longer_toml_default() {
    let (accepted_tx, accepted_rx) = oneshot::channel();
    let (cancelled_tx, cancelled_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(never_responds))
            .with_state(HeaderTimeoutState {
                accepted: Arc::new(Mutex::new(Some(accepted_tx))),
                cancelled: Arc::new(Mutex::new(Some(cancelled_tx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_network_policy(
        &format!("http://{}", upstream.address),
        UpstreamConfig {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 5,
            images_response_header_timeout_seconds: 5,
            stream_idle_timeout_seconds: 5,
        },
        (Some(50), Some(100), None),
        None,
        serde_json::json!({}),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let response = timeout(
        Duration::from_secs(1),
        request(&client(), gateway.address).send(),
    )
    .await
    .expect("channel response-header override was ignored")
    .unwrap();
    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "response_header_timeout");
    timeout(OUTER_TIMEOUT, accepted_rx).await.unwrap().unwrap();
    timeout(CANCELLATION_TIMEOUT, cancelled_rx)
        .await
        .expect("channel response-header timeout did not cancel upstream")
        .unwrap();
}

#[derive(Clone)]
struct TwoChunkState {
    release_second: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

async fn two_chunk_sse(State(state): State<TwoChunkState>) -> Response {
    let release_second = state.release_second.lock().unwrap().take().unwrap();
    let first = stream::once(async {
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\"}\n\n",
        ))
    });
    let second = stream::once(async move {
        release_second.await.unwrap();
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\"}\n\n",
        ))
    });
    sse_response(Body::from_stream(first.chain(second)))
}

#[tokio::test]
async fn sse_first_chunk_is_forwarded_without_waiting_for_the_second_chunk() {
    let (release_second_tx, release_second_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(two_chunk_sse))
            .with_state(TwoChunkState {
                release_second: Arc::new(Mutex::new(Some(release_second_rx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let mut response = timeout(OUTER_TIMEOUT, request(&client(), gateway.address).send())
        .await
        .expect("gateway did not return SSE response headers")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        timeout(CANCELLATION_TIMEOUT, response.chunk())
            .await
            .expect("first SSE chunk was buffered")
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n")
    );

    release_second_tx.send(()).unwrap();
    assert_eq!(
        timeout(OUTER_TIMEOUT, response.chunk())
            .await
            .expect("second SSE chunk was not forwarded")
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n")
    );
}

#[derive(Clone)]
struct HangingStreamState {
    body_dropped: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn hanging_sse(State(state): State<HangingStreamState>) -> Response {
    let first = stream::once(async {
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\"}\n\n",
        ))
    });
    let wait_forever = stream::unfold(take_signal(&state.body_dropped), |guard| async move {
        pending::<()>().await;
        drop(guard);
        None::<(Result<Bytes, Infallible>, DropSignal)>
    });
    sse_response(Body::from_stream(first.chain(wait_forever)))
}

#[tokio::test]
async fn idle_upstream_stream_keeps_status_then_terminates_and_releases_body() {
    let (dropped_tx, dropped_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hanging_sse))
            .with_state(HangingStreamState {
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        1,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let mut response = timeout(OUTER_TIMEOUT, request(&client(), gateway.address).send())
        .await
        .expect("gateway did not return SSE response headers")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        timeout(OUTER_TIMEOUT, response.chunk())
            .await
            .expect("gateway did not forward the first SSE chunk")
            .unwrap()
            .unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n".as_slice()
    );
    match timeout(OUTER_TIMEOUT, response.chunk())
        .await
        .expect("gateway did not enforce stream idle timeout")
    {
        Ok(None) | Err(_) => {}
        Ok(Some(chunk)) => panic!("unexpected chunk after idle timeout: {chunk:?}"),
    }
    timeout(CANCELLATION_TIMEOUT, dropped_rx)
        .await
        .expect("idle timeout did not release the upstream response body")
        .unwrap();
}

#[tokio::test]
async fn channel_stream_idle_timeout_overrides_the_longer_toml_default() {
    let (dropped_tx, dropped_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hanging_sse))
            .with_state(HangingStreamState {
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_network_policy(
        &format!("http://{}", upstream.address),
        UpstreamConfig {
            connect_timeout_seconds: 5,
            response_header_timeout_seconds: 6,
            images_response_header_timeout_seconds: 6,
            stream_idle_timeout_seconds: 5,
        },
        (None, None, Some(100)),
        None,
        serde_json::json!({}),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let mut response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.chunk().await.unwrap().is_some());
    assert!(matches!(
        timeout(Duration::from_secs(1), response.chunk())
            .await
            .expect("channel stream-idle override was ignored"),
        Ok(None) | Err(_)
    ));
    timeout(CANCELLATION_TIMEOUT, dropped_rx)
        .await
        .expect("channel stream-idle timeout did not release upstream")
        .unwrap();
}

#[tokio::test]
async fn client_disconnect_drops_upstream_response_body_without_background_pump() {
    let (dropped_tx, mut dropped_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hanging_sse))
            .with_state(HangingStreamState {
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let mut response = timeout(OUTER_TIMEOUT, request(&client(), gateway.address).send())
        .await
        .expect("gateway did not return SSE response headers")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        timeout(OUTER_TIMEOUT, response.chunk())
            .await
            .expect("gateway did not forward the first SSE chunk")
            .unwrap()
            .unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n".as_slice()
    );
    assert!(
        timeout(Duration::from_millis(100), &mut dropped_rx)
            .await
            .is_err(),
        "upstream body dropped before the downstream client disconnected"
    );
    drop(response);

    timeout(CANCELLATION_TIMEOUT, dropped_rx)
        .await
        .expect("dropping the client response did not release the upstream body")
        .unwrap();
}

async fn split_crlf_sse() -> Response {
    let chunks = stream::iter([
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\",\"meta\":{}}\r",
        )),
        Ok(Bytes::from_static(b"\n\r")),
        Ok(Bytes::from_static(b"\n")),
    ]);
    sse_response(Body::from_stream(chunks))
}

#[tokio::test]
async fn active_sse_transform_handles_a_frame_split_between_cr_and_lf() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(split_crlf_sse))).await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/meta/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"data: {\"meta\":{\"patched\":true},\"object\":\"chat.completion.chunk\"}\r\n\r\n"
            .as_slice()
    );
}

async fn decorated_sse() -> Response {
    let body = b"data: {\"object\":\"chat.completion.chunk\"}\n\n";
    sse_response_with_headers(
        Body::from(body.as_slice()),
        &[
            ("content-length", "42"),
            ("etag", "upstream-etag"),
            ("last-modified", "Wed, 21 Oct 2015 07:28:00 GMT"),
            ("content-md5", "upstream-md5"),
            ("digest", "sha-256=upstream"),
        ],
    )
}

#[tokio::test]
async fn transformed_sse_response_applies_layered_headers_and_removes_entity_metadata() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(decorated_sse))).await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        Some(serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "response_headers": {"set": {"x-layer": "template"}}
        })),
        serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "response_headers": {"set": {"x-layer": "channel"}},
            "sse": [{
                "event": "chat.completion.chunk",
                "json": [{"op": "add", "path": "/patched", "value": true}]
            }]
        }),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-layer").unwrap(), "channel");
    for name in [
        "content-length",
        "etag",
        "last-modified",
        "content-md5",
        "digest",
    ] {
        assert!(
            response.headers().get(name).is_none(),
            "stale {name} leaked"
        );
    }
    assert_eq!(
        response.bytes().await.unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n".as_slice()
    );
}

async fn ordinary_json_response() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("etag", "ordinary-etag")
        .body(Body::from(Bytes::from_static(b"{ \"upstream\" : true }")))
        .unwrap()
}

async fn unusual_sse_response() -> Response {
    sse_response_with_headers(
        Body::from(Bytes::from_static(b": note\r\ndata: [DONE]\r\n\r\n")),
        &[("etag", "transparent-etag")],
    )
}

async fn chat_usage_sse() -> Response {
    sse_response(Body::from(Bytes::from_static(
        br#"data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"pong"},"finish_reason":null}],"usage":null}

data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":null}

data: {"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":4,"total_tokens":15,"prompt_tokens_details":{"cached_tokens":2},"completion_tokens_details":{"reasoning_tokens":1}}}

data: [DONE]

"#,
    )))
}

async fn deepseek_chat_usage_sse_with_unterminated_final_frame() -> Response {
    sse_response(Body::from(Bytes::from_static(
        br#"data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":null,"reasoning_content":"The answer is pong."},"finish_reason":null}]}

data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"pong","reasoning_content":null},"finish_reason":null}]}

data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"","reasoning_content":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":45,"total_tokens":56,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":42},"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":11}}"#,
    )))
}

#[tokio::test]
async fn chat_sse_usage_is_collected_without_changing_forwarded_bytes() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(chat_usage_sse))).await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        serde_json::json!({}),
        logs.clone(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.bytes().await.unwrap();
    assert!(
        body.windows(b"\"prompt_tokens\":11".len())
            .any(|window| { window == b"\"prompt_tokens\":11" })
    );
    let events = logs.events();
    assert_eq!(events.len(), 1);
    let billing = events[0].billing.as_ref().unwrap();
    assert_eq!(
        billing.usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 11,
            cached_input_tokens: 2,
            cache_write_tokens: 0,
            output_tokens: 4,
            reasoning_tokens: 1,
        })
    );
    assert_eq!(billing.price.currency, "USD");
}

#[tokio::test]
async fn deepseek_chat_sse_usage_in_unterminated_final_frame_is_collected() {
    let upstream = start_server(Router::new().route(
        "/v1/chat/completions",
        post(deepseek_chat_usage_sse_with_unterminated_final_frame),
    ))
    .await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        serde_json::json!({}),
        logs.clone(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .bytes()
            .await
            .unwrap()
            .windows(b"\"prompt_tokens\":11".len())
            .any(|window| window == b"\"prompt_tokens\":11")
    );

    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 11,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 45,
            reasoning_tokens: 42,
        })
    );
}

#[tokio::test]
async fn no_sse_plan_keeps_sse_bytes_and_entity_headers_transparent() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(unusual_sse_response))).await;
    let gateway = start_server(http::router(proxy_service(
        &format!("http://{}", upstream.address),
        5,
        5,
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.headers().get("etag").unwrap(), "transparent-etag");
    assert_eq!(
        response.bytes().await.unwrap(),
        b": note\r\ndata: [DONE]\r\n\r\n".as_slice()
    );
}

#[tokio::test]
async fn active_plan_leaves_non_sse_response_bytes_and_entity_headers_transparent() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(ordinary_json_response)))
            .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.headers().get("etag").unwrap(), "ordinary-etag");
    assert_eq!(
        response.bytes().await.unwrap(),
        b"{ \"upstream\" : true }".as_slice()
    );
}

#[derive(Clone)]
struct AcceptEncodingState(Arc<Mutex<Option<String>>>);

async fn capture_accept_encoding(
    State(state): State<AcceptEncodingState>,
    request: Request,
) -> Response {
    *state.0.lock().unwrap() = request
        .headers()
        .get("accept-encoding")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    sse_response(Body::from(Bytes::from_static(
        b"data: {\"object\":\"chat.completion.chunk\"}\n\n",
    )))
}

#[tokio::test]
async fn active_sse_plan_uses_gateway_controlled_upstream_encoding() {
    let captured = Arc::new(Mutex::new(None));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_accept_encoding))
            .with_state(AcceptEncodingState(Arc::clone(&captured))),
    )
    .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n".as_slice()
    );
    assert_eq!(
        captured.lock().unwrap().as_deref(),
        Some("gzip, deflate, br, zstd")
    );
}

async fn compressed_sse() -> Response {
    let mut encoder = GzipEncoder::new(Vec::new());
    encoder
        .write_all(b"data: {\"object\":\"chat.completion.chunk\"}\n\n")
        .await
        .unwrap();
    encoder.shutdown().await.unwrap();
    sse_response_with_headers(
        Body::from(encoder.into_inner()),
        &[("content-encoding", "gzip")],
    )
}

#[tokio::test]
async fn compressed_sse_with_an_active_plan_is_decoded_and_transformed() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(compressed_sse))).await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        logs.clone(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n".as_slice()
    );
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_str(), "succeeded");
}

async fn response_with_dynamic_hop_header() -> Response {
    Response::builder()
        .status(StatusCode::IM_A_TEAPOT)
        .header("content-type", "text/event-stream")
        .header("connection", "x-upstream-hop")
        .header("x-upstream-hop", "upstream")
        .body(Body::from(Bytes::from_static(b"data: [DONE]\n\n")))
        .unwrap()
}

#[tokio::test]
async fn response_header_plan_failure_before_headers_logs_the_client_visible_status() {
    let upstream = start_server(Router::new().route(
        "/v1/chat/completions",
        post(response_with_dynamic_hop_header),
    ))
    .await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "response_headers": {"set": {"x-upstream-hop": "configured"}}
        }),
        logs.clone(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].response_status_code,
        Some(StatusCode::BAD_GATEWAY.as_u16())
    );
}

#[derive(Clone)]
struct PatchFailureState {
    release_first: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    body_dropped: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn failing_patch_sse(State(state): State<PatchFailureState>) -> Response {
    let release_first = state.release_first.lock().unwrap().take().unwrap();
    let first = stream::once(async move {
        release_first.await.unwrap();
        Ok::<Bytes, Infallible>(Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\"}\n\n",
        ))
    });
    let wait_forever = stream::unfold(take_signal(&state.body_dropped), |guard| async move {
        pending::<()>().await;
        drop(guard);
        None::<(Result<Bytes, Infallible>, DropSignal)>
    });
    sse_response(Body::from_stream(first.chain(wait_forever)))
}

#[tokio::test]
async fn event_patch_failure_after_headers_terminates_the_body_releases_upstream_and_logs_once() {
    let (dropped_tx, dropped_rx) = oneshot::channel();
    let (release_first_tx, release_first_rx) = oneshot::channel();
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(failing_patch_sse))
            .with_state(PatchFailureState {
                release_first: Arc::new(Mutex::new(Some(release_first_rx))),
                body_dropped: Arc::new(Mutex::new(Some(dropped_tx))),
            }),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "replace", "path": "/missing", "value": true}
        ])),
        logs.clone(),
    )))
    .await;

    let mut response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    release_first_tx.send(()).unwrap();
    assert!(matches!(response.chunk().await, Err(_) | Ok(None)));
    timeout(CANCELLATION_TIMEOUT, dropped_rx)
        .await
        .expect("event patch failure did not release the upstream body")
        .unwrap();
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_str(), "failed");
    assert_eq!(
        events[0].error_code.as_deref(),
        Some("response_transform_failed")
    );
    assert_eq!(
        events[0].response_status_code,
        Some(StatusCode::OK.as_u16())
    );
}

async fn done_then_failing_frame_in_one_chunk() -> Response {
    sse_response(Body::from(Bytes::from_static(
        b"data: [DONE]\n\ndata: {\"object\":\"chat.completion.chunk\"}\n\n",
    )))
}

#[tokio::test]
async fn later_same_chunk_sse_failure_preserves_earlier_unmatched_frame() {
    let upstream = start_server(Router::new().route(
        "/v1/chat/completions",
        post(done_then_failing_frame_in_one_chunk),
    ))
    .await;
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "replace", "path": "/missing", "value": true}
        ])),
        RecordingRequestLogSink::default(),
    )))
    .await;

    let mut response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.chunk().await.unwrap().unwrap(),
        Bytes::from_static(b"data: [DONE]\n\n")
    );
    assert!(matches!(response.chunk().await, Err(_) | Ok(None)));
}

async fn residual_sse() -> Response {
    sse_response(Body::from(Bytes::from_static(
        b"data: {\"object\":\"chat.completion.chunk\"}",
    )))
}

#[tokio::test]
async fn eof_residual_is_accounted_before_success_completion() {
    let upstream =
        start_server(Router::new().route("/v1/chat/completions", post(residual_sse))).await;
    let logs = RecordingRequestLogSink::default();
    let gateway = start_server(http::router(proxy_service_with_documents(
        &format!("http://{}", upstream.address),
        5,
        5,
        None,
        active_chat_sse_document(serde_json::json!([
            {"op": "add", "path": "/patched", "value": true}
        ])),
        logs.clone(),
    )))
    .await;

    let response = request(&client(), gateway.address).send().await.unwrap();

    assert_eq!(
        response.bytes().await.unwrap(),
        b"data: {\"object\":\"chat.completion.chunk\"}".as_slice()
    );
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_str(), "succeeded");
    assert!(events[0].ttft_ms.is_some());
}
