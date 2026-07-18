//! Explicit, credential-backed smoke coverage for a real OpenAI-compatible
//! upstream. This test is ignored by default and must be started through
//! `scripts/run-real-upstream-smoke.sh`.

use std::{env, sync::Arc, time::Duration};

use ai_gateway::{
    application::{ProxyService, RecordingRequestLogSink},
    domain::{ApiFormat, RequestLogEvent, RequestLogOutcome},
    http,
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
    runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane},
};
use axum::{
    body::{Body, Bytes},
    http::{Request, header},
};
use http_body_util::BodyExt;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::time::timeout;
use tower::ServiceExt;
use uuid::Uuid;

const CLIENT_KEY: &str = "gateway-real-upstream-smoke-client-key";
const CLIENT_MODEL: &str = "gateway-real-upstream-smoke-model";

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

fn gateway(settings: &SmokeSettings, format: SmokeFormat, upstream_model: &str) -> SmokeGateway {
    let group_id = Uuid::new_v4();
    let channel_id = Uuid::new_v4();
    let logs = RecordingRequestLogSink::default();
    let records = ControlPlaneRecords {
        api_keys: vec![ApiKeyRecord {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            user_status: "active".into(),
            secret_value: CLIENT_KEY.into(),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: vec![format.api_format_name().into()],
            permissions: vec!["proxy".into(), "models.read".into()],
            allowed_group_ids: Some(vec![group_id]),
            requests_per_minute: None,
            tokens_per_minute: None,
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
            auto_disabled: false,
            weight: 1,
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
            health_check: json!({}),
        }],
        model_rules: vec![ModelRuleRecord {
            id: Uuid::new_v4(),
            client_model: CLIENT_MODEL.into(),
            api_format: format.api_format_name().into(),
            model_id: Uuid::new_v4(),
            model_enabled: true,
            model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: chrono::Utc::now(),
            input_unit_price: Decimal::ONE,
            cached_input_unit_price: Decimal::new(5, 1),
            cache_write_unit_price: Decimal::new(25, 2),
            output_unit_price: Decimal::from(2_i64),
            upstream_model: upstream_model.into(),
            channel_group_ids: vec![],
            channel_ids: vec![channel_id],
            enabled: true,
        }],
        proxies: vec![],
        templates: vec![],
    };
    let runtime = Arc::new(RuntimeConfig::new(
        compile_control_plane(records).expect("the smoke-test route must compile"),
    ));
    let upstream = UpstreamConfig {
        connect_timeout_seconds: settings.timeout.as_secs().saturating_sub(1).clamp(1, 10),
        response_header_timeout_seconds: settings.timeout.as_secs(),
        stream_idle_timeout_seconds: settings.timeout.as_secs(),
    };
    let proxy = ProxyService::with_log_sink(runtime, 1_048_576, &upstream, Arc::new(logs.clone()))
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
    let gateway = gateway(settings, format, upstream_model);

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
    let gateway = gateway(settings, format, upstream_model);
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
    timeout(settings.timeout, body.collect())
        .await
        .expect("the real upstream streaming body did not finish in time")
        .expect("the real upstream streaming body failed");
    assert_usage_was_logged(&gateway.logs.events(), format, true);
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
