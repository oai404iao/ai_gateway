//! Explicit, credential-backed smoke coverage for a real OpenAI-compatible
//! upstream. This test is ignored by default and must be started through
//! `scripts/run-real-upstream-smoke.sh`.

use std::{
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{
        ApiFormat, ApiOperation, PassiveHealthSettings, RequestLogEvent, RequestLogOutcome,
        RequestProtocol, RequestUsage, ResponsesWebSocketSettings, SystemRuntimeSettings,
        UpstreamTimeoutDefaults,
    },
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, compile_control_plane_with_system_settings},
};
use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, Request, StatusCode, header},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    task::JoinHandle,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "gateway-real-upstream-smoke-client-key";
const CLIENT_MODEL: &str = "gateway-real-upstream-smoke-model";
const MAX_WEBSOCKET_ATTEMPTS: usize = 3;
const IMAGE_EDIT_PNG_BASE64: &str = include_str!("fixtures/solid-1024.png.b64");

#[derive(Clone, Copy)]
pub(super) enum SmokeFormat {
    ChatCompletions,
    Responses,
    StandaloneWebSearch,
    Images,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResponsesUpstreamProfile {
    OpenAiCompatible,
    CodexOauth,
}

impl ResponsesUpstreamProfile {
    fn from_environment(value: Option<String>) -> Result<Self, &'static str> {
        match value.as_deref() {
            None | Some("") | Some("openai_compatible") => Ok(Self::OpenAiCompatible),
            Some("codex_oauth") => Ok(Self::CodexOauth),
            Some(_) => {
                Err("REAL_UPSTREAM_RESPONSES_PROFILE must be openai_compatible or codex_oauth")
            }
        }
    }
}

impl SmokeFormat {
    const fn api_format(self) -> ApiFormat {
        match self {
            Self::ChatCompletions => ApiFormat::OpenAiChatCompletions,
            Self::Responses | Self::StandaloneWebSearch => ApiFormat::OpenAiResponses,
            Self::Images => ApiFormat::OpenAiImages,
        }
    }

    const fn api_format_name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "open_ai_chat_completions",
            Self::Responses | Self::StandaloneWebSearch => "open_ai_responses",
            Self::Images => "open_ai_images",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::StandaloneWebSearch => "/v1/alpha/search",
            Self::Images => "/v1/images/generations",
        }
    }

    const fn api_operation(self) -> ApiOperation {
        match self {
            Self::ChatCompletions => ApiOperation::ChatCompletions,
            Self::Responses => ApiOperation::Responses,
            Self::StandaloneWebSearch => ApiOperation::StandaloneWebSearch,
            Self::Images => ApiOperation::ImagesGeneration,
        }
    }

    fn request_body(self, streamed: bool) -> Value {
        match self {
            Self::ChatCompletions if streamed => json!({
                "model": CLIENT_MODEL,
                "messages": [{"role": "user", "content": "Reply with OK."}],
                "max_tokens": 1,
                "stream": true,
                "stream_options": {"include_usage": true},
            }),
            Self::ChatCompletions => json!({
                "model": CLIENT_MODEL,
                "messages": [{"role": "user", "content": "Reply with OK."}],
                "max_tokens": 1,
                "stream": false,
            }),
            Self::Responses => json!({
                "model": CLIENT_MODEL,
                "input": [{
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Reply exactly OK."}],
                }],
                "max_output_tokens": 1,
                "stream": streamed,
            }),
            Self::StandaloneWebSearch => json!({
                "id": "gateway-real-upstream-search-session",
                "model": CLIENT_MODEL,
                "input": "Find one authoritative source about Rust.",
                "commands": {
                    "search_query": [{
                        "q": "official Rust programming language",
                        "domains": ["rust-lang.org"]
                    }]
                },
                "settings": {"external_web_access": true},
                "max_output_tokens": 300,
            }),
            Self::Images => json!({
                "model": CLIENT_MODEL,
                "prompt": "Create a solid red square.",
                "n": 1,
                "quality": "low",
                "size": "1024x1024",
            }),
        }
    }
}

#[derive(Clone)]
struct SmokeUpstream {
    base_url: String,
    api_key: String,
}

struct ImagesSmokeSettings {
    upstream: SmokeUpstream,
    model: String,
}

struct SearchSmokeSettings {
    upstream: SmokeUpstream,
    model: String,
}

pub(super) struct SmokeSettings {
    default_upstream: SmokeUpstream,
    websocket_upstream: SmokeUpstream,
    images: Option<ImagesSmokeSettings>,
    search: Option<SearchSmokeSettings>,
    pub(super) chat_completions_model: String,
    pub(super) responses_model: String,
    responses_profile: ResponsesUpstreamProfile,
    timeout: Duration,
}

impl SmokeSettings {
    pub(super) fn from_environment() -> Self {
        assert_eq!(
            optional_environment("RUN_REAL_UPSTREAM_SMOKE").as_deref(),
            Some("1"),
            "set RUN_REAL_UPSTREAM_SMOKE=1 to permit a real upstream request"
        );
        let timeout_seconds = optional_environment("REAL_UPSTREAM_TIMEOUT_SECONDS")
            .map(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value >= 3)
                    .expect("REAL_UPSTREAM_TIMEOUT_SECONDS must be an integer of at least 3")
            })
            .unwrap_or(60);
        let default_upstream = SmokeUpstream {
            base_url: required_environment("REAL_UPSTREAM_BASE_URL"),
            api_key: required_environment("REAL_UPSTREAM_API_KEY"),
        };
        let websocket_upstream = paired_upstream_override(
            optional_environment("REAL_UPSTREAM_WEBSOCKET_BASE_URL"),
            optional_environment("REAL_UPSTREAM_WEBSOCKET_API_KEY"),
            &default_upstream,
            "REAL_UPSTREAM_WEBSOCKET_BASE_URL and REAL_UPSTREAM_WEBSOCKET_API_KEY must be set together",
        )
        .unwrap_or_else(|message| panic!("{message}"));
        let images = optional_images_settings(
            optional_environment("REAL_UPSTREAM_IMAGES_BASE_URL"),
            optional_environment("REAL_UPSTREAM_IMAGES_API_KEY"),
            optional_environment("REAL_UPSTREAM_IMAGES_MODEL"),
        )
        .unwrap_or_else(|message| panic!("{message}"));
        let search = optional_search_settings(
            optional_environment("REAL_UPSTREAM_SEARCH_BASE_URL"),
            optional_environment("REAL_UPSTREAM_SEARCH_API_KEY"),
            optional_environment("REAL_UPSTREAM_SEARCH_MODEL"),
        )
        .unwrap_or_else(|message| panic!("{message}"));
        let responses_profile = ResponsesUpstreamProfile::from_environment(optional_environment(
            "REAL_UPSTREAM_RESPONSES_PROFILE",
        ))
        .unwrap_or_else(|message| panic!("{message}"));
        Self {
            default_upstream,
            websocket_upstream,
            images,
            search,
            chat_completions_model: required_environment("REAL_UPSTREAM_CHAT_COMPLETIONS_MODEL"),
            responses_model: required_environment("REAL_UPSTREAM_RESPONSES_MODEL"),
            responses_profile,
            timeout: Duration::from_secs(timeout_seconds),
        }
    }
}

fn required_environment(name: &str) -> String {
    optional_environment(name).unwrap_or_else(|| panic!("{name} must be set"))
}

fn optional_environment(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn paired_upstream_override(
    base_url: Option<String>,
    api_key: Option<String>,
    fallback: &SmokeUpstream,
    partial_error: &'static str,
) -> Result<SmokeUpstream, &'static str> {
    match (base_url, api_key) {
        (None, None) => Ok(fallback.clone()),
        (Some(base_url), Some(api_key)) => Ok(SmokeUpstream { base_url, api_key }),
        _ => Err(partial_error),
    }
}

fn optional_images_settings(
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
) -> Result<Option<ImagesSmokeSettings>, &'static str> {
    match (base_url, api_key, model) {
        (None, None, None) => Ok(None),
        (Some(base_url), Some(api_key), Some(model)) => Ok(Some(ImagesSmokeSettings {
            upstream: SmokeUpstream { base_url, api_key },
            model,
        })),
        _ => Err(
            "REAL_UPSTREAM_IMAGES_BASE_URL, REAL_UPSTREAM_IMAGES_API_KEY, and REAL_UPSTREAM_IMAGES_MODEL must be set together",
        ),
    }
}

fn optional_search_settings(
    base_url: Option<String>,
    api_key: Option<String>,
    model: Option<String>,
) -> Result<Option<SearchSmokeSettings>, &'static str> {
    match (base_url, api_key, model) {
        (None, None, None) => Ok(None),
        (Some(base_url), Some(api_key), Some(model)) => Ok(Some(SearchSmokeSettings {
            upstream: SmokeUpstream { base_url, api_key },
            model,
        })),
        _ => Err(
            "REAL_UPSTREAM_SEARCH_BASE_URL, REAL_UPSTREAM_SEARCH_API_KEY, and REAL_UPSTREAM_SEARCH_MODEL must be set together",
        ),
    }
}

struct SmokeGateway {
    app: axum::Router,
    logs: RecordingRequestLogSink,
}

struct NonStreamingResult {
    value: Value,
    elapsed: Duration,
}

struct SmokeServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for SmokeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start_gateway_server(app: axum::Router) -> SmokeServer {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind real-upstream websocket smoke gateway");
    let address = listener
        .local_addr()
        .expect("real-upstream websocket smoke gateway address");
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve real-upstream websocket smoke gateway");
    });
    SmokeServer { address, task }
}

fn gateway(
    settings: &SmokeSettings,
    upstream_settings: &SmokeUpstream,
    format: SmokeFormat,
    client_model: &str,
    upstream_model: &str,
) -> SmokeGateway {
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let logs = RecordingRequestLogSink::default();
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            user_websocket_enabled: true,
            secret_value: CLIENT_KEY.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec![format.api_format_name().into()],
            permissions: vec!["proxy".into(), "models.read".into()],
            allowed_group_ids: vec![group_id],
            allowed_channel_ids: vec![],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Default::default(),
        }],
        groups: vec![ChannelGroupRecord {
            id: group_id,
            name: "real-upstream-smoke".into(),
            api_format: format.api_format_name().into(),
            connector_kind: "openai_compatible".into(),
            priority: 0,
            selection_strategy: "weighted_random".into(),
            enabled: true,
        }],
        channels: vec![ChannelRecord {
            id: channel_id,
            channel_group_id: group_id,
            api_format: format.api_format_name().into(),
            name: "real-upstream-smoke".into(),
            base_url: upstream_settings.base_url.clone(),
            enabled: true,
            supports_websocket: matches!(format, SmokeFormat::Responses),
            supports_standalone_web_search: matches!(format, SmokeFormat::StandaloneWebSearch),
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
            upstream_api_key: Some(upstream_settings.api_key.clone()),
            available_models: vec![upstream_model.into()],
            test_model: None,
        }],
        models: vec![],
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: client_model.into(),
            api_format: format.api_format_name().into(),
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
            upstream_model: upstream_model.into(),
            channel_group_ids: vec![],
            channel_ids: vec![channel_id],
            enabled: true,
        }],
        proxies: vec![],
        templates: vec![],
    };
    let upstream = UpstreamTimeoutDefaults::new(
        Duration::from_secs(settings.timeout.as_secs().saturating_sub(1).clamp(1, 10)),
        settings.timeout,
        settings.timeout,
    );
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane_with_system_settings(
            records,
            SystemRuntimeSettings::new_with_websocket(
                upstream,
                PassiveHealthSettings::default(),
                ResponsesWebSocketSettings::new(
                    true,
                    128,
                    Duration::from_secs(5 * 60),
                    Duration::from_secs(55 * 60),
                ),
            ),
        )
        .expect("the smoke-test route must compile"),
    ));
    let proxy = ProxyService::with_log_sink(runtime, 1_048_576, Arc::new(logs.clone()))
        .expect("the smoke-test upstream client must build");
    SmokeGateway {
        app: http::router(proxy),
        logs,
    }
}

fn request(format: SmokeFormat, streamed: bool) -> Request<Body> {
    Request::post(format.path())
        .header(header::AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::to_vec(&format.request_body(streamed))
                .expect("smoke-test request JSON serializes"),
        ))
        .expect("smoke-test request builds")
}

fn responses_websocket_request_body(
    profile: ResponsesUpstreamProfile,
    session_id: &str,
    thread_id: &str,
) -> Value {
    let mut body = json!({
        "type": "response.create",
        "model": CLIENT_MODEL,
        "instructions": "Reply exactly OK.",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "Reply exactly OK."}],
        }],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "reasoning": null,
        "store": false,
        "stream": true,
        "include": [],
        "client_metadata": {
            "session_id": session_id,
            "thread_id": thread_id,
        },
    });
    if profile == ResponsesUpstreamProfile::CodexOauth {
        body["max_output_tokens"] = json!(1);
    }
    body
}

/// Makes one small JSON request for one API format. The Codex profile verifies
/// its documented non-streaming rejection without reaching generation. The
/// helper creates no database records and does not use process TOML.
pub(super) async fn smoke_nonstreaming_format(
    settings: &SmokeSettings,
    format: SmokeFormat,
    upstream_model: &str,
) {
    let gateway = gateway(
        settings,
        &settings.default_upstream,
        format,
        CLIENT_MODEL,
        upstream_model,
    );
    let request = request(format, false);
    if matches!(format, SmokeFormat::Responses)
        && settings.responses_profile == ResponsesUpstreamProfile::CodexOauth
    {
        assert_codex_nonstreaming_rejection(settings, gateway, request).await;
        return;
    }
    let _ =
        complete_nonstreaming_request(settings, gateway, request, format, format.api_operation())
            .await;
}

pub(super) async fn smoke_standalone_web_search(settings: &SmokeSettings) {
    let search = settings
        .search
        .as_ref()
        .expect("standalone web-search smoke settings must be configured");
    let gateway = gateway(
        settings,
        &search.upstream,
        SmokeFormat::StandaloneWebSearch,
        CLIENT_MODEL,
        &search.model,
    );
    let request = Request::post(SmokeFormat::StandaloneWebSearch.path())
        .header(header::AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("originator", "codex_cli_rs")
        .header(
            "x-codex-turn-metadata",
            r#"{"search_context_size":"low","model_id":"gateway-real-upstream-smoke-model"}"#,
        )
        .body(Body::from(
            serde_json::to_vec(&SmokeFormat::StandaloneWebSearch.request_body(false))
                .expect("standalone web-search smoke request serializes"),
        ))
        .expect("standalone web-search smoke request builds");
    let started = Instant::now();
    let response = timeout(settings.timeout, gateway.app.oneshot(request))
        .await
        .expect("standalone web-search smoke timed out")
        .expect("standalone web-search gateway request failed");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "standalone web-search upstream returned a non-success status"
    );
    let bytes = timeout(settings.timeout, response.into_body().collect())
        .await
        .expect("standalone web-search response body timed out")
        .expect("standalone web-search response body failed")
        .to_bytes();
    let value: Value =
        serde_json::from_slice(&bytes).expect("standalone web-search response must be JSON");
    let output = value
        .get("output")
        .and_then(Value::as_str)
        .filter(|output| !output.trim().is_empty())
        .expect("standalone web-search response must contain nonempty output");
    let result_count = value
        .get("results")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    let events = gateway.logs.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.api_format, ApiFormat::OpenAiResponses);
    assert_eq!(event.api_operation, ApiOperation::StandaloneWebSearch);
    assert_eq!(event.request_protocol, RequestProtocol::NonStream);
    assert_eq!(event.outcome, RequestLogOutcome::Succeeded);
    assert!(
        event.billing.is_some(),
        "the selected search route must retain its price snapshot"
    );

    eprintln!(
        "standalone web-search smoke succeeded: elapsed_ms={} output_chars={} results={result_count}",
        started.elapsed().as_millis(),
        output.chars().count(),
    );
}

async fn complete_nonstreaming_request(
    settings: &SmokeSettings,
    gateway: SmokeGateway,
    request: Request<Body>,
    format: SmokeFormat,
    operation: ApiOperation,
) -> NonStreamingResult {
    let started = Instant::now();
    let response = timeout(settings.timeout, gateway.app.oneshot(request))
        .await
        .expect("non-streaming gateway request timed out")
        .expect("non-streaming gateway request completed");
    let status = response.status();
    let body = timeout(settings.timeout, response.into_body().collect())
        .await
        .expect("non-streaming upstream body timed out")
        .expect("non-streaming upstream body completed")
        .to_bytes();
    assert!(
        status.is_success(),
        "the real upstream returned non-success status {status}; {}",
        sanitized_error_details(&body)
    );
    let value: Value =
        serde_json::from_slice(&body).expect("the real upstream response must be JSON");
    assert!(
        value.is_object(),
        "the real upstream response JSON must be an object"
    );
    assert_response_has_usage(format, &value);
    assert_usage_was_logged(
        &gateway.logs.events(),
        format,
        operation,
        RequestProtocol::NonStream,
    );
    NonStreamingResult {
        value,
        elapsed: started.elapsed(),
    }
}

async fn assert_codex_nonstreaming_rejection(
    settings: &SmokeSettings,
    gateway: SmokeGateway,
    request: Request<Body>,
) {
    let response = timeout(settings.timeout, gateway.app.oneshot(request))
        .await
        .expect("Codex non-streaming gateway request timed out")
        .expect("Codex non-streaming gateway request completed");
    let status = response.status();
    let body = timeout(settings.timeout, response.into_body().collect())
        .await
        .expect("Codex non-streaming upstream body timed out")
        .expect("Codex non-streaming upstream body completed")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Codex-backed Responses non-streaming must fail with HTTP 400; {}",
        sanitized_error_details(&body)
    );
    let value: Value =
        serde_json::from_slice(&body).expect("Codex non-streaming rejection must be JSON");
    assert_eq!(
        value.pointer("/error/code").and_then(Value::as_str),
        Some("codex_streaming_required"),
        "Codex non-streaming rejection must retain the gateway error code"
    );

    let events = gateway.logs.events();
    assert_eq!(
        events.len(),
        1,
        "the Codex non-streaming rejection must produce exactly one terminal log"
    );
    let event = &events[0];
    assert_eq!(event.api_format, SmokeFormat::Responses.api_format());
    assert_eq!(event.api_operation, ApiOperation::Responses);
    assert_eq!(event.request_protocol, RequestProtocol::NonStream);
    assert!(!event.streamed);
    assert_eq!(event.outcome, RequestLogOutcome::Failed);
    assert_eq!(
        event.response_status_code,
        Some(StatusCode::BAD_REQUEST.as_u16())
    );
    assert_eq!(
        event.error_code.as_deref(),
        Some("codex_streaming_required")
    );
    let summary = event.error_summary.as_deref().unwrap();
    assert!(summary.contains("\"code\": \"codex_streaming_required\""));
    assert!(summary.contains("\"param\": \"stream\""));
    assert_eq!(
        event.billing.as_ref().and_then(|billing| billing.usage),
        None
    );
    println!(
        "Responses non-streaming correctly returned 400 codex_streaming_required for the Codex profile"
    );
}

/// Makes one small, paid SSE request for one API format and fully consumes the
/// stream so the terminal request log includes the upstream's final usage.
pub(super) async fn smoke_streaming_format(
    settings: &SmokeSettings,
    format: SmokeFormat,
    upstream_model: &str,
) {
    let gateway = gateway(
        settings,
        &settings.default_upstream,
        format,
        CLIENT_MODEL,
        upstream_model,
    );
    let response = timeout(settings.timeout, gateway.app.oneshot(request(format, true)))
        .await
        .expect("streaming gateway request timed out")
        .expect("streaming gateway request completed");
    let status = response.status();
    if !status.is_success() {
        let body = timeout(settings.timeout, response.into_body().collect())
            .await
            .expect("streaming upstream error body timed out")
            .expect("streaming upstream error body completed")
            .to_bytes();
        panic!(
            "the real upstream returned non-success status {status} for a streaming request; {}",
            sanitized_error_details(&body)
        );
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    assert!(
        content_type.is_some_and(|value| value
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))),
        "the real upstream streaming response must use text/event-stream, got {content_type:?}"
    );
    let mut body = response.into_body();
    let first = timeout(settings.timeout, body.frame())
        .await
        .expect("the real upstream did not send a streaming frame in time")
        .expect("the real upstream streaming response ended before its first frame")
        .expect("the real upstream streaming response failed");
    let bytes: Bytes = first
        .into_data()
        .expect("the real upstream first streaming frame must contain data");
    assert!(
        !bytes.is_empty(),
        "the real upstream first streaming frame must not be empty"
    );
    let mut raw_sse = bytes.to_vec();
    let remainder = timeout(settings.timeout, body.collect())
        .await
        .expect("the real upstream streaming body did not finish in time")
        .expect("the real upstream streaming body failed")
        .to_bytes();
    raw_sse.extend_from_slice(&remainder);
    let events = gateway.logs.events();
    assert_usage_was_logged(
        &events,
        format,
        format.api_operation(),
        RequestProtocol::Sse,
    );
    assert_streaming_usage_matches_terminal_sse_event(&events, format, &raw_sse);
}

pub(super) async fn smoke_images_generation(settings: &SmokeSettings) {
    let images = settings
        .images
        .as_ref()
        .expect("configure all REAL_UPSTREAM_IMAGES_* settings to run Images smoke tests");
    let gateway = gateway(
        settings,
        &images.upstream,
        SmokeFormat::Images,
        CLIENT_MODEL,
        &images.model,
    );
    let result = complete_nonstreaming_request(
        settings,
        gateway,
        request(SmokeFormat::Images, false),
        SmokeFormat::Images,
        ApiOperation::ImagesGeneration,
    )
    .await;
    report_images_result("generation", &result);
}

pub(super) async fn smoke_images_edit(settings: &SmokeSettings) {
    let images = settings
        .images
        .as_ref()
        .expect("configure all REAL_UPSTREAM_IMAGES_* settings to run Images smoke tests");
    let gateway = gateway(
        settings,
        &images.upstream,
        SmokeFormat::Images,
        CLIENT_MODEL,
        &images.model,
    );
    let result = complete_nonstreaming_request(
        settings,
        gateway,
        images_edit_request(),
        SmokeFormat::Images,
        ApiOperation::ImagesEdit,
    )
    .await;
    report_images_result("edit", &result);
}

fn report_images_result(operation: &str, result: &NonStreamingResult) {
    let outputs = assert_images_response_has_output(&result.value);
    let usage = result
        .value
        .get("usage")
        .expect("Images smoke response usage was already validated");
    let input_tokens = usage
        .get("input_tokens")
        .and_then(Value::as_i64)
        .expect("Images smoke input_tokens was already validated");
    let output_tokens = usage
        .get("output_tokens")
        .and_then(Value::as_i64)
        .expect("Images smoke output_tokens was already validated");
    println!(
        "Images {operation} completed in {} ms with {outputs} output(s), input_tokens={input_tokens}, output_tokens={output_tokens}",
        result.elapsed.as_millis()
    );
}

fn assert_images_response_has_output(value: &Value) -> usize {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .filter(|data| !data.is_empty())
        .expect("the real Images response must include a nonempty data array");
    for item in data {
        let has_base64 = item
            .get("b64_json")
            .and_then(Value::as_str)
            .is_some_and(|value| value.len() >= 128);
        let has_url = item
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(|value| {
                value.len() >= 16 && (value.starts_with("https://") || value.starts_with("http://"))
            });
        assert!(
            has_base64 || has_url,
            "each real Images output must contain a nontrivial b64_json payload or HTTP(S) URL"
        );
    }
    data.len()
}

fn images_edit_request() -> Request<Body> {
    const BOUNDARY: &str = "ai-gateway-real-upstream-image-edit";

    let image = BASE64_STANDARD
        .decode(IMAGE_EDIT_PNG_BASE64.trim())
        .expect("embedded Images edit PNG must decode");
    let mut body = Vec::new();
    for (name, value) in [
        ("model", CLIENT_MODEL),
        ("prompt", "Keep this image visually unchanged."),
        ("n", "1"),
        ("quality", "low"),
        ("size", "1024x1024"),
    ] {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"image\"; filename=\"input.png\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(&image);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());

    Request::post("/v1/images/edits")
        .header(header::AUTHORIZATION, format!("Bearer {CLIENT_KEY}"))
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("Images edit smoke-test request builds")
}

/// Sends one deterministic Responses WebSocket request through a real TCP
/// listener so both downstream and upstream upgrade paths are exercised.
pub(super) async fn smoke_responses_websocket(settings: &SmokeSettings, upstream_model: &str) {
    let gateway = gateway(
        settings,
        &settings.websocket_upstream,
        SmokeFormat::Responses,
        CLIENT_MODEL,
        upstream_model,
    );
    let server = start_gateway_server(gateway.app).await;
    let session_id = Uuid::new_v4().to_string();
    let thread_id = Uuid::new_v4().to_string();
    let mut attempts = 0;
    let completed = loop {
        attempts += 1;
        match send_responses_websocket_attempt(settings, server.address, &session_id, &thread_id)
            .await
        {
            Ok(completed) => break completed,
            Err(()) if attempts < MAX_WEBSOCKET_ATTEMPTS => {
                sleep(Duration::from_millis(250 * attempts as u64)).await;
            }
            Err(()) => {
                panic!(
                    "real-upstream websocket closed upstream on all {MAX_WEBSOCKET_ATTEMPTS} client attempts"
                );
            }
        }
    };

    let events = gateway.logs.events();
    assert_eq!(
        events.len(),
        attempts,
        "each manual WebSocket client attempt must produce one terminal log"
    );
    for event in &events[..attempts - 1] {
        assert_eq!(event.api_format, SmokeFormat::Responses.api_format());
        assert_eq!(event.request_protocol, RequestProtocol::WebSocket);
        assert!(event.streamed);
        assert_eq!(event.outcome, RequestLogOutcome::Failed);
        assert!(
            event
                .response_status_code
                .is_none_or(|status| status == 502)
        );
        assert_eq!(
            event.error_code.as_deref(),
            Some("upstream_websocket_closed")
        );
    }
    let successful_event = events
        .last()
        .expect("manual WebSocket smoke must retain a successful terminal log");
    assert_usage_was_logged(
        std::slice::from_ref(successful_event),
        SmokeFormat::Responses,
        ApiOperation::Responses,
        RequestProtocol::WebSocket,
    );
    let expected = usage_from_sse_value(SmokeFormat::Responses, &completed)
        .expect("real-upstream websocket response.completed must include usage");
    assert_eq!(
        successful_event
            .billing
            .as_ref()
            .and_then(|billing| billing.usage),
        Some(expected)
    );
}

async fn send_responses_websocket_attempt(
    settings: &SmokeSettings,
    address: SocketAddr,
    session_id: &str,
    thread_id: &str,
) -> Result<Value, ()> {
    let mut request = format!("ws://{address}/v1/responses")
        .into_client_request()
        .expect("real-upstream websocket smoke request builds");
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer gateway-real-upstream-smoke-client-key"),
    );
    request.headers_mut().insert(
        "openai-beta",
        HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    request.headers_mut().insert(
        "session-id",
        HeaderValue::from_str(session_id).expect("smoke session id is a valid header"),
    );
    request.headers_mut().insert(
        "thread-id",
        HeaderValue::from_str(thread_id).expect("smoke thread id is a valid header"),
    );
    request.headers_mut().insert(
        "x-client-request-id",
        HeaderValue::from_str(thread_id).expect("smoke client request id is a valid header"),
    );
    request.headers_mut().insert(
        "originator",
        HeaderValue::from_static("ai-gateway-real-upstream-smoke"),
    );
    request.headers_mut().insert(
        header::USER_AGENT,
        HeaderValue::from_static(concat!(
            "ai-gateway-real-upstream-smoke/",
            env!("CARGO_PKG_VERSION")
        )),
    );
    let (mut websocket, response) = timeout(settings.timeout, connect_async(request))
        .await
        .expect("real-upstream websocket upgrade timed out")
        .expect("real-upstream websocket upgrade failed");
    assert_eq!(response.status(), 101);
    let body = responses_websocket_request_body(settings.responses_profile, session_id, thread_id);
    websocket
        .send(Message::Text(body.to_string().into()))
        .await
        .expect("real-upstream websocket request send failed");

    let completed = loop {
        let message = timeout(settings.timeout, websocket.next())
            .await
            .expect("real-upstream websocket event timed out")
            .expect("real-upstream websocket closed before response.completed")
            .expect("real-upstream websocket event failed");
        let Message::Text(text) = message else {
            continue;
        };
        let event: Value =
            serde_json::from_str(&text).expect("real-upstream websocket event must be JSON");
        match event.get("type").and_then(Value::as_str) {
            Some("response.completed") => break Ok(event),
            Some("error" | "response.failed" | "response.incomplete" | "response.cancelled") => {
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("<missing>");
                let status = event.get("status").and_then(Value::as_u64);
                let code = event
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .map(safe_error_label)
                    .unwrap_or_else(|| "<missing>".into());
                if kind == "error" && status == Some(502) && code == "upstream_websocket_closed" {
                    break Err(());
                }
                panic!(
                    "real-upstream websocket returned terminal event type={kind} status={status:?} code={code}"
                );
            }
            _ => {}
        }
    };
    let _ = websocket.close(None).await;
    completed
}

fn safe_error_label(value: &str) -> String {
    if value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        value.to_owned()
    } else {
        "<redacted>".into()
    }
}

fn sanitized_error_details(body: &[u8]) -> String {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return "error_body=non_json".into();
    };
    let error = value.get("error").unwrap_or(&value);
    let mut fields = Vec::new();
    for name in ["type", "code", "param"] {
        if let Some(value) = error.get(name).and_then(Value::as_str) {
            fields.push(format!("{name}={}", safe_error_label(value)));
        }
    }
    if fields.is_empty() {
        "error_body=json_without_safe_fields".into()
    } else {
        fields.join(" ")
    }
}

fn assert_response_has_usage(format: SmokeFormat, value: &Value) {
    let usage = match format {
        SmokeFormat::ChatCompletions => value.get("usage"),
        SmokeFormat::Responses => value.get("usage").or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        }),
        SmokeFormat::StandaloneWebSearch => return,
        SmokeFormat::Images => value.get("usage"),
    };
    assert!(
        usage.is_some_and(Value::is_object),
        "the real upstream non-streaming response must include a usage object"
    );
}

fn assert_usage_was_logged(
    events: &[RequestLogEvent],
    format: SmokeFormat,
    operation: ApiOperation,
    request_protocol: RequestProtocol,
) {
    assert_eq!(
        events.len(),
        1,
        "the real upstream request must produce exactly one terminal log"
    );
    let event = &events[0];
    assert_eq!(event.api_format, format.api_format());
    assert_eq!(event.api_operation, operation);
    assert_eq!(event.request_protocol, request_protocol);
    assert_eq!(event.streamed, request_protocol.is_streamed());
    assert_eq!(event.outcome, RequestLogOutcome::Succeeded);
    assert!(
        event
            .response_status_code
            .is_some_and(|status| (200..300).contains(&status)),
        "the terminal log must retain the successful client-visible status"
    );
    assert_eq!(event.error_code, None);

    let billing = event
        .billing
        .as_ref()
        .expect("a selected real-upstream route must retain its price snapshot");
    let usage = billing
        .usage
        .as_ref()
        .expect("the real upstream usage must be extracted into the terminal log");
    // Some OpenAI-compatible streaming providers report zero prompt tokens
    // even when they provide a final usage object. Presence plus a positive
    // generated-token count still proves the format-specific extraction path.
    assert!(usage.output_tokens > 0);
    assert!(usage.cached_input_tokens <= usage.input_tokens);
    assert!(usage.cache_write_tokens <= usage.input_tokens);
    assert!(usage.reasoning_tokens <= usage.output_tokens);

    assert_eq!(billing.price.currency, "USD");
    assert_eq!(billing.price.price_unit_tokens, 1_000_000);
    assert_eq!(billing.price.input_unit_price, Decimal::ONE);
    assert_eq!(billing.price.cached_input_unit_price, Decimal::new(5, 1));
    assert_eq!(billing.price.cache_write_unit_price, Decimal::new(25, 2));
    assert_eq!(billing.price.output_unit_price, Decimal::from(2_i64));
    assert!(
        billing
            .cost_amount
            .is_some_and(|amount| amount > Decimal::ZERO),
        "usage with the configured nonzero price snapshot must have a positive cost"
    );
    match format {
        SmokeFormat::Images => {
            assert_eq!(
                billing.output_tokens_per_second, None,
                "Images request logs do not derive output TPS"
            );
        }
        SmokeFormat::ChatCompletions
        | SmokeFormat::Responses
        | SmokeFormat::StandaloneWebSearch => {
            assert!(
                billing
                    .output_tokens_per_second
                    .is_some_and(|tps| tps > Decimal::ZERO),
                "a nonempty text response with output tokens must have positive output TPS"
            );
        }
    }
}

fn assert_streaming_usage_matches_terminal_sse_event(
    events: &[RequestLogEvent],
    format: SmokeFormat,
    bytes: &[u8],
) {
    let expected = terminal_sse_usage(format, bytes)
        .expect("the real upstream SSE stream must contain a compatible usage object");
    let actual = events[0]
        .billing
        .as_ref()
        .and_then(|billing| billing.usage)
        .expect("the terminal request log must retain upstream usage");
    assert_eq!(
        actual, expected,
        "the logged usage must match the terminal upstream SSE usage"
    );
}

fn terminal_sse_usage(format: SmokeFormat, bytes: &[u8]) -> Option<RequestUsage> {
    let mut remaining = bytes;
    let mut latest = None;
    let mut terminal = None;
    while !remaining.is_empty() {
        let (frame, next) = match sse_frame_end(remaining) {
            Some(end) => (&remaining[..end], &remaining[end..]),
            None => (remaining, &[][..]),
        };
        remaining = next;
        let Some(value) = sse_frame_json(frame) else {
            continue;
        };
        let Some(usage) = usage_from_sse_value(format, &value) else {
            continue;
        };
        if is_terminal_sse_usage(format, &value) {
            terminal = Some(usage);
        }
        latest = Some(usage);
    }
    terminal.or(latest)
}

fn sse_frame_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|end| end + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|end| end + 4)
        })
}

fn sse_frame_json(frame: &[u8]) -> Option<Value> {
    let mut data = Vec::new();
    for line in frame.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        let value = value.strip_prefix(b" ").unwrap_or(value);
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value);
    }
    (!data.is_empty() && data.as_slice() != b"[DONE]")
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn usage_from_sse_value(format: SmokeFormat, value: &Value) -> Option<RequestUsage> {
    if matches!(format, SmokeFormat::StandaloneWebSearch) {
        return None;
    }
    let usage = value
        .get("usage")
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        })
        .unwrap_or(value);
    let (input_field, output_field, input_details_field, output_details_field) = match format {
        SmokeFormat::ChatCompletions => (
            "prompt_tokens",
            "completion_tokens",
            "prompt_tokens_details",
            "completion_tokens_details",
        ),
        SmokeFormat::Responses => (
            "input_tokens",
            "output_tokens",
            "input_tokens_details",
            "output_tokens_details",
        ),
        SmokeFormat::StandaloneWebSearch => unreachable!("handled above"),
        SmokeFormat::Images => (
            "input_tokens",
            "output_tokens",
            "input_tokens_details",
            "output_tokens_details",
        ),
    };
    let input_tokens = nonnegative_token(usage.get(input_field))?;
    let output_tokens = nonnegative_token(usage.get(output_field))?;
    let input_details = usage.get(input_details_field);
    let cached_input_tokens = match format {
        SmokeFormat::ChatCompletions => nonnegative_token(usage.get("prompt_cache_hit_tokens"))
            .or_else(|| {
                input_details.and_then(|details| nonnegative_token(details.get("cached_tokens")))
            }),
        SmokeFormat::Responses | SmokeFormat::Images => {
            input_details.and_then(|details| nonnegative_token(details.get("cached_tokens")))
        }
        SmokeFormat::StandaloneWebSearch => unreachable!("handled above"),
    }
    .unwrap_or(0);
    let cache_write_tokens = input_details
        .and_then(|details| {
            nonnegative_token(details.get("cache_write_tokens"))
                .or_else(|| nonnegative_token(details.get("cache_creation_tokens")))
        })
        .unwrap_or(0);
    let reasoning_tokens = usage
        .get(output_details_field)
        .and_then(|details| nonnegative_token(details.get("reasoning_tokens")))
        .unwrap_or(0);
    (cached_input_tokens <= input_tokens
        && cache_write_tokens <= input_tokens
        && reasoning_tokens <= output_tokens)
        .then_some(RequestUsage {
            input_tokens,
            cached_input_tokens,
            cache_write_tokens,
            output_tokens,
            reasoning_tokens,
        })
}

fn nonnegative_token(value: Option<&Value>) -> Option<i64> {
    value?.as_i64().filter(|value| *value >= 0)
}

fn is_terminal_sse_usage(format: SmokeFormat, value: &Value) -> bool {
    match format {
        SmokeFormat::ChatCompletions => {
            value
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(|choices| {
                    choices.is_empty()
                        || choices.iter().any(|choice| {
                            choice
                                .get("finish_reason")
                                .is_some_and(|reason| !reason.is_null())
                        })
                })
        }
        SmokeFormat::Responses => {
            value.get("type").and_then(Value::as_str) == Some("response.completed")
        }
        SmokeFormat::StandaloneWebSearch => false,
        SmokeFormat::Images => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{OriginalUri, State},
        http::HeaderMap,
        response::IntoResponse,
        routing::post,
    };
    use std::sync::Mutex;

    #[derive(Clone, Debug)]
    struct CapturedImagesRequest {
        path: String,
        authorization: Option<String>,
        content_type: Option<String>,
        body: Bytes,
    }

    fn fallback_upstream() -> SmokeUpstream {
        SmokeUpstream {
            base_url: "https://default.example.invalid".into(),
            api_key: "default-key".into(),
        }
    }

    async fn capture_images_request(
        State(captured): State<Arc<Mutex<Vec<CapturedImagesRequest>>>>,
        OriginalUri(uri): OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Json<Value> {
        captured.lock().unwrap().push(CapturedImagesRequest {
            path: uri.path().into(),
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            content_type: headers
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned),
            body,
        });
        Json(json!({
            "created": 1,
            "data": [{"b64_json": BASE64_STANDARD.encode([0_u8; 96])}],
            "usage": {
                "input_tokens": 7,
                "output_tokens": 11,
                "input_tokens_details": {
                    "cached_tokens": 0,
                },
                "output_tokens_details": {
                    "reasoning_tokens": 0,
                },
            },
        }))
    }

    async fn mock_codex_responses(
        State(captured): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> axum::response::Response {
        captured.lock().unwrap().push(body.clone());
        if body.get("stream").and_then(Value::as_bool) != Some(true) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": "Codex OAuth channels currently require `stream: true`.",
                        "type": "invalid_request_error",
                        "param": "stream",
                        "code": "codex_streaming_required",
                    }
                })),
            )
                .into_response();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        let event = json!({
            "type": "response.completed",
            "response": {
                "id": "resp_codex_smoke",
                "usage": {
                    "input_tokens": 3,
                    "output_tokens": 1,
                    "input_tokens_details": {
                        "cached_tokens": 0,
                    },
                    "output_tokens_details": {
                        "reasoning_tokens": 0,
                    },
                },
            },
        });
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            format!("data: {event}\n\n"),
        )
            .into_response()
    }

    #[test]
    fn websocket_override_is_optional_but_must_be_complete() {
        let fallback = fallback_upstream();
        let inherited = paired_upstream_override(None, None, &fallback, "partial").unwrap();
        assert_eq!(inherited.base_url, fallback.base_url);
        assert_eq!(inherited.api_key, fallback.api_key);
        let overridden = paired_upstream_override(
            Some("https://websocket.example.invalid".into()),
            Some("websocket-key".into()),
            &fallback,
            "partial",
        )
        .unwrap();
        assert_eq!(overridden.base_url, "https://websocket.example.invalid");
        assert_eq!(overridden.api_key, "websocket-key");
        assert!(
            paired_upstream_override(
                Some("https://websocket.example.invalid".into()),
                None,
                &fallback,
                "partial",
            )
            .is_err()
        );
    }

    #[test]
    fn images_settings_are_optional_but_must_be_complete() {
        assert!(
            optional_images_settings(None, None, None)
                .unwrap()
                .is_none()
        );
        let configured = optional_images_settings(
            Some("https://images.example.invalid".into()),
            Some("images-key".into()),
            Some("images-model".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            configured.upstream.base_url,
            "https://images.example.invalid"
        );
        assert_eq!(configured.upstream.api_key, "images-key");
        assert_eq!(configured.model, "images-model");
        assert!(
            optional_images_settings(
                Some("https://images.example.invalid".into()),
                None,
                Some("images-model".into()),
            )
            .is_err()
        );
    }

    #[test]
    fn search_settings_are_optional_but_must_be_complete() {
        assert!(
            optional_search_settings(None, None, None)
                .unwrap()
                .is_none()
        );
        let configured = optional_search_settings(
            Some("https://search.example.invalid".into()),
            Some("search-key".into()),
            Some("search-model".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            configured.upstream.base_url,
            "https://search.example.invalid"
        );
        assert_eq!(configured.upstream.api_key, "search-key");
        assert_eq!(configured.model, "search-model");
        assert!(
            optional_search_settings(
                Some("https://search.example.invalid".into()),
                None,
                Some("search-model".into()),
            )
            .is_err()
        );
    }

    #[test]
    fn standalone_search_request_uses_the_typed_commands_object() {
        let body = SmokeFormat::StandaloneWebSearch.request_body(false);
        assert_eq!(
            body["commands"]["search_query"][0]["q"],
            "official Rust programming language"
        );
        assert_eq!(
            body["commands"]["search_query"][0]["domains"][0],
            "rust-lang.org"
        );
        assert!(body["commands"].is_object());
    }

    #[test]
    fn responses_requests_use_explicit_message_arrays() {
        let body = SmokeFormat::Responses.request_body(false);
        assert_eq!(body["input"][0]["type"], "message");
        assert_eq!(body["input"][0]["role"], "user");
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["max_output_tokens"], 1);
    }

    #[test]
    fn responses_profile_defaults_and_validates_known_values() {
        assert_eq!(
            ResponsesUpstreamProfile::from_environment(None).unwrap(),
            ResponsesUpstreamProfile::OpenAiCompatible
        );
        assert_eq!(
            ResponsesUpstreamProfile::from_environment(Some("openai_compatible".into())).unwrap(),
            ResponsesUpstreamProfile::OpenAiCompatible
        );
        assert_eq!(
            ResponsesUpstreamProfile::from_environment(Some("codex_oauth".into())).unwrap(),
            ResponsesUpstreamProfile::CodexOauth
        );
        assert!(ResponsesUpstreamProfile::from_environment(Some("unknown".into())).is_err());

        let standard = responses_websocket_request_body(
            ResponsesUpstreamProfile::OpenAiCompatible,
            "session",
            "thread",
        );
        assert!(standard.get("max_output_tokens").is_none());
        let codex = responses_websocket_request_body(
            ResponsesUpstreamProfile::CodexOauth,
            "session",
            "thread",
        );
        assert_eq!(codex["max_output_tokens"], 1);
    }

    #[test]
    fn embedded_images_edit_fixture_is_a_1024_square_png() {
        let image = BASE64_STANDARD
            .decode(IMAGE_EDIT_PNG_BASE64.trim())
            .unwrap();
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(image[16..20].try_into().unwrap()), 1024);
        assert_eq!(u32::from_be_bytes(image[20..24].try_into().unwrap()), 1024);
    }

    #[tokio::test]
    async fn images_smoke_uses_dedicated_target_for_generation_and_edit() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/images/generations", post(capture_images_request))
            .route("/v1/images/edits", post(capture_images_request))
            .with_state(Arc::clone(&captured));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let images_upstream = SmokeUpstream {
            base_url: format!("http://{address}"),
            api_key: "images-key".into(),
        };
        let settings = SmokeSettings {
            default_upstream: fallback_upstream(),
            websocket_upstream: fallback_upstream(),
            images: Some(ImagesSmokeSettings {
                upstream: images_upstream,
                model: "images-upstream-model".into(),
            }),
            search: None,
            chat_completions_model: "unused-chat-model".into(),
            responses_model: "unused-responses-model".into(),
            responses_profile: ResponsesUpstreamProfile::OpenAiCompatible,
            timeout: Duration::from_secs(5),
        };

        smoke_images_generation(&settings).await;
        smoke_images_edit(&settings).await;
        server.abort();

        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].path, "/v1/images/generations");
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer images-key")
        );
        assert_eq!(
            requests[0].content_type.as_deref(),
            Some("application/json")
        );
        let generation: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(generation["model"], "images-upstream-model");

        assert_eq!(requests[1].path, "/v1/images/edits");
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer images-key")
        );
        assert!(
            requests[1]
                .content_type
                .as_deref()
                .is_some_and(|value| value.starts_with("multipart/form-data; boundary="))
        );
        assert!(
            requests[1]
                .body
                .windows(b"images-upstream-model".len())
                .any(|window| window == b"images-upstream-model")
        );
    }

    #[tokio::test]
    async fn codex_profile_keeps_the_client_shape_and_streaming_only_boundary() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/responses", post(mock_codex_responses))
            .with_state(Arc::clone(&captured));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let upstream = SmokeUpstream {
            base_url: format!("http://{address}"),
            api_key: "codex-gateway-key".into(),
        };
        let settings = SmokeSettings {
            default_upstream: upstream.clone(),
            websocket_upstream: upstream,
            images: None,
            search: None,
            chat_completions_model: "unused-chat-model".into(),
            responses_model: "gateway-codex-model".into(),
            responses_profile: ResponsesUpstreamProfile::CodexOauth,
            timeout: Duration::from_secs(5),
        };

        smoke_nonstreaming_format(&settings, SmokeFormat::Responses, &settings.responses_model)
            .await;
        smoke_streaming_format(&settings, SmokeFormat::Responses, &settings.responses_model).await;
        server.abort();

        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for body in requests.iter() {
            assert_eq!(body["model"], "gateway-codex-model");
            assert_eq!(body["max_output_tokens"], 1);
        }
        assert_eq!(requests[0]["stream"], false);
        assert_eq!(requests[1]["stream"], true);
    }
}
