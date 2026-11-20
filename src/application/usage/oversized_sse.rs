//! Streaming projection of terminal envelopes when a Responses event exceeds the frame buffer.

use super::{
    ApiFormat, MAX_ERROR_SUMMARY_BYTES, MAX_USAGE_BYTES, SseFrameObservation, observe_sse_payload,
};
use json_skimmer::{ContainerKind, Event, Mode, Parser, StringKind};
use serde_json::{Map, Value};

mod scalar;
use scalar::{JsonNumber, StringEscapes};

pub(super) struct ResponsesProjection {
    line: Line,
    skip_space: bool,
    has_data: bool,
    done_prefix: Option<usize>,
    event: Vec<u8>,
    parser: Parser,
    projection: Envelope,
}

enum Line {
    Name(Vec<u8>),
    Data,
    Event,
    Ignored,
}

impl Default for ResponsesProjection {
    fn default() -> Self {
        Self {
            line: Line::Name(Vec::new()),
            skip_space: false,
            has_data: false,
            done_prefix: Some(0),
            event: Vec::new(),
            // serde_json reserves its last recursion slot before opening a container.
            parser: Parser::with_max_depth(Mode::Single, 127.try_into().unwrap()),
            projection: Envelope::default(),
        }
    }
}

impl ResponsesProjection {
    pub(super) fn push_line_part(&mut self, part: &[u8]) {
        let ended = part.ends_with(b"\r") || part.ends_with(b"\n");
        let mut bytes = &part[..part.len() - usize::from(ended)];
        if let Line::Name(name) = &mut self.line {
            let colon = bytes.iter().position(|&b| b == b':');
            let length = colon.unwrap_or(bytes.len());
            if name.len().saturating_add(length) > 5 {
                self.line = Line::Ignored;
            } else {
                name.extend_from_slice(&bytes[..length]);
                if colon.is_none() && !ended {
                    return;
                }
                self.line = match name.as_slice() {
                    b"data" => Line::Data,
                    b"event" => Line::Event,
                    _ => Line::Ignored,
                };
                bytes = colon.map_or(&[][..], |i| &bytes[i + 1..]);
                self.skip_space = true;
                match self.line {
                    Line::Data => {
                        if self.has_data {
                            self.push_json(b"\n");
                        }
                        self.has_data = true;
                    }
                    Line::Event => self.event.clear(),
                    _ => {}
                }
            }
        }
        if self.skip_space && !bytes.is_empty() {
            bytes = bytes.strip_prefix(b" ").unwrap_or(bytes);
            self.skip_space = false;
        }
        match self.line {
            Line::Data => self.push_json(bytes),
            Line::Event if self.event.len().saturating_add(bytes.len()) <= 128 => {
                self.event.extend_from_slice(bytes)
            }
            Line::Event => {
                self.event.clear();
                self.line = Line::Ignored;
            }
            _ => {}
        }
        if ended {
            self.line = Line::Name(Vec::new());
        }
    }

    fn push_json(&mut self, bytes: &[u8]) {
        self.done_prefix = self
            .done_prefix
            .and_then(|n| b"[DONE]"[n..].starts_with(bytes).then(|| n + bytes.len()));
        if self.projection.invalid {
            return;
        }
        let base = self.parser.offset();
        self.parser
            .push(bytes, |event| self.projection.observe(event, bytes, base));
        if let Some(raw) = &mut self.projection.raw_usage {
            raw.append(bytes, base, base + bytes.len() as u64);
        }
    }

    pub(super) fn finish(mut self, codex: bool) -> Option<SseFrameObservation> {
        let base = self.parser.offset();
        self.parser
            .finish(|event| self.projection.observe(event, &[], base));
        let value = (!self.projection.invalid)
            .then_some(self.projection.root)
            .flatten();
        let data: &[u8] = if self.done_prefix == Some(b"[DONE]".len()) {
            b"[DONE]"
        } else {
            &[]
        };
        let mut observation = observe_sse_payload(
            value,
            Some(&self.event),
            data,
            ApiFormat::OpenAiResponses,
            codex,
        );
        if let Some(value) = &mut observation.value {
            for (oversized, path) in self.projection.oversized_usage.into_iter().zip([
                "/usage",
                "/response/usage",
                "/message/usage",
            ]) {
                if oversized && let Some(usage) = value.pointer_mut(path) {
                    *usage = Value::Null;
                }
            }
        }
        Some(observation)
    }
}

#[derive(Default)]
struct Envelope {
    stack: Vec<Container>,
    root: Option<Value>,
    key: &'static str,
    token: Option<Vec<u8>>,
    escapes: StringEscapes,
    number: JsonNumber,
    oversized_usage: [bool; 3],
    raw_usage: Option<RawUsage>,
    invalid: bool,
}

struct Container {
    name: &'static str,
    value: Value,
}

struct RawUsage {
    start: u64,
    depth: usize,
    slot: usize,
    bytes: Option<Vec<u8>>,
}

impl RawUsage {
    fn append(&mut self, input: &[u8], base: u64, end: u64) {
        let Some(bytes) = &mut self.bytes else { return };
        let slice = &input[self.start.saturating_sub(base) as usize..(end - base) as usize];
        if bytes.len().saturating_add(slice.len()) <= MAX_USAGE_BYTES {
            bytes.extend_from_slice(slice);
        } else {
            self.bytes = None;
        }
    }
}

impl Envelope {
    fn observe(&mut self, event: Event<'_>, input: &[u8], base: u64) {
        if self.invalid {
            return;
        }
        match event {
            Event::Invalid(_) => self.invalid = true,
            Event::CandidateStart {
                kind: ContainerKind::Array,
                ..
            } => self.invalid = true,
            Event::ObjectStart { offset } | Event::ArrayStart { offset } => {
                let object = matches!(event, Event::ObjectStart { .. });
                let key = if self.stack.is_empty() {
                    "$"
                } else {
                    std::mem::take(&mut self.key)
                };
                if key == "usage" && object {
                    let slot = match self.stack.last().map(|parent| parent.name) {
                        Some("response") => 1,
                        Some("message") => 2,
                        _ => 0,
                    };
                    self.raw_usage = Some(RawUsage {
                        start: offset,
                        depth: self.stack.len() + 1,
                        slot,
                        bytes: Some(Vec::new()),
                    });
                }
                self.stack.push(Container {
                    name: key,
                    value: if key.is_empty() {
                        Value::Null
                    } else if object {
                        Value::Object(Map::new())
                    } else {
                        Value::Array(vec![])
                    },
                });
            }
            Event::ObjectEnd { offset } | Event::ArrayEnd { offset } => {
                let depth = self.stack.len();
                let Some(mut closed) = self.stack.pop() else {
                    return;
                };
                if self
                    .raw_usage
                    .as_ref()
                    .is_some_and(|raw| raw.depth == depth)
                {
                    let mut raw = self.raw_usage.take().unwrap();
                    raw.append(input, base, offset + 1);
                    self.oversized_usage[raw.slot] = raw.bytes.is_none();
                    if let Some(bytes) = raw.bytes {
                        match serde_json::from_slice(&bytes) {
                            Ok(value) => closed.value = value,
                            Err(_) => self.invalid = true,
                        }
                    }
                }
                if self.stack.is_empty() {
                    self.root = Some(closed.value);
                } else {
                    self.insert(closed.name, closed.value);
                }
            }
            Event::StringStart { kind, .. } => {
                let keep = if kind == StringKind::Key {
                    self.stack
                        .last()
                        .is_some_and(|parent| parent.value.is_object())
                } else {
                    !self.key.is_empty()
                };
                self.token = keep.then(|| vec![b'"']);
            }
            Event::NumberFragment { bytes, .. } => self.number.push(bytes),
            Event::NumberEnd { .. } => {
                let key = std::mem::take(&mut self.key);
                match self.number.finish() {
                    Some(number) => self.insert(key, Value::Number(number)),
                    None => self.invalid = true,
                }
            }
            Event::StringFragment { bytes, .. } => {
                if !self.escapes.push(bytes) {
                    self.invalid = true;
                    return;
                }
                if let Some(token) = &mut self.token {
                    let limit = if matches!(
                        event,
                        Event::StringFragment {
                            kind: StringKind::Key,
                            ..
                        }
                    ) {
                        256
                    } else {
                        MAX_ERROR_SUMMARY_BYTES
                    };
                    if token.len().saturating_add(bytes.len()) <= limit {
                        token.extend_from_slice(bytes);
                    } else {
                        self.token = None;
                    }
                }
            }
            Event::StringEnd { .. } if !self.escapes.complete() => self.invalid = true,
            Event::StringEnd {
                kind: StringKind::Key,
                ..
            } => {
                self.key = "";
                if let Some(Value::String(key)) = self.end_string()
                    && let Some(parent) = self.stack.last()
                {
                    self.key = selected_key(parent.name, &key);
                }
            }
            Event::StringEnd {
                kind: StringKind::Value,
                ..
            } => {
                let key = std::mem::take(&mut self.key);
                if !key.is_empty() {
                    // Oversized strings still have string type, but cannot match an enum/code.
                    let value = self
                        .end_string()
                        .unwrap_or_else(|| Value::String(String::new()));
                    self.insert(key, value);
                }
            }
            Event::Boolean { value, .. } => {
                let key = std::mem::take(&mut self.key);
                self.insert(key, Value::Bool(value));
            }
            Event::Null { .. } => {
                let key = std::mem::take(&mut self.key);
                self.insert(key, Value::Null);
            }
            _ => {}
        }
    }

    fn end_string(&mut self) -> Option<Value> {
        let mut token = self.token.take()?;
        token.push(b'"');
        serde_json::from_slice(&token)
            .map_err(|_| self.invalid = true)
            .ok()
    }

    fn insert(&mut self, key: &'static str, value: Value) {
        if !key.is_empty()
            && let Some(map) = self
                .stack
                .last_mut()
                .and_then(|parent| parent.value.as_object_mut())
        {
            map.insert(key.to_owned(), value);
        }
    }
}

fn selected_key(parent: &str, key: &str) -> &'static str {
    let keys: &[&str] = match parent {
        "$" => &["type", "response", "error", "code", "message", "usage"],
        "response" => &[
            "id",
            "usage",
            "end_turn",
            "usage_metadata",
            "error",
            "incomplete_details",
        ],
        "message" => &["usage"],
        "error" => &["code", "type", "message"],
        "incomplete_details" => &["reason"],
        "usage_metadata" => &["amount"],
        "usage" => &[
            "input_tokens",
            "output_tokens",
            "total_tokens",
            "input_tokens_details",
            "output_tokens_details",
            "codex_rollout_budget_units",
        ],
        "input_tokens_details" => &["cached_tokens", "cache_write_tokens"],
        "output_tokens_details" => &["reasoning_tokens"],
        _ => &[],
    };
    keys.iter()
        .copied()
        .find(|candidate| *candidate == key)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::usage::SseTerminalOutcome;

    fn project(frame: &str, codex: bool) -> SseFrameObservation {
        let mut projection = ResponsesProjection::default();
        for part in frame
            .as_bytes()
            .chunks(3)
            .flat_map(|chunk| chunk.split_inclusive(|b| *b == b'\n'))
        {
            projection.push_line_part(part);
        }
        projection.finish(codex).unwrap_or_default()
    }

    #[test]
    fn nested_decoys_and_malformed_json_cannot_project_a_completion() {
        for data in [
            r#"{"type":"response.created","output":[{"type":"response.completed","response":{"id":"fake","usage":{"input_tokens":999}}}]}"#,
            r#"{"noise":"\"},\"type\":\"response.completed\"","nested":{"response":{"id":"r"},"type":"response.completed"}}"#,
            r#"{"type":"response.completed","response":{"id":"r"}"#,
            r#"{"type":"response.completed","response":{"id":"r"},"ignored":[1,]}"#,
            r#"{"type":"response.completed","response":{"id":"r"}} extra"#,
        ] {
            for codex in [true, false] {
                let observed = project(&format!("data: {data}\n"), codex);
                assert_eq!(observed.terminal_outcome, None, "{data}");
            }
        }
    }

    #[test]
    fn projection_keeps_decoded_keys_provider_usage_and_generic_event_name() {
        let frame = "event: response.completed\ndata: {\"respo\\u006ese\":{\"output\":[{\"usage\":99}],\ndata: \"usage\":{\"vendor_counter\":7}}}\n";
        let observed = project(frame, false);
        assert_eq!(
            observed.terminal_outcome,
            Some(SseTerminalOutcome::Completed)
        );
        assert_eq!(
            observed
                .value
                .unwrap()
                .pointer("/response/usage/vendor_counter")
                .unwrap(),
            7
        );
        assert_eq!(project(frame, true).terminal_outcome, None);
    }

    #[test]
    fn oversized_usage_preserves_codex_validation_but_is_not_metered() {
        for (total, expected) in [
            ("8", SseTerminalOutcome::Completed),
            ("null", SseTerminalOutcome::Failed),
        ] {
            let observed = project(
                &format!(
                    r#"data: {{"response":{{"id":"r","end_turn":true,"usage":{{"other":"{}","input_tokens":6,"output_tokens":2,"total_tokens":{total}}}}},"type":"response.completed"}}"#,
                    "x".repeat(MAX_USAGE_BYTES)
                ),
                true,
            );
            assert_eq!(observed.terminal_outcome, Some(expected));
            assert!(
                observed
                    .value
                    .as_ref()
                    .and_then(|v| v.pointer("/response/usage"))
                    .is_none_or(Value::is_null)
            );
        }
    }

    #[test]
    fn projected_ignored_scalars_keys_and_depth_match_small_json_validation() {
        let mut ignored = Vec::new();
        for text in [
            r"\uD800",
            r"\uDC00",
            r"\uD800\uD801",
            r"\uD800\\uDC00",
            r"\uD800x",
            r"\uD800\u1234",
            r"\ud800\uDfff",
            r"\\uD800",
            r"\\\uD800\uDC00",
        ] {
            ignored.push(format!(r#"{{"unread":"{text}"}}"#));
            ignored.push(format!(r#"{{"{text}":1}}"#));
        }
        ignored
            .extend(["1e400", "0e99999999999999999", "-1e-99999999999999999"].map(str::to_owned));
        for depth in [124, 125, 126, 127, 128] {
            ignored.push(format!("{}null{}", "[".repeat(depth), "]".repeat(depth)));
        }
        for value in ignored {
            let json = format!(
                r#"{{"type":"response.completed","response":{{"id":"r","unread":{value}}}}}"#
            );
            let expected = serde_json::from_str::<Value>(&json)
                .is_ok()
                .then_some(SseTerminalOutcome::Completed);
            for codex in [false, true] {
                assert_eq!(
                    project(&format!("data: {json}\n"), codex).terminal_outcome,
                    expected,
                    "{json}"
                );
            }
        }
    }

    #[test]
    fn generic_wire_terminators_remain_independent_of_parsed_json() {
        let comment = ":unused\n";
        for data in [
            "data: [DONE]\n",
            "event: response.completed\ndata: {invalid\n",
            "event: response.completed\n",
        ] {
            let frame = format!("{comment}{data}");
            let generic = project(&frame, false);
            assert_eq!(
                generic.terminal_outcome,
                Some(SseTerminalOutcome::Completed)
            );
            assert!(
                generic
                    .value
                    .as_ref()
                    .and_then(super::super::usage_object)
                    .is_none_or(Value::is_null)
            );
            assert_eq!(project(&frame, true).terminal_outcome, None);
        }
        for not_done in ["[DONE]x", " [DONE]", "[DONE]\ndata:"] {
            assert_eq!(
                project(&format!("{comment}data: {not_done}\n"), false).terminal_outcome,
                None
            );
        }
    }
}
