#![cfg(feature = "mcp-server")]

use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
};

use ai_gateway::{
    application::{ProxyService, RequestLogSink},
    domain::{
        ApiOperation, McpTransportSettings, RequestLogEvent, RequestLogOutcome, RequestLogSource,
        SystemRuntimeSettings,
    },
    mcp::McpService,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, McpServerRecord,
        ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, compile_control_plane_with_system_settings},
};
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::State,
    http::{
        HeaderMap, Request, StatusCode,
        header::{
            ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_REQUEST_HEADERS,
            ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION, CONTENT_TYPE, HOST, ORIGIN,
        },
    },
    response::IntoResponse,
    routing::post,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "mcp-client-key";
const NO_ROUTE_KEY: &str = "mcp-no-route-key";
const IMAGE_CLIENT_KEY: &str = "mcp-image-client-key";
const IMAGE_NO_ROUTE_KEY: &str = "mcp-image-no-route-key";
const MCP_VERSION: &str = "2026-07-28";
const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

#[derive(Clone, Default)]
struct RecordingRequestLogSink {
    events: Arc<Mutex<Vec<RequestLogEvent>>>,
}

impl RequestLogSink for RecordingRequestLogSink {
    fn try_record(&self, event: RequestLogEvent) {
        self.events.lock().unwrap().push(event);
    }
}

struct TestServer {
    address: std::net::SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
struct MockSearch {
    request: Arc<Mutex<Option<Value>>>,
}

async fn search_upstream(
    State(state): State<MockSearch>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.request.lock().unwrap() = Some(body);
    let query = state
        .request
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|request| request.pointer("/commands/search_query/0/q"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    if query.as_deref() == Some("sensitive customer query") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": "invalid_search",
                    "message": "sensitive customer query must never enter logs"
                }
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "encrypted_output": "must-not-leak",
            "output": "A bounded search answer.",
            "results": [{
                "type": "text_result",
                "ref_id": "turn0search0",
                "url": "https://example.test/result"
            }]
        })),
    )
}

async fn start_server(router: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    TestServer { address, task }
}

async fn harness() -> (
    Router,
    Arc<Mutex<Option<Value>>>,
    RecordingRequestLogSink,
    TestServer,
) {
    harness_with_options(vec![], false).await
}

async fn harness_with_origins(
    allowed_origins: Vec<String>,
) -> (
    Router,
    Arc<Mutex<Option<Value>>>,
    RecordingRequestLogSink,
    TestServer,
) {
    harness_with_options(allowed_origins, false).await
}

async fn harness_with_options(
    allowed_origins: Vec<String>,
    allow_legacy_2025_11_25: bool,
) -> (
    Router,
    Arc<Mutex<Option<Value>>>,
    RecordingRequestLogSink,
    TestServer,
) {
    let (router, captured, logs, upstream, _) =
        harness_with_options_and_runtime(allowed_origins, allow_legacy_2025_11_25).await;
    (router, captured, logs, upstream)
}

async fn harness_with_options_and_runtime(
    allowed_origins: Vec<String>,
    allow_legacy_2025_11_25: bool,
) -> (
    Router,
    Arc<Mutex<Option<Value>>>,
    RecordingRequestLogSink,
    TestServer,
    Arc<RuntimeConfig>,
) {
    let captured = Arc::new(Mutex::new(None));
    let upstream = start_server(
        Router::new()
            .route("/v1/alpha/search", post(search_upstream))
            .with_state(MockSearch {
                request: Arc::clone(&captured),
            }),
    )
    .await;

    let api_key_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let model_rule_id = Uuid::new_v4();
    let api_key = |id, secret: &str, allowed_group_ids: Vec<Uuid>| ApiKeyRecord {
        id,
        user_id: Uuid::new_v4(),
        user_status: "active".into(),
        user_websocket_enabled: false,
        secret_value: secret.into(),
        status: "active".into(),
        expires_at: None,
        allowed_api_formats: vec!["open_ai_responses".into()],
        permissions: vec!["proxy".into()],
        allowed_group_ids,
        allowed_channel_ids: vec![],
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
        quota_used_amount: Decimal::ZERO,
    };
    let records = ControlPlaneRecords {
        api_keys: vec![
            api_key(api_key_id, CLIENT_KEY, vec![group_id]),
            api_key(Uuid::new_v4(), NO_ROUTE_KEY, vec![]),
        ],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: model_rule_id,
            client_model: "mcp-search".into(),
            api_format: "open_ai_responses".into(),
            upstream_model_id: Uuid::new_v4(),
            upstream_model_enabled: true,
            upstream_model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Decimal::ZERO,
            cached_input_unit_price: Decimal::ZERO,
            cache_write_unit_price: Decimal::ZERO,
            output_unit_price: Decimal::ZERO,
            advanced_billing: json!({
                "long_context_tiers": [],
                "request_multipliers": []
            }),
            upstream_model: "provider-search".into(),
            channel_group_ids: vec![group_id],
            channel_ids: vec![],
            enabled: true,
        }],
        groups: vec![ChannelGroupRecord {
            id: group_id,
            name: "responses".into(),
            api_format: "open_ai_responses".into(),
            connector_kind: "openai_compatible".into(),
            request_compression: "default".into(),
            priority: 0,
            selection_strategy: "weighted_random".into(),
            enabled: true,
        }],
        channels: vec![ChannelRecord {
            id: channel_id,
            channel_group_id: group_id,
            api_format: "open_ai_responses".into(),
            name: "search".into(),
            base_url: format!("http://{}", upstream.address),
            enabled: true,
            supports_websocket: false,
            supports_standalone_web_search: true,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: Decimal::ONE,
            proxy_id: None,
            config_template_id: None,
            override_document: json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "bearer".into(),
            upstream_auth_header_name: None,
            upstream_api_key: Some("upstream-key".into()),
            available_models: vec!["provider-search".into()],
            test_model: None,
        }],
        proxies: vec![],
        templates: vec![],
        mcp_servers: vec![
            McpServerRecord {
                id: Uuid::new_v4(),
                slug: "search".into(),
                kind: "web_search".into(),
                name: "Web search".into(),
                description: Some("Search the public web.".into()),
                model_rule_id,
                settings_version: 1,
                settings: json!({
                    "external_web_access": "live",
                    "search_context_size": "medium",
                    "allowed_domains": ["example.test"],
                    "blocked_domains": [],
                    "max_output_tokens": {
                        "short": 1000,
                        "medium": 3000,
                        "long": 6000
                    }
                }),
                enabled: true,
            },
            McpServerRecord {
                id: Uuid::new_v4(),
                slug: "search-docs".into(),
                kind: "web_search".into(),
                name: "Documentation search".into(),
                description: Some("A second managed MCP instance.".into()),
                model_rule_id,
                settings_version: 1,
                settings: json!({
                    "allowed_domains": ["example.test"]
                }),
                enabled: true,
            },
        ],
    };
    let mcp_settings = McpTransportSettings::new(
        true,
        Some(Arc::from("https://mcp.example.test")),
        vec!["mcp.example.test".into()].into(),
        allowed_origins.into(),
        allow_legacy_2025_11_25,
        64 * 1024,
        512 * 1024,
        64 * 1024,
        64 * 1024,
    );
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::default().with_mcp(mcp_settings),
        )
        .unwrap(),
    ));
    let logs = RecordingRequestLogSink::default();
    let proxy =
        ProxyService::with_log_sink(Arc::clone(&runtime), 1_048_576, Arc::new(logs.clone()))
            .unwrap();
    let service = McpService::new(proxy, Arc::clone(&runtime));
    (service.router(), captured, logs, upstream, runtime)
}

#[derive(Clone, Default)]
struct MockImage {
    generation: Arc<Mutex<Option<Value>>>,
    edit: Arc<Mutex<Option<CapturedImageEdit>>>,
}

#[derive(Clone)]
struct CapturedImageEdit {
    authorization: Option<String>,
    content_type: String,
    body: Bytes,
}

async fn image_upstream(
    State(state): State<MockImage>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    *state.generation.lock().unwrap() = Some(body.clone());
    if headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer upstream-image-key")
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "wrong upstream credential"}})),
        );
    }
    if body["prompt"] == "sensitive image prompt" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": "invalid_image_prompt",
                    "message": "sensitive image prompt must never enter logs"
                }
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "created": 1,
            "data": [{"b64_json": PNG_BASE64}],
            "usage": {"input_tokens": 7, "output_tokens": 11}
        })),
    )
}

async fn image_edit_upstream(
    State(state): State<MockImage>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    *state.edit.lock().unwrap() = Some(CapturedImageEdit {
        authorization: authorization.clone(),
        content_type,
        body,
    });
    if authorization.as_deref() != Some("Bearer upstream-image-key") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"message": "wrong upstream credential"}})),
        );
    }
    if state.edit.lock().unwrap().as_ref().is_some_and(|request| {
        request
            .body
            .windows(b"sensitive edit prompt".len())
            .any(|window| window == b"sensitive edit prompt")
    }) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "code": "invalid_edit_prompt",
                    "message": "sensitive edit prompt must never enter logs"
                }
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "created": 1,
            "data": [{"b64_json": PNG_BASE64}],
            "usage": {"input_tokens": 13, "output_tokens": 17}
        })),
    )
}

async fn image_harness(
    image_result_bytes: usize,
) -> (Router, MockImage, RecordingRequestLogSink, TestServer) {
    image_harness_with_limits(image_result_bytes, 64 * 1024, 512 * 1024).await
}

async fn image_harness_with_limits(
    image_result_bytes: usize,
    request_body_bytes: usize,
    image_request_body_bytes: usize,
) -> (Router, MockImage, RecordingRequestLogSink, TestServer) {
    let captured = MockImage::default();
    let upstream = start_server(
        Router::new()
            .route("/v1/images/generations", post(image_upstream))
            .route("/v1/images/edits", post(image_edit_upstream))
            .with_state(captured.clone()),
    )
    .await;

    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let model_rule_id = Uuid::new_v4();
    let api_key = |id, secret: &str, allowed_group_ids: Vec<Uuid>| ApiKeyRecord {
        id,
        user_id: Uuid::new_v4(),
        user_status: "active".into(),
        user_websocket_enabled: false,
        secret_value: secret.into(),
        status: "active".into(),
        expires_at: None,
        allowed_api_formats: vec!["open_ai_images".into()],
        permissions: vec!["proxy".into()],
        allowed_group_ids,
        allowed_channel_ids: vec![],
        requests_per_minute: None,
        max_concurrent_requests: None,
        quota_limit_amount: None,
        quota_used_amount: Decimal::ZERO,
    };
    let records = ControlPlaneRecords {
        api_keys: vec![
            api_key(Uuid::new_v4(), IMAGE_CLIENT_KEY, vec![group_id]),
            api_key(Uuid::new_v4(), IMAGE_NO_ROUTE_KEY, vec![]),
        ],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: model_rule_id,
            client_model: "mcp-image".into(),
            api_format: "open_ai_images".into(),
            upstream_model_id: Uuid::new_v4(),
            upstream_model_enabled: true,
            upstream_model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Decimal::ZERO,
            cached_input_unit_price: Decimal::ZERO,
            cache_write_unit_price: Decimal::ZERO,
            output_unit_price: Decimal::ZERO,
            advanced_billing: json!({
                "long_context_tiers": [],
                "request_multipliers": []
            }),
            upstream_model: "provider-image".into(),
            channel_group_ids: vec![group_id],
            channel_ids: vec![],
            enabled: true,
        }],
        groups: vec![ChannelGroupRecord {
            id: group_id,
            name: "images".into(),
            api_format: "open_ai_images".into(),
            connector_kind: "openai_compatible".into(),
            request_compression: "default".into(),
            priority: 0,
            selection_strategy: "weighted_random".into(),
            enabled: true,
        }],
        channels: vec![ChannelRecord {
            id: channel_id,
            channel_group_id: group_id,
            api_format: "open_ai_images".into(),
            name: "images".into(),
            base_url: format!("http://{}", upstream.address),
            enabled: true,
            supports_websocket: false,
            supports_standalone_web_search: false,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: Decimal::ONE,
            proxy_id: None,
            config_template_id: None,
            override_document: json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "bearer".into(),
            upstream_auth_header_name: None,
            upstream_api_key: Some("upstream-image-key".into()),
            available_models: vec!["provider-image".into()],
            test_model: None,
        }],
        proxies: vec![],
        templates: vec![],
        mcp_servers: vec![McpServerRecord {
            id: Uuid::new_v4(),
            slug: "image".into(),
            kind: "image".into(),
            name: "Image generation".into(),
            description: Some("Generate one managed PNG image.".into()),
            model_rule_id,
            settings_version: 1,
            settings: json!({
                "background": "opaque",
                "quality": "high",
                "size": "1536x1024"
            }),
            enabled: true,
        }],
    };
    let mcp_settings = McpTransportSettings::new(
        true,
        Some(Arc::from("https://mcp.example.test")),
        vec!["mcp.example.test".into()].into(),
        Arc::from([]),
        false,
        request_body_bytes,
        image_request_body_bytes,
        64 * 1024,
        image_result_bytes,
    );
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::default().with_mcp(mcp_settings),
        )
        .unwrap(),
    ));
    let logs = RecordingRequestLogSink::default();
    let proxy =
        ProxyService::with_log_sink(Arc::clone(&runtime), 1_048_576, Arc::new(logs.clone()))
            .unwrap();
    let service = McpService::new(proxy, runtime);
    (service.router(), captured, logs, upstream)
}

fn request_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MCP_VERSION,
        "io.modelcontextprotocol/clientInfo": {
            "name": "ai-gateway-test",
            "version": "1.0.0"
        },
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

fn mcp_request(key: &str, method: &str, name: Option<&str>, params: Value) -> Request<Body> {
    mcp_request_at("/mcp/search", key, method, name, params)
}

fn mcp_request_at(
    path: &str,
    key: &str,
    method: &str,
    name: Option<&str>,
    params: Value,
) -> Request<Body> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let mut builder = Request::post(path)
        .header(HOST, "mcp.example.test")
        .header(AUTHORIZATION, format!("Bearer {key}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", MCP_VERSION)
        .header("Mcp-Method", method);
    if let Some(name) = name {
        builder = builder.header("Mcp-Name", name);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn mcp_response_json(response: axum::response::Response) -> Value {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    if content_type.starts_with("application/json") {
        return serde_json::from_slice(&bytes).unwrap();
    }
    let body = std::str::from_utf8(&bytes).unwrap();
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find_map(|data| serde_json::from_str::<Value>(data).ok())
        .expect("SSE response contains a JSON-RPC data event")
}

struct ParsedImageEdit {
    fields: BTreeMap<String, String>,
    images: Vec<(String, Bytes)>,
}

async fn parse_image_edit(captured: &CapturedImageEdit) -> ParsedImageEdit {
    let boundary = multer::parse_boundary(&captured.content_type).unwrap();
    let body = captured.body.clone();
    let stream = stream::once(async move { Ok::<Bytes, io::Error>(body) });
    let mut multipart = multer::Multipart::new(stream, boundary);
    let mut fields = BTreeMap::new();
    let mut images = Vec::new();
    while let Some(field) = multipart.next_field().await.unwrap() {
        let name = field.name().unwrap().to_owned();
        let content_type = field
            .content_type()
            .map(|value| value.essence_str().to_owned());
        let bytes = field.bytes().await.unwrap();
        if matches!(name.as_str(), "image" | "image[]") {
            images.push((content_type.unwrap(), bytes));
        } else {
            assert!(
                fields
                    .insert(name, String::from_utf8(bytes.to_vec()).unwrap())
                    .is_none()
            );
        }
    }
    ParsedImageEdit { fields, images }
}

#[tokio::test]
async fn database_backed_transport_settings_apply_without_a_restart() {
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            ControlPlaneRecords::default(),
            SystemRuntimeSettings::default(),
        )
        .unwrap(),
    ));
    let proxy = ProxyService::new(Arc::clone(&runtime), 1_048_576).unwrap();
    let router = McpService::new(proxy, Arc::clone(&runtime)).router();
    let request = || {
        Request::post("/mcp/missing")
            .header(HOST, "mcp.example.test")
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(Body::from(
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "server/discover",
                    "params": {}
                })
                .to_string(),
            ))
            .unwrap()
    };

    assert_eq!(
        router.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    let enabled = McpTransportSettings::new(
        true,
        Some(Arc::from("https://mcp.example.test")),
        vec!["mcp.example.test".into()].into(),
        Arc::from([]),
        true,
        64 * 1024,
        512 * 1024,
        64 * 1024,
        64 * 1024,
    );
    runtime.replace_snapshot(Arc::new(
        compile_control_plane_with_system_settings(
            ControlPlaneRecords::default(),
            SystemRuntimeSettings::default().with_mcp(enabled),
        )
        .unwrap(),
    ));
    assert_eq!(
        router.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    runtime.replace_snapshot(Arc::new(
        compile_control_plane_with_system_settings(
            ControlPlaneRecords::default(),
            SystemRuntimeSettings::default(),
        )
        .unwrap(),
    ));
    assert_eq!(
        router.oneshot(request()).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn stateless_web_run_forwards_through_gateway_and_attributes_logs() {
    let (router, captured, logs, _upstream) = harness().await;
    let response = router
        .clone()
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": {
                    "search_query": [{
                        "q": "MCP protocol",
                        "domains": ["example.test"]
                    }],
                    "response_length": "short"
                }
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(
        response["result"]["content"][0]["text"],
        "A bounded search answer."
    );
    assert_eq!(
        response["result"]["structuredContent"]["results"][0]["ref_id"],
        "turn0search0"
    );
    let search_session_id = response["result"]["structuredContent"]["search_session_id"]
        .as_str()
        .filter(|value| Uuid::parse_str(value).is_ok())
        .unwrap()
        .to_owned();
    assert!(!response.to_string().contains("must-not-leak"));

    let first_upstream = captured.lock().unwrap().clone().unwrap();
    assert_eq!(first_upstream["model"], "provider-search");
    assert_eq!(
        first_upstream["commands"]["search_query"][0]["q"],
        "MCP protocol"
    );
    assert_eq!(first_upstream["max_output_tokens"], 1000);
    assert_eq!(
        first_upstream["settings"]["allowed_callers"],
        json!(["direct"])
    );
    assert_eq!(
        first_upstream["settings"]["filters"]["allowed_domains"],
        json!(["example.test"])
    );
    let provider_id = first_upstream["id"].as_str().unwrap().to_owned();
    assert!(Uuid::parse_str(&provider_id).is_ok());

    let continuation = router
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": {
                    "search_session_id": search_session_id,
                    "open": [{"ref_id": "turn0search0"}]
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(continuation.status(), StatusCode::OK);
    let continuation: Value =
        serde_json::from_slice(&to_bytes(continuation.into_body(), 64 * 1024).await.unwrap())
            .unwrap();
    assert!(!continuation["result"]["isError"].as_bool().unwrap_or(false));
    let continued_upstream = captured.lock().unwrap().clone().unwrap();
    assert_eq!(continued_upstream["id"], provider_id);
    assert_eq!(
        continued_upstream["commands"]["open"][0]["ref_id"],
        "turn0search0"
    );
    assert!(
        continued_upstream["commands"]
            .get("search_session_id")
            .is_none()
    );

    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.request_source == RequestLogSource::Mcp)
    );
}

#[tokio::test]
async fn mcp_upstream_errors_do_not_expose_or_log_provider_messages() {
    let (router, _, logs, _upstream) = harness().await;
    let response = router
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": {
                    "search_query": [{"q": "sensitive customer query"}]
                }
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("sensitive customer query"));

    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request_source, RequestLogSource::Mcp);
    assert_eq!(
        events[0].error_summary.as_deref(),
        Some("The upstream returned HTTP 400.")
    );
    assert_eq!(events[0].error_code.as_deref(), Some("upstream_http_error"));
}

#[tokio::test]
async fn stateless_imagegen_returns_one_mcp_image_and_attributes_logs() {
    let (router, captured, logs, _upstream) = image_harness(64 * 1024).await;
    let tools = router
        .clone()
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/list",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    let tools: Value =
        serde_json::from_slice(&to_bytes(tools.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(tools["result"]["tools"][0]["name"], "image_gen.imagegen");
    assert_eq!(
        tools["result"]["tools"][0]["inputSchema"]["additionalProperties"],
        false
    );
    assert!(tools["result"]["tools"][0]["inputSchema"]["properties"]["prompt"].is_object());
    assert_eq!(
        tools["result"]["tools"][0]["inputSchema"]["properties"]["prompt"]["maxLength"],
        32_000
    );
    assert_eq!(
        tools["result"]["tools"][0]["inputSchema"]["properties"]["referenced_image_urls"]["maxItems"],
        5
    );

    let response = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {"prompt": "paint a moonlit lake"}
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["content"][0]["type"], "image");
    assert_eq!(response["result"]["content"][0]["data"], PNG_BASE64);
    assert_eq!(response["result"]["content"][0]["mimeType"], "image/png");
    assert_eq!(
        response["result"]["content"][0]["_meta"]["codex/imageDetail"],
        "original"
    );
    assert_eq!(
        response["result"]["structuredContent"],
        json!({"status": "completed", "mime_type": "image/png"})
    );
    assert!(
        !response["result"]["structuredContent"]
            .to_string()
            .contains(PNG_BASE64)
    );

    let upstream = captured.generation.lock().unwrap().clone().unwrap();
    assert_eq!(upstream["model"], "provider-image");
    assert_eq!(upstream["prompt"], "paint a moonlit lake");
    assert_eq!(upstream["n"], 1);
    assert_eq!(upstream["background"], "opaque");
    assert_eq!(upstream["quality"], "high");
    assert_eq!(upstream["size"], "1536x1024");
    assert!(upstream.get("output_format").is_none());
    assert!(upstream.get("response_format").is_none());
    assert!(upstream.get("stream").is_none());

    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request_source, RequestLogSource::Mcp);
    assert_eq!(events[0].api_operation, ApiOperation::ImagesGeneration);
    assert!(
        !serde_json::to_string(&events[0])
            .unwrap()
            .contains("moonlit")
    );
    assert!(
        !serde_json::to_string(&events[0])
            .unwrap()
            .contains(PNG_BASE64)
    );
}

#[tokio::test]
async fn stateless_imagegen_edits_explicit_data_urls_through_the_images_proxy() {
    let (router, captured, logs, _upstream) = image_harness(64 * 1024).await;
    let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0xff, 0xd9];
    let jpeg_url = format!("data:image/jpeg;base64,{}", BASE64_STANDARD.encode(&jpeg));
    let response = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {
                    "prompt": "add a red hat",
                    "referenced_image_urls": [
                        format!("data:image/png;base64,{PNG_BASE64}"),
                        jpeg_url
                    ]
                }
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["content"][0]["type"], "image");
    assert_eq!(response["result"]["content"][0]["data"], PNG_BASE64);
    assert_eq!(
        response["result"]["structuredContent"]["status"],
        "completed"
    );

    assert!(captured.generation.lock().unwrap().is_none());
    let edit = captured.edit.lock().unwrap().clone().unwrap();
    assert_eq!(
        edit.authorization.as_deref(),
        Some("Bearer upstream-image-key")
    );
    let edit = parse_image_edit(&edit).await;
    assert_eq!(edit.fields["model"], "provider-image");
    assert_eq!(edit.fields["prompt"], "add a red hat");
    assert_eq!(edit.fields["n"], "1");
    assert_eq!(edit.fields["background"], "opaque");
    assert_eq!(edit.fields["quality"], "high");
    assert_eq!(edit.fields["size"], "1536x1024");
    assert_eq!(edit.images.len(), 2);
    assert_eq!(edit.images[0].0, "image/png");
    assert_eq!(
        edit.images[0].1,
        Bytes::from(BASE64_STANDARD.decode(PNG_BASE64).unwrap())
    );
    assert_eq!(edit.images[1], ("image/jpeg".into(), Bytes::from(jpeg)));

    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request_source, RequestLogSource::Mcp);
    assert_eq!(events[0].api_operation, ApiOperation::ImagesEdit);
    let logged = serde_json::to_string(&events[0]).unwrap();
    assert!(!logged.contains("red hat"));
    assert!(!logged.contains(PNG_BASE64));
}

#[tokio::test]
async fn imagegen_edit_errors_hide_provider_payloads() {
    let (router, _, logs, _upstream) = image_harness(64 * 1024).await;
    let response = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {
                    "prompt": "sensitive edit prompt",
                    "referenced_image_urls": [format!("data:image/png;base64,{PNG_BASE64}")]
                }
            }),
        ))
        .await
        .unwrap();
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();

    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("sensitive edit prompt"));
    assert!(!response.to_string().contains("invalid_edit_prompt"));
    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].api_operation, ApiOperation::ImagesEdit);
    assert_eq!(
        events[0].error_summary.as_deref(),
        Some("The upstream returned HTTP 400.")
    );
    assert_eq!(events[0].error_code.as_deref(), Some("upstream_http_error"));
}

#[tokio::test]
async fn imagegen_rejects_untrusted_edit_references_before_forwarding() {
    let (router, captured, logs, _upstream) = image_harness(64 * 1024).await;
    let cases = [
        json!(["https://example.test/image.png"]),
        json!([format!("data:image/jpeg;base64,{PNG_BASE64}")]),
        Value::Array(
            (0..6)
                .map(|_| Value::String(format!("data:image/png;base64,{PNG_BASE64}")))
                .collect(),
        ),
    ];

    for referenced_image_urls in cases {
        let response = router
            .clone()
            .oneshot(mcp_request_at(
                "/mcp/image",
                IMAGE_CLIENT_KEY,
                "tools/call",
                Some("image_gen.imagegen"),
                json!({
                    "_meta": request_meta(),
                    "name": "image_gen.imagegen",
                    "arguments": {
                        "prompt": "must not execute",
                        "referenced_image_urls": referenced_image_urls
                    }
                }),
            ))
            .await
            .unwrap();
        let response: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(response["result"]["isError"], true);
    }

    assert!(captured.generation.lock().unwrap().is_none());
    assert!(captured.edit.lock().unwrap().is_none());
    assert!(logs.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn image_mcp_uses_its_independent_request_envelope_limit() {
    let (router, captured, _, _upstream) =
        image_harness_with_limits(64 * 1024, 1_024, 8 * 1_024).await;
    let mut accepted_image = BASE64_STANDARD.decode(PNG_BASE64).unwrap();
    accepted_image.resize(2 * 1_024, 0);
    let accepted = router
        .clone()
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {
                    "prompt": "accept the image-specific envelope",
                    "referenced_image_urls": [format!(
                        "data:image/png;base64,{}",
                        BASE64_STANDARD.encode(&accepted_image)
                    )]
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);
    assert!(captured.edit.lock().unwrap().is_some());

    let mut rejected_image = BASE64_STANDARD.decode(PNG_BASE64).unwrap();
    rejected_image.resize(8 * 1_024, 0);
    let rejected = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {
                    "prompt": "exceed the image envelope",
                    "referenced_image_urls": [format!(
                        "data:image/png;base64,{}",
                        BASE64_STANDARD.encode(&rejected_image)
                    )]
                }
            }),
        ))
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn imagegen_filters_tools_and_hides_provider_error_payloads() {
    let (router, captured, logs, _upstream) = image_harness(64 * 1024).await;
    let hidden = router
        .clone()
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_NO_ROUTE_KEY,
            "tools/list",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    let hidden: Value =
        serde_json::from_slice(&to_bytes(hidden.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(hidden["result"]["tools"], json!([]));

    let denied = router
        .clone()
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_NO_ROUTE_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {"prompt": "must not execute"}
            }),
        ))
        .await
        .unwrap();
    let denied: Value =
        serde_json::from_slice(&to_bytes(denied.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(denied["result"]["isError"], true);
    assert!(captured.generation.lock().unwrap().is_none());
    assert!(captured.edit.lock().unwrap().is_none());

    let override_attempt = router
        .clone()
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {
                    "prompt": "must not override the configured model",
                    "model": "caller-selected-model"
                }
            }),
        ))
        .await
        .unwrap();
    let override_attempt: Value = serde_json::from_slice(
        &to_bytes(override_attempt.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(override_attempt["result"]["isError"], true);
    assert!(captured.generation.lock().unwrap().is_none());
    assert!(captured.edit.lock().unwrap().is_none());

    let failed = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {"prompt": "sensitive image prompt"}
            }),
        ))
        .await
        .unwrap();
    let failed: Value =
        serde_json::from_slice(&to_bytes(failed.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(failed["result"]["isError"], true);
    assert!(!failed.to_string().contains("sensitive image prompt"));
    assert!(!failed.to_string().contains("invalid_image_prompt"));

    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request_source, RequestLogSource::Mcp);
    assert_eq!(
        events[0].error_summary.as_deref(),
        Some("The upstream returned HTTP 400.")
    );
    assert_eq!(events[0].error_code.as_deref(), Some("upstream_http_error"));
}

#[tokio::test]
async fn imagegen_enforces_the_independent_result_limit() {
    let (router, captured, logs, _upstream) = image_harness(64).await;
    let response = router
        .oneshot(mcp_request_at(
            "/mcp/image",
            IMAGE_CLIENT_KEY,
            "tools/call",
            Some("image_gen.imagegen"),
            json!({
                "_meta": request_meta(),
                "name": "image_gen.imagegen",
                "arguments": {"prompt": "bounded result"}
            }),
        ))
        .await
        .unwrap();
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();

    assert_eq!(response["result"]["isError"], true);
    assert!(
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("result limit")
    );
    assert!(!response.to_string().contains(PNG_BASE64));
    assert!(captured.generation.lock().unwrap().is_some());
    let events = logs.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome, RequestLogOutcome::Succeeded);
}

#[tokio::test]
async fn search_sessions_are_isolated_between_mcp_instances() {
    let (router, captured, _, _upstream) = harness().await;
    let session_id = "93b259ef-edb8-412a-b6c5-e442d3a9da6f";
    let arguments = json!({
        "search_session_id": session_id,
        "search_query": [{
            "q": "MCP isolation",
            "domains": ["example.test"]
        }]
    });

    let first = router
        .clone()
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": arguments.clone()
            }),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first_provider_id = captured.lock().unwrap().as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut second = mcp_request(
        CLIENT_KEY,
        "tools/call",
        Some("web.run"),
        json!({
            "_meta": request_meta(),
            "name": "web.run",
            "arguments": arguments
        }),
    );
    *second.uri_mut() = "/mcp/search-docs".parse().unwrap();
    let second = router.oneshot(second).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let second_provider_id = captured.lock().unwrap().as_ref().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_ne!(first_provider_id, second_provider_id);
}

#[tokio::test]
async fn mcp_boundary_rejects_untrusted_requests_and_filters_tools() {
    let (router, captured, _, _upstream) = harness().await;

    let missing_key = Request::post("/mcp/search")
        .header(HOST, "mcp.example.test")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        router.clone().oneshot(missing_key).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let mut wrong_host = mcp_request(
        CLIENT_KEY,
        "tools/list",
        None,
        json!({"_meta": request_meta()}),
    );
    wrong_host
        .headers_mut()
        .insert(HOST, "wrong.example.test".parse().unwrap());
    assert_eq!(
        router.clone().oneshot(wrong_host).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let mut session_request = mcp_request(
        CLIENT_KEY,
        "tools/list",
        None,
        json!({"_meta": request_meta()}),
    );
    session_request
        .headers_mut()
        .insert("Mcp-Session-Id", "legacy-session".parse().unwrap());
    assert_eq!(
        router
            .clone()
            .oneshot(session_request)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let missing_metadata = mcp_request(CLIENT_KEY, "tools/list", None, json!({}));
    assert_eq!(
        router
            .clone()
            .oneshot(missing_metadata)
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let mut browser_request = mcp_request(
        CLIENT_KEY,
        "tools/list",
        None,
        json!({"_meta": request_meta()}),
    );
    browser_request
        .headers_mut()
        .insert(ORIGIN, "https://console.example.test".parse().unwrap());
    assert_eq!(
        router
            .clone()
            .oneshot(browser_request)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    let discovered = router
        .clone()
        .oneshot(mcp_request(
            CLIENT_KEY,
            "server/discover",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    assert_eq!(discovered.status(), StatusCode::OK);
    assert!(discovered.headers().get("Mcp-Session-Id").is_none());
    let discovered: Value =
        serde_json::from_slice(&to_bytes(discovered.into_body(), 64 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(
        discovered["result"]["supportedVersions"],
        json!([MCP_VERSION])
    );
    assert_eq!(
        discovered["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "search"
    );

    let mut docs_discover = mcp_request(
        CLIENT_KEY,
        "server/discover",
        None,
        json!({"_meta": request_meta()}),
    );
    *docs_discover.uri_mut() = "/mcp/search-docs".parse().unwrap();
    let docs_discover = router.clone().oneshot(docs_discover).await.unwrap();
    assert_eq!(docs_discover.status(), StatusCode::OK);
    let docs_discover: Value = serde_json::from_slice(
        &to_bytes(docs_discover.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        docs_discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "search-docs"
    );
    assert_eq!(
        docs_discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["title"],
        "Documentation search"
    );

    let tools = router
        .clone()
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/list",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    let tools: Value =
        serde_json::from_slice(&to_bytes(tools.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(tools["result"]["tools"][0]["name"], "web.run");
    assert_eq!(
        tools["result"]["tools"][0]["inputSchema"]["additionalProperties"],
        false
    );
    assert!(
        tools["result"]["tools"][0]["inputSchema"]["properties"]["search_session_id"].is_object()
    );

    let initialize = Request::post("/mcp/search")
        .header(HOST, "mcp.example.test")
        .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "initialize",
                "params": {
                    "protocolVersion": MCP_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "legacy-client", "version": "1.0.0"}
                }
            })
            .to_string(),
        ))
        .unwrap();
    let initialize = router.clone().oneshot(initialize).await.unwrap();
    assert!(initialize.headers().get("Mcp-Session-Id").is_none());
    let initialize: Value =
        serde_json::from_slice(&to_bytes(initialize.into_body(), 64 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(initialize["error"]["code"], -32600);

    let response = router
        .clone()
        .oneshot(mcp_request(
            NO_ROUTE_KEY,
            "tools/list",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["tools"], json!([]));

    let response = router
        .clone()
        .oneshot(mcp_request(
            NO_ROUTE_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": {"search_query": [{"q": "must not execute"}]}
            }),
        ))
        .await
        .unwrap();
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["isError"], true);
    assert!(captured.lock().unwrap().is_none());

    let invalid_arguments = router
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/call",
            Some("web.run"),
            json!({
                "_meta": request_meta(),
                "name": "web.run",
                "arguments": {"search_query": []}
            }),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_arguments.status(), StatusCode::OK);
    let invalid_arguments: Value = serde_json::from_slice(
        &to_bytes(invalid_arguments.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(invalid_arguments["result"]["isError"], true);
    assert!(captured.lock().unwrap().is_none());
}

#[tokio::test]
async fn transport_setting_changes_close_active_legacy_sse_sessions() {
    let (router, _, _, _upstream, runtime) = harness_with_options_and_runtime(vec![], true).await;
    let initialize = Request::post("/mcp/search")
        .header(HOST, "mcp.example.test")
        .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "legacy-client", "version": "1.0.0"}
                }
            })
            .to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(initialize).await.unwrap();
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let _ = mcp_response_json(response).await;

    let initialized = router
        .clone()
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/initialized"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);

    let stream = router
        .oneshot(
            Request::get("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(ACCEPT, "text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", session_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);

    runtime.replace_snapshot(Arc::new(
        compile_control_plane_with_system_settings(
            ControlPlaneRecords::default(),
            SystemRuntimeSettings::default(),
        )
        .unwrap(),
    ));
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        to_bytes(stream.into_body(), 64 * 1024),
    )
    .await
    .expect("legacy SSE stream closes after transport settings change")
    .unwrap();
}

#[tokio::test]
async fn optional_legacy_mode_supports_the_complete_session_lifecycle() {
    let browser_origin = "https://client.example.test";
    let (router, captured, _, _upstream) =
        harness_with_options(vec![browser_origin.into()], true).await;
    let initialize = Request::post("/mcp/search")
        .header(HOST, "mcp.example.test")
        .header(ORIGIN, browser_origin)
        .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "legacy-client", "version": "1.0.0"}
                }
            })
            .to_string(),
        ))
        .unwrap();

    let response = router.clone().oneshot(initialize).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&browser_origin.parse().unwrap())
    );
    assert_eq!(
        response.headers().get(ACCESS_CONTROL_EXPOSE_HEADERS),
        Some(&"mcp-session-id".parse().unwrap())
    );
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .expect("legacy initialize returns a session ID")
        .to_str()
        .unwrap()
        .to_owned();
    let response = mcp_response_json(response).await;
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");

    let initialized = router
        .clone()
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/initialized"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);

    let tools = router
        .clone()
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/list",
                        "params": {}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    let tools = mcp_response_json(tools).await;
    assert_eq!(tools["result"]["tools"][0]["name"], "web.run");

    let call = router
        .clone()
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 4,
                        "method": "tools/call",
                        "params": {
                            "name": "web.run",
                            "arguments": {
                                "search_query": [{"q": "legacy session search"}]
                            }
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(call.status(), StatusCode::OK);
    let call = mcp_response_json(call).await;
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(
        captured.lock().unwrap().as_ref().unwrap()["commands"]["search_query"][0]["q"],
        "legacy session search"
    );

    let modern = router
        .clone()
        .oneshot(mcp_request(
            CLIENT_KEY,
            "tools/list",
            None,
            json!({"_meta": request_meta()}),
        ))
        .await
        .unwrap();
    assert_eq!(modern.status(), StatusCode::OK);
    assert!(modern.headers().get("Mcp-Session-Id").is_none());

    let stream = router
        .clone()
        .oneshot(
            Request::get("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(ACCEPT, "text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert_eq!(
        stream
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    drop(stream);

    let deleted = router
        .clone()
        .oneshot(
            Request::delete("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::ACCEPTED);

    let closed = router
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-11-25")
                .header("Mcp-Session-Id", session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 5,
                        "method": "tools/list",
                        "params": {}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn codex_legacy_2025_06_18_client_completes_the_session_handshake() {
    let (router, _, _, _upstream) = harness_with_options(vec![], true).await;
    let initialize = Request::post("/mcp/search")
        .header(HOST, "mcp.example.test")
        .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-06-18")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"elicitation": {}},
                    "clientInfo": {
                        "name": "codex-mcp-client",
                        "title": "Codex",
                        "version": "0.146.0"
                    }
                }
            })
            .to_string(),
        ))
        .unwrap();

    let response = router.clone().oneshot(initialize).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session_id = response
        .headers()
        .get("Mcp-Session-Id")
        .expect("Codex-compatible initialize returns a session ID")
        .to_str()
        .unwrap()
        .to_owned();
    let response = mcp_response_json(response).await;
    assert_eq!(
        response["result"]["protocolVersion"], "2025-06-18",
        "{response}"
    );

    let initialized = router
        .clone()
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-06-18")
                .header("Mcp-Session-Id", &session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "method": "notifications/initialized"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initialized.status(), StatusCode::ACCEPTED);

    let tools = router
        .oneshot(
            Request::post("/mcp/search")
                .header(HOST, "mcp.example.test")
                .header(AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-06-18")
                .header("Mcp-Session-Id", session_id)
                .body(Body::from(
                    json!({
                        "jsonrpc": "2.0",
                        "id": 2,
                        "method": "tools/list",
                        "params": {}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tools.status(), StatusCode::OK);
    let tools = mcp_response_json(tools).await;
    assert_eq!(tools["result"]["tools"][0]["name"], "web.run");
}

#[tokio::test]
async fn configured_browser_origin_receives_cors_and_passes_origin_validation() {
    let origin = "https://client.example.test";
    let (router, _, _, _upstream) = harness_with_origins(vec![origin.into()]).await;
    let preflight = Request::builder()
        .method("OPTIONS")
        .uri("/mcp/search")
        .header(HOST, "mcp.example.test")
        .header(ORIGIN, origin)
        .header(ACCESS_CONTROL_REQUEST_METHOD, "POST")
        .header(
            ACCESS_CONTROL_REQUEST_HEADERS,
            "authorization,content-type,mcp-protocol-version,mcp-method",
        )
        .body(Body::empty())
        .unwrap();
    let preflight = router.clone().oneshot(preflight).await.unwrap();
    assert_eq!(preflight.status(), StatusCode::OK);
    assert_eq!(
        preflight.headers().get(ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&origin.parse().unwrap())
    );
    assert_eq!(
        preflight.headers().get(ACCESS_CONTROL_ALLOW_HEADERS),
        Some(
            &"authorization,content-type,mcp-protocol-version,mcp-method"
                .parse()
                .unwrap()
        )
    );

    let mut discover = mcp_request(
        CLIENT_KEY,
        "server/discover",
        None,
        json!({"_meta": request_meta()}),
    );
    discover
        .headers_mut()
        .insert(ORIGIN, origin.parse().unwrap());
    assert_eq!(
        router.oneshot(discover).await.unwrap().status(),
        StatusCode::OK
    );
}
