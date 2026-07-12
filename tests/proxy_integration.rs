use std::{
    collections::BTreeSet,
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
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
};
use serde_json::Value;
use tokio::{net::TcpListener, task::JoinHandle};
use uuid::Uuid;

const CLIENT_KEY: &str = "client-key";
const CHAT_ONLY_KEY: &str = "chat-only-key";
const MODELS_READ_ONLY_KEY: &str = "models-read-only-key";
const NO_REACHABLE_MODELS_KEY: &str = "no-reachable-models-key";
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
    let proxy = proxy_service(&format!("http://{}", upstream.address));
    let gateway = start_server(http::router(proxy)).await;

    Harness {
        gateway,
        _upstream: upstream,
        requests,
    }
}

fn proxy_service(upstream_url: &str) -> ProxyService {
    let chat_group = Uuid::new_v4();
    let responses_group = Uuid::new_v4();
    let empty_chat_group = Uuid::new_v4();
    let chat = Uuid::new_v4();
    let responses = Uuid::new_v4();
    let group = |id: Uuid, api_format: &str| ChannelGroupRecord {
        id,
        name: id.to_string(),
        api_format: api_format.to_owned(),
        priority: 0,
        selection_strategy: "weighted_random".into(),
        enabled: true,
    };
    let channel = |id: Uuid, group_id: Uuid, api_format: &str| ChannelRecord {
        id,
        channel_group_id: group_id,
        api_format: api_format.into(),
        name: id.to_string(),
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
        upstream_api_key: Some(UPSTREAM_KEY.into()),
        available_models: match api_format {
            "open_ai_chat_completions" => vec![
                "same-model".into(),
                "upstream-alias-model".into(),
                "chat-only-model".into(),
            ],
            "open_ai_responses" => vec!["responses-model".into()],
            _ => vec![],
        },
        health_check: serde_json::json!({}),
    };
    let key = |secret: &str, formats: Vec<&str>, groups: Vec<Uuid>, permissions: Vec<&str>| {
        ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            secret_value: secret.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: formats.into_iter().map(str::to_owned).collect(),
            permissions: permissions.into_iter().map(str::to_owned).collect(),
            allowed_group_ids: Some(groups),
            requests_per_minute: None,
            tokens_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }
    };
    let rule = |model: &str, upstream: &str, format: &str, channel_id: Uuid| ModelRuleRecord {
        id: Uuid::new_v4(),
        client_model: model.into(),
        api_format: format.into(),
        model_id: Uuid::new_v4(),
        model_enabled: true,
        upstream_model: upstream.into(),
        channel_group_ids: vec![],
        channel_ids: vec![channel_id],
        enabled: true,
    };
    let records = ControlPlaneRecords {
        api_keys: vec![
            key(
                CLIENT_KEY,
                vec!["open_ai_chat_completions", "open_ai_responses"],
                vec![chat_group, responses_group],
                vec!["proxy", "models.read"],
            ),
            key(
                CHAT_ONLY_KEY,
                vec!["open_ai_chat_completions"],
                vec![chat_group],
                vec!["proxy", "models.read"],
            ),
            key(
                MODELS_READ_ONLY_KEY,
                vec!["open_ai_chat_completions"],
                vec![chat_group],
                vec!["models.read"],
            ),
            key(
                NO_REACHABLE_MODELS_KEY,
                vec!["open_ai_chat_completions"],
                vec![empty_chat_group],
                vec!["proxy", "models.read"],
            ),
        ],
        groups: vec![
            group(chat_group, "open_ai_chat_completions"),
            group(responses_group, "open_ai_responses"),
            group(empty_chat_group, "open_ai_chat_completions"),
        ],
        channels: vec![
            channel(chat, chat_group, "open_ai_chat_completions"),
            channel(responses, responses_group, "open_ai_responses"),
        ],
        model_rules: vec![
            rule("same-model", "same-model", "open_ai_chat_completions", chat),
            rule(
                "alias-model",
                "upstream-alias-model",
                "open_ai_chat_completions",
                chat,
            ),
            rule(
                "chat-only-model",
                "chat-only-model",
                "open_ai_chat_completions",
                chat,
            ),
            rule(
                "responses-model",
                "responses-model",
                "open_ai_responses",
                responses,
            ),
        ],
    };
    let runtime = Arc::new(RuntimeConfig::new(compile_control_plane(records).unwrap()));
    ProxyService::new(
        runtime,
        1_048_576,
        &UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    )
    .unwrap()
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
async fn health_is_available_without_authentication() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = client().get(harness.url("/health")).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(harness.upstream_requests().is_empty());
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

#[tokio::test]
async fn models_endpoint_requires_proxy_and_models_read_permissions() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = client()
        .get(harness.url("/v1/models"))
        .header("authorization", format!("Bearer {MODELS_READ_ONLY_KEY}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn models_endpoint_returns_an_empty_list_when_authorized_key_has_no_reachable_models() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = client()
        .get(harness.url("/v1/models"))
        .header("authorization", format!("Bearer {NO_REACHABLE_MODELS_KEY}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["data"], serde_json::json!([]));
}
