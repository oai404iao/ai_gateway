//! Periodic direct upstream test requests for enabled and temporarily disabled
//! channels.

use std::{sync::Arc, time::Duration};

use axum::http::{
    HeaderMap, HeaderValue, Method,
    header::{AUTHORIZATION, CONTENT_TYPE},
};
use bytes::Bytes;
use futures_util::StreamExt;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;

use crate::{
    application::{
        AutomaticDisableService, ControlPlaneCoordinator, ErrorKeywordMatcher, RequestLogSink,
        ResponseUsage, UsageCollector, request_billing, request_billing_multiplier,
    },
    domain::{
        ApiFormat, AutomaticDisableTrigger, CompiledChannel, CompiledScheduledTestModel,
        RequestLogEvent, RequestLogOutcome, RequestLogSource, RequestProtocol,
        ScheduledTestingMode, UpstreamAuth,
    },
    persistence::SystemProbeIdentity,
    runtime_config::RuntimeConfig,
    transforms::{apply_header_plan, apply_json_patch_plan},
    upstream::{ResolvedUpstreamPolicy, UpstreamClientRegistry},
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PROBE_RESPONSE_BYTES: usize = 1_048_576;

/// Owns periodic direct channel probes. It deliberately waits one configured
/// interval before the first run so process startup never immediately sends
/// billable upstream traffic.
pub struct ChannelProbeWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl ChannelProbeWorker {
    #[must_use]
    pub fn start(
        runtime: Arc<RuntimeConfig>,
        coordinator: ControlPlaneCoordinator,
        upstream_clients: Arc<UpstreamClientRegistry>,
        request_log_sink: Arc<dyn RequestLogSink>,
        automatic_disable: AutomaticDisableService,
        identity: SystemProbeIdentity,
    ) -> Self {
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                let interval = runtime
                    .snapshot()
                    .system_settings()
                    .scheduled_testing()
                    .interval();
                tokio::select! {
                    _ = sleep(interval) => {
                        run_probe_round(
                            &runtime,
                            &coordinator,
                            &upstream_clients,
                            &request_log_sink,
                            &automatic_disable,
                            identity,
                        ).await;
                    }
                    _ = &mut shutdown_requested => return,
                }
            }
        });
        Self { shutdown, task }
    }

    pub async fn shutdown(self) {
        let Self { shutdown, mut task } = self;
        let _ = shutdown.send(());
        match timeout(SHUTDOWN_TIMEOUT, &mut task).await {
            Ok(Ok(())) => tracing::info!("scheduled channel-test worker stopped"),
            Ok(Err(error)) => {
                tracing::error!(%error, "scheduled channel-test worker terminated unexpectedly")
            }
            Err(_) => {
                tracing::warn!(
                    "scheduled channel-test worker did not stop before shutdown deadline; aborting"
                );
                task.abort();
                let _ = task.await;
            }
        }
    }
}

async fn run_probe_round(
    runtime: &Arc<RuntimeConfig>,
    coordinator: &ControlPlaneCoordinator,
    upstream_clients: &Arc<UpstreamClientRegistry>,
    request_log_sink: &Arc<dyn RequestLogSink>,
    automatic_disable: &AutomaticDisableService,
    identity: SystemProbeIdentity,
) {
    let snapshot = runtime.snapshot();
    let scheduled = snapshot.system_settings().scheduled_testing().clone();
    let automatic_settings = snapshot.system_settings().automatic_disable().clone();
    let channels = snapshot
        .probe_channels()
        .filter(|channel| {
            snapshot.group(channel.group_id()).is_some()
                && channel.test_model().is_some()
                && match scheduled.mode() {
                    ScheduledTestingMode::Global => true,
                    ScheduledTestingMode::FailureOnly => channel.auto_disabled(),
                }
        })
        .cloned()
        .collect::<Vec<_>>();

    for channel in channels {
        let model = channel
            .test_model()
            .expect("selected scheduled test channels always have a test model");
        let billing_model = snapshot
            .scheduled_test_model(model)
            .expect("runtime compilation validates scheduled test model pricing");
        let result = probe_channel(
            &snapshot.system_settings().upstream_timeouts(),
            &channel,
            &billing_model,
            scheduled.prompt(),
            upstream_clients,
            automatic_disable,
            &automatic_settings,
            identity,
        )
        .await;
        request_log_sink.try_record(result.event);

        if result.succeeded && channel.auto_disabled() && scheduled.auto_recover() {
            match coordinator
                .automatically_recover_channel(channel.id())
                .await
            {
                Ok(true) => {}
                Ok(false) => tracing::debug!(
                    channel_id = %channel.id(),
                    "scheduled test recovery no longer matched current channel state"
                ),
                Err(error) => tracing::error!(
                    channel_id = %channel.id(),
                    error = %error,
                    "scheduled test could not recover channel"
                ),
            }
        }
    }
}

struct ProbeResult {
    event: RequestLogEvent,
    succeeded: bool,
}

struct ProbeContext<'a> {
    channel: &'a CompiledChannel,
    billing_model: &'a CompiledScheduledTestModel,
    identity: SystemProbeIdentity,
    started_at: chrono::DateTime<chrono::Utc>,
    started: Instant,
    request_billing_multiplier: Decimal,
}

#[allow(clippy::too_many_arguments)] // direct scheduled probes need all immutable dependencies
async fn probe_channel(
    upstream_defaults: &crate::domain::UpstreamTimeoutDefaults,
    channel: &CompiledChannel,
    billing_model: &CompiledScheduledTestModel,
    prompt: &str,
    upstream_clients: &UpstreamClientRegistry,
    automatic_disable: &AutomaticDisableService,
    automatic_settings: &crate::domain::AutomaticDisableSettings,
    identity: SystemProbeIdentity,
) -> ProbeResult {
    let mut context = ProbeContext {
        channel,
        billing_model,
        identity,
        started_at: chrono::Utc::now(),
        started: Instant::now(),
        request_billing_multiplier: Decimal::ONE,
    };
    let model = channel
        .test_model()
        .expect("selected scheduled test channels always have a test model");
    let mut response_status_code = None;
    let mut ttft_ms = None;
    let mut outcome = RequestLogOutcome::Failed;
    let mut error_code = Some("scheduled_test_setup_failed");

    let body = match build_probe_body(channel.api_format(), model, prompt) {
        Ok(body) => body,
        Err(()) => {
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
    };
    context.request_billing_multiplier =
        request_billing_multiplier(billing_model.advanced_billing(), &body);
    let transforms = channel.upstream_policy().effective_transforms();
    let body = match apply_json_patch_plan(body, transforms.request_json()) {
        Ok(body) => body,
        Err(_) => {
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
    };
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if apply_header_plan(&mut headers, transforms.request_headers()).is_err()
        || inject_upstream_auth(&mut headers, channel).is_err()
    {
        return finished_probe(
            &context,
            outcome,
            response_status_code,
            ttft_ms,
            error_code,
            None,
        );
    }
    let url = match probe_url(channel) {
        Ok(url) => url,
        Err(()) => {
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
    };
    let policy =
        match ResolvedUpstreamPolicy::try_resolve(upstream_defaults, channel.upstream_policy()) {
            Ok(policy) => policy,
            Err(_) => {
                return finished_probe(
                    &context,
                    outcome,
                    response_status_code,
                    ttft_ms,
                    error_code,
                    None,
                );
            }
        };
    let client = match upstream_clients.client_for(channel.upstream_policy(), policy) {
        Ok(client) => client,
        Err(_) => {
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
    };

    let response = match timeout(
        policy.timeouts().response_header(),
        client
            .request(Method::POST, url)
            .headers(headers)
            .body(body)
            .send(),
    )
    .await
    {
        Err(_) => {
            error_code = Some("scheduled_test_response_header_timeout");
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
        Ok(Err(error)) => {
            error_code = Some(if error.is_connect() {
                "scheduled_test_connection_failed"
            } else {
                "scheduled_test_upstream_unavailable"
            });
            return finished_probe(
                &context,
                outcome,
                response_status_code,
                ttft_ms,
                error_code,
                None,
            );
        }
        Ok(Ok(response)) => response,
    };

    let status = response.status().as_u16();
    response_status_code = Some(status);
    let upstream_succeeded = response.status().is_success();
    if !upstream_succeeded
        && channel.auto_disable_allowed()
        && automatic_settings.matches_status(status)
    {
        automatic_disable.try_report(channel.id(), AutomaticDisableTrigger::HttpStatus(status));
    }
    let mut keyword_matcher = (!upstream_succeeded && channel.auto_disable_allowed())
        .then(|| ErrorKeywordMatcher::new(automatic_settings))
        .flatten();
    let mut usage = UsageCollector::new(channel.api_format(), is_sse_response(response.headers()));
    let mut response_stream = response.bytes_stream();
    let mut total_bytes = 0_usize;
    loop {
        match timeout(policy.timeouts().stream_idle(), response_stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                ttft_ms.get_or_insert_with(|| clamp_duration_ms(context.started.elapsed()));
                total_bytes = total_bytes.saturating_add(bytes.len());
                usage.observe(&bytes);
                if let Some(matcher) = &mut keyword_matcher
                    && let Some(trigger) = matcher.observe(&bytes)
                {
                    automatic_disable.try_report(channel.id(), trigger);
                    keyword_matcher = None;
                }
                if total_bytes > MAX_PROBE_RESPONSE_BYTES {
                    error_code = Some("scheduled_test_response_too_large");
                    return finished_probe(
                        &context,
                        outcome,
                        response_status_code,
                        ttft_ms,
                        error_code,
                        usage.latest(),
                    );
                }
            }
            Ok(Some(Err(_))) => {
                error_code = Some("scheduled_test_response_body_error");
                return finished_probe(
                    &context,
                    outcome,
                    response_status_code,
                    ttft_ms,
                    error_code,
                    usage.latest(),
                );
            }
            Ok(None) => {
                outcome = if upstream_succeeded {
                    RequestLogOutcome::Succeeded
                } else {
                    RequestLogOutcome::Failed
                };
                error_code = (!upstream_succeeded).then_some("scheduled_test_http_error");
                usage.finalize();
                return finished_probe(
                    &context,
                    outcome,
                    response_status_code,
                    ttft_ms,
                    error_code,
                    usage.latest(),
                );
            }
            Err(_) => {
                error_code = Some("scheduled_test_stream_idle_timeout");
                return finished_probe(
                    &context,
                    outcome,
                    response_status_code,
                    ttft_ms,
                    error_code,
                    usage.latest(),
                );
            }
        }
    }
}

fn finished_probe(
    context: &ProbeContext<'_>,
    outcome: RequestLogOutcome,
    response_status_code: Option<u16>,
    ttft_ms: Option<i32>,
    error_code: Option<&'static str>,
    usage: Option<ResponseUsage>,
) -> ProbeResult {
    let succeeded = outcome == RequestLogOutcome::Succeeded;
    let model = context
        .channel
        .test_model()
        .expect("selected scheduled test channels always have a test model");
    let total_duration_ms = clamp_duration_ms(context.started.elapsed());
    let billing = request_billing(
        context.billing_model.price_snapshot(),
        context.billing_model.advanced_billing(),
        context.channel.billing_multiplier(),
        context.request_billing_multiplier,
        usage,
        total_duration_ms,
        ttft_ms,
    );
    tracing::info!(
        event = "scheduled_channel_test_completed",
        channel_id = %context.channel.id(),
        api_format = ?context.channel.api_format(),
        upstream_status = ?response_status_code,
        latency_ms = context.started.elapsed().as_millis(),
        input_tokens = ?billing.usage.as_ref().map(|usage| usage.input_tokens),
        output_tokens = ?billing.usage.as_ref().map(|usage| usage.output_tokens),
        outcome = outcome.as_str(),
        "scheduled channel test completed"
    );
    ProbeResult {
        event: RequestLogEvent {
            id: Uuid::new_v4(),
            started_at: context.started_at,
            completed_at: completed_at(context.started_at, context.started.elapsed()),
            user_id: context.identity.user_id,
            api_key_id: context.identity.api_key_id,
            request_source: RequestLogSource::ScheduledTest,
            api_format: context.channel.api_format(),
            request_protocol: RequestProtocol::NonStream,
            client_model: model.to_owned(),
            reasoning_effort: None,
            fast_mode: false,
            upstream_model: Some(model.to_owned()),
            model_rule_id: None,
            channel_group_id: Some(context.channel.group_id()),
            channel_id: Some(context.channel.id()),
            model_id: Some(context.billing_model.id()),
            outcome,
            response_status_code,
            streamed: false,
            ttft_ms,
            total_duration_ms,
            billing: Some(billing),
            error_code: error_code.map(str::to_owned),
            error_summary: None,
        },
        succeeded,
    }
}

fn build_probe_body(api_format: ApiFormat, model: &str, prompt: &str) -> Result<Bytes, ()> {
    let value = match api_format {
        ApiFormat::OpenAiChatCompletions => json!({
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": false,
        }),
        ApiFormat::OpenAiResponses => json!({
            "model": model,
            "input": prompt,
            "stream": false,
        }),
    };
    serde_json::to_vec(&value).map(Bytes::from).map_err(|_| ())
}

fn probe_url(channel: &CompiledChannel) -> Result<reqwest::Url, ()> {
    let path = match channel.api_format() {
        ApiFormat::OpenAiChatCompletions => "/v1/chat/completions",
        ApiFormat::OpenAiResponses => "/v1/responses",
    };
    reqwest::Url::parse(&format!(
        "{}{path}",
        channel.base_url().as_str().trim_end_matches('/')
    ))
    .map_err(|_| ())
}

fn inject_upstream_auth(headers: &mut HeaderMap, channel: &CompiledChannel) -> Result<(), ()> {
    match channel.upstream_auth() {
        UpstreamAuth::None => Ok(()),
        UpstreamAuth::Bearer(token) => {
            let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ())?;
            headers.insert(AUTHORIZATION, value);
            Ok(())
        }
        UpstreamAuth::Header { name, value } => {
            let value = HeaderValue::from_str(value).map_err(|_| ())?;
            headers.insert(name.clone(), value);
            Ok(())
        }
    }
}

fn is_sse_response(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn clamp_duration_ms(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

fn completed_at(
    started_at: chrono::DateTime<chrono::Utc>,
    elapsed: Duration,
) -> chrono::DateTime<chrono::Utc> {
    let elapsed = chrono::Duration::from_std(elapsed)
        .unwrap_or_else(|_| chrono::Duration::milliseconds(i64::MAX));
    started_at.checked_add_signed(elapsed).unwrap_or(started_at)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };

    use axum::{
        Router,
        body::{Body, to_bytes},
        extract::{Request, State},
        http::StatusCode,
        response::Response,
        routing::post,
    };
    use rust_decimal::Decimal;
    use serde_json::Value;
    use tokio::{
        net::TcpListener,
        sync::mpsc,
        task::JoinHandle,
        time::{Duration, timeout},
    };
    use uuid::Uuid;

    use super::probe_channel;
    use crate::{
        application::AutomaticDisableService,
        domain::{
            ApiFormat, AutomaticDisableSettings, AutomaticDisableTrigger, CompiledAdvancedBilling,
            CompiledChannel, CompiledChannelUpstreamPolicy, CompiledScheduledTestModel,
            ModelPriceSnapshot, RequestLogOutcome, RequestLogSource, RequestUsage, UpstreamAuth,
            UpstreamTimeoutDefaults,
        },
        persistence::SystemProbeIdentity,
        upstream::UpstreamClientRegistry,
    };

    #[derive(Clone)]
    struct TestUpstream {
        requests: Arc<Mutex<Vec<Value>>>,
        status: StatusCode,
        response_body: &'static str,
    }

    async fn upstream(State(state): State<TestUpstream>, request: Request) -> Response {
        let body = to_bytes(request.into_body(), usize::MAX).await.unwrap();
        state
            .requests
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Response::builder()
            .status(state.status)
            .body(Body::from(state.response_body))
            .unwrap()
    }

    fn scheduled_test_model(id: Uuid) -> CompiledScheduledTestModel {
        CompiledScheduledTestModel::new(
            id,
            ModelPriceSnapshot::new(
                "USD".into(),
                1,
                chrono::Utc::now(),
                Decimal::from(2_i64),
                Decimal::ONE,
                Decimal::from(3_i64),
                Decimal::from(4_i64),
            ),
            CompiledAdvancedBilling::default(),
        )
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

    async fn start_server(state: TestUpstream) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/v1/chat/completions", post(upstream))
                    .with_state(state),
            )
            .await
            .unwrap();
        });
        TestServer { address, task }
    }

    #[tokio::test]
    async fn scheduled_probe_logs_a_system_request_and_reports_matching_error_keyword() {
        let requests = Arc::new(Mutex::new(vec![]));
        let server = start_server(TestUpstream {
            requests: Arc::clone(&requests),
            status: StatusCode::TOO_MANY_REQUESTS,
            response_body: r#"{"error":{"message":"quota exceeded"}}"#,
        })
        .await;
        let model_id = Uuid::new_v4();
        let billing_model = scheduled_test_model(model_id);
        let channel = CompiledChannel::new_with_policy_and_automation(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiChatCompletions,
            reqwest::Url::parse(&format!("http://{}", server.address)).unwrap(),
            1,
            UpstreamAuth::None,
            HashSet::from([Arc::<str>::from("probe-model")]),
            true,
            false,
            Some(Arc::from("probe-model")),
            CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiChatCompletions),
        );
        let settings = AutomaticDisableSettings::new(
            true,
            Arc::from([]),
            vec![Arc::from("quota exceeded")].into(),
        );
        let (sender, mut receiver) = mpsc::channel(2);
        let service = AutomaticDisableService::new(sender);
        let result = probe_channel(
            &UpstreamTimeoutDefaults::new(
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(2),
            ),
            &channel,
            &billing_model,
            "reply '1'",
            &UpstreamClientRegistry::new(),
            &service,
            &settings,
            SystemProbeIdentity {
                user_id: Uuid::new_v4(),
                api_key_id: Uuid::new_v4(),
            },
        )
        .await;

        assert!(!result.succeeded);
        assert_eq!(result.event.request_source, RequestLogSource::ScheduledTest);
        assert_eq!(result.event.outcome, RequestLogOutcome::Failed);
        assert_eq!(result.event.response_status_code, Some(429));
        assert_eq!(result.event.model_id, Some(model_id));
        assert!(result.event.billing.is_some());
        assert_eq!(
            result.event.error_code.as_deref(),
            Some("scheduled_test_http_error")
        );
        assert_eq!(
            timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap()
                .trigger,
            AutomaticDisableTrigger::ErrorMessageKeyword(Arc::from("quota exceeded"))
        );
        let bodies = requests.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["model"], "probe-model");
        assert_eq!(bodies[0]["messages"][0]["content"], "reply '1'");
    }

    #[tokio::test]
    async fn scheduled_probe_records_usage_and_bills_the_system_administrator() {
        let requests = Arc::new(Mutex::new(vec![]));
        let server = start_server(TestUpstream {
            requests: Arc::clone(&requests),
            status: StatusCode::OK,
            response_body: r#"{"usage":{"prompt_tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":2,"cache_write_tokens":1},"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        })
        .await;
        let model_id = Uuid::new_v4();
        let billing_model = scheduled_test_model(model_id);
        let channel = CompiledChannel::new_with_policy_automation_and_billing(
            Uuid::new_v4(),
            Uuid::new_v4(),
            ApiFormat::OpenAiChatCompletions,
            reqwest::Url::parse(&format!("http://{}", server.address)).unwrap(),
            1,
            Decimal::new(15, 1),
            UpstreamAuth::None,
            HashSet::from([Arc::<str>::from("probe-model")]),
            false,
            false,
            false,
            Some(Arc::from("probe-model")),
            CompiledChannelUpstreamPolicy::transparent(ApiFormat::OpenAiChatCompletions),
        );
        let (sender, _receiver) = mpsc::channel(1);
        let service = AutomaticDisableService::new(sender);
        let result = probe_channel(
            &UpstreamTimeoutDefaults::new(
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(2),
            ),
            &channel,
            &billing_model,
            "reply '1'",
            &UpstreamClientRegistry::new(),
            &service,
            &AutomaticDisableSettings::default(),
            SystemProbeIdentity {
                user_id: Uuid::new_v4(),
                api_key_id: Uuid::new_v4(),
            },
        )
        .await;

        let billing = result
            .event
            .billing
            .expect("scheduled probe must be billable");
        assert!(result.succeeded);
        assert_eq!(result.event.outcome, RequestLogOutcome::Succeeded);
        assert_eq!(result.event.model_id, Some(model_id));
        assert_eq!(
            billing.usage,
            Some(RequestUsage {
                input_tokens: 10,
                cached_input_tokens: 2,
                cache_write_tokens: 1,
                output_tokens: 4,
                reasoning_tokens: 1,
            })
        );
        assert_eq!(billing.cost_amount, Some(Decimal::new(555, 1)));
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
}
