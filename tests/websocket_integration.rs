use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ai_gateway::{
    admission::AdmissionRuntime,
    application::{ProxyService, RecordingRequestLogSink, SystemMetricsService},
    domain::{
        PassiveHealthSettings, RequestProtocol, ResponsesWebSocketSettings, SystemRuntimeSettings,
        UpstreamTimeoutDefaults,
    },
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
        ProxyRecord,
    },
    routing::{PassiveHealthPolicy, RoutingRuntime},
    runtime_config::{RuntimeConfig, compile_control_plane_with_system_settings},
    upstream::UpstreamClientRegistry,
};
use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    accept_hdr_async_with_config, connect_async,
    tungstenite::{
        Error as WebSocketError, Message,
        client::IntoClientRequest,
        extensions::compression::deflate::DeflateConfig,
        handshake::server::{Request, Response},
        protocol::WebSocketConfig,
    },
};
use uuid::Uuid;

const CLIENT_KEY: &str = "gateway-client-key";
const UPSTREAM_KEY: &str = "upstream-key";
const CLIENT_MODEL: &str = "client-model";
const UPSTREAM_MODEL: &str = "upstream-model";
const OPENAI_BETA: &str = "responses_websockets=2026-02-06";

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_gateway(app: axum::Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer { address, task }
}

#[derive(Clone, Debug)]
struct CapturedHandshake {
    uri: String,
    headers: HeaderMap,
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    connection: usize,
    body: Value,
}

struct MockResponsesWebSocket {
    address: SocketAddr,
    handshakes: Arc<Mutex<Vec<CapturedHandshake>>>,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    task: JoinHandle<()>,
}

impl Drop for MockResponsesWebSocket {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockResponsesWebSocket {
    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn handshakes(&self) -> Vec<CapturedHandshake> {
        self.handshakes.lock().unwrap().clone()
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

// tokio-tungstenite fixes the handshake callback's error type to a full HTTP response.
#[allow(clippy::result_large_err)]
async fn start_mock_upstream() -> MockResponsesWebSocket {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshakes = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let next_connection = Arc::new(AtomicUsize::new(0));
    let next_response = Arc::new(AtomicUsize::new(1));
    let task = {
        let handshakes = Arc::clone(&handshakes);
        let requests = Arc::clone(&requests);
        let next_connection = Arc::clone(&next_connection);
        let next_response = Arc::clone(&next_response);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let connection = next_connection.fetch_add(1, Ordering::SeqCst);
                let handshakes = Arc::clone(&handshakes);
                let requests = Arc::clone(&requests);
                let next_response = Arc::clone(&next_response);
                tokio::spawn(async move {
                    let callback = move |request: &Request, response: Response| {
                        handshakes.lock().unwrap().push(CapturedHandshake {
                            uri: request.uri().to_string(),
                            headers: request.headers().clone(),
                        });
                        Ok(response)
                    };
                    let mut config = WebSocketConfig::default();
                    config.extensions.permessage_deflate = Some(DeflateConfig::default());
                    let mut websocket =
                        accept_hdr_async_with_config(stream, callback, Some(config))
                            .await
                            .unwrap();
                    let mut last_response_id = None::<String>;
                    while let Some(message) = websocket.next().await {
                        let Ok(Message::Text(text)) = message else {
                            break;
                        };
                        let body: Value = serde_json::from_str(&text).unwrap();
                        requests.lock().unwrap().push(CapturedRequest {
                            connection,
                            body: body.clone(),
                        });
                        let previous = body.get("previous_response_id").and_then(Value::as_str);
                        if previous.is_some() && previous != last_response_id.as_deref() {
                            websocket
                                .send(Message::Text(
                                    json!({
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
                        let response_number = next_response.fetch_add(1, Ordering::SeqCst);
                        let response_id = format!("resp-{response_number}");
                        websocket
                            .send(Message::Text(
                                json!({
                                    "type": "response.created",
                                    "response": {"id": response_id}
                                })
                                .to_string()
                                .into(),
                            ))
                            .await
                            .unwrap();
                        websocket
                            .send(Message::Text(
                                json!({
                                    "type": "response.completed",
                                    "response": {
                                        "id": response_id,
                                        "usage": {
                                            "input_tokens": 5,
                                            "input_tokens_details": {"cached_tokens": 1},
                                            "output_tokens": 2,
                                            "output_tokens_details": {"reasoning_tokens": 1},
                                            "total_tokens": 7
                                        }
                                    }
                                })
                                .to_string()
                                .into(),
                            ))
                            .await
                            .unwrap();
                        last_response_id = Some(response_id);
                        if body["client_metadata"]["force_residual"] == true {
                            websocket
                                .send(Message::Text(
                                    json!({
                                        "type": "response.output_text.delta",
                                        "delta": "unexpected residual event"
                                    })
                                    .to_string()
                                    .into(),
                                ))
                                .await
                                .unwrap();
                        }
                    }
                });
            }
        })
    };
    MockResponsesWebSocket {
        address,
        handshakes,
        requests,
        task,
    }
}

struct HttpConnectProxy {
    address: SocketAddr,
    request: Arc<Mutex<Option<String>>>,
    task: JoinHandle<()>,
}

impl Drop for HttpConnectProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_http_connect_proxy() -> HttpConnectProxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&request);
    let task = tokio::spawn(async move {
        let (mut client, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 1024];
            let read = client.read(&mut chunk).await.unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&chunk[..read]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(bytes).unwrap();
        *captured.lock().unwrap() = Some(request.clone());
        let authority = request
            .lines()
            .next()
            .and_then(|line| line.split_ascii_whitespace().nth(1))
            .unwrap();
        let mut upstream = TcpStream::connect(authority).await.unwrap();
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .unwrap();
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
            .unwrap();
    });
    HttpConnectProxy {
        address,
        request,
        task,
    }
}

struct Socks5Proxy {
    address: SocketAddr,
    credentials: Arc<Mutex<Option<(String, String)>>>,
    target: Arc<Mutex<Option<(String, u16)>>>,
    task: JoinHandle<()>,
}

impl Drop for Socks5Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_socks5_proxy() -> Socks5Proxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let credentials = Arc::new(Mutex::new(None));
    let target = Arc::new(Mutex::new(None));
    let captured_credentials = Arc::clone(&credentials);
    let captured_target = Arc::clone(&target);
    let task = tokio::spawn(async move {
        let (mut client, _) = listener.accept().await.unwrap();
        let mut greeting = [0_u8; 2];
        client.read_exact(&mut greeting).await.unwrap();
        let mut methods = vec![0_u8; usize::from(greeting[1])];
        client.read_exact(&mut methods).await.unwrap();
        assert_eq!(greeting[0], 5);
        assert!(methods.contains(&2));
        client.write_all(&[5, 2]).await.unwrap();

        let mut auth = [0_u8; 2];
        client.read_exact(&mut auth).await.unwrap();
        let mut username = vec![0_u8; usize::from(auth[1])];
        client.read_exact(&mut username).await.unwrap();
        let mut password_length = [0_u8; 1];
        client.read_exact(&mut password_length).await.unwrap();
        let mut password = vec![0_u8; usize::from(password_length[0])];
        client.read_exact(&mut password).await.unwrap();
        *captured_credentials.lock().unwrap() = Some((
            String::from_utf8(username).unwrap(),
            String::from_utf8(password).unwrap(),
        ));
        client.write_all(&[1, 0]).await.unwrap();

        let mut request = [0_u8; 4];
        client.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[..3], &[5, 1, 0]);
        let host = match request[3] {
            1 => {
                let mut address = [0_u8; 4];
                client.read_exact(&mut address).await.unwrap();
                std::net::Ipv4Addr::from(address).to_string()
            }
            3 => {
                let mut length = [0_u8; 1];
                client.read_exact(&mut length).await.unwrap();
                let mut host = vec![0_u8; usize::from(length[0])];
                client.read_exact(&mut host).await.unwrap();
                String::from_utf8(host).unwrap()
            }
            4 => {
                let mut address = [0_u8; 16];
                client.read_exact(&mut address).await.unwrap();
                std::net::Ipv6Addr::from(address).to_string()
            }
            address_type => panic!("unexpected SOCKS5 address type {address_type}"),
        };
        let mut port = [0_u8; 2];
        client.read_exact(&mut port).await.unwrap();
        let port = u16::from_be_bytes(port);
        *captured_target.lock().unwrap() = Some((host.clone(), port));
        let mut upstream = TcpStream::connect((host.as_str(), port)).await.unwrap();
        client
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
            .unwrap();
    });
    Socks5Proxy {
        address,
        credentials,
        target,
        task,
    }
}

struct GatewayHarness {
    server: TestServer,
    proxy: ProxyService,
    logs: RecordingRequestLogSink,
}

#[derive(Clone, Copy)]
struct WebSocketControls {
    system_enabled: bool,
    user_enabled: bool,
    channel_supported: bool,
    max_idle_connections: usize,
}

impl Default for WebSocketControls {
    fn default() -> Self {
        Self {
            system_enabled: true,
            user_enabled: true,
            channel_supported: true,
            max_idle_connections: 128,
        }
    }
}

async fn gateway_harness(upstream: &MockResponsesWebSocket) -> GatewayHarness {
    gateway_harness_with_proxy(upstream, None).await
}

async fn gateway_harness_with_proxy(
    upstream: &MockResponsesWebSocket,
    outbound_proxy: Option<ProxyRecord>,
) -> GatewayHarness {
    gateway_harness_with_controls(upstream, outbound_proxy, WebSocketControls::default()).await
}

async fn gateway_harness_with_controls(
    upstream: &MockResponsesWebSocket,
    outbound_proxy: Option<ProxyRecord>,
    controls: WebSocketControls,
) -> GatewayHarness {
    let api_key_id = Uuid::new_v4();
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let proxy_id = outbound_proxy.as_ref().map(|proxy| proxy.id);
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: api_key_id,
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            user_websocket_enabled: controls.user_enabled,
            secret_value: CLIENT_KEY.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec!["open_ai_responses".into()],
            permissions: vec!["proxy".into(), "models.read".into()],
            allowed_group_ids: vec![group_id],
            allowed_channel_ids: vec![],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Decimal::ZERO,
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
            name: "responses".into(),
            base_url: upstream.base_url(),
            enabled: true,
            supports_websocket: controls.channel_supported,
            supports_standalone_web_search: false,
            auto_disabled: false,
            auto_disable_allowed: false,
            weight: 1,
            billing_multiplier: Decimal::ONE,
            proxy_id,
            config_template_id: None,
            override_document: json!({
                "version": 1,
                "api_format": "open_ai_responses",
                "request_headers": {
                    // These names are also present on the downstream handshake.
                    // Re-adding them here verifies post-allowlist cleanup.
                    "set": {
                        "cf-connecting-ip": "192.0.2.2",
                        "forwarded": "for=192.0.2.2;proto=https",
                        "x-forwarded-for": "192.0.2.2",
                        "x-gateway-transform": "enabled"
                    }
                },
                "request_json": [{
                    "op": "add",
                    "path": "/metadata",
                    "value": {"gateway": true}
                }],
                "sse": [{
                    "event": "response.completed",
                    "json": [{
                        "op": "add",
                        "path": "/gateway_patched",
                        "value": true
                    }]
                }]
            }),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "bearer".into(),
            upstream_auth_header_name: None,
            upstream_api_key: Some(UPSTREAM_KEY.into()),
            available_models: vec![UPSTREAM_MODEL.into()],
            test_model: None,
        }],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: CLIENT_MODEL.into(),
            api_format: "open_ai_responses".into(),
            upstream_model_id: Uuid::new_v4(),
            upstream_model_enabled: true,
            upstream_model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Decimal::ONE,
            cached_input_unit_price: Decimal::new(5, 1),
            cache_write_unit_price: Decimal::new(25, 2),
            output_unit_price: Decimal::from(2_i64),
            advanced_billing: json!({
                "long_context_tiers": [],
                "request_multipliers": [],
            }),
            upstream_model: UPSTREAM_MODEL.into(),
            channel_group_ids: vec![],
            channel_ids: vec![channel_id],
            enabled: true,
        }],
        proxies: outbound_proxy.into_iter().collect(),
        templates: vec![],
    };
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::new_with_websocket(
                UpstreamTimeoutDefaults::new(
                    Duration::from_secs(1),
                    Duration::from_secs(2),
                    Duration::from_secs(2),
                ),
                PassiveHealthSettings::default(),
                ResponsesWebSocketSettings::new(
                    controls.system_enabled,
                    controls.max_idle_connections,
                    Duration::from_secs(5 * 60),
                    Duration::from_secs(55 * 60),
                ),
            ),
        )
        .unwrap(),
    ));
    let registry = Arc::new(UpstreamClientRegistry::new());
    let logs = RecordingRequestLogSink::default();
    let proxy = ProxyService::with_dependencies_and_registry(
        runtime,
        1_048_576,
        Arc::clone(&registry),
        Arc::new(logs.clone()),
        RoutingRuntime::new(PassiveHealthPolicy::default()),
        AdmissionRuntime::new(),
    )
    .unwrap();
    let server = start_gateway(http::router(proxy.clone())).await;
    GatewayHarness {
        server,
        proxy,
        logs,
    }
}

fn websocket_request(
    gateway: SocketAddr,
    client_key: &str,
    session: &str,
) -> tokio_tungstenite::tungstenite::handshake::client::Request {
    let mut request = format!("ws://{gateway}/v1/responses")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {client_key}")).unwrap(),
    );
    request
        .headers_mut()
        .insert("x-session-id", HeaderValue::from_str(session).unwrap());
    request
}

async fn response_create(
    websocket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    previous_response_id: Option<&str>,
) -> Vec<Value> {
    response_create_with_residual(websocket, previous_response_id, false).await
}

async fn response_create_with_residual(
    websocket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    previous_response_id: Option<&str>,
    force_residual: bool,
) -> Vec<Value> {
    let mut body = json!({
        "type": "response.create",
        "model": CLIENT_MODEL,
        "input": [{"role": "user", "content": "hello"}],
        "reasoning": {"effort": "high"},
        "service_tier": "priority",
    });
    if let Some(previous_response_id) = previous_response_id {
        body["previous_response_id"] = json!(previous_response_id);
    }
    if force_residual {
        body["client_metadata"] = json!({"force_residual": true});
    }
    websocket
        .send(Message::Text(body.to_string().into()))
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let message = timeout(Duration::from_secs(2), websocket.next())
            .await
            .expect("gateway websocket response timed out")
            .expect("gateway websocket closed before terminal event")
            .expect("gateway websocket response failed");
        let Message::Text(text) = message else {
            continue;
        };
        let event: Value = serde_json::from_str(&text).unwrap();
        let terminal = matches!(
            event.get("type").and_then(Value::as_str),
            Some("response.completed" | "response.failed" | "error")
        );
        events.push(event);
        if terminal {
            return events;
        }
    }
}

fn completed_response_id(events: &[Value]) -> &str {
    events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .and_then(|event| event["response"]["id"].as_str())
        .expect("missing response.completed id")
}

async fn close_and_wait(
    mut websocket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    websocket.close(None).await.unwrap();
    let _ = timeout(Duration::from_secs(1), websocket.next()).await;
}

#[tokio::test]
async fn responses_websocket_forwards_transforms_reuses_connection_and_logs_requests() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness(&upstream).await;

    let mut first_request = websocket_request(gateway.server.address, CLIENT_KEY, "session-a");
    first_request.headers_mut().insert(
        "forwarded",
        HeaderValue::from_static("for=192.0.2.1;proto=https"),
    );
    first_request
        .headers_mut()
        .insert("x-forwarded-for", HeaderValue::from_static("192.0.2.1"));
    first_request
        .headers_mut()
        .insert("cf-connecting-ip", HeaderValue::from_static("192.0.2.1"));

    let (mut first, response) = connect_async(first_request).await.unwrap();
    assert_eq!(response.status(), 101);
    let first_events = response_create(&mut first, None).await;
    assert_eq!(first_events[0]["type"], "response.created");
    assert_eq!(
        first_events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .unwrap()["gateway_patched"],
        true
    );
    let first_response_id = completed_response_id(&first_events).to_owned();
    let second_events = response_create(&mut first, Some(&first_response_id)).await;
    assert_eq!(completed_response_id(&second_events), "resp-2");
    let second_response_id = completed_response_id(&second_events).to_owned();
    close_and_wait(first).await;

    let (mut reconnected, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "session-a",
    ))
    .await
    .unwrap();
    let reconnected_events = response_create(&mut reconnected, Some(&second_response_id)).await;
    assert_eq!(
        completed_response_id(&reconnected_events),
        "resp-3",
        "connection-local previous_response_id should survive downstream reconnect"
    );
    close_and_wait(reconnected).await;

    let (mut isolated, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "session-b",
    ))
    .await
    .unwrap();
    let isolated_events = response_create(&mut isolated, None).await;
    assert_eq!(completed_response_id(&isolated_events), "resp-4");
    close_and_wait(isolated).await;

    let handshakes = upstream.handshakes();
    assert_eq!(
        handshakes.len(),
        2,
        "same session should reuse one upstream socket while a different session is isolated"
    );
    assert_eq!(handshakes[0].uri, "/v1/responses");
    assert_eq!(
        handshakes[0].headers.get(AUTHORIZATION).unwrap(),
        "Bearer upstream-key"
    );
    assert_eq!(
        handshakes[0].headers.get("openai-beta").unwrap(),
        OPENAI_BETA
    );
    assert!(
        handshakes[0]
            .headers
            .get("sec-websocket-extensions")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .is_some_and(|extension| extension.trim() == "permessage-deflate")
    );
    assert_eq!(
        handshakes[0].headers.get("x-gateway-transform").unwrap(),
        "enabled"
    );
    assert_eq!(
        handshakes[0].headers.get("x-session-id").unwrap(),
        "session-a"
    );
    for name in ["forwarded", "x-forwarded-for", "cf-connecting-ip"] {
        assert!(
            handshakes[0].headers.get(name).is_none(),
            "{name} was forwarded"
        );
    }

    let requests = upstream.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].connection, 0);
    assert_eq!(requests[1].connection, 0);
    assert_eq!(requests[2].connection, 0);
    assert_eq!(requests[3].connection, 1);
    assert_eq!(requests[0].body["model"], UPSTREAM_MODEL);
    assert_eq!(requests[0].body["metadata"]["gateway"], true);
    assert_eq!(requests[1].body["previous_response_id"], first_response_id);
    assert_eq!(requests[2].body["previous_response_id"], second_response_id);

    let logs = gateway.logs.events();
    assert_eq!(logs.len(), 4);
    for event in logs {
        assert_eq!(event.response_status_code, Some(200));
        assert_eq!(event.request_protocol, RequestProtocol::WebSocket);
        assert_eq!(event.reasoning_effort.as_deref(), Some("high"));
        assert!(event.fast_mode);
        assert!(event.streamed);
        let usage = event.billing.unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.cached_input_tokens, 1);
        assert_eq!(usage.output_tokens, 2);
        assert_eq!(usage.reasoning_tokens, 1);
    }

    let metrics = SystemMetricsService::new(
        PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/unused")
            .unwrap(),
        1,
    )
    .with_websocket_proxy(gateway.proxy.clone())
    .snapshot()
    .await;
    assert!(metrics.websocket.enabled);
    assert_eq!(metrics.websocket.active_downstream_sessions, 0);
    assert_eq!(metrics.websocket.idle_upstream_connections, 2);
    assert_eq!(metrics.websocket.leased_upstream_connections, 0);
    assert_eq!(metrics.websocket.pool_hits_total, 2);
    assert_eq!(metrics.websocket.pool_misses_total, 2);
}

#[tokio::test]
async fn responses_websocket_rejects_an_invalid_gateway_api_key_during_upgrade() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness(&upstream).await;

    let error = connect_async(websocket_request(
        gateway.server.address,
        "invalid-key",
        "session-a",
    ))
    .await
    .expect_err("invalid gateway key should reject the websocket upgrade");
    let WebSocketError::Http(response) = error else {
        panic!("expected an HTTP websocket handshake error");
    };
    assert_eq!(response.status(), 401);
    assert!(upstream.handshakes().is_empty());
}

#[tokio::test]
async fn responses_websocket_rejects_unknown_client_body_fields_before_upstream_contact() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness(&upstream).await;
    let (mut websocket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "unknown-field-session",
    ))
    .await
    .unwrap();

    websocket
        .send(Message::Text(
            json!({
                "type": "response.create",
                "model": CLIENT_MODEL,
                "input": [],
                "future_field": true
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();
    let message = timeout(Duration::from_secs(2), websocket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(text) = message else {
        panic!("expected a JSON WebSocket error");
    };
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["error"]["code"], "request_body_field_unsupported");
    assert!(upstream.handshakes().is_empty());
    assert!(upstream.requests().is_empty());
}

#[tokio::test]
async fn responses_websocket_requires_system_and_user_opt_in() {
    for controls in [
        WebSocketControls {
            system_enabled: false,
            ..WebSocketControls::default()
        },
        WebSocketControls {
            user_enabled: false,
            ..WebSocketControls::default()
        },
    ] {
        let upstream = start_mock_upstream().await;
        let gateway = gateway_harness_with_controls(&upstream, None, controls).await;
        let error = connect_async(websocket_request(
            gateway.server.address,
            CLIENT_KEY,
            "disabled-session",
        ))
        .await
        .expect_err("disabled WebSocket upgrade must be rejected");
        let WebSocketError::Http(response) = error else {
            panic!("expected HTTP upgrade rejection, got {error:?}");
        };
        assert_eq!(response.status(), 403);
        assert!(upstream.handshakes().is_empty());
    }
}

#[tokio::test]
async fn responses_websocket_excludes_channels_without_websocket_support() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness_with_controls(
        &upstream,
        None,
        WebSocketControls {
            channel_supported: false,
            ..WebSocketControls::default()
        },
    )
    .await;
    let (mut websocket, response) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "unsupported-channel",
    ))
    .await
    .unwrap();
    assert_eq!(response.status(), 101);

    let events = response_create(&mut websocket, None).await;
    let terminal = events.last().expect("terminal error event");
    assert_eq!(terminal["type"], "error");
    assert_eq!(terminal["status"], 503);
    assert_eq!(terminal["error"]["code"], "no_healthy_channel");
    assert!(upstream.handshakes().is_empty());
}

#[tokio::test]
async fn responses_websocket_zero_idle_capacity_disables_connection_reuse() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness_with_controls(
        &upstream,
        None,
        WebSocketControls {
            max_idle_connections: 0,
            ..WebSocketControls::default()
        },
    )
    .await;

    for _ in 0..2 {
        let (mut websocket, _) = connect_async(websocket_request(
            gateway.server.address,
            CLIENT_KEY,
            "no-pool-session",
        ))
        .await
        .unwrap();
        response_create(&mut websocket, None).await;
        close_and_wait(websocket).await;
    }

    assert_eq!(upstream.handshakes().len(), 2);
    let metrics = SystemMetricsService::new(
        PgPoolOptions::new()
            .connect_lazy("postgres://postgres@localhost/unused")
            .unwrap(),
        1,
    )
    .with_websocket_proxy(gateway.proxy.clone())
    .snapshot()
    .await;
    assert_eq!(metrics.websocket.pool_capacity, 0);
    assert_eq!(metrics.websocket.idle_upstream_connections, 0);
    assert_eq!(metrics.websocket.pool_hits_total, 0);
    assert_eq!(metrics.websocket.pool_misses_total, 2);
    assert_eq!(metrics.websocket.pool_discarded_total, 2);
}

#[tokio::test]
async fn responses_websocket_shutdown_closes_idle_sessions_and_rejects_new_upgrades() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness(&upstream).await;
    let (mut websocket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "shutdown-session",
    ))
    .await
    .unwrap();
    assert_eq!(gateway.proxy.active_websocket_sessions(), 1);

    gateway.proxy.begin_websocket_shutdown();
    let close = timeout(Duration::from_secs(1), websocket.next())
        .await
        .expect("idle websocket was not closed during shutdown")
        .expect("idle websocket ended without a close frame")
        .expect("idle websocket close failed");
    assert!(matches!(close, Message::Close(_)));
    timeout(
        Duration::from_secs(1),
        gateway.proxy.wait_for_websocket_shutdown(),
    )
    .await
    .expect("websocket lifecycle did not drain");

    let error = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "late-session",
    ))
    .await
    .expect_err("new websocket upgrades must be rejected during shutdown");
    let WebSocketError::Http(response) = error else {
        panic!("expected an HTTP websocket handshake error");
    };
    assert_eq!(response.status(), 503);
    assert!(upstream.handshakes().is_empty());
}

#[tokio::test]
async fn responses_websocket_pool_discards_connections_with_residual_events() {
    let upstream = start_mock_upstream().await;
    let gateway = gateway_harness(&upstream).await;
    let (mut websocket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "residual-session",
    ))
    .await
    .unwrap();

    let first = response_create_with_residual(&mut websocket, None, true).await;
    assert_eq!(completed_response_id(&first), "resp-1");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = response_create(&mut websocket, None).await;
    assert_eq!(completed_response_id(&second), "resp-2");
    close_and_wait(websocket).await;

    let requests = upstream.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].connection, 0);
    assert_eq!(
        requests[1].connection, 1,
        "a connection with an event queued after response.completed must not be reused"
    );
}

#[tokio::test]
async fn responses_websocket_uses_authenticated_http_connect_proxy() {
    let upstream = start_mock_upstream().await;
    let outbound_proxy = start_http_connect_proxy().await;
    let gateway = gateway_harness_with_proxy(
        &upstream,
        Some(ProxyRecord {
            id: Uuid::new_v4(),
            name: "websocket-proxy".into(),
            proxy_url: format!("http://{}", outbound_proxy.address),
            username: Some("proxy-user".into()),
            password: Some("proxy-pass".into()),
            no_proxy_hosts: vec![],
            enabled: true,
        }),
    )
    .await;

    let (mut websocket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "proxy-session",
    ))
    .await
    .unwrap();
    let events = response_create(&mut websocket, None).await;
    assert_eq!(completed_response_id(&events), "resp-1");
    close_and_wait(websocket).await;

    let request = outbound_proxy
        .request
        .lock()
        .unwrap()
        .clone()
        .expect("proxy should receive CONNECT");
    assert!(request.starts_with(&format!("CONNECT {} HTTP/1.1\r\n", upstream.address)));
    assert!(request.contains("Proxy-Authorization: Basic cHJveHktdXNlcjpwcm94eS1wYXNz\r\n"));
}

#[tokio::test]
async fn responses_websocket_uses_authenticated_socks5h_proxy() {
    let upstream = start_mock_upstream().await;
    let outbound_proxy = start_socks5_proxy().await;
    let gateway = gateway_harness_with_proxy(
        &upstream,
        Some(ProxyRecord {
            id: Uuid::new_v4(),
            name: "websocket-socks".into(),
            proxy_url: format!("socks5h://{}", outbound_proxy.address),
            username: Some("socks-user".into()),
            password: Some("socks-pass".into()),
            no_proxy_hosts: vec![],
            enabled: true,
        }),
    )
    .await;

    let (mut websocket, _) = connect_async(websocket_request(
        gateway.server.address,
        CLIENT_KEY,
        "socks-session",
    ))
    .await
    .unwrap();
    let events = response_create(&mut websocket, None).await;
    assert_eq!(completed_response_id(&events), "resp-1");
    close_and_wait(websocket).await;

    assert_eq!(
        outbound_proxy.credentials.lock().unwrap().clone(),
        Some(("socks-user".into(), "socks-pass".into()))
    );
    assert_eq!(
        outbound_proxy.target.lock().unwrap().clone(),
        Some(("127.0.0.1".into(), upstream.address.port()))
    );
}
