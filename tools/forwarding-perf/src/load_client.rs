//! Fixed-concurrency HTTP/SSE load client with bounded percentile aggregation.

use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use bytes::Bytes;
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use tokio::{task::JoinSet, time::sleep_until};

use crate::{
    metrics::{LoadResult, PhaseAccumulator, RequestObservation, RequestOutcome},
    scenario::ApiKind,
};

const MAX_TERMINAL_TAIL_BYTES: usize = 32 * 1_024;

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub scenario: String,
    pub target: String,
    pub api_kind: ApiKind,
    pub streamed: bool,
    pub concurrency: usize,
    pub warmup_seconds: u64,
    pub duration_seconds: u64,
    pub timeout_seconds: u64,
    pub api_key: String,
    pub model: String,
}

impl LoadOptions {
    pub fn validate(&self) -> Result<(), String> {
        if self.concurrency == 0 {
            return Err("load concurrency must be greater than zero".into());
        }
        if self.duration_seconds == 0 {
            return Err("load duration_seconds must be greater than zero".into());
        }
        if self.timeout_seconds == 0 {
            return Err("load timeout_seconds must be greater than zero".into());
        }
        if self.target.trim().is_empty()
            || self.api_key.trim().is_empty()
            || self.model.trim().is_empty()
        {
            return Err("load target, api_key, and model must not be empty".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
struct WorkerConfig {
    client: reqwest::Client,
    url: Arc<str>,
    authorization: Arc<str>,
    request_body: Bytes,
    api_kind: ApiKind,
    streamed: bool,
    start_at: tokio::time::Instant,
    warmup_end: tokio::time::Instant,
    measurement_end: tokio::time::Instant,
}

pub async fn run(options: LoadOptions) -> Result<LoadResult, Box<dyn Error + Send + Sync>> {
    options.validate()?;
    let target = options.target.trim_end_matches('/');
    let url: Arc<str> = format!("{target}{}", options.api_kind.path()).into();
    let request_body = request_body(options.api_kind, options.streamed, &options.model)?;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(options.concurrency)
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .timeout(Duration::from_secs(options.timeout_seconds))
        .build()?;
    let start_at = tokio::time::Instant::now() + Duration::from_millis(250);
    let warmup_end = start_at + Duration::from_secs(options.warmup_seconds);
    let measurement_end = warmup_end + Duration::from_secs(options.duration_seconds);
    let worker = WorkerConfig {
        client,
        url,
        authorization: format!("Bearer {}", options.api_key).into(),
        request_body,
        api_kind: options.api_kind,
        streamed: options.streamed,
        start_at,
        warmup_end,
        measurement_end,
    };

    let started_at = Utc::now();
    let mut workers = JoinSet::new();
    for _ in 0..options.concurrency {
        workers.spawn(run_worker(worker.clone()));
    }

    let mut warmup = PhaseAccumulator::default();
    let mut measurement = PhaseAccumulator::default();
    while let Some(result) = workers.join_next().await {
        let (worker_warmup, worker_measurement) = result?;
        warmup.merge(&worker_warmup);
        measurement.merge(&worker_measurement);
    }
    let completed_at = Utc::now();

    let seconds = options.duration_seconds as f64;
    let errors = measurement.requests.saturating_sub(measurement.succeeded);
    let result = LoadResult {
        scenario: options.scenario,
        target: target.into(),
        api_format: options.api_kind.database_name().into(),
        streamed: options.streamed,
        concurrency: options.concurrency,
        warmup_seconds: options.warmup_seconds,
        duration_seconds: options.duration_seconds,
        started_at,
        completed_at,
        warmup: warmup.counters(),
        measurement: measurement.counters(),
        achieved_rps: measurement.requests as f64 / seconds,
        success_rps: measurement.succeeded as f64 / seconds,
        error_rate: if measurement.requests == 0 {
            0.0
        } else {
            errors as f64 / measurement.requests as f64
        },
        bytes_per_second: measurement.bytes_received as f64 / seconds,
        latency: measurement.latency.summary(),
        ttft: measurement.ttft.summary(),
    };
    Ok(result)
}

async fn run_worker(config: WorkerConfig) -> (PhaseAccumulator, PhaseAccumulator) {
    sleep_until(config.start_at).await;
    let mut warmup = PhaseAccumulator::default();
    let mut measurement = PhaseAccumulator::default();
    loop {
        let request_started = tokio::time::Instant::now();
        if request_started >= config.measurement_end {
            break;
        }
        let observation = execute_request(&config).await;
        if request_started < config.warmup_end {
            warmup.record(observation);
        } else {
            measurement.record(observation);
        }
    }
    (warmup, measurement)
}

async fn execute_request(config: &WorkerConfig) -> RequestObservation {
    let started = Instant::now();
    let response = config
        .client
        .post(config.url.as_ref())
        .header(AUTHORIZATION, config.authorization.as_ref())
        .header(CONTENT_TYPE, "application/json")
        .body(config.request_body.clone())
        .send()
        .await;
    let Ok(response) = response else {
        return RequestObservation {
            outcome: RequestOutcome::TransportError,
            latency_us: elapsed_micros(started),
            ttft_us: None,
            bytes_received: 0,
        };
    };

    let success_status = response.status().is_success();
    let valid_content_type = !config.streamed
        || response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|media_type| media_type.trim() == "text/event-stream")
            });
    let mut body = response.bytes_stream();
    let mut bytes_received = 0_u64;
    let mut ttft_us = None;
    let mut tail = Vec::new();
    while let Some(chunk) = body.next().await {
        let Ok(chunk) = chunk else {
            return RequestObservation {
                outcome: RequestOutcome::BodyError,
                latency_us: elapsed_micros(started),
                ttft_us,
                bytes_received,
            };
        };
        if chunk.is_empty() {
            continue;
        }
        ttft_us.get_or_insert_with(|| elapsed_micros(started));
        bytes_received = bytes_received.saturating_add(chunk.len() as u64);
        if config.streamed {
            append_tail(&mut tail, &chunk);
        }
    }

    let stream_complete = !config.streamed || terminal_seen(config.api_kind, &tail);
    let outcome = if !success_status {
        RequestOutcome::HttpError
    } else if bytes_received == 0 || !valid_content_type || !stream_complete {
        RequestOutcome::BodyError
    } else {
        RequestOutcome::Succeeded
    };
    RequestObservation {
        outcome,
        latency_us: elapsed_micros(started),
        ttft_us,
        bytes_received,
    }
}

fn request_body(
    api_kind: ApiKind,
    streamed: bool,
    model: &str,
) -> Result<Bytes, serde_json::Error> {
    let value = match api_kind {
        ApiKind::ChatCompletions => serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "Reply with OK."}],
            "max_tokens": 4,
            "stream": streamed,
            "stream_options": streamed.then_some(serde_json::json!({"include_usage": true})),
        }),
        ApiKind::Responses => serde_json::json!({
            "model": model,
            "input": "Reply with OK.",
            "max_output_tokens": 4,
            "stream": streamed,
        }),
    };
    serde_json::to_vec(&value).map(Bytes::from)
}

fn append_tail(tail: &mut Vec<u8>, chunk: &[u8]) {
    if chunk.len() >= MAX_TERMINAL_TAIL_BYTES {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - MAX_TERMINAL_TAIL_BYTES..]);
        return;
    }
    let overflow = tail
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(MAX_TERMINAL_TAIL_BYTES);
    if overflow > 0 {
        tail.drain(..overflow);
    }
    tail.extend_from_slice(chunk);
}

fn terminal_seen(api_kind: ApiKind, bytes: &[u8]) -> bool {
    let marker = match api_kind {
        ApiKind::ChatCompletions => b"data: [DONE]".as_slice(),
        ApiKind::Responses => b"response.completed".as_slice(),
    };
    bytes
        .windows(marker.len())
        .any(|candidate| candidate == marker)
}

fn elapsed_micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::{append_tail, terminal_seen};
    use crate::scenario::ApiKind;

    #[test]
    fn terminal_markers_are_format_specific() {
        assert!(terminal_seen(ApiKind::ChatCompletions, b"data: [DONE]\n\n"));
        assert!(terminal_seen(
            ApiKind::Responses,
            b"event: response.completed\n"
        ));
        assert!(!terminal_seen(ApiKind::Responses, b"data: [DONE]\n\n"));
    }

    #[test]
    fn terminal_tail_is_bounded_and_keeps_the_suffix() {
        let mut tail = vec![b'a'; 32 * 1_024];
        append_tail(&mut tail, b"data: [DONE]\n\n");
        assert_eq!(tail.len(), 32 * 1_024);
        assert!(tail.ends_with(b"data: [DONE]\n\n"));
    }
}
