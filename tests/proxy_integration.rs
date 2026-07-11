use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    application::ProxyService,
    http,
    runtime_config::{AppConfig, GatewayConfig},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use serde_json::Value;
use tokio::{net::TcpListener, task::JoinHandle};

const CLIENT_KEY: &str = "client-key";
const CHAT_ONLY_KEY: &str = "chat-only-key";
const UPSTREAM_KEY: &str = "upstream-key";

#[derive(Clone)]
struct MockUpstream {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    status: StatusCode,
    body: Vec<u8>,
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    headers: HeaderMap,
    body: Vec<u8>,
}

async fn capture_upstream(State(upstream): State<MockUpstream>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    upstream.requests.lock().unwrap().push(CapturedRequest {
        headers: parts.headers,
        body,
    });

    Response::builder()
        .status(upstream.status)
        .header("x-upstream", "mock")
        .body(Body::from(upstream.body))
        .unwrap()
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

struct Harness {
    gateway: TestServer,
    _upstream: TestServer,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
}

impl Harness {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.gateway.address, path)
    }

    fn upstream_requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

async fn harness(status: StatusCode, body: impl Into<Vec<u8>>) -> Harness {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .route("/v1/responses", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status,
                body: body.into(),
            }),
    )
    .await;
    let config = compile_config(&format!("http://{}", upstream.address));
    let proxy = proxy_service(config);
    let gateway = start_server(http::router(proxy)).await;

    Harness {
        gateway,
        _upstream: upstream,
        requests,
    }
}

fn compile_config(upstream_url: &str) -> GatewayConfig {
    toml::from_str::<AppConfig>(&format!(
        r#"
[server]
host = "127.0.0.1"
port = 0
max_request_body_bytes = 1048576

[database]
url = "postgres://gateway:gateway@127.0.0.1/gateway"
max_connections = 1
connect_timeout_seconds = 1

[upstream]
connect_timeout_seconds = 1
response_header_timeout_seconds = 1
stream_idle_timeout_seconds = 1

[runtime_config]
reload_interval_seconds = 60

[observability]
filter = "off"

[[api_keys]]
id = "full-access"
key = "{CLIENT_KEY}"
allowed_api_formats = ["open_ai_chat_completions", "open_ai_responses"]
permissions = ["proxy", "models.read"]

[[api_keys]]
id = "chat-only"
key = "{CHAT_ONLY_KEY}"
allowed_api_formats = ["open_ai_chat_completions"]
permissions = ["proxy", "models.read"]

[[channels]]
id = "chat"
api_format = "open_ai_chat_completions"
base_url = "{upstream_url}"
upstream_bearer_token = "{UPSTREAM_KEY}"

[[channels]]
id = "responses"
api_format = "open_ai_responses"
base_url = "{upstream_url}"
upstream_bearer_token = "{UPSTREAM_KEY}"

[[model_rules]]
client_model = "same-model"
api_format = "open_ai_chat_completions"
upstream_model = "same-model"
channel_id = "chat"

[[model_rules]]
client_model = "alias-model"
api_format = "open_ai_chat_completions"
upstream_model = "upstream-alias-model"
channel_id = "chat"

[[model_rules]]
client_model = "chat-only-model"
api_format = "open_ai_chat_completions"
upstream_model = "chat-only-model"
channel_id = "chat"

[[model_rules]]
client_model = "responses-model"
api_format = "open_ai_responses"
upstream_model = "responses-model"
channel_id = "responses"
"#
    ))
    .unwrap()
    .compile()
    .unwrap()
}

fn proxy_service(config: GatewayConfig) -> ProxyService {
    let max_request_body_bytes = config.server.max_request_body_bytes;
    let upstream = config.upstream.clone();
    ProxyService::new(Arc::new(config.runtime), max_request_body_bytes, &upstream).unwrap()
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
}

fn authorized_post(
    client: &reqwest::Client,
    url: String,
    key: &str,
    body: impl Into<Vec<u8>>,
) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(body.into())
}

#[tokio::test]
async fn missing_bearer_key_returns_unauthorized_without_upstream_contact() {
    let harness = harness(StatusCode::OK, br#"{"unexpected":true}"#.to_vec()).await;

    let response = client()
        .post(harness.url("/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(r#"{"model":"same-model"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn unknown_model_returns_not_found_without_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"unknown-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn api_key_and_model_rules_do_not_fall_back_between_formats() {
    let harness = harness(StatusCode::OK, Vec::new()).await;
    let client = client();

    let forbidden = authorized_post(
        &client,
        harness.url("/v1/responses"),
        CHAT_ONLY_KEY,
        br#"{"model":"chat-only-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let missing_route = authorized_post(
        &client,
        harness.url("/v1/responses"),
        CLIENT_KEY,
        br#"{"model":"chat-only-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(missing_route.status(), StatusCode::NOT_FOUND);
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn matching_chat_model_preserves_body_and_forwards_response_safely() {
    let upstream_body = br#"{"id":"upstream-result","ok":true}"#.to_vec();
    let harness = harness(StatusCode::CREATED, upstream_body.clone()).await;
    let request_body = br#"{ "z": [3, 2], "model" : "same-model", "nested": { "a": 1 } }"#.to_vec();

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        request_body.clone(),
    )
    .header("connection", "x-internal-hop, keep-alive")
    .header("x-internal-hop", "do-not-forward")
    .header("x-request-id", "forward-me")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);

    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.body, request_body);
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer upstream-key"
    );
    assert!(request.headers.get("connection").is_none());
    assert!(request.headers.get("x-internal-hop").is_none());
    assert_eq!(request.headers.get("x-request-id").unwrap(), "forward-me");
}

#[tokio::test]
async fn alias_rewrites_only_the_top_level_model() {
    let harness = harness(StatusCode::OK, br#"{"ok":true}"#.to_vec()).await;
    let request_body = br#"{"model":"alias-model","nested":{"model":"unchanged"},"items":[{"model":"also-unchanged"}]}"#;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        request_body.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    let forwarded: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(forwarded["model"], "upstream-alias-model");
    assert_eq!(forwarded["nested"]["model"], "unchanged");
    assert_eq!(forwarded["items"][0]["model"], "also-unchanged");
}

#[tokio::test]
async fn models_endpoint_filters_by_api_format_and_requires_authentication() {
    let harness = harness(StatusCode::OK, Vec::new()).await;
    let client = client();

    let response = client
        .get(harness.url("/v1/models"))
        .header("authorization", format!("Bearer {CHAT_ONLY_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    let models = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        models,
        BTreeSet::from([
            "alias-model".to_owned(),
            "chat-only-model".to_owned(),
            "same-model".to_owned(),
        ])
    );
    assert!(!models.contains("responses-model"));

    let unauthenticated = client.get(harness.url("/v1/models")).send().await.unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauthenticated.headers().get("www-authenticate").unwrap(),
        "Bearer"
    );
}
