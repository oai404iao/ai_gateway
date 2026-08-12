use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{
        ApiFormat, AutomaticDisableSettings, PassiveHealthSettings, ScheduledTestingSettings,
        SessionAffinityKeySource, SessionAffinityRule, SessionAffinitySettings,
        SystemRuntimeSettings, UpstreamTimeoutDefaults,
    },
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        ModelRuleRecord, ProxyRecord,
    },
    runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane_with_system_settings},
};
use async_compression::tokio::{
    bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder},
    write::{BrotliEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder},
};
use axum::{
    Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{any, post},
};
use futures_util::StreamExt;
use regex::Regex;
use reqwest::header::HeaderName;
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "client-key";
const CHAT_ONLY_KEY: &str = "chat-only-key";
const MODELS_READ_ONLY_KEY: &str = "models-read-only-key";
const NO_REACHABLE_MODELS_KEY: &str = "no-reachable-models-key";
const UPSTREAM_KEY: &str = "upstream-key";
const UPSTREAM_ACCEPT_ENCODING: &str = "gzip, deflate, br, zstd";
const FORWARDING_METADATA_HEADERS: &[&str] = &[
    "forwarded",
    "via",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
    "x-client-ip",
    "x-original-forwarded-for",
    "true-client-ip",
    "cf-connecting-ip",
    "cf-connecting-ipv6",
    "cf-pseudo-ipv4",
    "cf-ipcountry",
    "cf-ray",
    "cf-visitor",
];

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

#[derive(Clone, Copy, Debug)]
enum TestContentCoding {
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

impl TestContentCoding {
    const ALL: [Self; 4] = [Self::Gzip, Self::Deflate, Self::Brotli, Self::Zstd];

    const fn name(self) -> &'static str {
        match self {
            Self::Gzip => "gzip",
            Self::Deflate => "deflate",
            Self::Brotli => "br",
            Self::Zstd => "zstd",
        }
    }
}

#[derive(Clone)]
struct EncodedUpstream {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    content_encoding: &'static str,
    body: Arc<Vec<u8>>,
}

struct EncodedHarness {
    _upstream: TestServer,
    app: Router,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    logs: RecordingRequestLogSink,
}

async fn capture_encoded_upstream(
    State(upstream): State<EncodedUpstream>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    upstream.requests.lock().unwrap().push(CapturedRequest {
        headers: parts.headers,
        body,
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("content-encoding", upstream.content_encoding)
        .header("content-length", upstream.body.len())
        .header("accept-ranges", "bytes")
        .header("etag", "\"encoded-etag\"")
        .header("content-md5", "encoded-md5")
        .header("digest", "sha-256=:ZW5jb2RlZA==:")
        .header("content-digest", "sha-256=:ZW5jb2RlZA==:")
        .header("repr-digest", "sha-256=:ZW5jb2RlZA==:")
        .body(Body::from(upstream.body.as_ref().clone()))
        .unwrap()
}

async fn encoded_harness(content_encoding: &'static str, body: Vec<u8>) -> EncodedHarness {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/images/generations", post(capture_encoded_upstream))
            .with_state(EncodedUpstream {
                requests: Arc::clone(&requests),
                content_encoding,
                body: Arc::new(body),
            }),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let configured = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
    );
    EncodedHarness {
        _upstream: upstream,
        app: http::router(configured.proxy),
        requests,
        logs,
    }
}

async fn encode_test_body(coding: TestContentCoding, body: &[u8]) -> Vec<u8> {
    macro_rules! encode {
        ($encoder:ident, $input:expr) => {{
            let mut encoder = $encoder::new(Vec::new());
            encoder.write_all($input).await.unwrap();
            encoder.shutdown().await.unwrap();
            encoder.into_inner()
        }};
    }

    match coding {
        TestContentCoding::Gzip => encode!(GzipEncoder, body),
        TestContentCoding::Deflate => encode!(ZlibEncoder, body),
        TestContentCoding::Brotli => encode!(BrotliEncoder, body),
        TestContentCoding::Zstd => {
            let midpoint = body.len() / 2;
            let mut encoded = encode!(ZstdEncoder, &body[..midpoint]);
            encoded.extend(encode!(ZstdEncoder, &body[midpoint..]));
            encoded
        }
    }
}

async fn decode_test_body(coding: TestContentCoding, body: &[u8]) -> Vec<u8> {
    macro_rules! decode {
        ($decoder:ident) => {{
            let mut decoder = $decoder::new(BufReader::new(body));
            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded).await.unwrap();
            decoded
        }};
    }

    match coding {
        TestContentCoding::Gzip => decode!(GzipDecoder),
        TestContentCoding::Deflate => decode!(ZlibDecoder),
        TestContentCoding::Brotli => decode!(BrotliDecoder),
        TestContentCoding::Zstd => decode!(ZstdDecoder),
    }
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

async fn delayed_upstream_response(
    State(upstream): State<MockUpstream>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    upstream.requests.lock().unwrap().push(CapturedRequest {
        headers: parts.headers,
        body,
    });
    tokio::time::sleep(Duration::from_millis(2_100)).await;
    Response::builder()
        .status(upstream.status)
        .body(Body::from(upstream.body))
        .unwrap()
}

async fn hanging_upstream(State(upstream): State<MockUpstream>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    upstream.requests.lock().unwrap().push(CapturedRequest {
        headers: parts.headers,
        body,
    });
    Response::builder()
        .status(upstream.status)
        .body(Body::from_stream(futures_util::stream::pending::<
            Result<axum::body::Bytes, std::io::Error>,
        >()))
        .unwrap()
}

fn terminal_sse_then_hangs(bytes: &'static [u8]) -> Response {
    let first = futures_util::stream::once(async move {
        Ok::<Bytes, std::io::Error>(Bytes::from_static(bytes))
    });
    let pending = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(first.chain(pending)))
        .unwrap()
}

async fn chat_done_then_hangs() -> Response {
    terminal_sse_then_hangs(b"data: [DONE]\n\n")
}

async fn responses_completed_then_hangs() -> Response {
    terminal_sse_then_hangs(
        b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":3,\"input_tokens_details\":{\"cached_tokens\":2},\"output_tokens_details\":{\"reasoning_tokens\":1}}}}\n\n",
    )
}

async fn chat_error_then_hangs() -> Response {
    terminal_sse_then_hangs(
        b"data: {\"error\":{\"message\":\"sensitive upstream detail\",\"type\":\"server_error\",\"code\":\"provider_error\"}}\n\n",
    )
}

async fn responses_error_then_hangs() -> Response {
    terminal_sse_then_hangs(
        b"event: error\ndata: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"sensitive upstream detail\",\"param\":null,\"sequence_number\":3}\n\n",
    )
}

async fn first_response_header_hangs(
    State(upstream): State<MockUpstream>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    let first = {
        let mut requests = upstream.requests.lock().unwrap();
        let first = requests.is_empty();
        requests.push(CapturedRequest {
            headers: parts.headers,
            body,
        });
        first
    };
    if first {
        std::future::pending::<Response>().await
    } else {
        Response::builder()
            .status(upstream.status)
            .body(Body::from(upstream.body))
            .unwrap()
    }
}

async fn first_stream_hangs_then_succeeds(
    State(upstream): State<MockUpstream>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, usize::MAX).await.unwrap().to_vec();
    let first = {
        let mut requests = upstream.requests.lock().unwrap();
        let first = requests.is_empty();
        requests.push(CapturedRequest {
            headers: parts.headers,
            body,
        });
        first
    };
    if first {
        Response::builder()
            .status(upstream.status)
            .body(Body::from_stream(futures_util::stream::pending::<
                Result<axum::body::Bytes, std::io::Error>,
            >()))
            .unwrap()
    } else {
        Response::builder()
            .status(upstream.status)
            .body(Body::from(upstream.body))
            .unwrap()
    }
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

#[derive(Clone, Debug)]
struct ProxyCapture {
    target: String,
    proxy_authorization: Option<String>,
}

#[derive(Clone)]
struct ForwardingProxyState {
    captures: Arc<Mutex<Vec<ProxyCapture>>>,
}

async fn forward_http_proxy(
    State(state): State<ForwardingProxyState>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    state.captures.lock().unwrap().push(ProxyCapture {
        target: parts.uri.to_string(),
        proxy_authorization: parts
            .headers
            .get("proxy-authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
    let headers = parts
        .headers
        .iter()
        .filter(|(name, _)| *name != "proxy-authorization" && *name != "proxy-connection")
        .fold(HeaderMap::new(), |mut headers, (name, value)| {
            headers.append(name, value.clone());
            headers
        });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .request(parts.method, parts.uri.to_string())
        .headers(headers)
        .body(to_bytes(body, usize::MAX).await.unwrap())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    Response::builder()
        .status(status)
        .body(Body::from(bytes))
        .unwrap()
}

async fn start_http_proxy() -> (TestServer, Arc<Mutex<Vec<ProxyCapture>>>) {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let server = start_server(Router::new().fallback(any(forward_http_proxy)).with_state(
        ForwardingProxyState {
            captures: Arc::clone(&captures),
        },
    ))
    .await;
    (server, captures)
}

struct SocksProxy {
    address: SocketAddr,
    task: JoinHandle<()>,
    credentials: Arc<Mutex<Option<(String, String)>>>,
}

impl Drop for SocksProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_socks5_proxy() -> SocksProxy {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let credentials = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&credentials);
    let task = tokio::spawn(async move {
        let (mut client, _) = listener.accept().await.unwrap();
        let mut greeting = [0; 2];
        client.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting[0], 5);
        let mut methods = vec![0; usize::from(greeting[1])];
        client.read_exact(&mut methods).await.unwrap();
        assert!(methods.contains(&2));
        client.write_all(&[5, 2]).await.unwrap();

        let mut auth_header = [0; 2];
        client.read_exact(&mut auth_header).await.unwrap();
        assert_eq!(auth_header[0], 1);
        let mut username = vec![0; usize::from(auth_header[1])];
        client.read_exact(&mut username).await.unwrap();
        let mut password_length = [0; 1];
        client.read_exact(&mut password_length).await.unwrap();
        let mut password = vec![0; usize::from(password_length[0])];
        client.read_exact(&mut password).await.unwrap();
        *captured.lock().unwrap() = Some((
            String::from_utf8(username).unwrap(),
            String::from_utf8(password).unwrap(),
        ));
        client.write_all(&[1, 0]).await.unwrap();

        let mut request_header = [0; 4];
        client.read_exact(&mut request_header).await.unwrap();
        assert_eq!(&request_header[..3], &[5, 1, 0]);
        let host = match request_header[3] {
            1 => {
                let mut address = [0; 4];
                client.read_exact(&mut address).await.unwrap();
                std::net::Ipv4Addr::from(address).to_string()
            }
            3 => {
                let mut length = [0; 1];
                client.read_exact(&mut length).await.unwrap();
                let mut name = vec![0; usize::from(length[0])];
                client.read_exact(&mut name).await.unwrap();
                String::from_utf8(name).unwrap()
            }
            4 => {
                let mut address = [0; 16];
                client.read_exact(&mut address).await.unwrap();
                std::net::Ipv6Addr::from(address).to_string()
            }
            atyp => panic!("unexpected SOCKS address type {atyp}"),
        };
        let mut port = [0; 2];
        client.read_exact(&mut port).await.unwrap();
        let mut upstream = TcpStream::connect((host.as_str(), u16::from_be_bytes(port)))
            .await
            .unwrap();
        client
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
            .unwrap();
    });
    SocksProxy {
        address,
        task,
        credentials,
    }
}

fn proxy_record(
    proxy_url: String,
    username: Option<&str>,
    password: Option<&str>,
    no_proxy_hosts: Vec<&str>,
) -> ProxyRecord {
    ProxyRecord {
        id: Uuid::new_v4(),
        name: "test-proxy".into(),
        proxy_url,
        username: username.map(str::to_owned),
        password: password.map(str::to_owned),
        no_proxy_hosts: no_proxy_hosts.into_iter().map(str::to_owned).collect(),
        enabled: true,
    }
}

struct Harness {
    gateway: TestServer,
    _upstream: TestServer,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    logs: RecordingRequestLogSink,
}

impl Harness {
    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.gateway.address, path)
    }

    fn upstream_requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn logs(&self) -> Vec<ai_gateway::domain::RequestLogEvent> {
        self.logs.events()
    }
}

async fn harness(status: StatusCode, body: impl Into<Vec<u8>>) -> Harness {
    harness_with_policy(status, body, None, None, None, Default::default()).await
}

async fn harness_with_policy(
    status: StatusCode,
    body: impl Into<Vec<u8>>,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    quota_used_amount: rust_decimal::Decimal,
) -> Harness {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .route("/v1/responses", post(capture_upstream))
            .route("/v1/alpha/search", post(capture_upstream))
            .route("/v1/images/generations", post(capture_upstream))
            .route("/v1/images/edits", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status,
                body: body.into(),
            }),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let proxy = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
        quota_used_amount,
    );
    let gateway = start_server(http::router(proxy.proxy)).await;

    Harness {
        gateway,
        _upstream: upstream,
        requests,
        logs,
    }
}

async fn harness_with_transforms(transforms: TransformDocuments) -> Harness {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .route("/v1/responses", post(capture_upstream))
            .route("/v1/alpha/search", post(capture_upstream))
            .route("/v1/images/generations", post(capture_upstream))
            .route("/v1/images/edits", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"ok".to_vec(),
            }),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let configured = configured_proxy_with_policy_and_transforms(
        &format!("http://{}", upstream.address),
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        transforms,
        OutboundTestPolicy::default(),
        true,
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    Harness {
        gateway,
        _upstream: upstream,
        requests,
        logs,
    }
}

struct ConfiguredProxy {
    proxy: ProxyService,
    runtime: Arc<RuntimeConfig>,
    client_key_id: Uuid,
}

struct TransformDocuments {
    template: Option<Value>,
    chat_override: Value,
    responses_override: Value,
    responses_search_supported: bool,
    responses_request_compression: &'static str,
    images_override: Value,
    upstream_auth_kind: &'static str,
    upstream_auth_header_name: Option<&'static str>,
    upstream_api_key: Option<&'static str>,
    filter_fast_mode: bool,
}

#[derive(Default)]
struct OutboundTestPolicy {
    proxy: Option<ProxyRecord>,
    connect_timeout_ms: Option<i32>,
    response_header_timeout_ms: Option<i32>,
    stream_idle_timeout_ms: Option<i32>,
}

impl Default for TransformDocuments {
    fn default() -> Self {
        Self {
            template: None,
            chat_override: serde_json::json!({}),
            responses_override: serde_json::json!({}),
            responses_search_supported: true,
            responses_request_compression: "default",
            images_override: serde_json::json!({}),
            upstream_auth_kind: "bearer",
            upstream_auth_header_name: None,
            upstream_api_key: Some(UPSTREAM_KEY),
            filter_fast_mode: false,
        }
    }
}

fn proxy_service_with_policy(
    upstream_url: &str,
    logs: RecordingRequestLogSink,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    quota_used_amount: rust_decimal::Decimal,
) -> ConfiguredProxy {
    configured_proxy_with_policy(
        upstream_url,
        logs,
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
        quota_used_amount,
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn configured_proxy_with_policy(
    upstream_url: &str,
    logs: RecordingRequestLogSink,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    quota_used_amount: rust_decimal::Decimal,
    client_key_id: Option<Uuid>,
    upstream_config: UpstreamConfig,
) -> ConfiguredProxy {
    configured_proxy_with_policy_and_transforms(
        upstream_url,
        logs,
        requests_per_minute,
        max_concurrent_requests,
        quota_limit_amount,
        quota_used_amount,
        client_key_id,
        upstream_config,
        TransformDocuments::default(),
        OutboundTestPolicy::default(),
        true,
    )
}

fn configured_proxy_with_outbound_policy(
    upstream_url: &str,
    logs: RecordingRequestLogSink,
    upstream_config: UpstreamConfig,
    outbound: OutboundTestPolicy,
) -> ConfiguredProxy {
    configured_proxy_with_policy_and_transforms(
        upstream_url,
        logs,
        None,
        None,
        None,
        Default::default(),
        None,
        upstream_config,
        TransformDocuments::default(),
        outbound,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn configured_proxy_with_policy_and_transforms(
    upstream_url: &str,
    logs: RecordingRequestLogSink,
    requests_per_minute: Option<i32>,
    max_concurrent_requests: Option<i32>,
    quota_limit_amount: Option<rust_decimal::Decimal>,
    quota_used_amount: rust_decimal::Decimal,
    client_key_id: Option<Uuid>,
    upstream_config: UpstreamConfig,
    transforms: TransformDocuments,
    outbound: OutboundTestPolicy,
    chat_channel_enabled: bool,
) -> ConfiguredProxy {
    let chat_group = Uuid::new_v4();
    let responses_group = Uuid::new_v4();
    let images_group = Uuid::new_v4();
    let empty_chat_group = Uuid::new_v4();
    let chat = Uuid::new_v4();
    let responses = Uuid::new_v4();
    let images = Uuid::new_v4();
    let images_alt = Uuid::new_v4();
    let group = |id: Uuid, api_format: &str| ChannelGroupRecord {
        id,
        name: id.to_string(),
        api_format: api_format.to_owned(),
        connector_kind: "openai_compatible".into(),
        request_compression: if api_format == "open_ai_responses" {
            transforms.responses_request_compression.into()
        } else {
            "default".into()
        },
        priority: 0,
        selection_strategy: "weighted_random".into(),
        enabled: true,
    };
    let template_id = transforms.template.as_ref().map(|_| Uuid::new_v4());
    let channel = |id: Uuid, group_id: Uuid, api_format: &str| ChannelRecord {
        id,
        channel_group_id: group_id,
        api_format: api_format.into(),
        name: id.to_string(),
        base_url: upstream_url.into(),
        enabled: true,
        supports_websocket: false,
        supports_standalone_web_search: api_format == "open_ai_responses"
            && transforms.responses_search_supported,
        auto_disabled: false,
        auto_disable_allowed: false,
        weight: 1,
        billing_multiplier: rust_decimal::Decimal::ONE,
        proxy_id: None,
        config_template_id: (api_format == "open_ai_chat_completions")
            .then_some(template_id)
            .flatten(),
        override_document: match api_format {
            "open_ai_chat_completions" => transforms.chat_override.clone(),
            "open_ai_responses" => transforms.responses_override.clone(),
            "open_ai_images" => transforms.images_override.clone(),
            _ => serde_json::json!({}),
        },
        connect_timeout_ms: None,
        response_header_timeout_ms: None,
        stream_idle_timeout_ms: None,
        upstream_auth_kind: transforms.upstream_auth_kind.into(),
        upstream_auth_header_name: transforms.upstream_auth_header_name.map(str::to_owned),
        upstream_api_key: transforms.upstream_api_key.map(str::to_owned),
        available_models: match api_format {
            "open_ai_chat_completions" => vec![
                "same-model".into(),
                "upstream-alias-model".into(),
                "chat-only-model".into(),
            ],
            "open_ai_responses" => vec!["responses-model".into()],
            "open_ai_images" => vec!["gpt-image-2".into()],
            _ => vec![],
        },
        test_model: None,
    };
    let key = |secret: &str, formats: Vec<&str>, groups: Vec<Uuid>, permissions: Vec<&str>| {
        ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            user_websocket_enabled: false,
            user_filter_fast_mode: transforms.filter_fast_mode,
            secret_value: secret.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: formats.into_iter().map(str::to_owned).collect(),
            permissions: permissions.into_iter().map(str::to_owned).collect(),
            allowed_group_ids: groups,
            allowed_channel_ids: vec![],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }
    };
    let rule = |model: &str, upstream: &str, format: &str, channel_id: Uuid| ModelRuleRecord {
        id: Uuid::new_v4(),
        client_model: model.into(),
        api_format: format.into(),
        upstream_model_id: Uuid::new_v4(),
        upstream_model_enabled: true,
        upstream_model_currency: "USD".into(),
        price_unit_tokens: 1_000_000,
        price_effective_at: chrono::Utc::now(),
        input_unit_price: if transforms.filter_fast_mode {
            Decimal::ONE
        } else {
            Decimal::ZERO
        },
        cached_input_unit_price: if transforms.filter_fast_mode {
            Decimal::ONE
        } else {
            Decimal::ZERO
        },
        cache_write_unit_price: if transforms.filter_fast_mode {
            Decimal::ONE
        } else {
            Decimal::ZERO
        },
        output_unit_price: if transforms.filter_fast_mode {
            Decimal::ONE
        } else {
            Decimal::ZERO
        },
        advanced_billing: if transforms.filter_fast_mode {
            serde_json::json!({
                "long_context_tiers": [],
                "request_multipliers": [{
                    "json_pointer": "/service_tier",
                    "value": "priority",
                    "multiplier": "2"
                }],
            })
        } else {
            serde_json::json!({
                "long_context_tiers": [],
                "request_multipliers": [],
            })
        },
        upstream_model: upstream.into(),
        channel_group_ids: vec![],
        channel_ids: vec![channel_id],
        enabled: true,
    };
    let mut records = ControlPlaneRecords {
        api_keys: vec![
            key(
                CLIENT_KEY,
                vec![
                    "open_ai_chat_completions",
                    "open_ai_responses",
                    "open_ai_images",
                ],
                vec![chat_group, responses_group, images_group],
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
            group(images_group, "open_ai_images"),
            group(empty_chat_group, "open_ai_chat_completions"),
        ],
        channels: vec![
            channel(chat, chat_group, "open_ai_chat_completions"),
            channel(responses, responses_group, "open_ai_responses"),
            channel(images, images_group, "open_ai_images"),
            channel(images_alt, images_group, "open_ai_images"),
        ],
        models: vec![],
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
            rule(
                "search-alias",
                "responses-model",
                "open_ai_responses",
                responses,
            ),
            {
                let mut rule = rule("gpt-image-2", "gpt-image-2", "open_ai_images", images);
                rule.channel_ids.push(images_alt);
                rule
            },
            {
                let mut rule = rule("image-alias", "gpt-image-2", "open_ai_images", images);
                rule.channel_ids.push(images_alt);
                rule
            },
        ],
        proxies: vec![],
        templates: template_id
            .zip(transforms.template)
            .map(|(id, document)| {
                vec![ConfigTemplateRecord {
                    id,
                    name: "transform-template".into(),
                    description: None,
                    document,
                    enabled: true,
                }]
            })
            .unwrap_or_default(),
        mcp_servers: vec![],
    };
    let OutboundTestPolicy {
        proxy,
        connect_timeout_ms,
        response_header_timeout_ms,
        stream_idle_timeout_ms,
    } = outbound;
    records.channels[0].enabled = chat_channel_enabled;
    records.channels[0].connect_timeout_ms = connect_timeout_ms;
    records.channels[0].response_header_timeout_ms = response_header_timeout_ms;
    records.channels[0].stream_idle_timeout_ms = stream_idle_timeout_ms;
    if let Some(proxy) = proxy {
        records.channels[0].proxy_id = Some(proxy.id);
        records.proxies.push(proxy);
    }
    let client_key = records
        .api_keys
        .iter_mut()
        .find(|key| key.secret_value == CLIENT_KEY)
        .unwrap();
    if let Some(id) = client_key_id {
        client_key.id = id;
    }
    client_key.requests_per_minute = requests_per_minute;
    client_key.max_concurrent_requests = max_concurrent_requests;
    client_key.quota_limit_amount = quota_limit_amount;
    client_key.quota_used_amount = quota_used_amount;
    let client_key_id = client_key.id;
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::new(
                UpstreamTimeoutDefaults::new(
                    Duration::from_secs(upstream_config.connect_timeout_seconds),
                    Duration::from_secs(upstream_config.response_header_timeout_seconds),
                    Duration::from_secs(upstream_config.stream_idle_timeout_seconds),
                )
                .with_images_response_header(Duration::from_secs(
                    upstream_config.images_response_header_timeout_seconds,
                ))
                .with_standalone_web_search_response_header(Duration::from_secs(
                    upstream_config.standalone_web_search_response_header_timeout_seconds,
                )),
                PassiveHealthSettings::default(),
            ),
        )
        .unwrap(),
    ));
    let proxy =
        ProxyService::with_log_sink(Arc::clone(&runtime), 1_048_576, Arc::new(logs)).unwrap();
    ConfiguredProxy {
        proxy,
        runtime,
        client_key_id,
    }
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

fn with_forwarding_metadata(mut request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for name in FORWARDING_METADATA_HEADERS {
        request = request.header(*name, "discard");
    }
    request
}

fn forwarding_metadata_transform(api_format: &str) -> Value {
    let headers = FORWARDING_METADATA_HEADERS
        .iter()
        .map(|name| ((*name).to_owned(), Value::String("discard".into())))
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({
        "version": 1,
        "api_format": api_format,
        "request_headers": {"set": headers}
    })
}

fn multipart_edit_body(
    boundary: &str,
    model: &str,
    extra_fields: &[(&str, &str)],
    image: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    let text_field = |body: &mut Vec<u8>, name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    text_field(&mut body, "model", model);
    for (name, value) in extra_fields {
        text_field(&mut body, name, value);
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"image\"; filename=\"input.png\"\r\nContent-Type: image/png\r\n\r\n",
    );
    body.extend_from_slice(image);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn authorized_multipart_post(
    client: &reqwest::Client,
    url: String,
    key: &str,
    boundary: &str,
    body: Vec<u8>,
) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header("authorization", format!("Bearer {key}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
}

async fn parse_captured_edit(body: &[u8], boundary: &str) -> (String, Vec<u8>) {
    let bytes = Bytes::copy_from_slice(body);
    let stream = futures_util::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let mut multipart = multer::Multipart::new(stream, boundary.to_owned());
    let mut model = None;
    let mut image = None;
    while let Some(field) = multipart.next_field().await.unwrap() {
        match field.name() {
            Some("model") => model = Some(field.text().await.unwrap()),
            Some("image" | "image[]") => image = Some(field.bytes().await.unwrap().to_vec()),
            _ => {}
        }
    }
    (model.unwrap(), image.unwrap())
}

fn proxy_request(model: &str) -> axum::http::Request<Body> {
    axum::http::Request::post("/v1/chat/completions")
        .header("authorization", format!("Bearer {CLIENT_KEY}"))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"model": model})).unwrap(),
        ))
        .unwrap()
}

fn session_affinity_proxy(first_upstream_url: &str, second_upstream_url: &str) -> ProxyService {
    let group_id = Uuid::new_v4();
    let first_channel_id = Uuid::new_v4();
    let second_channel_id = Uuid::new_v4();
    let model_rule_id = Uuid::new_v4();
    let channel = |id: Uuid, name: &str, base_url: &str| ChannelRecord {
        id,
        channel_group_id: group_id,
        api_format: "open_ai_chat_completions".into(),
        name: name.into(),
        base_url: base_url.into(),
        enabled: true,
        supports_websocket: false,
        supports_standalone_web_search: false,
        auto_disabled: false,
        auto_disable_allowed: false,
        weight: 1,
        billing_multiplier: rust_decimal::Decimal::ONE,
        proxy_id: None,
        config_template_id: None,
        override_document: serde_json::json!({}),
        connect_timeout_ms: None,
        response_header_timeout_ms: None,
        stream_idle_timeout_ms: None,
        upstream_auth_kind: "bearer".into(),
        upstream_auth_header_name: None,
        upstream_api_key: Some(UPSTREAM_KEY.into()),
        available_models: vec!["affinity-model".into()],
        test_model: None,
    };
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            user_websocket_enabled: false,
            user_filter_fast_mode: false,
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
            name: "affinity".into(),
            api_format: "open_ai_chat_completions".into(),
            connector_kind: "openai_compatible".into(),
            request_compression: "default".into(),
            priority: 0,
            selection_strategy: "weighted_round_robin".into(),
            enabled: true,
        }],
        channels: vec![
            channel(first_channel_id, "first", first_upstream_url),
            channel(second_channel_id, "second", second_upstream_url),
        ],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: model_rule_id,
            client_model: "affinity-model".into(),
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
            upstream_model: "affinity-model".into(),
            channel_group_ids: vec![group_id],
            channel_ids: vec![],
            enabled: true,
        }],
        proxies: vec![],
        templates: vec![],
        mcp_servers: vec![],
    };
    let system_settings = SystemRuntimeSettings::new_with_all(
        UpstreamTimeoutDefaults::new(
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(2),
        ),
        ai_gateway::domain::RequestRetrySettings::default(),
        PassiveHealthSettings::default(),
        AutomaticDisableSettings::default(),
        ScheduledTestingSettings::default(),
        SessionAffinitySettings::new(
            true,
            100,
            Duration::from_secs(60),
            vec![SessionAffinityRule::new(
                Arc::from("header-session"),
                [42; 32],
                vec![ApiFormat::OpenAiChatCompletions].into(),
                vec![Regex::new("^affinity-model$").unwrap()].into(),
                vec![SessionAffinityKeySource::RequestHeader(
                    HeaderName::from_static("x-session-id"),
                )]
                .into(),
                None,
                Duration::from_secs(60),
            )]
            .into(),
        ),
    );
    ProxyService::with_log_sink(
        Arc::new(RuntimeConfig::new(
            compile_control_plane_with_system_settings(records, system_settings).unwrap(),
        )),
        1_048_576,
        Arc::new(RecordingRequestLogSink::default()),
    )
    .unwrap()
}

#[tokio::test]
async fn health_is_available_without_authentication() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = client().get(harness.url("/health")).send().await.unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn successful_session_requests_reuse_the_same_upstream_channel() {
    let first_requests = Arc::new(Mutex::new(Vec::new()));
    let second_requests = Arc::new(Mutex::new(Vec::new()));
    let first = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&first_requests),
                status: StatusCode::OK,
                body: b"first".to_vec(),
            }),
    )
    .await;
    let second = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&second_requests),
                status: StatusCode::OK,
                body: b"second".to_vec(),
            }),
    )
    .await;
    let proxy = session_affinity_proxy(
        &format!("http://{}", first.address),
        &format!("http://{}", second.address),
    );
    let gateway = start_server(http::router(proxy)).await;
    let client = client();
    let body = br#"{"model":"affinity-model","messages":[{"role":"user","content":"hello"}]}"#;

    for _ in 0..2 {
        let response = authorized_post(
            &client,
            format!("http://{}/v1/chat/completions", gateway.address),
            CLIENT_KEY,
            body.to_vec(),
        )
        .header("x-session-id", "session-1")
        .send()
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.bytes().await.unwrap();
    }

    let counts = [
        first_requests.lock().unwrap().len(),
        second_requests.lock().unwrap().len(),
    ];
    assert!(
        counts == [2, 0] || counts == [0, 2],
        "both requests should use one affinity channel, got {counts:?}"
    );
    let mut captured = first_requests
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.body.clone())
        .collect::<Vec<_>>();
    captured.extend(
        second_requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.body.clone()),
    );
    assert_eq!(captured, vec![body.to_vec(), body.to_vec()]);
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
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn unknown_model_returns_not_found_without_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"unknown-model","reasoning_effort":"high","service_tier":"priority"}"#
            .to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(harness.upstream_requests().is_empty());
    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome.as_str(), "rejected");
    assert_eq!(logs[0].response_status_code, Some(404));
    assert_eq!(logs[0].reasoning_effort.as_deref(), Some("high"));
    assert!(logs[0].fast_mode);
    assert_eq!(logs[0].error_code.as_deref(), Some("model_not_found"));
    let summary = logs[0].error_summary.as_deref().unwrap();
    assert!(summary.starts_with("The model `unknown-model` does not exist or is unavailable."));
    assert!(summary.contains("\"type\": \"invalid_request_error\""));
}

#[tokio::test]
async fn known_model_without_an_authorized_candidate_returns_not_found() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        NO_REACHABLE_MODELS_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "model_not_found");
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn configured_model_with_only_disabled_channels_returns_service_unavailable() {
    let logs = RecordingRequestLogSink::default();
    let configured = configured_proxy_with_policy_and_transforms(
        "https://disabled-channel.example.test",
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        TransformDocuments::default(),
        OutboundTestPolicy::default(),
        false,
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/chat/completions", gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "no_healthy_channel");
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].response_status_code, Some(503));
    assert_eq!(events[0].error_code.as_deref(), Some("no_healthy_channel"));
    let summary = events[0].error_summary.as_deref().unwrap();
    assert!(
        summary.starts_with("No healthy upstream channel is currently available for this model.")
    );
    assert!(summary.contains("\"code\": \"no_healthy_channel\""));
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

    let images_forbidden = authorized_post(
        &client,
        harness.url("/v1/images/generations"),
        CHAT_ONLY_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(images_forbidden.status(), StatusCode::FORBIDDEN);
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn matching_chat_model_preserves_body_and_forwards_response_safely() {
    let upstream_body = br#"{"id":"upstream-result","ok":true}"#.to_vec();
    let harness = harness(StatusCode::CREATED, upstream_body.clone()).await;
    let request_body = br#"{ "messages": [{"role":"user","content":{"nested":{"a":1}}}], "model" : "same-model", "thinking": {"type":"enabled"}, "enable_thinking": true, "reasoning_effort": "high", "service_tier": "priority", "metadata": { "key": "value" } }"#.to_vec();

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        request_body.clone(),
    )
    .header("connection", "x-internal-hop, keep-alive")
    .header("x-internal-hop", "do-not-forward")
    .header("x-request-id", "forward-me")
    .header("x-unlisted-client-header", "drop-me")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);

    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome.as_str(), "succeeded");
    assert_eq!(
        logs[0].api_operation,
        ai_gateway::domain::ApiOperation::ChatCompletions
    );
    assert_eq!(logs[0].response_status_code, Some(201));
    assert_eq!(logs[0].reasoning_effort.as_deref(), Some("high"));
    assert!(logs[0].fast_mode);

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
    assert!(request.headers.get("x-unlisted-client-header").is_none());
}

#[tokio::test]
async fn user_group_fast_filter_removes_service_tier_from_forwarding_logs_and_billing() {
    let upstream_body = br#"{
        "id":"response-id",
        "usage":{
            "input_tokens":9,
            "output_tokens":3,
            "input_tokens_details":{"cached_tokens":2},
            "output_tokens_details":{"reasoning_tokens":1}
        }
    }"#
    .to_vec();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/responses", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: upstream_body,
            }),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let configured = configured_proxy_with_policy_and_transforms(
        &format!("http://{}", upstream.address),
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        TransformDocuments {
            filter_fast_mode: true,
            ..Default::default()
        },
        OutboundTestPolicy::default(),
        true,
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/responses", gateway.address),
        CLIENT_KEY,
        br#"{"model":"responses-model","reasoning":{"effort":"high"},"service_tier":"priority"}"#
            .to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let forwarded: Value = serde_json::from_slice(&requests.lock().unwrap()[0].body).unwrap();
    assert!(forwarded.get("service_tier").is_none());
    assert_eq!(forwarded["reasoning"]["effort"], "high");

    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert!(!events[0].fast_mode);
    let billing = events[0].billing.as_ref().unwrap();
    assert_eq!(billing.price.input_unit_price, Decimal::ONE);
    assert_eq!(billing.price.output_unit_price, Decimal::ONE);
    assert_eq!(billing.cost_amount, Some(Decimal::new(12, 6)));
}

#[tokio::test]
async fn responses_accepts_zstd_request_bodies_and_forwards_identity_by_default() {
    let harness = harness(StatusCode::OK, br#"{"id":"resp_1"}"#.to_vec()).await;
    let request_body =
        br#"{ "model" : "responses-model", "input": [{"role":"user","content":"hello"}] }"#;
    let encoded_body = encode_test_body(TestContentCoding::Zstd, request_body).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        encoded_body,
    )
    .header("content-encoding", "zstd")
    .header("content-md5", "stale")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, request_body);
    assert!(requests[0].headers.get("content-encoding").is_none());
    assert!(requests[0].headers.get("content-md5").is_none());
    let expected_length = request_body.len().to_string();
    assert_eq!(
        requests[0]
            .headers
            .get("content-length")
            .and_then(|value| value.to_str().ok()),
        Some(expected_length.as_str())
    );
}

#[tokio::test]
async fn responses_channel_group_can_compress_upstream_json_with_zstd() {
    let harness = harness_with_transforms(TransformDocuments {
        responses_request_compression: "zstd",
        ..Default::default()
    })
    .await;
    let request_body = br#"{"model":"responses-model","input":"hello"}"#;

    let response = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        request_body.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("content-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("zstd")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let expected_length = requests[0].body.len().to_string();
    assert_eq!(
        requests[0]
            .headers
            .get("content-length")
            .and_then(|value| value.to_str().ok()),
        Some(expected_length.as_str())
    );
    assert_eq!(
        zstd::stream::decode_all(std::io::Cursor::new(&requests[0].body)).unwrap(),
        request_body
    );
}

#[tokio::test]
async fn zstd_request_bodies_are_rejected_outside_responses() {
    let harness = harness(StatusCode::OK, Vec::new()).await;
    let request_body = br#"{"model":"same-model","messages":[]}"#;
    let encoded_body = encode_test_body(TestContentCoding::Zstd, request_body).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        encoded_body,
    )
    .header("content-encoding", "zstd")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        body["error"]["code"],
        "request_content_encoding_unsupported"
    );
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn responses_rejects_stacked_request_content_encodings() {
    let harness = harness(StatusCode::OK, Vec::new()).await;
    let request_body = br#"{"model":"responses-model","input":"hello"}"#;
    let encoded_body = encode_test_body(TestContentCoding::Zstd, request_body).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        encoded_body,
    )
    .header("content-encoding", "identity, zstd")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        body["error"]["code"],
        "request_content_encoding_unsupported"
    );
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn responses_rejects_invalid_or_oversized_decoded_zstd_bodies() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let invalid = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        b"not-zstd".to_vec(),
    )
    .header("content-encoding", "zstd")
    .send()
    .await
    .unwrap();
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    let invalid: Value = serde_json::from_slice(&invalid.bytes().await.unwrap()).unwrap();
    assert_eq!(invalid["error"]["code"], "request_content_encoding_invalid");

    let oversized_body = serde_json::to_vec(&serde_json::json!({
        "model": "responses-model",
        "input": "x".repeat(1_048_576),
    }))
    .unwrap();
    let encoded_body = zstd::stream::encode_all(std::io::Cursor::new(oversized_body), 3).unwrap();
    let oversized = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        encoded_body,
    )
    .header("content-encoding", "zstd")
    .send()
    .await
    .unwrap();
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let oversized: Value = serde_json::from_slice(&oversized.bytes().await.unwrap()).unwrap();
    assert_eq!(oversized["error"]["code"], "request_too_large");

    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn standalone_web_search_forwards_alias_headers_opaque_results_and_logs_operation() {
    let upstream_body = br#"{
        "encrypted_output":"opaque-state",
        "output":"Search summary",
        "results":[
            {"type":"computer_initialize_state","id":"future-result","future":{"nested":true}},
            {"type":"search_query","sources":[{"url":"https://example.test","title":"Example"}]}
        ]
    }"#
    .to_vec();
    let harness = harness(StatusCode::OK, upstream_body.clone()).await;
    let request_body = serde_json::json!({
        "id": "session-search-123",
        "model": "search-alias",
        "reasoning": {"effort": "medium"},
        "input": "Find the current source.",
        "commands": {"search_query": [{"q": "example"}]},
        "settings": {"external_web_access": true},
        "max_output_tokens": 300
    });

    let response = authorized_post(
        &client(),
        harness.url("/v1/alpha/search?trace=1"),
        CLIENT_KEY,
        serde_json::to_vec(&request_body).unwrap(),
    )
    .header("originator", "codex_cli_rs")
    .header(
        "x-codex-turn-metadata",
        r#"{"search_context_size":"medium","model_id":"search-alias"}"#,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);

    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    let forwarded = &requests[0];
    assert_eq!(forwarded.headers.get("originator").unwrap(), "codex_cli_rs");
    assert_eq!(
        forwarded.headers.get("x-codex-turn-metadata").unwrap(),
        r#"{"search_context_size":"medium","model_id":"search-alias"}"#
    );
    let forwarded_body: Value = serde_json::from_slice(&forwarded.body).unwrap();
    assert_eq!(forwarded_body["model"], "responses-model");
    assert_eq!(forwarded_body["id"], "session-search-123");
    assert_eq!(forwarded_body["commands"], request_body["commands"]);
    assert_eq!(forwarded_body["settings"], request_body["settings"]);

    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].api_operation,
        ai_gateway::domain::ApiOperation::StandaloneWebSearch
    );
    assert_eq!(logs[0].request_protocol.as_str(), "non_stream");
    assert_eq!(logs[0].reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(
        logs[0].billing.as_ref().and_then(|billing| billing.usage),
        None
    );
}

#[tokio::test]
async fn standalone_web_search_requires_explicit_channel_capability() {
    let harness = harness_with_transforms(TransformDocuments {
        responses_search_supported: false,
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/alpha/search"),
        CLIENT_KEY,
        br#"{"id":"session-search-123","model":"responses-model","commands":{}}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "no_healthy_channel");
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn standalone_web_search_rejects_unknown_top_level_fields_before_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/alpha/search"),
        CLIENT_KEY,
        br#"{"id":"session-search-123","model":"responses-model","commands":{},"future_field":true}"#
            .to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "request_body_field_unsupported");
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn deepseek_chat_nonstream_usage_preserves_total_output_and_reasoning() {
    let upstream_body = br#"{
        "id":"b6de8b7e-d52a-4e36-9032-7362d940c5fd",
        "object":"chat.completion",
        "created":1785837502,
        "model":"deepseek-v4-flash",
        "choices":[{
            "index":0,
            "message":{
                "role":"assistant",
                "content":"pong",
                "reasoning_content":"The answer is pong."
            },
            "finish_reason":"stop"
        }],
        "usage":{
            "prompt_tokens":11,
            "completion_tokens":49,
            "total_tokens":60,
            "prompt_tokens_details":{"cached_tokens":0},
            "completion_tokens_details":{"reasoning_tokens":46},
            "prompt_cache_hit_tokens":0,
            "prompt_cache_miss_tokens":11
        }
    }"#
    .to_vec();
    let harness = harness(StatusCode::OK, upstream_body.clone()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model","messages":[{"role":"user","content":"ping"}]}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 11,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 49,
            reasoning_tokens: 46,
        })
    );
}

#[tokio::test]
async fn common_forwarding_metadata_cannot_be_reintroduced_by_http_transforms() {
    // Client copies are removed by the ingress allowlist. Re-adding the same
    // names through each format's Header Transform verifies the later
    // outbound guard instead of letting the ingress filter make this test pass.
    let harness = harness_with_transforms(TransformDocuments {
        chat_override: forwarding_metadata_transform("open_ai_chat_completions"),
        responses_override: forwarding_metadata_transform("open_ai_responses"),
        images_override: forwarding_metadata_transform("open_ai_images"),
        ..TransformDocuments::default()
    })
    .await;
    let client = client();

    let requests = [
        with_forwarding_metadata(authorized_post(
            &client,
            harness.url("/v1/chat/completions"),
            CLIENT_KEY,
            br#"{"model":"same-model"}"#.to_vec(),
        )),
        with_forwarding_metadata(authorized_post(
            &client,
            harness.url("/v1/responses"),
            CLIENT_KEY,
            br#"{"model":"responses-model"}"#.to_vec(),
        )),
        with_forwarding_metadata(authorized_post(
            &client,
            harness.url("/v1/images/generations"),
            CLIENT_KEY,
            br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
        )),
    ];
    for request in requests {
        assert_eq!(request.send().await.unwrap().status(), StatusCode::OK);
    }

    let boundary = "gateway-forwarding-header-filter-boundary";
    let edit = with_forwarding_metadata(authorized_multipart_post(
        &client,
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        multipart_edit_body(boundary, "gpt-image-2", &[], b"image"),
    ))
    .send()
    .await
    .unwrap();
    assert_eq!(edit.status(), StatusCode::OK);

    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 4);
    for request in requests {
        for name in FORWARDING_METADATA_HEADERS {
            assert!(request.headers.get(*name).is_none(), "{name} was forwarded");
        }
        assert_eq!(
            request
                .headers
                .get("accept-encoding")
                .and_then(|value| value.to_str().ok()),
            Some(UPSTREAM_ACCEPT_ENCODING)
        );
    }
}

#[tokio::test]
async fn range_requests_force_identity_content_coding_upstream() {
    let harness = harness(StatusCode::OK, br#"{}"#.to_vec()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .header("accept-encoding", "br")
    .header("range", "bytes=0-10")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("content-encoding").is_none());
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("accept-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("identity")
    );
}

#[tokio::test]
async fn images_generation_preserves_json_and_collects_top_level_usage() {
    let upstream_body = br#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":7,"output_tokens":11,"input_tokens_details":{"image_tokens":0,"text_tokens":7},"output_tokens_details":{"image_tokens":11,"text_tokens":0}}}"#.to_vec();
    let harness = harness(StatusCode::OK, upstream_body.clone()).await;
    let request_body =
        br#"{ "model" : "gpt-image-2", "prompt":"draw a blue whale", "size":"auto" }"#.to_vec();

    let response = authorized_post(
        &client(),
        harness.url("/v1/images/generations"),
        CLIENT_KEY,
        request_body.clone(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, request_body);
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer upstream-key"
    );

    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].api_format, ApiFormat::OpenAiImages);
    assert_eq!(
        logs[0].api_operation,
        ai_gateway::domain::ApiOperation::ImagesGeneration
    );
    assert_eq!(logs[0].request_protocol.as_str(), "non_stream");
    assert_eq!(
        logs[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 7,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 11,
            reasoning_tokens: 0,
        })
    );
    assert_eq!(
        logs[0].billing.as_ref().unwrap().output_tokens_per_second,
        None
    );
}

#[tokio::test]
async fn independently_negotiates_supported_upstream_and_downstream_content_codings() {
    for coding in TestContentCoding::ALL {
        let upstream_body = serde_json::to_vec(&serde_json::json!({
            "created": 1,
            "data": [{"b64_json": "a".repeat(4_096)}],
            "usage": {
                "input_tokens": 7,
                "output_tokens": 11,
                "input_tokens_details": {"image_tokens": 0, "text_tokens": 7},
                "output_tokens_details": {"image_tokens": 11, "text_tokens": 0}
            }
        }))
        .unwrap();
        let encoded_body = encode_test_body(coding, &upstream_body).await;
        let harness = encoded_harness(coding.name(), encoded_body).await;
        let gateway = start_server(harness.app.clone()).await;

        let downstream_accept_encoding = if matches!(coding, TestContentCoding::Brotli) {
            "gzip;q=0.2, br;q=1, zstd;q=0.5, identity;q=0.1"
        } else {
            coding.name()
        };
        let response = authorized_post(
            &client(),
            format!("http://{}/v1/images/generations", gateway.address),
            CLIENT_KEY,
            br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
        )
        .header("accept-encoding", downstream_accept_encoding)
        .send()
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK, "{coding:?}");
        assert_eq!(
            response
                .headers()
                .get("content-encoding")
                .and_then(|value| value.to_str().ok()),
            Some(coding.name()),
            "{coding:?}"
        );
        assert!(
            response.headers().get_all("vary").iter().any(|value| value
                .to_str()
                .is_ok_and(|value| value.eq_ignore_ascii_case("accept-encoding"))),
            "{coding:?}"
        );
        for name in [
            "content-length",
            "accept-ranges",
            "etag",
            "content-md5",
            "digest",
            "content-digest",
            "repr-digest",
        ] {
            assert!(response.headers().get(name).is_none(), "{coding:?}: {name}");
        }
        let encoded_downstream = response.bytes().await.unwrap();
        assert_eq!(
            decode_test_body(coding, &encoded_downstream).await,
            upstream_body,
            "{coding:?}"
        );

        let requests = harness.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "{coding:?}");
        assert_eq!(
            requests[0]
                .headers
                .get("accept-encoding")
                .and_then(|value| value.to_str().ok()),
            Some(UPSTREAM_ACCEPT_ENCODING),
            "{coding:?}"
        );
        drop(requests);

        let events = harness.logs.events();
        assert_eq!(events.len(), 1, "{coding:?}");
        assert_eq!(
            events[0].billing.as_ref().unwrap().usage,
            Some(ai_gateway::domain::RequestUsage {
                input_tokens: 7,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 11,
                reasoning_tokens: 0,
            }),
            "{coding:?}"
        );
    }
}

#[tokio::test]
async fn compressed_upstream_response_can_be_forwarded_as_downstream_identity() {
    let upstream_body = serde_json::to_vec(&serde_json::json!({
        "created": 1,
        "data": [{"b64_json": "a".repeat(4_096)}],
        "usage": {"input_tokens": 7, "output_tokens": 11}
    }))
    .unwrap();
    let encoded_body = encode_test_body(TestContentCoding::Brotli, &upstream_body).await;
    let harness = encoded_harness("br", encoded_body).await;
    let gateway = start_server(harness.app.clone()).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/images/generations", gateway.address),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .header(
        "accept-encoding",
        "gzip;q=0, deflate;q=0, br;q=0, zstd;q=0, identity;q=1",
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("content-encoding").is_none());
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    assert_eq!(
        harness.logs.events()[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 7,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 11,
            reasoning_tokens: 0,
        })
    );
}

#[tokio::test]
async fn stacked_supported_upstream_content_codings_decode_in_reverse_order() {
    let upstream_body = serde_json::to_vec(&serde_json::json!({
        "created": 1,
        "data": [{"b64_json": "a".repeat(4_096)}],
        "usage": {"input_tokens": 7, "output_tokens": 11}
    }))
    .unwrap();
    let gzip = encode_test_body(TestContentCoding::Gzip, &upstream_body).await;
    let gzip_then_brotli = encode_test_body(TestContentCoding::Brotli, &gzip).await;
    let harness = encoded_harness("gzip, br", gzip_then_brotli).await;
    let gateway = start_server(harness.app.clone()).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/images/generations", gateway.address),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .header("accept-encoding", "gzip")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    assert_eq!(
        decode_test_body(TestContentCoding::Gzip, &response.bytes().await.unwrap()).await,
        upstream_body
    );
    assert_eq!(
        harness.logs.events()[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 7,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 11,
            reasoning_tokens: 0,
        })
    );
}

#[tokio::test]
async fn corrupt_upstream_content_coding_terminates_the_stream_and_fails_the_log() {
    let harness = encoded_harness("gzip", b"not a gzip stream".to_vec()).await;
    let response = harness
        .app
        .oneshot(
            Request::post("/v1/images/generations")
                .header("authorization", format!("Bearer {CLIENT_KEY}"))
                .header("content-type", "application/json")
                .header("accept-encoding", "identity")
                .body(Body::from(
                    br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(to_bytes(response.into_body(), usize::MAX).await.is_err());
    let events = harness.logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_str(), "failed");
    assert_eq!(events[0].error_code.as_deref(), Some("upstream_body_error"));
}

#[tokio::test]
async fn unsupported_upstream_content_coding_fails_before_downstream_headers() {
    let harness = encoded_harness("compress", br#"{"created":1,"data":[]}"#.to_vec()).await;
    let gateway = start_server(harness.app.clone()).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/images/generations", gateway.address),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        body["error"]["code"],
        "upstream_content_encoding_unsupported"
    );
    let events = harness.logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_code.as_deref(),
        Some("upstream_content_encoding_unsupported")
    );
}

#[tokio::test]
async fn images_generation_uses_the_longer_images_response_header_timeout() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/images/generations", post(delayed_upstream_response))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: br#"{"data":[]}"#.to_vec(),
            }),
    )
    .await;
    let configured = configured_proxy_with_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        None,
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 3,
            standalone_web_search_response_header_timeout_seconds: 3,
            stream_idle_timeout_seconds: 1,
        },
    );
    let gateway = start_server(http::router(configured.proxy)).await;
    let timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = authorized_post(
        &timeout_client,
        format!("http://{}/v1/images/generations", gateway.address),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn standalone_web_search_uses_its_longer_response_header_timeout() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/alpha/search", post(delayed_upstream_response))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: br#"{"output":"done","results":[]}"#.to_vec(),
            }),
    )
    .await;
    let configured = configured_proxy_with_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        None,
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 3,
            standalone_web_search_response_header_timeout_seconds: 3,
            stream_idle_timeout_seconds: 1,
        },
    );
    let gateway = start_server(http::router(configured.proxy)).await;
    let timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = authorized_post(
        &timeout_client,
        format!("http://{}/v1/alpha/search", gateway.address),
        CLIENT_KEY,
        br#"{"id":"session-search-123","model":"responses-model","commands":{}}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn images_generation_applies_images_scoped_header_and_json_transforms() {
    let harness = harness_with_transforms(TransformDocuments {
        images_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_images",
            "request_headers": {"set": {"x-image-route": "generation"}},
            "request_json": [{"op": "add", "path": "/quality", "value": "high"}]
        }),
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/images/generations"),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("x-image-route").unwrap(),
        "generation"
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["quality"], "high");
}

#[tokio::test]
async fn images_generation_rejects_streaming_without_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/images/generations"),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test","stream":true}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "image_streaming_unsupported");
    assert_eq!(body["error"]["param"], "stream");
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn images_generation_does_not_retry_after_an_upstream_attempt_starts() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/images/generations", post(first_response_header_hangs))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: br#"{"data":[]}"#.to_vec(),
            }),
    )
    .await;
    let configured = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        None,
        None,
        None,
        Default::default(),
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    let timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = authorized_post(
        &timeout_client,
        format!("http://{}/v1/images/generations", gateway.address),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2","prompt":"test"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn images_edit_preserves_multipart_spools_large_input_and_collects_usage() {
    let upstream_body = br#"{"created":1,"data":[{"b64_json":"aW1hZ2U="}],"usage":{"input_tokens":17,"output_tokens":23}}"#.to_vec();
    let harness = harness(StatusCode::OK, upstream_body.clone()).await;
    let boundary = "gateway-edit-boundary";
    let image = vec![5_u8; 128 * 1_024];
    let request_body = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[("prompt", "add a red hat"), ("quality", "high")],
        &image,
    );

    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        request_body.clone(),
    )
    .header("content-md5", "stale")
    .header("digest", "sha-256=stale")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, request_body);
    let expected_content_type = format!("multipart/form-data; boundary={boundary}");
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some(expected_content_type.as_str())
    );
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        "Bearer upstream-key"
    );
    assert_eq!(requests[0].headers.get("content-md5").unwrap(), "stale");
    assert_eq!(requests[0].headers.get("digest").unwrap(), "sha-256=stale");

    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].api_format, ApiFormat::OpenAiImages);
    assert_eq!(
        logs[0].api_operation,
        ai_gateway::domain::ApiOperation::ImagesEdit
    );
    assert_eq!(logs[0].request_protocol.as_str(), "non_stream");
    assert_eq!(
        logs[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 17,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 23,
            reasoning_tokens: 0,
        })
    );
}

#[tokio::test]
async fn client_body_allowlists_reject_unknown_fields_before_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;
    let client = client();
    for (path, body) in [
        (
            "/v1/chat/completions",
            br#"{"model":"same-model","messages":[],"future_field":true}"#.as_slice(),
        ),
        (
            "/v1/responses",
            br#"{"model":"responses-model","input":[],"future_field":true}"#.as_slice(),
        ),
        (
            "/v1/images/generations",
            br#"{"model":"gpt-image-2","prompt":"test","future_field":true}"#.as_slice(),
        ),
    ] {
        let response = authorized_post(&client, harness.url(path), CLIENT_KEY, body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!(
            value["error"]["code"], "request_body_field_unsupported",
            "{path}"
        );
    }

    let boundary = "unknown-edit-field-boundary";
    let response = authorized_multipart_post(
        &client,
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        multipart_edit_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "test"), ("future_field", "true")],
            b"image",
        ),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let value: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(value["error"]["code"], "request_body_field_unsupported");
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn images_edit_ignores_only_declared_client_compatibility_fields() {
    let harness = harness(StatusCode::OK, br#"{"data":[]}"#.to_vec()).await;
    let boundary = "gateway-edit-client-policy-boundary";
    let request_body = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[
            ("prompt", "change the clothes to blue"),
            ("output_format", "png"),
            ("moderation", "auto"),
        ],
        b"image",
    );

    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        request_body.clone(),
    )
    .header("content-md5", "stale")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_ne!(requests[0].body, request_body);
    assert!(requests[0].headers.get("content-md5").is_none());
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("name=\"output_format\""));
    assert!(!body.contains("name=\"moderation\""));
}

#[tokio::test]
async fn images_edit_rebuilds_multipart_only_when_model_aliasing_is_required() {
    let harness = harness(StatusCode::OK, br#"{"data":[]}"#.to_vec()).await;
    let boundary = "gateway-edit-alias-boundary";
    let image = b"alias-image-bytes";
    let request_body = multipart_edit_body(
        boundary,
        "image-alias",
        &[("prompt", "keep the composition")],
        image,
    );

    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        request_body.clone(),
    )
    .header("content-md5", "stale")
    .header("digest", "sha-256=stale")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_ne!(requests[0].body, request_body);
    assert!(requests[0].headers.get("content-md5").is_none());
    assert!(requests[0].headers.get("digest").is_none());
    let (model, forwarded_image) = parse_captured_edit(&requests[0].body, boundary).await;
    assert_eq!(model, "gpt-image-2");
    assert_eq!(forwarded_image, image);
}

#[tokio::test]
async fn images_edit_applies_header_transforms_without_rewriting_the_multipart() {
    let harness = harness_with_transforms(TransformDocuments {
        images_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_images",
            "request_headers": {"set": {"x-image-route": "edit"}}
        }),
        ..Default::default()
    })
    .await;
    let boundary = "gateway-edit-header-transform-boundary";
    let request_body = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[("prompt", "keep the bytes")],
        b"image",
    );

    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        request_body.clone(),
    )
    .header("digest", "sha-256=preserved")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, request_body);
    assert_eq!(requests[0].headers.get("x-image-route").unwrap(), "edit");
    assert_eq!(
        requests[0].headers.get("digest").unwrap(),
        "sha-256=preserved"
    );
}

#[tokio::test]
async fn images_edit_rejects_non_multipart_and_oversized_bodies_without_upstream_contact() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let unsupported = authorized_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        br#"{"model":"gpt-image-2"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(unsupported.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let body: Value = serde_json::from_slice(&unsupported.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "image_edit_content_type_unsupported");

    let boundary = "gateway-edit-oversized-boundary";
    let oversized = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[("prompt", "oversized")],
        &vec![8_u8; 1_048_576],
    );
    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        oversized,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(harness.upstream_requests().is_empty());
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn images_edit_rejects_json_transforms_after_routing_without_forwarding_sensitive_body() {
    let harness = harness_with_transforms(TransformDocuments {
        images_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_images",
            "request_json": [{"op": "add", "path": "/quality", "value": "high"}]
        }),
        ..Default::default()
    })
    .await;
    let boundary = "gateway-edit-transform-boundary";
    let request_body = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[("prompt", "do not log this")],
        b"sensitive-image",
    );

    let response = authorized_multipart_post(
        &client(),
        harness.url("/v1/images/edits"),
        CLIENT_KEY,
        boundary,
        request_body,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response_bytes = response.bytes().await.unwrap();
    let rendered = String::from_utf8_lossy(&response_bytes);
    assert!(!rendered.contains("do not log this"));
    assert!(!rendered.contains("sensitive-image"));
    let body: Value = serde_json::from_slice(&response_bytes).unwrap();
    assert_eq!(
        body["error"]["code"],
        "image_edit_json_transform_unsupported"
    );
    assert!(harness.upstream_requests().is_empty());
    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].api_operation,
        ai_gateway::domain::ApiOperation::ImagesEdit
    );
    assert_eq!(logs[0].error_code.as_deref(), Some("invalid_request"));
    let summary = logs[0].error_summary.as_deref().unwrap();
    assert!(
        summary.starts_with("Images edit multipart bodies do not support request JSON transforms.")
    );
    assert!(summary.contains("\"code\": \"image_edit_json_transform_unsupported\""));
    assert!(!summary.contains("do not log this"));
    assert!(!summary.contains("sensitive-image"));
}

#[tokio::test]
async fn images_edit_does_not_retry_after_an_upstream_attempt_starts() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/images/edits", post(first_response_header_hangs))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: br#"{"data":[]}"#.to_vec(),
            }),
    )
    .await;
    let configured = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        None,
        None,
        None,
        Default::default(),
    );
    let gateway = start_server(http::router(configured.proxy)).await;
    let boundary = "gateway-edit-retry-boundary";
    let request_body = multipart_edit_body(
        boundary,
        "gpt-image-2",
        &[("prompt", "single attempt")],
        b"image",
    );
    let timeout_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = authorized_multipart_post(
        &timeout_client,
        format!("http://{}/v1/images/edits", gateway.address),
        CLIENT_KEY,
        boundary,
        request_body,
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn responses_nonstream_usage_is_collected_without_buffering_the_response() {
    let upstream_body = br#"{"id":"response-id","usage":{"input_tokens":9,"output_tokens":3,"input_tokens_details":{"cached_tokens":2},"output_tokens_details":{"reasoning_tokens":1}}}"#.to_vec();
    let harness = harness(StatusCode::OK, upstream_body.clone()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/responses"),
        CLIENT_KEY,
        br#"{"model":"responses-model","reasoning":{"effort":"xhigh"},"service_tier":"priority"}"#
            .to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].reasoning_effort.as_deref(), Some("xhigh"));
    assert!(logs[0].fast_mode);
    assert_eq!(
        logs[0].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 9,
            cached_input_tokens: 2,
            cache_write_tokens: 0,
            output_tokens: 3,
            reasoning_tokens: 1,
        })
    );
}

#[tokio::test]
async fn protocol_terminal_sse_events_complete_logs_before_transport_eof() {
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(chat_done_then_hangs))
            .route("/v1/responses", post(responses_completed_then_hangs)),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let configured = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
    );
    let gateway = start_server(http::router(configured.proxy)).await;
    let client = client();

    let mut chat = authorized_post(
        &client,
        format!("http://{}/v1/chat/completions", gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model","stream":true}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);
    assert_eq!(
        chat.chunk().await.unwrap().unwrap(),
        Bytes::from_static(b"data: [DONE]\n\n")
    );
    assert_eq!(logs.events().len(), 1);
    assert_eq!(logs.events()[0].outcome.as_str(), "succeeded");
    assert_eq!(logs.events()[0].error_code, None);
    drop(chat);
    assert_eq!(logs.events().len(), 1);

    let mut responses = authorized_post(
        &client,
        format!("http://{}/v1/responses", gateway.address),
        CLIENT_KEY,
        br#"{"model":"responses-model","stream":true}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(responses.status(), StatusCode::OK);
    assert!(
        responses
            .chunk()
            .await
            .unwrap()
            .unwrap()
            .windows(b"response.completed".len())
            .any(|window| window == b"response.completed")
    );
    let events = logs.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].outcome.as_str(), "succeeded");
    assert_eq!(events[1].error_code, None);
    assert_eq!(
        events[1].billing.as_ref().unwrap().usage,
        Some(ai_gateway::domain::RequestUsage {
            input_tokens: 9,
            cached_input_tokens: 2,
            cache_write_tokens: 0,
            output_tokens: 3,
            reasoning_tokens: 1,
        })
    );
    drop(responses);
    assert_eq!(logs.events().len(), 2);
}

#[tokio::test]
async fn protocol_sse_errors_fail_logs_before_client_disconnect() {
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(chat_error_then_hangs))
            .route("/v1/responses", post(responses_error_then_hangs)),
    )
    .await;
    let logs = RecordingRequestLogSink::default();
    let configured = proxy_service_with_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        None,
        None,
        None,
        Default::default(),
    );
    let gateway = start_server(http::router(configured.proxy)).await;
    let client = client();

    let mut chat = authorized_post(
        &client,
        format!("http://{}/v1/chat/completions", gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model","stream":true}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);
    assert!(
        chat.chunk()
            .await
            .unwrap()
            .unwrap()
            .windows(b"\"error\"".len())
            .any(|window| window == b"\"error\"")
    );
    let events = logs.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].outcome.as_str(), "failed");
    assert_eq!(events[0].error_code.as_deref(), Some("provider_error"));
    let summary = events[0].error_summary.as_deref().unwrap();
    assert!(summary.starts_with("sensitive upstream detail\n\n{"));
    assert!(summary.contains("\"type\": \"server_error\""));
    assert!(summary.contains("\"code\": \"provider_error\""));
    assert_eq!(
        events[0].response_status_code,
        Some(StatusCode::OK.as_u16())
    );
    drop(chat);
    assert_eq!(logs.events().len(), 1);

    let mut responses = authorized_post(
        &client,
        format!("http://{}/v1/responses", gateway.address),
        CLIENT_KEY,
        br#"{"model":"responses-model","stream":true}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(responses.status(), StatusCode::OK);
    assert!(
        responses
            .chunk()
            .await
            .unwrap()
            .unwrap()
            .windows(b"event: error".len())
            .any(|window| window == b"event: error")
    );
    let events = logs.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].outcome.as_str(), "failed");
    assert_eq!(events[1].error_code.as_deref(), Some("server_error"));
    let summary = events[1].error_summary.as_deref().unwrap();
    assert!(summary.starts_with("sensitive upstream detail\n\n{"));
    assert!(summary.contains("\"param\": null"));
    assert!(summary.contains("\"sequence_number\": 3"));
    assert_eq!(
        events[1].response_status_code,
        Some(StatusCode::OK.as_u16())
    );
    drop(responses);
    assert_eq!(logs.events().len(), 2);
}

#[tokio::test]
async fn template_then_channel_override_layers_request_headers_and_json_body() {
    let harness = harness_with_transforms(TransformDocuments {
        template: Some(serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-layer": "template", "x-template": "enabled"}},
            "request_json": [{"op": "add", "path": "/metadata/template", "value": true}]
        })),
        chat_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-layer": "channel", "x-channel": "enabled"}},
            "request_json": [{"op": "add", "path": "/metadata/channel", "value": true}]
        }),
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model","metadata":{}}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.headers.get("x-layer").unwrap(), "channel");
    assert_eq!(request.headers.get("x-template").unwrap(), "enabled");
    assert_eq!(request.headers.get("x-channel").unwrap(), "enabled");
    let body: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(body["metadata"]["template"], true);
    assert_eq!(body["metadata"]["channel"], true);
}

#[tokio::test]
async fn client_authorization_is_never_forwarded_when_the_channel_has_no_upstream_auth() {
    let harness = harness_with_transforms(TransformDocuments {
        upstream_auth_kind: "none",
        upstream_api_key: None,
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn configured_custom_upstream_auth_is_injected_after_header_plans() {
    let harness = harness_with_transforms(TransformDocuments {
        template: Some(serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-api-key": "template-value"}}
        })),
        chat_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-api-key": "channel-value"}}
        }),
        upstream_auth_kind: "header",
        upstream_auth_header_name: Some("x-api-key"),
        upstream_api_key: Some("configured-upstream-key"),
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .header("x-api-key", "client-value")
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("x-api-key").unwrap(),
        "configured-upstream-key"
    );
}

#[tokio::test]
async fn no_transform_plan_preserves_unusual_json_body_bytes_exactly() {
    let harness = harness_with_transforms(TransformDocuments::default()).await;
    let request_body = br#"{ "messages" : [{"role":"user","content":{"b":2,"a":1}}], "model":"same-model", "metadata" : { "z":[3,2] } }"#.to_vec();

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        request_body.clone(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body, request_body);
}

#[tokio::test]
async fn chat_and_responses_transform_plans_remain_format_isolated() {
    let harness = harness_with_transforms(TransformDocuments {
        template: Some(serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-chat-template": "enabled"}}
        })),
        chat_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-chat-override": "enabled"}}
        }),
        responses_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_responses",
            "request_headers": {"set": {"x-responses-override": "enabled"}}
        }),
        ..Default::default()
    })
    .await;
    let client = client();

    let chat = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    let responses = authorized_post(
        &client,
        harness.url("/v1/responses"),
        CLIENT_KEY,
        br#"{"model":"responses-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(chat.status(), StatusCode::OK);
    assert_eq!(responses.status(), StatusCode::OK);
    let requests = harness.upstream_requests();
    assert_eq!(requests.len(), 2);
    let chat = requests
        .iter()
        .find(|request| request.headers.get("x-chat-template").is_some())
        .unwrap();
    let responses = requests
        .iter()
        .find(|request| request.headers.get("x-responses-override").is_some())
        .unwrap();
    assert_eq!(chat.headers.get("x-chat-override").unwrap(), "enabled");
    assert!(chat.headers.get("x-responses-override").is_none());
    assert!(responses.headers.get("x-chat-template").is_none());
    assert!(responses.headers.get("x-chat-override").is_none());
}

#[tokio::test]
async fn failed_patch_for_a_routed_model_returns_safe_bad_request_without_upstream_contact() {
    let harness = harness_with_transforms(TransformDocuments {
        chat_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_json": [{"op": "remove", "path": "/missing"}]
        }),
        ..Default::default()
    })
    .await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(body["error"]["param"], "body");
    assert_eq!(
        body["error"]["message"],
        "Request transform could not be applied."
    );
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn connection_declared_header_plan_with_opaque_bytes_is_rejected_without_upstream_contact() {
    let harness = harness_with_transforms(TransformDocuments {
        chat_override: serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-hop": "changed"}}
        }),
        ..Default::default()
    })
    .await;
    let mut request = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .build()
    .unwrap();
    request.headers_mut().insert(
        "connection",
        HeaderValue::from_bytes(b"x-hop,\xff").unwrap(),
    );

    let response = client().execute(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(body["error"]["param"], "body");
    assert_eq!(
        body["error"]["message"],
        "Request transform could not be applied."
    );
    assert!(harness.upstream_requests().is_empty());
}

#[tokio::test]
async fn malformed_model_is_trace_only_and_not_persisted() {
    let harness = harness(StatusCode::OK, Vec::new()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":false}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(harness.logs().is_empty());
}

#[tokio::test]
async fn alias_rewrites_only_the_top_level_model() {
    let harness = harness(StatusCode::OK, br#"{"ok":true}"#.to_vec()).await;
    let request_body = br#"{"model":"alias-model","metadata":{"model":"unchanged"},"messages":[{"role":"user","content":"test","model":"also-unchanged"}]}"#;

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
    assert_eq!(forwarded["metadata"]["model"], "unchanged");
    assert_eq!(forwarded["messages"][0]["model"], "also-unchanged");
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

    let all_formats = client
        .get(harness.url("/v1/models"))
        .header("authorization", format!("Bearer {CLIENT_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(all_formats.status(), StatusCode::OK);
    let all_formats: Value = serde_json::from_slice(&all_formats.bytes().await.unwrap()).unwrap();
    let all_models = all_formats["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert!(all_models.contains("responses-model"));
    assert!(all_models.contains("gpt-image-2"));

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

#[tokio::test]
async fn admission_rpm_is_shared_by_chat_and_responses_without_upstream_contact_on_rejection() {
    let harness = harness_with_policy(
        StatusCode::OK,
        b"ok".to_vec(),
        Some(1),
        None,
        None,
        Default::default(),
    )
    .await;
    let client = client();
    let chat = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(chat.status(), StatusCode::OK);
    let response = authorized_post(
        &client,
        harness.url("/v1/responses"),
        CLIENT_KEY,
        br#"{"model":"responses-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers().get("retry-after").unwrap(), "60");
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "rate_limit_exceeded");
    assert_eq!(harness.upstream_requests().len(), 1);
}

#[tokio::test]
async fn soft_quota_rejects_equality_but_allows_under_limit() {
    let exhausted = harness_with_policy(
        StatusCode::OK,
        b"ok".to_vec(),
        None,
        None,
        Some(rust_decimal::Decimal::new(100, 2)),
        rust_decimal::Decimal::new(100, 2),
    )
    .await;
    let response = authorized_post(
        &client(),
        exhausted.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["error"]["code"], "insufficient_quota");
    assert!(exhausted.upstream_requests().is_empty());

    let under_limit = harness_with_policy(
        StatusCode::OK,
        b"ok".to_vec(),
        None,
        None,
        Some(rust_decimal::Decimal::new(100, 2)),
        rust_decimal::Decimal::new(99, 2),
    )
    .await;
    let response = authorized_post(
        &client(),
        under_limit.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(under_limit.upstream_requests().len(), 1);
}

#[tokio::test]
async fn admission_keeps_streaming_work_across_snapshot_replacement_and_consumes_rpm_on_denial() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(hanging_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: Vec::new(),
            }),
    )
    .await;
    let upstream_url = format!("http://{}", upstream.address);
    let current = configured_proxy_with_policy(
        &upstream_url,
        RecordingRequestLogSink::default(),
        Some(3),
        None,
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    );
    let next = configured_proxy_with_policy(
        &upstream_url,
        RecordingRequestLogSink::default(),
        Some(3),
        Some(1),
        None,
        Default::default(),
        Some(current.client_key_id),
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    );
    let app = http::router(current.proxy);

    // The old unlimited snapshot admits this response and its hanging body owns
    // the lease. Replacing the snapshot must retain that UUID-keyed state.
    let first = app
        .clone()
        .oneshot(proxy_request("same-model"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    current.runtime.replace_snapshot(next.runtime.snapshot());
    let concurrent = app
        .clone()
        .oneshot(proxy_request("same-model"))
        .await
        .unwrap();
    assert_eq!(concurrent.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        concurrent
            .headers()
            .get("retry-after")
            .map(|value| value.as_bytes()),
        None
    );
    assert_eq!(requests.lock().unwrap().len(), 1);

    // Dropping the response simulates downstream cancellation. The third
    // request is admitted, while the preceding terminal and concurrent denial
    // have both consumed the three RPM slots.
    drop(first);
    let third = app
        .clone()
        .oneshot(proxy_request("same-model"))
        .await
        .unwrap();
    assert_eq!(third.status(), StatusCode::OK);
    drop(third);
    let rate_limited = app.oneshot(proxy_request("same-model")).await.unwrap();
    assert_eq!(rate_limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(rate_limited.headers().contains_key("retry-after"));
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn admission_releases_capacity_after_malformed_and_unknown_model_requests() {
    let harness = harness_with_policy(
        StatusCode::OK,
        b"ok".to_vec(),
        Some(5),
        Some(1),
        None,
        Default::default(),
    )
    .await;
    let client = client();
    let malformed = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":false}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
    let after_malformed = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(after_malformed.status(), StatusCode::OK);
    drop(after_malformed);
    let unknown = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"unknown-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let after_unknown = authorized_post(
        &client,
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(after_unknown.status(), StatusCode::OK);
    assert_eq!(harness.upstream_requests().len(), 2);
}

#[tokio::test]
async fn admission_releases_capacity_after_response_header_timeout() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(first_response_header_hangs))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"ok".to_vec(),
            }),
    )
    .await;
    let configured = configured_proxy_with_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        Some(3),
        Some(1),
        None,
        Default::default(),
        None,
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
    );
    let app = http::router(configured.proxy);
    let timed_out = app
        .clone()
        .oneshot(proxy_request("same-model"))
        .await
        .unwrap();
    assert_eq!(timed_out.status(), StatusCode::GATEWAY_TIMEOUT);
    let next = app.oneshot(proxy_request("same-model")).await.unwrap();
    assert_eq!(next.status(), StatusCode::OK);
    assert_eq!(requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_effective_timeout_policy_never_contacts_upstream_or_cools_the_channel() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"unexpected".to_vec(),
            }),
    )
    .await;
    let configured = configured_proxy_with_outbound_policy(
        &format!("http://{}", upstream.address),
        RecordingRequestLogSink::default(),
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        OutboundTestPolicy {
            connect_timeout_ms: Some(2_000),
            response_header_timeout_ms: Some(1_000),
            ..Default::default()
        },
    );
    let app = http::router(configured.proxy);

    for _ in 0..4 {
        let response = app
            .clone()
            .oneshot(proxy_request("same-model"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }
    assert!(requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn http_proxy_routes_nonmatching_targets_and_explicit_no_proxy_hosts_bypass_it() {
    const PROXY_PASSWORD: &str = "sentinel-proxy-password";
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"upstream-response".to_vec(),
            }),
    )
    .await;
    let (http_proxy, captures) = start_http_proxy().await;
    let upstream_url = format!("http://{}", upstream.address);
    let defaults = UpstreamConfig {
        connect_timeout_seconds: 1,
        response_header_timeout_seconds: 2,
        images_response_header_timeout_seconds: 2,
        standalone_web_search_response_header_timeout_seconds: 2,
        stream_idle_timeout_seconds: 1,
    };
    let logs = RecordingRequestLogSink::default();
    let proxied = configured_proxy_with_outbound_policy(
        &upstream_url,
        logs.clone(),
        defaults.clone(),
        OutboundTestPolicy {
            proxy: Some(proxy_record(
                format!("http://{}", http_proxy.address),
                Some("user"),
                Some(PROXY_PASSWORD),
                vec![],
            )),
            ..Default::default()
        },
    );
    let proxied_gateway = start_server(http::router(proxied.proxy)).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/chat/completions", proxied_gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"upstream-response".as_slice()
    );
    let captured = captures.lock().unwrap().clone();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].target,
        format!("{upstream_url}/v1/chat/completions")
    );
    assert_eq!(
        captured[0].proxy_authorization.as_deref(),
        Some("Basic dXNlcjpzZW50aW5lbC1wcm94eS1wYXNzd29yZA==")
    );
    assert!(
        requests.lock().unwrap()[0]
            .headers
            .get("proxy-authorization")
            .is_none()
    );

    let bypassed = configured_proxy_with_outbound_policy(
        &upstream_url,
        logs.clone(),
        defaults,
        OutboundTestPolicy {
            proxy: Some(proxy_record(
                format!("http://{}", http_proxy.address),
                Some("user"),
                Some(PROXY_PASSWORD),
                vec!["127.0.0.1"],
            )),
            ..Default::default()
        },
    );
    let bypassed_gateway = start_server(http::router(bypassed.proxy)).await;
    let response = authorized_post(
        &client(),
        format!("http://{}/v1/chat/completions", bypassed_gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(captures.lock().unwrap().len(), 1);
    let upstream_requests = requests.lock().unwrap();
    assert_eq!(upstream_requests.len(), 2);
    assert!(
        upstream_requests
            .iter()
            .all(|request| request.headers.get("proxy-authorization").is_none())
    );
    assert!(!format!("{:?}", logs.events()).contains(PROXY_PASSWORD));
}

#[tokio::test]
async fn dead_configured_proxy_returns_safe_bad_gateway_without_direct_upstream_contact() {
    const PROXY_PASSWORD: &str = "dead-proxy-sentinel";
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"reachable".to_vec(),
            }),
    )
    .await;
    let unused_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_proxy_address = unused_listener.local_addr().unwrap();
    drop(unused_listener);
    let logs = RecordingRequestLogSink::default();
    let configured = configured_proxy_with_outbound_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        OutboundTestPolicy {
            proxy: Some(proxy_record(
                format!("http://{dead_proxy_address}"),
                Some("user"),
                Some(PROXY_PASSWORD),
                vec![],
            )),
            ..Default::default()
        },
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/chat/completions", gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!response.text().await.unwrap().contains(PROXY_PASSWORD));
    assert!(requests.lock().unwrap().is_empty());
    assert!(!format!("{:?}", logs.events()).contains(PROXY_PASSWORD));
}

#[tokio::test]
async fn socks5_proxy_connects_and_keeps_credentials_out_of_upstream_and_logs() {
    const SOCKS_PASSWORD: &str = "socks-sentinel";
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route("/v1/chat/completions", post(capture_upstream))
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"socks-upstream".to_vec(),
            }),
    )
    .await;
    let socks_proxy = start_socks5_proxy().await;
    let logs = RecordingRequestLogSink::default();
    let configured = configured_proxy_with_outbound_policy(
        &format!("http://{}", upstream.address),
        logs.clone(),
        UpstreamConfig {
            connect_timeout_seconds: 1,
            response_header_timeout_seconds: 2,
            images_response_header_timeout_seconds: 2,
            standalone_web_search_response_header_timeout_seconds: 2,
            stream_idle_timeout_seconds: 1,
        },
        OutboundTestPolicy {
            proxy: Some(proxy_record(
                format!("socks5://{}", socks_proxy.address),
                Some("socks-user"),
                Some(SOCKS_PASSWORD),
                vec![],
            )),
            ..Default::default()
        },
    );
    let gateway = start_server(http::router(configured.proxy)).await;

    let response = authorized_post(
        &client(),
        format!("http://{}/v1/chat/completions", gateway.address),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.bytes().await.unwrap(),
        b"socks-upstream".as_slice()
    );
    assert_eq!(
        socks_proxy
            .credentials
            .lock()
            .unwrap()
            .as_ref()
            .map(|(username, password)| (username.as_str(), password.as_str())),
        Some(("socks-user", SOCKS_PASSWORD))
    );
    let upstream_requests = requests.lock().unwrap();
    assert_eq!(upstream_requests.len(), 1);
    assert!(
        upstream_requests[0]
            .headers
            .get("proxy-authorization")
            .is_none()
    );
    assert!(!format!("{:?}", logs.events()).contains(SOCKS_PASSWORD));
}

#[tokio::test]
async fn rejected_upstream_response_is_attempted_once_without_automatic_retry() {
    let upstream_body = br#"{"error":{"message":"provider overloaded","type":"server_error","param":"capacity","code":"overloaded"},"request_id":"req_123"}"#.to_vec();
    let harness = harness(StatusCode::SERVICE_UNAVAILABLE, upstream_body.clone()).await;

    let response = authorized_post(
        &client(),
        harness.url("/v1/chat/completions"),
        CLIENT_KEY,
        br#"{"model":"same-model"}"#.to_vec(),
    )
    .send()
    .await
    .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.bytes().await.unwrap().as_ref(), upstream_body);
    assert_eq!(harness.upstream_requests().len(), 1);
    let logs = harness.logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].error_code.as_deref(), Some("overloaded"));
    let summary = logs[0].error_summary.as_deref().unwrap();
    assert!(summary.starts_with("provider overloaded\n\n{"));
    assert!(summary.contains("\"param\": \"capacity\""));
    assert!(summary.contains("\"request_id\": \"req_123\""));
}

#[tokio::test]
async fn snapshot_replacement_uses_new_proxy_policy_after_cancelling_an_existing_direct_stream() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let upstream = start_server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(first_stream_hangs_then_succeeds),
            )
            .with_state(MockUpstream {
                requests: Arc::clone(&requests),
                status: StatusCode::OK,
                body: b"after-replacement".to_vec(),
            }),
    )
    .await;
    let (http_proxy, captures) = start_http_proxy().await;
    let upstream_url = format!("http://{}", upstream.address);
    let defaults = UpstreamConfig {
        connect_timeout_seconds: 1,
        response_header_timeout_seconds: 2,
        images_response_header_timeout_seconds: 2,
        standalone_web_search_response_header_timeout_seconds: 2,
        stream_idle_timeout_seconds: 2,
    };
    let current = configured_proxy_with_policy(
        &upstream_url,
        RecordingRequestLogSink::default(),
        None,
        None,
        None,
        Default::default(),
        None,
        defaults.clone(),
    );
    let next = configured_proxy_with_policy_and_transforms(
        &upstream_url,
        RecordingRequestLogSink::default(),
        None,
        Some(1),
        None,
        Default::default(),
        Some(current.client_key_id),
        defaults,
        TransformDocuments::default(),
        OutboundTestPolicy {
            proxy: Some(proxy_record(
                format!("http://{}", http_proxy.address),
                None,
                None,
                vec![],
            )),
            ..Default::default()
        },
        true,
    );
    let app = http::router(current.proxy);

    let existing = app
        .clone()
        .oneshot(proxy_request("same-model"))
        .await
        .unwrap();
    assert_eq!(existing.status(), StatusCode::OK);
    current.runtime.replace_snapshot(next.runtime.snapshot());
    assert_eq!(requests.lock().unwrap().len(), 1);

    // The old stream remains owned by its original client and lease until the
    // downstream response is cancelled. The next request must then resolve the
    // replacement snapshot and its HTTP proxy client.
    drop(existing);
    let replacement = app.oneshot(proxy_request("same-model")).await.unwrap();
    assert_eq!(replacement.status(), StatusCode::OK);
    assert_eq!(captures.lock().unwrap().len(), 1);
    assert_eq!(requests.lock().unwrap().len(), 2);
    drop(replacement);
}
