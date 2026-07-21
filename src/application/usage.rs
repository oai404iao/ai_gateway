//! Bounded, format-specific response usage extraction.
//!
//! The collector never buffers an ordinary response body. For JSON it retains
//! only the top-level `usage` object; for SSE it retains one event frame at a
//! time and inspects its `data:` JSON payload.

use axum::body::Bytes;
use serde_json::Value;

use crate::domain::ApiFormat;

const MAX_USAGE_OBJECT_BYTES: usize = 64 * 1_024;
const MAX_SSE_FRAME_BYTES: usize = 8 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
}

impl ResponseUsage {
    fn from_value(api_format: ApiFormat, value: &Value) -> Option<Self> {
        let usage = value
            .get("usage")
            .or_else(|| {
                value
                    .get("response")
                    .and_then(|response| response.get("usage"))
            })
            .unwrap_or(value);
        let (input_field, output_field, details_field) = match api_format {
            ApiFormat::OpenAiChatCompletions => (
                "prompt_tokens",
                "completion_tokens",
                "prompt_tokens_details",
            ),
            ApiFormat::OpenAiResponses => ("input_tokens", "output_tokens", "input_tokens_details"),
        };
        let input_tokens = token(usage.get(input_field))?;
        let output_tokens = token(usage.get(output_field))?;
        let details = usage.get(details_field);
        let cached_input_tokens = details
            .and_then(|details| token(details.get("cached_tokens")))
            .or_else(|| match api_format {
                ApiFormat::OpenAiChatCompletions => token(usage.get("prompt_cache_hit_tokens")),
                ApiFormat::OpenAiResponses => None,
            })
            .unwrap_or(0);
        let cache_write_tokens = details
            .and_then(|details| {
                token(details.get("cache_write_tokens"))
                    .or_else(|| token(details.get("cache_creation_tokens")))
            })
            .unwrap_or(0);
        (cached_input_tokens <= input_tokens && cache_write_tokens <= input_tokens).then_some(
            Self {
                input_tokens,
                cached_input_tokens,
                cache_write_tokens,
                output_tokens,
            },
        )
    }
}

fn token(value: Option<&Value>) -> Option<i64> {
    value?.as_i64().filter(|value| *value >= 0)
}

pub struct UsageCollector {
    api_format: ApiFormat,
    mode: CollectorMode,
    latest: Option<ResponseUsage>,
    terminal: Option<ResponseUsage>,
    stream_completed: bool,
}

enum CollectorMode {
    Json(TopLevelUsageScanner),
    Sse(SseUsageScanner),
}

impl UsageCollector {
    #[must_use]
    pub fn new(api_format: ApiFormat, sse: bool) -> Self {
        Self {
            api_format,
            mode: if sse {
                CollectorMode::Sse(SseUsageScanner::default())
            } else {
                CollectorMode::Json(TopLevelUsageScanner::default())
            },
            latest: None,
            terminal: None,
            stream_completed: false,
        }
    }

    pub fn observe(&mut self, bytes: &Bytes) {
        let api_format = self.api_format;
        let (values, stream_completed) = match &mut self.mode {
            CollectorMode::Json(scanner) => (scanner.push(bytes), false),
            CollectorMode::Sse(scanner) => scanner.push(bytes, api_format),
        };
        self.stream_completed |= stream_completed;
        self.record(values);
    }

    /// Processes one terminal SSE frame that ended with the upstream body
    /// rather than an SSE blank-line delimiter. This must only be called once
    /// the upstream body completed cleanly.
    pub fn finalize(&mut self) {
        let api_format = self.api_format;
        let (values, stream_completed) = match &mut self.mode {
            CollectorMode::Json(_) => (Vec::new(), false),
            CollectorMode::Sse(scanner) => scanner.finalize(api_format),
        };
        self.stream_completed |= stream_completed;
        self.record(values);
    }

    fn record(&mut self, values: Vec<Value>) {
        for value in values {
            if let Some(usage) = ResponseUsage::from_value(self.api_format, &value) {
                if is_terminal_usage_event(self.api_format, &value) {
                    self.terminal = Some(usage);
                }
                self.latest = Some(usage);
            }
        }
    }

    #[must_use]
    pub fn latest(&self) -> Option<ResponseUsage> {
        self.terminal.or(self.latest)
    }

    /// Returns true once a successful application-level SSE terminator has
    /// been observed. Clients are allowed to close after this point without
    /// waiting for the upstream transport to reach EOF.
    #[must_use]
    pub fn stream_completed(&self) -> bool {
        self.stream_completed
    }
}

fn is_terminal_usage_event(api_format: ApiFormat, value: &Value) -> bool {
    match api_format {
        ApiFormat::OpenAiChatCompletions => value
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                choices.iter().any(|choice| {
                    choice
                        .get("finish_reason")
                        .is_some_and(|reason| !reason.is_null())
                })
            }),
        ApiFormat::OpenAiResponses => {
            value.get("type").and_then(Value::as_str) == Some("response.completed")
        }
    }
}

#[derive(Default)]
struct TopLevelUsageScanner {
    depth: usize,
    in_string: bool,
    escaped: bool,
    key: Vec<u8>,
    reading_top_level_key: bool,
    expecting_colon: bool,
    expecting_value: bool,
    capture: Option<JsonObjectCapture>,
    disabled: bool,
}

impl TopLevelUsageScanner {
    fn push(&mut self, bytes: &Bytes) -> Vec<Value> {
        let mut values = Vec::new();
        if self.disabled {
            return values;
        }
        for byte in bytes {
            if let Some(capture) = &mut self.capture {
                if let Some(value) = capture.push(*byte) {
                    self.capture = None;
                    if let Ok(value) = serde_json::from_slice(&value) {
                        values.push(value);
                    }
                }
                continue;
            }
            if self.expecting_colon {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                self.expecting_colon = false;
                if *byte == b':' {
                    self.expecting_value = true;
                    continue;
                }
            }
            if self.expecting_value {
                if byte.is_ascii_whitespace() {
                    continue;
                }
                self.expecting_value = false;
                if *byte == b'{' {
                    self.capture = JsonObjectCapture::new(*byte);
                }
                continue;
            }
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                    if self.reading_top_level_key {
                        self.key.push(*byte);
                    }
                    continue;
                }
                match *byte {
                    b'\\' => self.escaped = true,
                    b'"' => {
                        self.in_string = false;
                        if self.reading_top_level_key && self.key == b"usage" {
                            self.expecting_colon = true;
                        }
                        self.reading_top_level_key = false;
                    }
                    _ if self.reading_top_level_key => self.key.push(*byte),
                    _ => {}
                }
                continue;
            }
            match *byte {
                b'{' | b'[' => self.depth = self.depth.saturating_add(1),
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                b'"' => {
                    self.in_string = true;
                    self.reading_top_level_key = self.depth == 1;
                    self.key.clear();
                }
                _ => {}
            }
        }
        values
    }
}

struct JsonObjectCapture {
    bytes: Vec<u8>,
    depth: usize,
    in_string: bool,
    escaped: bool,
}

impl JsonObjectCapture {
    fn new(first: u8) -> Option<Self> {
        Some(Self {
            bytes: vec![first],
            depth: 1,
            in_string: false,
            escaped: false,
        })
    }

    fn push(&mut self, byte: u8) -> Option<Vec<u8>> {
        if self.bytes.len() == MAX_USAGE_OBJECT_BYTES {
            self.bytes.clear();
            self.depth = 0;
            return None;
        }
        self.bytes.push(byte);
        if self.in_string {
            if self.escaped {
                self.escaped = false;
            } else if byte == b'\\' {
                self.escaped = true;
            } else if byte == b'"' {
                self.in_string = false;
            }
            return None;
        }
        match byte {
            b'"' => self.in_string = true,
            b'{' | b'[' => self.depth += 1,
            b'}' | b']' => {
                self.depth = self.depth.saturating_sub(1);
                if self.depth == 0 {
                    return Some(std::mem::take(&mut self.bytes));
                }
            }
            _ => {}
        }
        None
    }
}

#[derive(Default)]
struct SseUsageScanner {
    bytes: Vec<u8>,
    disabled: bool,
}

impl SseUsageScanner {
    fn push(&mut self, bytes: &Bytes, api_format: ApiFormat) -> (Vec<Value>, bool) {
        if self.disabled {
            return (Vec::new(), false);
        }
        if self.bytes.len().saturating_add(bytes.len()) > MAX_SSE_FRAME_BYTES {
            self.bytes.clear();
            self.disabled = true;
            return (Vec::new(), false);
        }
        self.bytes.extend_from_slice(bytes);
        let mut values = Vec::new();
        let mut stream_completed = false;
        while let Some(end) = sse_frame_end(&self.bytes) {
            let frame = self.bytes.drain(..end).collect::<Vec<_>>();
            let observation = observe_sse_frame(&frame, api_format);
            stream_completed |= observation.stream_completed;
            if let Some(value) = observation.value {
                values.push(value);
            }
        }
        (values, stream_completed)
    }

    fn finalize(&mut self, api_format: ApiFormat) -> (Vec<Value>, bool) {
        if self.disabled {
            return (Vec::new(), false);
        }
        let frame = std::mem::take(&mut self.bytes);
        let observation = observe_sse_frame(&frame, api_format);
        (
            observation.value.into_iter().collect(),
            observation.stream_completed,
        )
    }
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

struct SseFrameObservation {
    value: Option<Value>,
    stream_completed: bool,
}

fn observe_sse_frame(frame: &[u8], api_format: ApiFormat) -> SseFrameObservation {
    let mut event = None;
    let mut data = Vec::new();
    for line in frame.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(value) = line.strip_prefix(b"event:") {
            event = Some(value.strip_prefix(b" ").unwrap_or(value));
        } else if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(value);
        }
    }
    if data.as_slice() == b"[DONE]" {
        return SseFrameObservation {
            value: None,
            stream_completed: true,
        };
    }
    let value: Option<Value> = (!data.is_empty())
        .then(|| serde_json::from_slice(&data).ok())
        .flatten();
    let stream_completed = api_format == ApiFormat::OpenAiResponses
        && (event == Some(b"response.completed".as_slice())
            || value
                .as_ref()
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                == Some("response.completed"));
    SseFrameObservation {
        value,
        stream_completed,
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;

    use super::{ResponseUsage, UsageCollector};
    use crate::domain::ApiFormat;

    #[test]
    fn extracts_chat_usage_from_split_nonstreaming_json_without_buffering_body() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, false);
        collector.observe(&Bytes::from_static(br#"{"id":"x","usage":{"prompt_"#));
        assert_eq!(collector.latest(), None);
        collector.observe(&Bytes::from_static(
            br#"tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":3}}}"#,
        ));
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 3,
                cache_write_tokens: 0,
                output_tokens: 4,
            })
        );
    }

    #[test]
    fn extracts_responses_usage_from_completed_sse_event() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        collector.observe(&Bytes::from_static(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":2,\"input_tokens_details\":{\"cached_tokens\":1}}}}\n\n",
        ));
        assert!(collector.stream_completed());
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 9,
                cached_input_tokens: 1,
                cache_write_tokens: 0,
                output_tokens: 2,
            })
        );
    }

    #[test]
    fn recognizes_done_sentinel_split_across_chunks() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(b"data: [DO"));
        assert!(!collector.stream_completed());
        collector.observe(&Bytes::from_static(b"NE]\n\n"));
        assert!(collector.stream_completed());
    }

    #[test]
    fn extracts_deepseek_chat_cache_hits_from_top_level_usage() {
        let value = serde_json::json!({
            "usage": {
                "prompt_tokens": 87,
                "completion_tokens": 4,
                "prompt_cache_hit_tokens": 43,
                "prompt_cache_miss_tokens": 44
            }
        });
        assert_eq!(
            ResponseUsage::from_value(ApiFormat::OpenAiChatCompletions, &value),
            Some(ResponseUsage {
                input_tokens: 87,
                cached_input_tokens: 43,
                cache_write_tokens: 0,
                output_tokens: 4,
            })
        );
    }

    #[test]
    fn finalizes_an_unterminated_terminal_sse_usage_frame() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            br#"data: {"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":0,"completion_tokens":1928}}

data: {"object":"chat.completion.chunk","choices":[{"delta":{"content":""},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":87,"completion_tokens":1361,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":87}}"#,
        ));
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 0,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 1928,
            })
        );

        collector.finalize();

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 87,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 1361,
            })
        );
    }

    #[test]
    fn prefers_usage_attached_to_a_chat_finish_chunk() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            br#"data: {"object":"chat.completion.chunk","choices":[{"delta":{"content":""},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":84,"completion_tokens":721}}

data: {"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":0,"completion_tokens":834}}

"#,
        ));

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 84,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 721,
            })
        );
    }
}
