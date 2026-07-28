//! Explicit, credential-backed smoke coverage for a real OpenAI-compatible
//! upstream. This test is ignored by default and must be started through
//! `scripts/run-real-upstream-smoke.sh`.

use std::{env, fs, net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{
        ApiFormat, PassiveHealthSettings, RequestLogEvent, RequestLogOutcome, RequestUsage,
        ResponsesWebSocketSettings, SystemRuntimeSettings, UpstreamTimeoutDefaults,
    },
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, compile_control_plane_with_system_settings},
};
use axum::{
    body::{Body, Bytes},
    http::{Request, header},
};
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::{net::TcpListener, process::Command, task::JoinHandle, time::timeout};
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "gateway-real-upstream-smoke-client-key";
const CLIENT_MODEL: &str = "gateway-real-upstream-smoke-model";
const CODEX_CLIENT_KEY_ENV: &str = "AI_GATEWAY_REAL_UPSTREAM_SMOKE_CLIENT_KEY";
const CODEX_PROVIDER_ID: &str = "ai_gateway_real_upstream_smoke";

#[derive(Clone, Copy)]
pub(super) enum SmokeFormat {
    ChatCompletions,
    Responses,
}

impl SmokeFormat {
    const fn api_format(self) -> ApiFormat {
        match self {
            Self::ChatCompletions => ApiFormat::OpenAiChatCompletions,
            Self::Responses => ApiFormat::OpenAiResponses,
        }
    }

    const fn api_format_name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "open_ai_chat_completions",
            Self::Responses => "open_ai_responses",
        }
    }

    const fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
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
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Reply with OK."}],
                }],
                "max_output_tokens": 1,
                "stream": streamed,
            }),
        }
    }
}

pub(super) struct SmokeSettings {
    base_url: String,
    upstream_api_key: String,
    pub(super) chat_completions_model: String,
    pub(super) responses_model: String,
    timeout: Duration,
    codex_bin: String,
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
        Self {
            base_url: required_environment("REAL_UPSTREAM_BASE_URL"),
            upstream_api_key: required_environment("REAL_UPSTREAM_API_KEY"),
            chat_completions_model: required_environment("REAL_UPSTREAM_CHAT_COMPLETIONS_MODEL"),
            responses_model: required_environment("REAL_UPSTREAM_RESPONSES_MODEL"),
            timeout: Duration::from_secs(timeout_seconds),
            codex_bin: optional_environment("REAL_UPSTREAM_CODEX_BIN")
                .unwrap_or_else(|| "codex".into()),
        }
    }
}

fn required_environment(name: &str) -> String {
    optional_environment(name).unwrap_or_else(|| panic!("{name} must be set"))
}

fn optional_environment(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

struct SmokeGateway {
    app: axum::Router,
    logs: RecordingRequestLogSink,
}

struct SmokeServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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
            priority: 0,
            selection_strategy: "weighted_random".into(),
            enabled: true,
        }],
        channels: vec![ChannelRecord {
            id: channel_id,
            channel_group_id: group_id,
            api_format: format.api_format_name().into(),
            name: "real-upstream-smoke".into(),
            base_url: settings.base_url.clone(),
            enabled: true,
            supports_websocket: matches!(format, SmokeFormat::Responses),
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
            upstream_api_key: Some(settings.upstream_api_key.clone()),
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

/// Makes one small, paid JSON request for one API format. It deliberately
/// creates no database records and does not use the process TOML configuration.
pub(super) async fn smoke_nonstreaming_format(
    settings: &SmokeSettings,
    format: SmokeFormat,
    upstream_model: &str,
) {
    let gateway = gateway(settings, format, CLIENT_MODEL, upstream_model);

    let response = timeout(
        settings.timeout,
        gateway.app.oneshot(request(format, false)),
    )
    .await
    .expect("non-streaming gateway request timed out")
    .expect("non-streaming gateway request completed");
    assert!(
        response.status().is_success(),
        "the real upstream returned non-success status {}",
        response.status()
    );
    let body = timeout(settings.timeout, response.into_body().collect())
        .await
        .expect("non-streaming upstream body timed out")
        .expect("non-streaming upstream body completed")
        .to_bytes();
    let value: Value =
        serde_json::from_slice(&body).expect("the real upstream response must be JSON");
    assert!(
        value.is_object(),
        "the real upstream response JSON must be an object"
    );
    assert_response_has_usage(format, &value);
    assert_usage_was_logged(&gateway.logs.events(), format, false);
}

/// Makes one small, paid SSE request for one API format and fully consumes the
/// stream so the terminal request log includes the upstream's final usage.
pub(super) async fn smoke_streaming_format(
    settings: &SmokeSettings,
    format: SmokeFormat,
    upstream_model: &str,
) {
    let gateway = gateway(settings, format, CLIENT_MODEL, upstream_model);
    let response = timeout(settings.timeout, gateway.app.oneshot(request(format, true)))
        .await
        .expect("streaming gateway request timed out")
        .expect("streaming gateway request completed");
    assert!(
        response.status().is_success(),
        "the real upstream returned non-success status {} for a streaming request",
        response.status()
    );
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(
                |value| value.split(';').next().is_some_and(|media_type| media_type
                    .trim()
                    .eq_ignore_ascii_case("text/event-stream"))
            ),
        "the real upstream streaming response must use text/event-stream"
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
    assert_usage_was_logged(&events, format, true);
    assert_streaming_usage_matches_terminal_sse_event(&events, format, &raw_sse);
}

/// Runs the installed Codex CLI through a real TCP listener so its exact
/// WebSocket prewarm and turn payloads exercise both gateway upgrade paths.
pub(super) async fn smoke_responses_websocket(settings: &SmokeSettings, upstream_model: &str) {
    let gateway = gateway(
        settings,
        SmokeFormat::Responses,
        upstream_model,
        upstream_model,
    );
    let server = start_gateway_server(gateway.app).await;
    let temporary_path =
        env::temp_dir().join(format!("ai-gateway-real-upstream-codex-{}", Uuid::new_v4()));
    fs::create_dir_all(&temporary_path).expect("create temporary Codex smoke directory");
    let _temporary_directory = TemporaryDirectory(temporary_path.clone());
    let instructions_path = temporary_path.join("instructions.md");
    fs::write(
        &instructions_path,
        "Reply exactly OK. Do not use tools or inspect files.\n",
    )
    .expect("write temporary Codex smoke instructions");

    let mut provider = toml::Table::new();
    provider.insert("name".into(), "ai-gateway real-upstream smoke".into());
    provider.insert(
        "base_url".into(),
        format!("http://{}/v1", server.address).into(),
    );
    provider.insert("env_key".into(), CODEX_CLIENT_KEY_ENV.into());
    provider.insert("wire_api".into(), "responses".into());
    provider.insert("supports_websockets".into(), true.into());
    let mut providers = toml::Table::new();
    providers.insert(CODEX_PROVIDER_ID.into(), provider.into());
    let mut config = toml::Table::new();
    config.insert("model".into(), upstream_model.into());
    config.insert("model_provider".into(), CODEX_PROVIDER_ID.into());
    config.insert(
        "model_instructions_file".into(),
        instructions_path.to_string_lossy().into_owned().into(),
    );
    config.insert("approval_policy".into(), "never".into());
    config.insert("sandbox_mode".into(), "read-only".into());
    config.insert("model_providers".into(), providers.into());
    fs::write(
        temporary_path.join("config.toml"),
        toml::to_string(&config).expect("serialize temporary Codex smoke configuration"),
    )
    .expect("write temporary Codex smoke configuration");

    let mut command = Command::new(&settings.codex_bin);
    command
        .arg("exec")
        .arg("--ephemeral")
        .arg("--ignore-rules")
        .arg("--skip-git-repo-check")
        .arg("-C")
        .arg(&temporary_path)
        .arg("Reply exactly OK. Do not use tools or inspect files.")
        .env("CODEX_HOME", &temporary_path)
        .env(CODEX_CLIENT_KEY_ENV, CLIENT_KEY)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = timeout(settings.timeout.saturating_mul(3), command.output())
        .await
        .expect("Codex CLI real-upstream WebSocket smoke timed out")
        .expect("launch Codex CLI for the real-upstream WebSocket smoke");
    assert!(
        output.status.success(),
        "Codex CLI real-upstream WebSocket smoke failed with status {}",
        output.status
    );
    let stdout = String::from_utf8(output.stdout).expect("Codex CLI stdout must be UTF-8");
    let stderr = String::from_utf8(output.stderr).expect("Codex CLI stderr must be UTF-8");
    assert!(
        stdout.lines().any(|line| line.trim() == "OK"),
        "Codex CLI real-upstream WebSocket smoke must return exactly OK"
    );
    assert!(
        !stderr
            .to_ascii_lowercase()
            .contains("falling back from websockets"),
        "Codex CLI must complete the real-upstream smoke without HTTP fallback"
    );

    let events = gateway.logs.events();
    assert!(
        events.len() >= 2,
        "Codex WebSocket prewarm and turn must produce terminal logs"
    );
    for event in &events {
        assert_eq!(event.api_format, SmokeFormat::Responses.api_format());
        assert!(event.streamed);
        match event.outcome {
            RequestLogOutcome::Succeeded => {
                assert!(
                    event
                        .response_status_code
                        .is_some_and(|status| (200..300).contains(&status))
                );
                assert_eq!(event.error_code, None);
            }
            RequestLogOutcome::Failed => assert!(
                event
                    .response_status_code
                    .is_none_or(|status| (100..600).contains(&status))
                    && event.error_code.is_some(),
                "failed Codex WebSocket attempts must retain a safe optional status and error code"
            ),
            RequestLogOutcome::Rejected | RequestLogOutcome::Cancelled => {
                panic!("Codex WebSocket smoke must not be rejected or cancelled")
            }
        }
    }
    if let Some(prewarm) = events.iter().find(|event| {
        event.outcome == RequestLogOutcome::Succeeded
            && event
                .billing
                .as_ref()
                .and_then(|billing| billing.usage)
                .is_some_and(|usage| usage.output_tokens == 0)
    }) {
        let billing = prewarm
            .billing
            .as_ref()
            .expect("successful Codex WebSocket prewarm retains billing");
        let usage = billing
            .usage
            .expect("successful Codex WebSocket prewarm records usage");
        assert!(usage.input_tokens > 0);
        assert!(
            billing
                .cost_amount
                .is_some_and(|amount| amount > Decimal::ZERO)
        );
    }
    let completed_turn = events
        .iter()
        .rev()
        .find(|event| {
            event.outcome == RequestLogOutcome::Succeeded
                && event
                    .billing
                    .as_ref()
                    .and_then(|billing| billing.usage)
                    .is_some_and(|usage| usage.output_tokens > 0)
        })
        .expect("Codex WebSocket smoke must complete a turn with output usage");
    assert_usage_was_logged(
        std::slice::from_ref(completed_turn),
        SmokeFormat::Responses,
        true,
    );
}

fn assert_response_has_usage(format: SmokeFormat, value: &Value) {
    let usage = match format {
        SmokeFormat::ChatCompletions => value.get("usage"),
        SmokeFormat::Responses => value.get("usage").or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        }),
    };
    assert!(
        usage.is_some_and(Value::is_object),
        "the real upstream non-streaming response must include a usage object"
    );
}

fn assert_usage_was_logged(events: &[RequestLogEvent], format: SmokeFormat, streamed: bool) {
    assert_eq!(
        events.len(),
        1,
        "the real upstream request must produce exactly one terminal log"
    );
    let event = &events[0];
    assert_eq!(event.api_format, format.api_format());
    assert_eq!(event.streamed, streamed);
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
    assert!(
        billing
            .output_tokens_per_second
            .is_some_and(|tps| tps > Decimal::ZERO),
        "a nonempty response body with output tokens must have positive output TPS"
    );
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
    let usage = value
        .get("usage")
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        })
        .unwrap_or(value);
    let (input_field, output_field, details_field) = match format {
        SmokeFormat::ChatCompletions => (
            "prompt_tokens",
            "completion_tokens",
            "prompt_tokens_details",
        ),
        SmokeFormat::Responses => ("input_tokens", "output_tokens", "input_tokens_details"),
    };
    let input_tokens = nonnegative_token(usage.get(input_field))?;
    let output_tokens = nonnegative_token(usage.get(output_field))?;
    let details = usage.get(details_field);
    let cached_input_tokens = details
        .and_then(|details| nonnegative_token(details.get("cached_tokens")))
        .or_else(|| match format {
            SmokeFormat::ChatCompletions => nonnegative_token(usage.get("prompt_cache_hit_tokens")),
            SmokeFormat::Responses => None,
        })
        .unwrap_or(0);
    let cache_write_tokens = details
        .and_then(|details| {
            nonnegative_token(details.get("cache_write_tokens"))
                .or_else(|| nonnegative_token(details.get("cache_creation_tokens")))
        })
        .unwrap_or(0);
    (cached_input_tokens <= input_tokens && cache_write_tokens <= input_tokens).then_some(
        RequestUsage {
            input_tokens,
            cached_input_tokens,
            cache_write_tokens,
            output_tokens,
        },
    )
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
                    choices.iter().any(|choice| {
                        choice
                            .get("finish_reason")
                            .is_some_and(|reason| !reason.is_null())
                    })
                })
        }
        SmokeFormat::Responses => {
            value.get("type").and_then(Value::as_str) == Some("response.completed")
        }
    }
}
