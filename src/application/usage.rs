//! Bounded, format-specific response usage, terminal-state, and error-detail
//! extraction.
//!
//! The collector never buffers an ordinary response body. For JSON it retains
//! only the top-level `usage` object, plus a bounded prefix when error-body
//! capture is explicitly enabled. For SSE it retains one event frame at a time
//! and inspects its event name and `data:` JSON payload.

use std::io::{self, Write};
use std::sync::Arc;

use ai_gateway_connector_sdk::{
    CanonicalUsage, MAX_USAGE_BYTES, USAGE_PARSE, UsageDescriptor, UsageFormat, UsageParseInput,
    UsageParseOutput, parse_general_usage,
};
use axum::body::Bytes;
use serde::Deserialize;
use serde_json::Value;

use crate::connector_plugins::Plugin;
use crate::domain::{ApiFormat, ApiOperation};

const MAX_USAGE_OBJECT_BYTES: usize = 64 * 1_024;
const MAX_SSE_FRAME_BYTES: usize = 8 * 1_024 * 1_024;
const MAX_ERROR_CODE_BYTES: usize = 100;
const MAX_ERROR_SUMMARY_BYTES: usize = 16 * 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
}

impl ResponseUsage {
    #[cfg(test)]
    fn from_value(api_format: ApiFormat, value: &Value) -> Option<Self> {
        parse_general_usage(
            default_usage_format(api_format),
            usage_object(value).unwrap_or(value),
        )
        .map(Into::into)
    }
}

impl From<CanonicalUsage> for ResponseUsage {
    fn from(usage: CanonicalUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_write_tokens: usage.cache_write_tokens,
            output_tokens: usage.output_tokens,
            reasoning_tokens: usage.reasoning_tokens,
        }
    }
}

#[derive(Clone)]
pub(crate) enum UsageParserConfig {
    General(UsageFormat),
    Plugin {
        plugin: Arc<Plugin>,
        operation: ApiOperation,
        interface: String,
    },
}

impl UsageParserConfig {
    pub(crate) fn for_operation(operation: ApiOperation) -> Self {
        Self::General(default_usage_format(operation.api_format()))
    }

    pub(crate) fn for_plugin(plugin: &Arc<Plugin>, operation: ApiOperation) -> Self {
        match plugin
            .attempt_descriptor(operation.as_str())
            .expect("prepared connectors have validated descriptors")
            .usage
            .as_ref()
        {
            Some(UsageDescriptor::General { format }) => Self::General(*format),
            Some(UsageDescriptor::Plugin { interface }) => Self::Plugin {
                plugin: Arc::clone(plugin),
                operation,
                interface: interface.clone(),
            },
            None => Self::General(default_usage_format(operation.api_format())),
        }
    }

    fn parse(&self, usage: &Value, partial: &mut AnthropicUsageSnapshot) -> Option<ResponseUsage> {
        let usage = match self {
            Self::General(UsageFormat::AnthropicMessages) => {
                if bounded_usage_body(usage).is_none() {
                    partial.invalidate();
                    return None;
                }
                partial.observe(usage)?
            }
            Self::General(format) => return parse_general_usage(*format, usage).map(Into::into),
            Self::Plugin {
                plugin,
                operation,
                interface,
            } => {
                let body = bounded_usage_body(usage)?;
                let input = UsageParseInput {
                    operation: operation.as_str().into(),
                    interface: interface.clone(),
                };
                if !input.validate_bounds(&body) {
                    return None;
                }
                let output = plugin
                    .call(USAGE_PARSE, &serde_json::to_value(input).ok()?, &body)
                    .ok()?;
                if !output.body.is_empty() || output.metadata.get("usage").is_none() {
                    return None;
                }
                let output: UsageParseOutput =
                    serde_json::from_value(output.metadata.clone()).ok()?;
                return output
                    .validate_bounds()
                    .then_some(output.usage)?
                    .map(Into::into);
            }
        };
        parse_general_usage(UsageFormat::AnthropicMessages, &usage).map(Into::into)
    }
}

fn default_usage_format(api_format: ApiFormat) -> UsageFormat {
    match api_format {
        ApiFormat::OpenAiChatCompletions => UsageFormat::OpenAiChatCompletions,
        ApiFormat::OpenAiResponses => UsageFormat::OpenAiResponses,
        ApiFormat::OpenAiImages => UsageFormat::OpenAiImages,
    }
}

fn usage_object(value: &Value) -> Option<&Value> {
    value
        .get("usage")
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("usage"))
        })
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("usage"))
        })
}

fn bounded_usage_body(value: &Value) -> Option<Vec<u8>> {
    struct Body(Vec<u8>);
    impl Write for Body {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_USAGE_BYTES - self.0.len() {
                return Err(io::Error::other("usage exceeds limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    if !value.is_object() {
        return None;
    }
    let mut body = Body(Vec::new());
    serde_json::to_writer(&mut body, value).ok()?;
    Some(body.0)
}

struct AnthropicUsageSnapshot {
    input: Option<i64>,
    cached: Option<i64>,
    written: Option<i64>,
    output: Option<i64>,
}

impl Default for AnthropicUsageSnapshot {
    fn default() -> Self {
        Self {
            input: None,
            cached: Some(0),
            written: Some(0),
            output: None,
        }
    }
}

impl AnthropicUsageSnapshot {
    fn invalidate(&mut self) {
        self.input = None;
        self.cached = None;
        self.written = None;
        self.output = None;
    }

    fn observe(&mut self, value: &Value) -> Option<Value> {
        // Anthropic stream counters are cumulative snapshots, not increments.
        for (key, counter) in [
            ("input_tokens", &mut self.input),
            ("cache_read_input_tokens", &mut self.cached),
            ("cache_creation_input_tokens", &mut self.written),
            ("output_tokens", &mut self.output),
        ] {
            if let Some(value) = value.get(key) {
                *counter = value.as_i64().filter(|value| *value >= 0);
            }
        }
        Some(serde_json::json!({
            "input_tokens": self.input?,
            "cache_read_input_tokens": self.cached?,
            "cache_creation_input_tokens": self.written?,
            "output_tokens": self.output?,
        }))
    }
}

pub struct UsageCollector {
    api_format: ApiFormat,
    parser: UsageParserConfig,
    partial: AnthropicUsageSnapshot,
    mode: CollectorMode,
    latest: Option<ResponseUsage>,
    terminal: Option<ResponseUsage>,
    // An invalid terminal summary must not fall back to earlier counts.
    terminal_usage_seen: bool,
    sse_terminal_outcome: Option<SseTerminalOutcome>,
    response_error: Option<ResponseErrorDetails>,
    error_body: Option<ErrorBodyCapture>,
}

enum CollectorMode {
    Json(TopLevelUsageScanner),
    Sse(SseUsageScanner),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SseTerminalOutcome {
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ResponseErrorDetails {
    pub code: Option<String>,
    pub summary: Option<String>,
}

impl ResponseErrorDetails {
    #[must_use]
    pub(crate) fn from_message(code: Option<&str>, message: &str) -> Self {
        Self {
            code: code
                .and_then(|code| sanitize_error_text(code, MAX_ERROR_CODE_BYTES, false, false)),
            summary: sanitize_error_text(message, MAX_ERROR_SUMMARY_BYTES, true, false),
        }
    }

    #[must_use]
    pub(crate) fn from_json(value: &Value) -> Self {
        extract_response_error(Some(value))
    }
}

impl UsageCollector {
    #[must_use]
    pub fn new(api_format: ApiFormat, sse: bool) -> Self {
        Self::with_parser(
            api_format,
            sse,
            UsageParserConfig::General(default_usage_format(api_format)),
        )
    }

    pub(crate) fn with_parser(api_format: ApiFormat, sse: bool, parser: UsageParserConfig) -> Self {
        Self {
            api_format,
            parser,
            partial: AnthropicUsageSnapshot::default(),
            mode: if sse {
                CollectorMode::Sse(SseUsageScanner::default())
            } else {
                CollectorMode::Json(TopLevelUsageScanner::default())
            },
            latest: None,
            terminal: None,
            terminal_usage_seen: false,
            sse_terminal_outcome: None,
            response_error: None,
            error_body: None,
        }
    }

    pub(crate) fn reset(&mut self, sse: bool) {
        *self = Self::with_parser(self.api_format, sse, self.parser.clone());
    }

    pub(crate) fn set_parser(&mut self, parser: UsageParserConfig) {
        *self = Self::with_parser(self.api_format, false, parser);
    }

    /// Retains a bounded prefix of an ordinary response body for diagnostics.
    ///
    /// Callers enable this only after receiving an unsuccessful upstream
    /// status, so successful JSON and Images responses remain unbuffered.
    pub fn capture_error_body(&mut self) {
        if matches!(self.mode, CollectorMode::Json(_)) && self.error_body.is_none() {
            self.error_body = Some(ErrorBodyCapture::default());
        }
    }

    pub fn observe(&mut self, bytes: &Bytes) {
        if self.sse_terminal_outcome.is_some() {
            return;
        }
        if let Some(error_body) = &mut self.error_body {
            error_body.push(bytes);
        }
        let api_format = self.api_format;
        let usage_objects = matches!(self.mode, CollectorMode::Json(_));
        let (values, terminal_outcome, error) = match &mut self.mode {
            CollectorMode::Json(scanner) => (scanner.push(bytes), None, None),
            CollectorMode::Sse(scanner) => scanner.push(bytes, api_format),
        };
        if self.sse_terminal_outcome.is_none() {
            self.sse_terminal_outcome = terminal_outcome;
            self.response_error = error;
        }
        self.record(values, usage_objects, terminal_outcome.is_some());
    }

    /// Observes one complete Responses WebSocket event JSON object.
    ///
    /// WebSocket messages already provide event boundaries, so this path avoids
    /// synthesizing an SSE envelope while preserving the same bounded usage and
    /// structured-error extraction semantics.
    pub fn observe_websocket_event(&mut self, bytes: &Bytes) -> Option<SseTerminalOutcome> {
        if self.sse_terminal_outcome.is_some() {
            return None;
        }
        #[derive(Deserialize)]
        struct EventTypeProbe<'a> {
            #[serde(borrow, rename = "type")]
            kind: Option<&'a str>,
        }

        let Ok(probe) = serde_json::from_slice::<EventTypeProbe<'_>>(bytes) else {
            return None;
        };
        let terminal_outcome = match probe.kind {
            Some("response.completed") => Some(SseTerminalOutcome::Completed),
            Some("error" | "response.failed" | "response.incomplete" | "response.cancelled") => {
                Some(SseTerminalOutcome::Failed)
            }
            _ => None,
        }?;
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return None;
        };
        if self.sse_terminal_outcome.is_none() {
            self.sse_terminal_outcome = Some(terminal_outcome);
            self.response_error = (terminal_outcome == SseTerminalOutcome::Failed)
                .then(|| extract_response_error(Some(&value)));
        }
        self.record(vec![value], false, true);
        Some(terminal_outcome)
    }

    /// Processes one terminal SSE frame that ended with the upstream body
    /// rather than an SSE blank-line delimiter. This must only be called once
    /// the upstream body completed cleanly.
    pub fn finalize(&mut self) {
        if self.sse_terminal_outcome.is_some() {
            return;
        }
        let api_format = self.api_format;
        let usage_objects = matches!(self.mode, CollectorMode::Json(_));
        let (values, terminal_outcome, error) = match &mut self.mode {
            CollectorMode::Json(scanner) => (scanner.finalize(), None, None),
            CollectorMode::Sse(scanner) => scanner.finalize(api_format),
        };
        if self.sse_terminal_outcome.is_none() {
            self.sse_terminal_outcome = terminal_outcome;
            self.response_error = error;
        }
        self.record(values, usage_objects, terminal_outcome.is_some());
    }

    fn record(&mut self, values: Vec<Value>, usage_objects: bool, terminal: bool) {
        let last = values.len().saturating_sub(1);
        for (index, value) in values.into_iter().enumerate() {
            let Some(source) = (if usage_objects {
                Some(&value)
            } else {
                usage_object(&value)
            }) else {
                continue;
            };
            let usage = self.parser.parse(source, &mut self.partial);
            if is_terminal_usage_event(self.api_format, &value) || (terminal && index == last) {
                self.terminal_usage_seen = true;
                self.terminal = usage;
            } else if usage.is_none() {
                self.terminal = None;
            }
            self.latest = usage;
        }
    }

    #[must_use]
    pub fn latest(&self) -> Option<ResponseUsage> {
        if self.terminal_usage_seen {
            self.terminal
        } else {
            self.latest
        }
    }

    /// Returns the first application-level SSE terminal event observed.
    ///
    /// Clients commonly close immediately after either a successful terminator
    /// or an error event, without waiting for the upstream transport to reach
    /// EOF. Remembering the protocol outcome prevents that disconnect from
    /// overwriting the real upstream result.
    #[must_use]
    pub fn sse_terminal_outcome(&self) -> Option<SseTerminalOutcome> {
        self.sse_terminal_outcome
    }

    /// Returns bounded upstream error fields extracted from the first failing
    /// SSE/WebSocket terminal event or captured non-streaming error body.
    #[must_use]
    pub fn error_details(&self) -> Option<ResponseErrorDetails> {
        self.response_error
            .clone()
            .or_else(|| self.error_body.as_ref().and_then(ErrorBodyCapture::details))
    }
}

#[derive(Default)]
struct ErrorBodyCapture {
    bytes: Vec<u8>,
    truncated: bool,
}

impl ErrorBodyCapture {
    fn push(&mut self, bytes: &Bytes) {
        let remaining = MAX_ERROR_SUMMARY_BYTES.saturating_sub(self.bytes.len());
        let copied = remaining.min(bytes.len());
        self.bytes.extend_from_slice(&bytes[..copied]);
        self.truncated |= copied < bytes.len();
    }

    fn details(&self) -> Option<ResponseErrorDetails> {
        if self.bytes.is_empty() {
            return None;
        }
        if !self.truncated
            && let Ok(value) = serde_json::from_slice::<Value>(&self.bytes)
        {
            return Some(extract_response_error(Some(&value)));
        }
        let text = String::from_utf8_lossy(&self.bytes);
        Some(ResponseErrorDetails {
            code: None,
            summary: sanitize_error_text(&text, MAX_ERROR_SUMMARY_BYTES, true, self.truncated),
        })
    }
}

fn is_terminal_usage_event(api_format: ApiFormat, value: &Value) -> bool {
    match api_format {
        ApiFormat::OpenAiChatCompletions => value
            .get("choices")
            .and_then(Value::as_array)
            .is_some_and(|choices| {
                // OpenAI emits a final usage-only chunk with `choices: []`,
                // while DeepSeek-compatible streams may attach usage to the
                // chunk carrying `finish_reason`. Treat both as terminal
                // usage summaries so a later OpenAI summary supersedes an
                // earlier finish-chunk value.
                choices.is_empty()
                    || choices.iter().any(|choice| {
                        choice
                            .get("finish_reason")
                            .is_some_and(|reason| !reason.is_null())
                    })
            }),
        ApiFormat::OpenAiResponses => {
            matches!(
                value.get("type").and_then(Value::as_str),
                Some(
                    "response.completed"
                        | "response.failed"
                        | "response.incomplete"
                        | "response.cancelled"
                )
            )
        }
        ApiFormat::OpenAiImages => false,
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
                if capture.bytes.len() == MAX_USAGE_OBJECT_BYTES {
                    self.capture = None;
                    self.disabled = true;
                    values.push(Value::Null);
                    break;
                }
                if let Some(value) = capture.push(*byte) {
                    self.capture = None;
                    values.push(serde_json::from_slice(&value).unwrap_or(Value::Null));
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
                    continue;
                }
                values.push(Value::Null);
            }
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                    if self.reading_top_level_key {
                        self.push_key(*byte);
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
                    _ if self.reading_top_level_key => self.push_key(*byte),
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

    fn push_key(&mut self, byte: u8) {
        if self.key.len() < b"usage".len() {
            self.key.push(byte);
        } else {
            self.reading_top_level_key = false;
        }
    }

    fn finalize(&mut self) -> Vec<Value> {
        if !self.disabled
            && (self.capture.is_some() || self.expecting_value || self.expecting_colon)
        {
            self.capture = None;
            self.disabled = true;
            vec![Value::Null]
        } else {
            Vec::new()
        }
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
    fn push(
        &mut self,
        bytes: &Bytes,
        api_format: ApiFormat,
    ) -> (
        Vec<Value>,
        Option<SseTerminalOutcome>,
        Option<ResponseErrorDetails>,
    ) {
        if self.disabled {
            return (Vec::new(), None, None);
        }
        if self.bytes.len().saturating_add(bytes.len()) > MAX_SSE_FRAME_BYTES {
            self.bytes.clear();
            self.disabled = true;
            return (vec![serde_json::json!({"usage":null})], None, None);
        }
        self.bytes.extend_from_slice(bytes);
        let mut values = Vec::new();
        let mut terminal_outcome = None;
        let mut error = None;
        while let Some(end) = sse_frame_end(&self.bytes) {
            let frame = self.bytes.drain(..end).collect::<Vec<_>>();
            let observation = observe_sse_frame(&frame, api_format);
            if let Some(value) = observation.value {
                values.push(value);
            }
            if observation.terminal_outcome.is_some() {
                terminal_outcome = observation.terminal_outcome;
                error = observation.error;
                break;
            }
        }
        (values, terminal_outcome, error)
    }

    fn finalize(
        &mut self,
        api_format: ApiFormat,
    ) -> (
        Vec<Value>,
        Option<SseTerminalOutcome>,
        Option<ResponseErrorDetails>,
    ) {
        if self.disabled {
            return (Vec::new(), None, None);
        }
        let frame = std::mem::take(&mut self.bytes);
        let observation = observe_sse_frame(&frame, api_format);
        (
            observation.value.into_iter().collect(),
            observation.terminal_outcome,
            observation.error,
        )
    }
}

fn sse_frame_end(bytes: &[u8]) -> Option<usize> {
    let mut line_start = 0;
    let mut index = 0;
    while index < bytes.len() {
        let line_end = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => index + 2,
            b'\r' | b'\n' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        if index == line_start {
            return Some(line_end);
        }
        line_start = line_end;
        index = line_end;
    }
    None
}

struct SseFrameObservation {
    value: Option<Value>,
    terminal_outcome: Option<SseTerminalOutcome>,
    error: Option<ResponseErrorDetails>,
}

fn observe_sse_frame(frame: &[u8], api_format: ApiFormat) -> SseFrameObservation {
    let mut event = None;
    let mut data = Vec::new();
    let mut has_data = false;
    let mut cursor = 0;
    while let Some(line) = next_sse_line(frame, &mut cursor) {
        if line.is_empty() {
            break;
        }
        let Some((field, value)) = sse_field(line) else {
            continue;
        };
        match field {
            b"event" => event = Some(value),
            b"data" => {
                if has_data {
                    data.push(b'\n');
                }
                data.extend_from_slice(value);
                has_data = true;
            }
            _ => {}
        }
    }
    let value: Option<Value> = has_data
        .then(|| serde_json::from_slice(&data).ok())
        .flatten();
    let event_type = value
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str);
    let error_envelope = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .is_some_and(|error| !error.is_null());
    let failed = event == Some(b"error".as_slice())
        || event_type == Some("error")
        || error_envelope
        || (api_format == ApiFormat::OpenAiResponses
            && (matches!(
                event,
                Some(b"response.failed" | b"response.incomplete" | b"response.cancelled")
            ) || matches!(
                event_type,
                Some("response.failed" | "response.incomplete" | "response.cancelled")
            )));
    let completed = data.as_slice() == b"[DONE]"
        || (api_format == ApiFormat::OpenAiResponses
            && (event == Some(b"response.completed".as_slice())
                || event_type == Some("response.completed")));
    let terminal_outcome = if failed {
        Some(SseTerminalOutcome::Failed)
    } else if completed {
        Some(SseTerminalOutcome::Completed)
    } else {
        None
    };
    let error = failed.then(|| {
        value.as_ref().map_or_else(
            || ResponseErrorDetails::from_message(None, &String::from_utf8_lossy(data.as_slice())),
            |value| extract_response_error(Some(value)),
        )
    });
    let value = if completed
        && data.as_slice() != b"[DONE]"
        && !value.as_ref().is_some_and(Value::is_object)
    {
        Some(serde_json::json!({"usage":null}))
    } else {
        value
    };
    SseFrameObservation {
        value,
        terminal_outcome,
        error,
    }
}

fn extract_response_error(value: Option<&Value>) -> ResponseErrorDetails {
    let Some(value) = value else {
        return ResponseErrorDetails::default();
    };
    let nested_error = value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
                .filter(|error| !error.is_null())
        });
    let code = nested_error
        .and_then(|error| {
            error
                .get("code")
                .and_then(error_scalar)
                .or_else(|| error.get("type").and_then(error_scalar))
        })
        .or_else(|| value.get("code").and_then(error_scalar))
        .and_then(|code| sanitize_error_text(&code, MAX_ERROR_CODE_BYTES, false, false));
    let message = nested_error
        .and_then(|error| match error {
            Value::String(message) => Some(message.as_str()),
            Value::Object(_) => error.get("message").and_then(Value::as_str),
            _ => None,
        })
        .or_else(|| value.get("message").and_then(Value::as_str));
    let summary = render_error_summary(value, message);
    ResponseErrorDetails { code, summary }
}

fn render_error_summary(value: &Value, message: Option<&str>) -> Option<String> {
    if let Value::String(value) = value {
        return sanitize_error_text(value, MAX_ERROR_SUMMARY_BYTES, true, false);
    }

    let mut summary = message
        .and_then(|message| sanitize_error_text(message, MAX_ERROR_SUMMARY_BYTES, true, false))
        .unwrap_or_default();
    if summary.len() < MAX_ERROR_SUMMARY_BYTES && !summary.is_empty() {
        summary.push_str("\n\n");
    }
    let remaining = MAX_ERROR_SUMMARY_BYTES.saturating_sub(summary.len());
    let mut writer = BoundedJsonWriter::new(remaining);
    let result = serde_json::to_writer_pretty(&mut writer, value);
    summary.push_str(&String::from_utf8_lossy(&writer.bytes));
    sanitize_error_text(
        &summary,
        MAX_ERROR_SUMMARY_BYTES,
        true,
        result.is_err() || writer.truncated,
    )
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl BoundedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(4 * 1_024)),
            limit,
            truncated: false,
        }
    }
}

impl Write for BoundedJsonWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if remaining == 0 {
            self.truncated = !buffer.is_empty();
            return if buffer.is_empty() {
                Ok(0)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "bounded error detail is full",
                ))
            };
        }
        let copied = remaining.min(buffer.len());
        self.bytes.extend_from_slice(&buffer[..copied]);
        if copied < buffer.len() {
            self.truncated = true;
        }
        Ok(copied)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn error_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn sanitize_error_text(
    value: &str,
    maximum_bytes: usize,
    multiline: bool,
    force_truncated: bool,
) -> Option<String> {
    let mut sanitized = String::new();
    let mut previous_was_cr = false;
    let mut truncated = force_truncated;
    for character in value.chars() {
        let character = match character {
            '\r' if multiline => {
                previous_was_cr = true;
                '\n'
            }
            '\n' if multiline && previous_was_cr => {
                previous_was_cr = false;
                continue;
            }
            '\n' | '\t' if multiline => {
                previous_was_cr = false;
                character
            }
            value if value.is_control() => {
                previous_was_cr = false;
                ' '
            }
            value => {
                previous_was_cr = false;
                value
            }
        };
        if sanitized.len().saturating_add(character.len_utf8()) > maximum_bytes {
            truncated = true;
            break;
        }
        sanitized.push(character);
    }
    let trimmed = sanitized.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut sanitized = trimmed.to_owned();
    if truncated {
        const ELLIPSIS: &str = "…";
        let target = maximum_bytes.saturating_sub(ELLIPSIS.len());
        let mut end = sanitized.len().min(target);
        while !sanitized.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        sanitized.truncate(end);
        sanitized.push_str(ELLIPSIS);
    }
    Some(sanitized)
}

fn next_sse_line<'a>(bytes: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    if *cursor == bytes.len() {
        return None;
    }
    let start = *cursor;
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                *cursor = if bytes.get(index + 1) == Some(&b'\n') {
                    index + 2
                } else {
                    index + 1
                };
                return Some(&bytes[start..index]);
            }
            b'\n' => {
                *cursor = index + 1;
                return Some(&bytes[start..index]);
            }
            _ => index += 1,
        }
    }
    *cursor = bytes.len();
    Some(&bytes[start..])
}

fn sse_field(line: &[u8]) -> Option<(&[u8], &[u8])> {
    if line.first() == Some(&b':') {
        return None;
    }
    let (field, value) = match line.iter().position(|byte| *byte == b':') {
        Some(position) => (&line[..position], &line[position + 1..]),
        None => (line, &[][..]),
    };
    Some((field, value.strip_prefix(b" ").unwrap_or(value)))
}

#[cfg(test)]
mod tests {
    use axum::body::Bytes;

    use super::{ResponseUsage, SseTerminalOutcome, UsageCollector, UsageParserConfig};
    use crate::domain::ApiFormat;
    use ai_gateway_connector_sdk::UsageFormat;

    #[test]
    fn extracts_chat_usage_from_split_nonstreaming_json_without_buffering_body() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, false);
        collector.observe(&Bytes::from_static(br#"{"id":"x","usage":{"prompt_"#));
        assert_eq!(collector.latest(), None);
        collector.observe(&Bytes::from_static(
            br#"tokens":10,"completion_tokens":4,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        ));
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 10,
                cached_input_tokens: 3,
                cache_write_tokens: 0,
                output_tokens: 4,
                reasoning_tokens: 1,
            })
        );
    }

    #[test]
    fn extracts_deepseek_nonstreaming_chat_usage_without_subtracting_reasoning() {
        let body = br#"{
            "id": "b6de8b7e-d52a-4e36-9032-7362d940c5fd",
            "object": "chat.completion",
            "created": 1785837502,
            "model": "deepseek-v4-flash",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "pong",
                    "reasoning_content": "The answer is pong."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 49,
                "total_tokens": 60,
                "prompt_tokens_details": {
                    "cached_tokens": 0
                },
                "completion_tokens_details": {
                    "reasoning_tokens": 46
                },
                "prompt_cache_hit_tokens": 0,
                "prompt_cache_miss_tokens": 11
            }
        }"#;
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, false);
        for chunk in body.chunks(73) {
            collector.observe(&Bytes::copy_from_slice(chunk));
        }

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 49,
                reasoning_tokens: 46,
            })
        );
    }

    #[test]
    fn extracts_deepseek_streaming_chat_usage_from_the_finish_chunk() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            br#"data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":null,"reasoning_content":"The answer is pong."},"finish_reason":null}]}

data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"pong","reasoning_content":null},"finish_reason":null}]}

data: {"object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"","reasoning_content":null},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":45,"total_tokens":56,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":42},"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":11}}

data: [DONE]

"#,
        ));

        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Completed)
        );
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 45,
                reasoning_tokens: 42,
            })
        );
    }

    #[test]
    fn extracts_codex_responses_usage_from_completed_sse_event() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        collector.observe(&Bytes::from_static(
            b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp1\",\"usage\":{\"input_tokens\":100,\"input_tokens_details\":{\"cached_tokens\":40,\"cache_write_tokens\":60},\"output_tokens\":10,\"output_tokens_details\":{\"reasoning_tokens\":5},\"total_tokens\":110}}}\n\n",
        ));
        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Completed)
        );
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 100,
                cached_input_tokens: 40,
                cache_write_tokens: 60,
                output_tokens: 10,
                reasoning_tokens: 5,
            })
        );
    }

    #[test]
    fn extracts_responses_usage_and_terminal_state_from_websocket_event() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, false);
        let terminal = collector.observe_websocket_event(&Bytes::from_static(
            br#"{"type":"response.completed","response":{"usage":{"input_tokens":9,"output_tokens":2,"input_tokens_details":{"cached_tokens":1},"output_tokens_details":{"reasoning_tokens":1}}}}"#,
        ));
        assert_eq!(terminal, Some(SseTerminalOutcome::Completed));
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 9,
                cached_input_tokens: 1,
                cache_write_tokens: 0,
                output_tokens: 2,
                reasoning_tokens: 1,
            })
        );
    }

    #[test]
    fn extracts_images_usage_without_buffering_base64_output() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiImages, false);
        collector.observe(&Bytes::from_static(
            br#"{"created":1,"data":[{"b64_json":"aW1h"#,
        ));
        collector.observe(&Bytes::from_static(
            br#"Z2U="}],"usage":{"input_tokens":7,"output_tokens":11,"input_tokens_details":{"image_tokens":0,"text_tokens":7},"output_tokens_details":{"image_tokens":11,"text_tokens":0}}}"#,
        ));

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 7,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 11,
                reasoning_tokens: 0,
            })
        );
    }

    #[test]
    fn extracts_responses_error_from_websocket_terminal_event() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, false);
        assert_eq!(
            collector.observe_websocket_event(&Bytes::from_static(
                br#"{"type":"error","status":404,"error":{"code":"previous_response_not_found","message":"Previous response was not found. Retrying the full request."}}"#,
            )),
            Some(SseTerminalOutcome::Failed)
        );
        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("previous_response_not_found"));
        let summary = error.summary.unwrap();
        assert!(
            summary.starts_with("Previous response was not found. Retrying the full request.\n\n{")
        );
        assert!(summary.contains("\"status\": 404"));
        assert!(summary.contains("\"previous_response_not_found\""));
    }

    #[test]
    fn recognizes_done_sentinel_split_across_chunks() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(b"data: [DO"));
        assert_eq!(collector.sse_terminal_outcome(), None);
        collector.observe(&Bytes::from_static(b"NE]\n\n"));
        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Completed)
        );
    }

    #[test]
    fn recognizes_responses_error_event_split_across_chunks() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        collector.observe(&Bytes::from_static(b"event: err"));
        assert_eq!(collector.sse_terminal_outcome(), None);
        collector.observe(&Bytes::from_static(
            b"or\r\ndata: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"failed\",\"param\":null,\"sequence_number\":3}\r\n\r\n",
        ));
        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Failed)
        );
        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("server_error"));
        let summary = error.summary.unwrap();
        assert!(summary.starts_with("failed\n\n{"));
        assert!(summary.contains("\"param\": null"));
        assert!(summary.contains("\"sequence_number\": 3"));
    }

    #[test]
    fn recognizes_response_failed_and_its_terminal_usage() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        collector.observe(&Bytes::from_static(
            b"event: response.failed\rdata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"model_error\",\"message\":\"generation failed\"},\"usage\":{\"input_tokens\":7,\"output_tokens\":1,\"input_tokens_details\":{\"cached_tokens\":2}}}}\r\r",
        ));
        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Failed)
        );
        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 7,
                cached_input_tokens: 2,
                cache_write_tokens: 0,
                output_tokens: 1,
                reasoning_tokens: 0,
            })
        );
        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("model_error"));
        let summary = error.summary.unwrap();
        assert!(summary.starts_with("generation failed\n\n{"));
        assert!(summary.contains("\"input_tokens\": 7"));
        assert!(summary.contains("\"output_tokens\": 1"));
    }

    #[test]
    fn recognizes_chat_error_envelope_without_misclassifying_normal_chunks() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            b"data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"content\":\"error\"}}]}\n\n",
        ));
        assert_eq!(collector.sse_terminal_outcome(), None);
        collector.observe(&Bytes::from_static(
            b"data: {\"error\":{\"message\":\"upstream failed\",\"type\":\"server_error\",\"code\":null}}\n\n",
        ));
        assert_eq!(
            collector.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Failed)
        );
        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("server_error"));
        let summary = error.summary.unwrap();
        assert!(summary.starts_with("upstream failed\n\n{"));
        assert!(summary.contains("\"type\": \"server_error\""));
        assert!(summary.contains("\"code\": null"));
    }

    #[test]
    fn sanitizes_and_bounds_sse_error_fields() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        let message = format!("first\r\nsecond\0{}", "界".repeat(10_000));
        collector.observe(&Bytes::from(
            format!(
                "event: error\ndata: {}\n\n",
                serde_json::json!({
                    "type": "error",
                    "code": "provider\ncode",
                    "message": message,
                })
            )
            .into_bytes(),
        ));
        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("provider code"));
        assert!(
            error
                .summary
                .as_ref()
                .unwrap()
                .starts_with("first\nsecond ")
        );
        assert!(error.summary.as_ref().unwrap().ends_with('…'));
        assert!(error.summary.as_ref().unwrap().len() <= super::MAX_ERROR_SUMMARY_BYTES);
    }

    #[test]
    fn captures_complete_nonstreaming_json_error_details() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, false);
        collector.capture_error_body();
        collector.observe(&Bytes::from_static(
            br#"{"error":{"message":"quota exhausted","type":"rate_limit_error","#,
        ));
        collector.observe(&Bytes::from_static(
            br#""param":"organization","code":"insufficient_quota"},"request_id":"req_123"}"#,
        ));

        let error = collector.error_details().unwrap();
        assert_eq!(error.code.as_deref(), Some("insufficient_quota"));
        let summary = error.summary.unwrap();
        assert!(summary.starts_with("quota exhausted\n\n{"));
        assert!(summary.contains("\"param\": \"organization\""));
        assert!(summary.contains("\"request_id\": \"req_123\""));
    }

    #[test]
    fn bounds_plain_text_error_bodies() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, false);
        collector.capture_error_body();
        collector.observe(&Bytes::from("failure ".repeat(4_096)));

        let error = collector.error_details().unwrap();
        assert_eq!(error.code, None);
        let summary = error.summary.unwrap();
        assert!(summary.starts_with("failure failure"));
        assert!(summary.ends_with('…'));
        assert!(summary.len() <= super::MAX_ERROR_SUMMARY_BYTES);
    }

    #[test]
    fn extracts_deepseek_chat_cache_hits_from_top_level_usage() {
        let value = serde_json::json!({
            "usage": {
                "prompt_tokens": 87,
                "completion_tokens": 4,
                "prompt_tokens_details": {
                    "cached_tokens": 0
                },
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
                reasoning_tokens: 0,
            })
        );
    }

    #[test]
    fn finalizes_an_unterminated_terminal_sse_usage_frame() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            br#"data: {"object":"chat.completion.chunk","choices":[{"delta":{"content":null,"reasoning_content":"The answer is pong."},"finish_reason":null}]}

data: {"object":"chat.completion.chunk","choices":[{"delta":{"content":""},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":11,"completion_tokens":45,"total_tokens":56,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":42},"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":11}}"#,
        ));
        assert_eq!(collector.latest(), None);

        collector.finalize();

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 45,
                reasoning_tokens: 42,
            })
        );
    }

    #[test]
    fn prefers_a_later_openai_usage_summary_over_finish_chunk_usage() {
        let mut collector = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        collector.observe(&Bytes::from_static(
            br#"data: {"object":"chat.completion.chunk","choices":[{"delta":{"content":""},"finish_reason":"stop","index":0}],"usage":{"prompt_tokens":11,"completion_tokens":44,"total_tokens":55,"completion_tokens_details":{"reasoning_tokens":41}}}

data: {"object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":45,"total_tokens":56,"completion_tokens_details":{"reasoning_tokens":42}}}

"#,
        ));

        assert_eq!(
            collector.latest(),
            Some(ResponseUsage {
                input_tokens: 11,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 45,
                reasoning_tokens: 42,
            })
        );
    }

    #[test]
    fn rejects_reasoning_tokens_larger_than_total_output() {
        let value = serde_json::json!({
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2,
                "output_tokens_details": {
                    "reasoning_tokens": 3
                }
            }
        });

        assert_eq!(
            ResponseUsage::from_value(ApiFormat::OpenAiResponses, &value),
            None
        );
    }

    #[test]
    fn incomplete_and_cancelled_responses_are_failed_terminals_with_usage() {
        for kind in ["response.incomplete", "response.cancelled"] {
            for include_type in [false, true] {
                let mut value = serde_json::json!({
                    "response":{"status":"incomplete","usage":{"input_tokens":5,"output_tokens":2}}
                });
                if include_type {
                    value["type"] = serde_json::json!(kind);
                }
                let bytes = Bytes::from(format!("event: {kind}\ndata: {value}\n\n"));
                let mut collector = UsageCollector::new(ApiFormat::OpenAiResponses, true);
                collector.observe(&bytes);
                assert_eq!(
                    collector.sse_terminal_outcome(),
                    Some(SseTerminalOutcome::Failed)
                );
                let usage = collector.latest().unwrap();
                assert_eq!((usage.input_tokens, usage.output_tokens), (5, 2));
            }
        }
    }

    #[test]
    fn upstream_usage_interface_is_independent_of_client_api_format() {
        let body = Bytes::from_static(br#"{"usage":{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":1,"output_tokens":4}}"#);
        for format in [
            ApiFormat::OpenAiChatCompletions,
            ApiFormat::OpenAiResponses,
            ApiFormat::OpenAiImages,
        ] {
            let mut usage = UsageCollector::with_parser(
                format,
                false,
                UsageParserConfig::General(UsageFormat::AnthropicMessages),
            );
            usage.observe(&body);
            assert_eq!(
                usage.latest(),
                Some(ResponseUsage {
                    input_tokens: 6,
                    cached_input_tokens: 3,
                    cache_write_tokens: 1,
                    output_tokens: 4,
                    reasoning_tokens: 0,
                })
            );
        }
    }

    #[test]
    fn anthropic_streaming_usage_is_a_request_local_cumulative_snapshot() {
        let parser = UsageParserConfig::General(UsageFormat::AnthropicMessages);
        let mut first =
            UsageCollector::with_parser(ApiFormat::OpenAiResponses, true, parser.clone());
        let mut second = UsageCollector::with_parser(ApiFormat::OpenAiResponses, true, parser);
        first.observe(&Bytes::from_static(b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":1,\"output_tokens\":0}}}\n\n"));
        second.observe(&Bytes::from_static(b"event: message_start\ndata: {\"message\":{\"usage\":{\"input_tokens\":7,\"output_tokens\":0}}}\n\n"));
        for output in [2, 4] {
            first.observe(&Bytes::from(format!(
                "event: message_delta\ndata: {{\"usage\":{{\"output_tokens\":{output}}}}}\n\n"
            )));
        }
        assert_eq!(
            first.latest(),
            Some(ResponseUsage {
                input_tokens: 6,
                cached_input_tokens: 3,
                cache_write_tokens: 1,
                output_tokens: 4,
                reasoning_tokens: 0,
            })
        );
        assert_eq!(second.latest().unwrap().input_tokens, 7);
        assert_eq!(second.latest().unwrap().output_tokens, 0);
        first.reset(false);
        first.observe(&Bytes::from_static(
            br#"{"usage":{"input_tokens":1,"output_tokens":2}}"#,
        ));
        assert_eq!(
            first.latest(),
            Some(ResponseUsage {
                input_tokens: 1,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 2,
                reasoning_tokens: 0,
            })
        );
    }

    #[test]
    fn invalid_terminal_usage_cannot_reuse_an_earlier_valid_summary() {
        let mut usage = UsageCollector::new(ApiFormat::OpenAiResponses, true);
        usage.observe(&Bytes::from_static(
            b"data: {\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
        ));
        assert!(usage.latest().is_some());
        usage.observe(&Bytes::from_static(b"event: response.completed\ndata: {\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":2,\"input_tokens_details\":{\"cached_tokens\":9}}}}\n\n"));
        assert_eq!(
            usage.sse_terminal_outcome(),
            Some(SseTerminalOutcome::Completed)
        );
        assert!(usage.latest().is_none());
        usage.observe(&Bytes::from_static(
            b"data: {\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n\n",
        ));
        assert!(usage.latest().is_none());
    }

    #[test]
    fn explicit_nonobject_or_oversized_usage_invalidates_earlier_counts() {
        for invalid in [
            "null".into(),
            r#""invalid""#.into(),
            r#"[{"usage":{"input_tokens":99,"output_tokens":99}}]"#.into(),
            format!(
                "{{\"input_tokens\":5,\"output_tokens\":2,\"padding\":\"{}\"}}",
                "x".repeat(super::MAX_USAGE_OBJECT_BYTES)
            ),
        ] {
            let mut usage = UsageCollector::new(ApiFormat::OpenAiResponses, true);
            usage.observe(&Bytes::from_static(
                b"data: {\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
            ));
            usage.observe(&Bytes::from(format!(
                "event: response.completed\ndata: {{\"response\":{{\"usage\":{invalid}}}}}\n\n"
            )));
            assert!(usage.latest().is_none(), "{invalid:.20}");
            let mut usage = UsageCollector::new(ApiFormat::OpenAiResponses, false);
            usage.observe(&Bytes::from_static(
                br#"{"usage":{"input_tokens":5,"output_tokens":2},"#,
            ));
            usage.observe(&Bytes::from(format!("\"usage\":{invalid}}}")));
            assert!(usage.latest().is_none(), "{invalid:.20}");
        }
    }

    #[test]
    fn malformed_or_truncated_usage_cannot_reuse_earlier_counts() {
        for invalid in [r#"{"input_tokens":}"#, r#"{"input_tokens":5"#] {
            let mut usage = UsageCollector::new(ApiFormat::OpenAiResponses, false);
            usage.observe(&Bytes::from_static(
                br#"{"usage":{"input_tokens":5,"output_tokens":2},"#,
            ));
            usage.observe(&Bytes::from(format!("\"usage\":{invalid}")));
            usage.finalize();
            assert!(usage.latest().is_none());
        }
        for data in [b"{invalid".as_slice(), b"[]", br#""invalid""#] {
            let mut usage = UsageCollector::new(ApiFormat::OpenAiResponses, true);
            usage.observe(&Bytes::from_static(
                b"data: {\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
            ));
            usage.observe(&Bytes::from(format!(
                "event: response.completed\ndata: {}\n\n",
                std::str::from_utf8(data).unwrap()
            )));
            assert!(usage.latest().is_none());
        }
    }

    #[test]
    fn invalid_late_chat_usage_clears_preferred_finish_chunk_counts() {
        let mut usage = UsageCollector::new(ApiFormat::OpenAiChatCompletions, true);
        usage.observe(&Bytes::from_static(b"data: {\"choices\":[{\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n"));
        assert!(usage.latest().is_some());
        usage.observe(&Bytes::from_static(b"data: {\"usage\":null}\n\n"));
        usage.observe(&Bytes::from_static(b"data: [DONE]\n\n"));
        assert!(usage.latest().is_none());
    }

    #[test]
    fn malformed_anthropic_components_remain_unknown_until_replaced() {
        let mut usage = UsageCollector::with_parser(
            ApiFormat::OpenAiResponses,
            true,
            UsageParserConfig::General(UsageFormat::AnthropicMessages),
        );
        for frame in [
            r#"{"usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":3}}"#,
            r#"{"usage":{"cache_read_input_tokens":-1}}"#,
            r#"{"usage":{"output_tokens":4}}"#,
        ] {
            usage.observe(&Bytes::from(format!("data: {frame}\n\n")));
        }
        assert!(usage.latest().is_none());
        usage.observe(&Bytes::from_static(
            b"data: {\"usage\":{\"cache_read_input_tokens\":3}}\n\n",
        ));
        assert_eq!(usage.latest().unwrap().input_tokens, 5);
        assert_eq!(usage.latest().unwrap().output_tokens, 4);
    }

    #[test]
    fn rejected_whole_anthropic_usage_cannot_resurrect_stale_partial_counts() {
        for invalid in [
            "null".into(),
            format!(
                "{{\"input_tokens\":-1,\"padding\":\"{}\"}}",
                "x".repeat(super::MAX_USAGE_OBJECT_BYTES)
            ),
        ] {
            let mut usage = UsageCollector::with_parser(
                ApiFormat::OpenAiResponses,
                true,
                UsageParserConfig::General(UsageFormat::AnthropicMessages),
            );
            usage.observe(&Bytes::from_static(b"data: {\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":1,\"output_tokens\":1}}\n\n"));
            usage.observe(&Bytes::from(format!("data: {{\"usage\":{invalid}}}\n\n")));
            usage.observe(&Bytes::from_static(
                b"data: {\"usage\":{\"output_tokens\":4}}\n\n",
            ));
            assert!(usage.latest().is_none());
            usage.observe(&Bytes::from_static(b"data: {\"usage\":{\"input_tokens\":2,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":1}}\n\n"));
            assert_eq!(usage.latest().unwrap().input_tokens, 6);
            assert_eq!(usage.latest().unwrap().output_tokens, 4);
        }
    }

    #[cfg(target_os = "linux")]
    fn usage_plugin(
        result: &str,
        output_body: bool,
    ) -> (
        tempfile::TempDir,
        std::sync::Arc<crate::connector_plugins::Plugin>,
    ) {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::PermissionsExt;
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let directory = tempfile::tempdir_in(&root).unwrap();
        let path = directory.path().join("usage.so");
        let descriptor = r#","usage":{"parser":"plugin","interface":"fixture.anthropic/v1"}"#;
        let mut command = std::process::Command::new("cc");
        command.args([
            "-shared",
            "-fPIC",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-DFIXTURE_PROTOCOL3",
            "-DFIXTURE_DESCRIPTOR_MODE=10",
            "-DFIXTURE_USAGE_PARSE",
        ]);
        command.arg(format!("-DFIXTURE_USAGE_DESCRIPTOR={descriptor:?}"));
        command.arg(format!("-DFIXTURE_USAGE_RESULT={result:?}"));
        if output_body {
            command.arg("-DFIXTURE_USAGE_OUTPUT_BODY");
        }
        let output = command
            .arg(root.join("crates/connector-sdk/tests/fixture.c"))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let digest = Sha256::digest(std::fs::read(&path).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let plugin = crate::connector_plugins::Plugin::load(&path, &digest, "fixture").unwrap();
        (directory, plugin)
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn native_usage_parser_receives_only_bounded_raw_usage_in_every_protocol() {
        let (_directory, plugin) = usage_plugin(
            r#"{"usage":{"input_tokens":5,"cached_input_tokens":3,"cache_write_tokens":0,"output_tokens":2,"reasoning_tokens":0}}"#,
            false,
        );
        let parser = UsageParserConfig::for_plugin(&plugin, crate::domain::ApiOperation::Responses);
        let raw = r#"{"id":"must-not-reach-parser","output":[{"text":"must-not-reach-parser"}],"usage":{"input_tokens":2,"cache_read_input_tokens":3,"output_tokens":2}}"#;
        for sse in [false, true] {
            let mut usage =
                UsageCollector::with_parser(ApiFormat::OpenAiResponses, sse, parser.clone());
            let bytes = if sse {
                format!("event: response.completed\ndata: {{\"response\":{raw}}}\n\n")
            } else {
                raw.into()
            };
            usage.observe(&Bytes::from(bytes));
            assert_eq!(usage.latest().unwrap().input_tokens, 5);
            assert_eq!(usage.latest().unwrap().cached_input_tokens, 3);
        }
        let mut usage =
            UsageCollector::with_parser(ApiFormat::OpenAiResponses, false, parser.clone());
        usage.observe_websocket_event(&Bytes::from(format!(
            "{{\"type\":\"response.completed\",\"response\":{raw}}}"
        )));
        assert_eq!(usage.latest().unwrap().input_tokens, 5);
        usage.reset(true);
        usage.observe(&Bytes::from(format!(
            "data: {{\"usage\":{{\"input_tokens\":2,\"output_tokens\":2,\"padding\":\"{}\"}}}}\n\n",
            "x".repeat(super::MAX_USAGE_OBJECT_BYTES)
        )));
        assert!(usage.latest().is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn native_usage_parser_errors_never_fall_back_to_openai_usage() {
        for (result, output_body) in [
            (r#"{"usage":null}"#, false),
            (r#"{}"#, false),
            (
                r#"{"usage":{"input_tokens":1,"cached_input_tokens":9,"cache_write_tokens":0,"output_tokens":2,"reasoning_tokens":0}}"#,
                false,
            ),
            (
                r#"{"usage":{"input_tokens":5,"cached_input_tokens":3,"cache_write_tokens":0,"output_tokens":2,"reasoning_tokens":0},"cost_amount":0}"#,
                false,
            ),
            (
                r#"{"usage":{"input_tokens":5,"cached_input_tokens":3,"cache_write_tokens":0,"output_tokens":2,"reasoning_tokens":0}}"#,
                true,
            ),
        ] {
            let (_directory, plugin) = usage_plugin(result, output_body);
            let mut usage = UsageCollector::with_parser(
                ApiFormat::OpenAiResponses,
                false,
                UsageParserConfig::for_plugin(&plugin, crate::domain::ApiOperation::Responses),
            );
            usage.observe(&Bytes::from_static(br#"{"usage":{"input_tokens":5,"output_tokens":2,"input_tokens_details":{"cached_tokens":3}}}"#));
            assert!(usage.latest().is_none(), "{result}");
        }
    }
}
