use std::{
    convert::Infallible,
    future::pending,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    application::ProxyService,
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane},
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
use serde_json::Value;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle, time::timeout};
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
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
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
            allowed_group_ids: Some(vec![group_id]),
            requests_per_minute: None,
            tokens_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }],
        groups: vec![ChannelGroupRecord {
            id: group_id,
            name: "chat".into(),
            api_format: "open_ai_chat_completions".into(),
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
            auto_disabled: false,
            weight: 1,
            proxy_id: None,
            config_template_id: None,
            override_document: serde_json::json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "bearer".into(),
            upstream_auth_header_name: None,
            upstream_api_key: Some("upstream-key".into()),
            available_models: vec!["stream-model".into()],
            health_check: serde_json::json!({}),
        }],
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: "stream-model".into(),
            api_format: "open_ai_chat_completions".into(),
            model_id: Uuid::new_v4(),
            model_enabled: true,
            upstream_model: "stream-model".into(),
            channel_group_ids: vec![],
            channel_ids: vec![channel_id],
            enabled: true,
        }],
        proxies: vec![],
        templates: vec![],
    };
    ProxyService::new(
        Arc::new(RuntimeConfig::new(compile_control_plane(records).unwrap())),
        1_048_576,
        &UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: response_header_timeout,
            stream_idle_timeout_seconds: stream_idle_timeout,
        },
    )
    .unwrap()
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

#[derive(Clone)]
struct TwoChunkState {
    release_second: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

async fn two_chunk_sse(State(state): State<TwoChunkState>) -> Response {
    let release_second = state.release_second.lock().unwrap().take().unwrap();
    let first =
        stream::once(async { Ok::<Bytes, Infallible>(Bytes::from_static(b"data: first\n\n")) });
    let second = stream::once(async move {
        release_second.await.unwrap();
        Ok::<Bytes, Infallible>(Bytes::from_static(b"data: second\n\n"))
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
    let gateway = start_server(http::router(proxy_service(
        &format!("http://{}", upstream.address),
        5,
        5,
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
        Bytes::from_static(b"data: first\n\n")
    );

    release_second_tx.send(()).unwrap();
    assert_eq!(
        timeout(OUTER_TIMEOUT, response.chunk())
            .await
            .expect("second SSE chunk was not forwarded")
            .unwrap()
            .unwrap(),
        Bytes::from_static(b"data: second\n\n")
    );
}

#[derive(Clone)]
struct HangingStreamState {
    body_dropped: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

async fn hanging_sse(State(state): State<HangingStreamState>) -> Response {
    let first =
        stream::once(async { Ok::<Bytes, Infallible>(Bytes::from_static(b"data: first\n\n")) });
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
    let gateway = start_server(http::router(proxy_service(
        &format!("http://{}", upstream.address),
        5,
        1,
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
        b"data: first\n\n".as_slice()
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
    let gateway = start_server(http::router(proxy_service(
        &format!("http://{}", upstream.address),
        5,
        5,
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
        b"data: first\n\n".as_slice()
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
