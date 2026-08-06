#![cfg(feature = "mcp-server")]

use std::sync::{Arc, Mutex};

use ai_gateway::{
    application::{ProxyService, RequestLogSink},
    domain::{RequestLogEvent, RequestLogSource},
    mcp::McpService,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, McpServerRecord,
        ModelRuleRecord,
    },
    runtime_config::{McpRuntimeConfig, RuntimeConfig, compile_control_plane},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{
        Request, StatusCode,
        header::{
            ACCEPT, ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_REQUEST_HEADERS, ACCESS_CONTROL_REQUEST_METHOD, AUTHORIZATION,
            CONTENT_TYPE, HOST, ORIGIN,
        },
    },
    response::IntoResponse,
    routing::post,
};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "mcp-client-key";
const NO_ROUTE_KEY: &str = "mcp-no-route-key";
const MCP_VERSION: &str = "2026-07-28";

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
    let runtime = Arc::new(RuntimeConfig::new(compile_control_plane(records).unwrap()));
    let logs = RecordingRequestLogSink::default();
    let proxy = ProxyService::with_log_sink(runtime, 1_048_576, Arc::new(logs.clone())).unwrap();
    let service = McpService::new(
        proxy,
        &McpRuntimeConfig {
            public_base_url: "https://mcp.example.test".into(),
            allowed_hosts: vec!["mcp.example.test".into()],
            allowed_origins,
            allow_legacy_2025_11_25,
            request_body_bytes: 64 * 1024,
            search_result_bytes: 64 * 1024,
        },
    );
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
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });
    let mut builder = Request::post("/mcp/search")
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
async fn optional_legacy_mode_accepts_initialize_without_creating_a_session() {
    let (router, _, _, _upstream) = harness_with_options(vec![], true).await;
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

    let response = router.oneshot(initialize).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("Mcp-Session-Id").is_none());
    let response: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
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
