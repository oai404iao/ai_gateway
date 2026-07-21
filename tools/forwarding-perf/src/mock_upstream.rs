//! Configurable local OpenAI-compatible JSON/SSE Mock LLM upstream.

use std::{
    convert::Infallible,
    error::Error,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use arc_swap::ArcSwap;
use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::Response,
    routing::{get, post},
};
use futures_util::stream;
use serde::{Deserialize, Serialize};
use tokio::{net::TcpListener, time::sleep};

use crate::scenario::{ApiKind, CLIENT_API_KEY, MockConfig, MockMode, UPSTREAM_API_KEY};

const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;

#[derive(Clone)]
struct MockState {
    config: Arc<ArcSwap<MockConfig>>,
    counters: Arc<MockCounters>,
}

#[derive(Default)]
struct MockCounters {
    accepted_requests: AtomicU64,
    completed_requests: AtomicU64,
    cancelled_requests: AtomicU64,
    current_in_flight: AtomicU64,
    peak_in_flight: AtomicU64,
    request_bytes: AtomicU64,
    response_bytes: AtomicU64,
    invalid_authorization: AtomicU64,
}

impl MockCounters {
    fn snapshot(&self) -> MockStats {
        MockStats {
            accepted_requests: self.accepted_requests.load(Ordering::Relaxed),
            completed_requests: self.completed_requests.load(Ordering::Relaxed),
            cancelled_requests: self.cancelled_requests.load(Ordering::Relaxed),
            current_in_flight: self.current_in_flight.load(Ordering::Relaxed),
            peak_in_flight: self.peak_in_flight.load(Ordering::Relaxed),
            request_bytes: self.request_bytes.load(Ordering::Relaxed),
            response_bytes: self.response_bytes.load(Ordering::Relaxed),
            invalid_authorization: self.invalid_authorization.load(Ordering::Relaxed),
        }
    }

    fn reset(&self) -> Result<(), &'static str> {
        if self.current_in_flight.load(Ordering::Acquire) != 0 {
            return Err("cannot reset mock counters while requests are in flight");
        }
        self.accepted_requests.store(0, Ordering::Relaxed);
        self.completed_requests.store(0, Ordering::Relaxed);
        self.cancelled_requests.store(0, Ordering::Relaxed);
        self.peak_in_flight.store(0, Ordering::Relaxed);
        self.request_bytes.store(0, Ordering::Relaxed);
        self.response_bytes.store(0, Ordering::Relaxed);
        self.invalid_authorization.store(0, Ordering::Relaxed);
        Ok(())
    }

    fn begin(self: &Arc<Self>) -> InFlightGuard {
        self.accepted_requests.fetch_add(1, Ordering::Relaxed);
        let current = self.current_in_flight.fetch_add(1, Ordering::AcqRel) + 1;
        let mut peak = self.peak_in_flight.load(Ordering::Relaxed);
        while current > peak {
            match self.peak_in_flight.compare_exchange_weak(
                peak,
                current,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(next) => peak = next,
            }
        }
        InFlightGuard {
            counters: Arc::clone(self),
            completed: false,
        }
    }
}

struct InFlightGuard {
    counters: Arc<MockCounters>,
    completed: bool,
}

impl InFlightGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.counters
            .current_in_flight
            .fetch_sub(1, Ordering::AcqRel);
        if self.completed {
            self.counters
                .completed_requests
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters
                .cancelled_requests
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MockStats {
    pub accepted_requests: u64,
    pub completed_requests: u64,
    pub cancelled_requests: u64,
    pub current_in_flight: u64,
    pub peak_in_flight: u64,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub invalid_authorization: u64,
}

pub async fn run(address: SocketAddr) -> Result<(), Box<dyn Error + Send + Sync>> {
    let state = MockState {
        config: Arc::new(ArcSwap::from_pointee(MockConfig::default())),
        counters: Arc::new(MockCounters::default()),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .route("/__perf/config", post(set_config))
        .route("/__perf/reset", post(reset))
        .route("/__perf/stats", get(stats))
        .with_state(state);
    let listener = TcpListener::bind(address).await?;
    println!("mock upstream listening on {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn set_config(
    State(state): State<MockState>,
    Json(config): Json<MockConfig>,
) -> Result<StatusCode, (StatusCode, String)> {
    config
        .validate()
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    if state.counters.current_in_flight.load(Ordering::Acquire) != 0 {
        return Err((
            StatusCode::CONFLICT,
            "cannot change mock configuration while requests are in flight".into(),
        ));
    }
    state.config.store(Arc::new(config));
    Ok(StatusCode::NO_CONTENT)
}

async fn reset(State(state): State<MockState>) -> Result<StatusCode, (StatusCode, &'static str)> {
    state
        .counters
        .reset()
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|message| (StatusCode::CONFLICT, message))
}

async fn stats(State(state): State<MockState>) -> Json<MockStats> {
    Json(state.counters.snapshot())
}

async fn chat_completions(State(state): State<MockState>, request: Request) -> Response {
    serve(ApiKind::ChatCompletions, state, request).await
}

async fn responses(State(state): State<MockState>, request: Request) -> Response {
    serve(ApiKind::Responses, state, request).await
}

async fn serve(api_kind: ApiKind, state: MockState, request: Request) -> Response {
    let authorization_valid = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .strip_prefix("Bearer ")
                .is_some_and(|token| token == CLIENT_API_KEY || token == UPSTREAM_API_KEY)
        });
    if !authorization_valid {
        state
            .counters
            .invalid_authorization
            .fetch_add(1, Ordering::Relaxed);
    }
    let body = match to_bytes(request.into_body(), MAX_REQUEST_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Body::empty())
                .expect("mock payload-too-large response must build");
        }
    };
    state
        .counters
        .request_bytes
        .fetch_add(body.len() as u64, Ordering::Relaxed);

    let config = state.config.load_full();
    let guard = state.counters.begin();
    let (content_type, chunks, first_delay, later_delay) = match config.mode {
        MockMode::Json => (
            "application/json",
            vec![json_response(api_kind)],
            Duration::from_millis(config.response_delay_ms),
            Duration::ZERO,
        ),
        MockMode::Sse => (
            "text/event-stream",
            sse_chunks(api_kind, config.chunk_count),
            Duration::from_millis(config.ttft_ms),
            Duration::from_millis(config.chunk_interval_ms),
        ),
    };
    let counters = Arc::clone(&state.counters);
    let body_stream = stream::unfold(
        StreamState {
            chunks,
            index: 0,
            first_delay,
            later_delay,
            guard,
            counters,
        },
        |mut stream_state| async move {
            if stream_state.index >= stream_state.chunks.len() {
                return None;
            }
            let delay = if stream_state.index == 0 {
                stream_state.first_delay
            } else {
                stream_state.later_delay
            };
            if !delay.is_zero() {
                sleep(delay).await;
            }
            let chunk = stream_state.chunks[stream_state.index].clone();
            stream_state.index += 1;
            stream_state
                .counters
                .response_bytes
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
            if stream_state.index == stream_state.chunks.len() {
                stream_state.guard.complete();
            }
            Some((Ok::<Bytes, Infallible>(chunk), stream_state))
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header("x-ai-gateway-perf-mock", "1")
        .body(Body::from_stream(body_stream))
        .expect("mock upstream response must build")
}

struct StreamState {
    chunks: Vec<Bytes>,
    index: usize,
    first_delay: Duration,
    later_delay: Duration,
    guard: InFlightGuard,
    counters: Arc<MockCounters>,
}

fn json_response(api_kind: ApiKind) -> Bytes {
    let value = match api_kind {
        ApiKind::ChatCompletions => serde_json::json!({
            "id": "perf-chat",
            "object": "chat.completion",
            "created": 0,
            "model": "perf",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "OK"},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 8,
                "completion_tokens": 4,
                "total_tokens": 12,
                "prompt_tokens_details": {"cached_tokens": 2}
            }
        }),
        ApiKind::Responses => serde_json::json!({
            "id": "perf-response",
            "object": "response",
            "status": "completed",
            "model": "perf",
            "output": [],
            "usage": {
                "input_tokens": 8,
                "output_tokens": 4,
                "total_tokens": 12,
                "input_tokens_details": {"cached_tokens": 2}
            }
        }),
    };
    Bytes::from(serde_json::to_vec(&value).expect("static mock JSON must serialize"))
}

fn sse_chunks(api_kind: ApiKind, chunk_count: usize) -> Vec<Bytes> {
    match api_kind {
        ApiKind::ChatCompletions => {
            let mut chunks = Vec::with_capacity(chunk_count + 2);
            for index in 0..chunk_count {
                chunks.push(Bytes::from(format!(
                    "data: {{\"id\":\"perf-chat\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{index}\"}},\"finish_reason\":null}}]}}\n\n"
                )));
            }
            chunks.push(Bytes::from_static(
                b"data: {\"id\":\"perf-chat\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":8,\"completion_tokens\":4,\"total_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\n",
            ));
            chunks.push(Bytes::from_static(b"data: [DONE]\n\n"));
            chunks
        }
        ApiKind::Responses => {
            let mut chunks = Vec::with_capacity(chunk_count + 1);
            for index in 0..chunk_count {
                chunks.push(Bytes::from(format!(
                    "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{index}\"}}\n\n"
                )));
            }
            chunks.push(Bytes::from_static(
                b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"perf-response\",\"status\":\"completed\",\"usage\":{\"input_tokens\":8,\"output_tokens\":4,\"total_tokens\":12,\"input_tokens_details\":{\"cached_tokens\":2}}}}\n\n",
            ));
            chunks
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{json_response, sse_chunks};
    use crate::scenario::ApiKind;

    #[test]
    fn mock_payloads_include_terminal_usage() {
        let chat: serde_json::Value =
            serde_json::from_slice(&json_response(ApiKind::ChatCompletions)).unwrap();
        let responses: serde_json::Value =
            serde_json::from_slice(&json_response(ApiKind::Responses)).unwrap();
        assert_eq!(chat["usage"]["completion_tokens"], 4);
        assert_eq!(responses["usage"]["output_tokens"], 4);
        assert!(
            sse_chunks(ApiKind::ChatCompletions, 1)
                .iter()
                .any(|chunk| chunk.as_ref().windows(6).any(|part| part == b"[DONE]"))
        );
        assert!(sse_chunks(ApiKind::Responses, 1).iter().any(|chunk| {
            chunk
                .as_ref()
                .windows("response.completed".len())
                .any(|part| part == b"response.completed")
        }));
    }
}
