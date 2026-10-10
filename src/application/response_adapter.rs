//! Bounded, same-format response normalization with host-owned metering and terminal state.

use std::{collections::VecDeque, sync::Arc};

use ai_gateway_connector_sdk::{
    MAX_RESPONSE_EVENT_BYTES, MAX_RESPONSE_JSON_BYTES, RESPONSE_EVENT, RESPONSE_JSON,
    ResponseEvent, ResponseEventOutput, ResponseMode,
};
use bytes::Bytes;
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{
    connector_plugins::Plugin,
    domain::{ApiFormat, ApiOperation},
};

use super::usage::UsageCollector;

#[derive(Clone)]
pub(crate) struct ResponseAdapterConfig {
    pub plugin: Arc<Plugin>,
    pub operation: ApiOperation,
    pub mode: ResponseMode,
}

#[derive(Debug, Error)]
#[error("connector response adaptation failed")]
pub(crate) struct ResponseAdaptationError;

impl ResponseAdapterConfig {
    pub fn adapt_json(&self, status: u16, body: &[u8]) -> Result<Bytes, ResponseAdaptationError> {
        if body.len() > MAX_RESPONSE_JSON_BYTES {
            return Err(ResponseAdaptationError);
        }
        let before = object(body)?;
        let output = self
            .plugin
            .call(
                RESPONSE_JSON,
                &json!({
                    "operation": self.operation,
                    "protocol": "non_stream",
                    "status": status,
                }),
                body,
            )
            .map_err(|_| ResponseAdaptationError)?;
        if !output.metadata.as_object().is_some_and(Map::is_empty)
            || output.body.len() > MAX_RESPONSE_JSON_BYTES
        {
            return Err(ResponseAdaptationError);
        }
        let after = object(&output.body)?;
        if protected_fields(&before) != protected_fields(&after) {
            return Err(ResponseAdaptationError);
        }
        Ok(Bytes::from(output.body))
    }
}

pub(crate) struct SseResponseAdapter {
    config: ResponseAdapterConfig,
    status: u16,
    state: Value,
    sequence: u64,
    original: UsageCollector,
    adapted: UsageCollector,
    finished: bool,
}

impl SseResponseAdapter {
    pub fn new(config: ResponseAdapterConfig, status: u16, format: ApiFormat) -> Self {
        Self {
            config,
            status,
            state: json!({}),
            sequence: 0,
            original: UsageCollector::new(format, true),
            adapted: UsageCollector::new(format, true),
            finished: false,
        }
    }

    pub fn adapt_frame(
        &mut self,
        frame: &Bytes,
    ) -> Result<VecDeque<Bytes>, ResponseAdaptationError> {
        if self.finished {
            // Bytes after a real terminal event are never interpreted as a new response.
            return Err(ResponseAdaptationError);
        }
        let Some(input) = decode_event(frame)? else {
            return Ok(VecDeque::from([frame.clone()]));
        };
        self.original.observe(frame);
        self.original.finalize();
        let output = self.call("event", Some(&input))?;
        validate_events(&input, &output.events)?;
        let mut frames = VecDeque::new();
        for event in &output.events {
            let encoded = encode_event(event)?;
            self.adapted.observe(&encoded);
            frames.push_back(encoded);
        }
        self.adapted.finalize();
        if self.original.latest() != self.adapted.latest()
            || self.original.sse_terminal_outcome() != self.adapted.sse_terminal_outcome()
            || self.original.error_details() != self.adapted.error_details()
        {
            return Err(ResponseAdaptationError);
        }
        if self.original.sse_terminal_outcome().is_some() {
            let tail = self.call("finish", None)?;
            if !tail.events.is_empty() {
                return Err(ResponseAdaptationError);
            }
            self.finished = true;
        }
        Ok(frames)
    }

    pub fn finish(&mut self) -> Result<(), ResponseAdaptationError> {
        if !self.finished {
            // EOF cannot stand in for a successful protocol terminator.
            let _ = self.call("finish", None)?;
            return Err(ResponseAdaptationError);
        }
        Ok(())
    }

    fn call(
        &mut self,
        phase: &str,
        event: Option<&ResponseEvent>,
    ) -> Result<ResponseEventOutput, ResponseAdaptationError> {
        let body = event.map_or(&[][..], |event| event.data.as_bytes());
        if body.len() > MAX_RESPONSE_EVENT_BYTES {
            return Err(ResponseAdaptationError);
        }
        let output = self
            .config
            .plugin
            .call(
                RESPONSE_EVENT,
                &json!({
                    "operation": self.config.operation,
                    "protocol": "sse",
                    "status": self.status,
                    "event": event.and_then(|event| event.event.as_deref()),
                    "id": event.and_then(|event| event.id.as_deref()),
                    "sequence": self.sequence,
                    "state": self.state,
                    "phase": phase,
                }),
                body,
            )
            .map_err(|_| ResponseAdaptationError)?;
        if !output.body.is_empty() {
            return Err(ResponseAdaptationError);
        }
        let output: ResponseEventOutput =
            serde_json::from_value(output.metadata).map_err(|_| ResponseAdaptationError)?;
        if !output.validate_bounds() {
            return Err(ResponseAdaptationError);
        }
        self.state = output.state.clone();
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ResponseAdaptationError)?;
        Ok(output)
    }
}

fn object(body: &[u8]) -> Result<Value, ResponseAdaptationError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| ResponseAdaptationError)?;
    value
        .is_object()
        .then_some(value)
        .ok_or(ResponseAdaptationError)
}

fn protected_fields(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut protected = Map::new();
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "usage"
                        | "id"
                        | "object"
                        | "model"
                        | "created"
                        | "created_at"
                        | "type"
                        | "status"
                        | "error"
                        | "incomplete_details"
                        | "finish_reason"
                        | "sequence_number"
                        | "index"
                        | "output_index"
                        | "content_index"
                        | "call_id"
                        | "item_id"
                        | "response_id"
                        | "name"
                        | "namespace"
                        | "role"
                ) {
                    protected.insert(key.clone(), value.clone());
                } else {
                    let child = protected_fields(value);
                    if !child.is_null() {
                        protected.insert(key.clone(), child);
                    }
                }
            }
            if protected.is_empty() {
                Value::Null
            } else {
                Value::Object(protected)
            }
        }
        Value::Array(values) => {
            let children: Vec<_> = values.iter().map(protected_fields).collect();
            if children.iter().any(|child| !child.is_null()) {
                Value::Array(children)
            } else {
                Value::Null
            }
        }
        _ => Value::Null,
    }
}

fn validate_events(
    input: &ResponseEvent,
    output: &[ResponseEvent],
) -> Result<(), ResponseAdaptationError> {
    if input.data.trim() == "[DONE]" {
        return (output.len() == 1 && output[0] == *input)
            .then_some(())
            .ok_or(ResponseAdaptationError);
    }
    let before = object(input.data.as_bytes())?;
    let protected = protected_fields(&before);
    let terminal = matches!(
        before.get("type").and_then(Value::as_str),
        Some(
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        )
    ) || matches!(
        input.event.as_deref(),
        Some(
            "response.completed"
                | "response.failed"
                | "response.incomplete"
                | "response.cancelled"
                | "error"
        )
    ) || before.get("error").is_some_and(|value| !value.is_null())
        || contains_finish_reason(&before);
    if output.is_empty() || ((requires_single_event(&before) || terminal) && output.len() != 1) {
        return Err(ResponseAdaptationError);
    }
    for event in output {
        if event.event != input.event
            || event.id != input.id
            || protected_fields(&object(event.data.as_bytes())?) != protected
        {
            return Err(ResponseAdaptationError);
        }
    }
    Ok(())
}

fn requires_single_event(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields.keys().any(|key| {
                matches!(
                    key.as_str(),
                    "usage" | "sequence_number" | "tool_calls" | "function_call"
                )
            }) || fields.values().any(requires_single_event)
        }
        Value::Array(values) => values.iter().any(requires_single_event),
        _ => false,
    }
}

fn contains_finish_reason(value: &Value) -> bool {
    match value {
        Value::Object(fields) => {
            fields
                .get("finish_reason")
                .is_some_and(|value| !value.is_null())
                || fields.values().any(contains_finish_reason)
        }
        Value::Array(values) => values.iter().any(contains_finish_reason),
        _ => false,
    }
}

fn decode_event(frame: &[u8]) -> Result<Option<ResponseEvent>, ResponseAdaptationError> {
    if frame.len() > crate::transforms::SseTransformer::MAX_FRAME_BYTES {
        return Err(ResponseAdaptationError);
    }
    let text = std::str::from_utf8(frame).map_err(|_| ResponseAdaptationError)?;
    let mut event = None;
    let mut id = None;
    let mut data = Vec::new();
    for line in text.split(['\r', '\n']).filter(|line| !line.is_empty()) {
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = Some(value.to_owned()),
            "id" if !value.contains('\0') => id = Some(value.to_owned()),
            "data" => data.push(value),
            _ => {}
        }
    }
    Ok((!data.is_empty()).then(|| ResponseEvent {
        event,
        id,
        data: data.join("\n"),
    }))
}

fn encode_event(event: &ResponseEvent) -> Result<Bytes, ResponseAdaptationError> {
    let mut encoded = String::new();
    for (field, value) in [("event", &event.event), ("id", &event.id)] {
        if let Some(value) = value {
            if value.contains(['\r', '\n', '\0']) {
                return Err(ResponseAdaptationError);
            }
            encoded.push_str(field);
            encoded.push_str(": ");
            encoded.push_str(value);
            encoded.push('\n');
        }
    }
    for line in event.data.split('\n') {
        if line.contains('\r') {
            return Err(ResponseAdaptationError);
        }
        encoded.push_str("data: ");
        encoded.push_str(line);
        encoded.push('\n');
    }
    encoded.push('\n');
    if encoded.len() > crate::transforms::SseTransformer::MAX_FRAME_BYTES {
        return Err(ResponseAdaptationError);
    }
    Ok(Bytes::from(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protects_usage_terminal_and_tool_identity_but_not_text() {
        let before = json!({
            "id":"r","choices":[{"index":0,"finish_reason":null,
                "delta":{"content":"before","tool_calls":[{"index":0,"id":"c","function":{"name":"run","arguments":"{}"}}]}}],
            "usage":{"prompt_tokens":5,"completion_tokens":2}
        });
        let mut after = before.clone();
        after["choices"][0]["delta"]["content"] = json!("after");
        assert_eq!(protected_fields(&before), protected_fields(&after));
        for path in [
            "/usage/prompt_tokens",
            "/choices/0/finish_reason",
            "/choices/0/delta/tool_calls/0/id",
            "/choices/0/delta/tool_calls/0/function/name",
        ] {
            let mut changed = before.clone();
            *changed.pointer_mut(path).unwrap() = json!("changed");
            assert_ne!(protected_fields(&before), protected_fields(&changed));
        }
    }

    #[test]
    fn parses_multiline_events_and_rejects_header_injection() {
        let frame = Bytes::from_static(
            b": keepalive\r\nid: 3\r\nevent: delta\r\ndata: {\"text\":\r\ndata: \"ok\"}\r\n\r\n",
        );
        let event = decode_event(&frame).unwrap().unwrap();
        assert_eq!(event.data, "{\"text\":\n\"ok\"}");
        assert_eq!(
            decode_event(&encode_event(&event).unwrap()).unwrap(),
            Some(event.clone())
        );
        assert!(
            encode_event(&ResponseEvent {
                event: Some("delta\nretry: 1".into()),
                ..event
            })
            .is_err()
        );
        assert!(
            decode_event(&Bytes::from_static(b": keepalive\n\n"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn payload_limit_does_not_count_sse_envelope_bytes() {
        let event = ResponseEvent {
            event: Some("delta".into()),
            id: None,
            data: "a".repeat(MAX_RESPONSE_EVENT_BYTES),
        };
        let encoded = encode_event(&event).unwrap();
        assert!(encoded.len() > MAX_RESPONSE_EVENT_BYTES);
        assert_eq!(decode_event(&encoded).unwrap(), Some(event));
    }

    #[test]
    fn forbids_dropping_usage_or_forging_terminal_events() {
        let event = ResponseEvent {
            event: None,
            id: None,
            data: r#"{"id":"r","usage":{"prompt_tokens":5,"completion_tokens":2}}"#.into(),
        };
        assert!(validate_events(&event, &[]).is_err());
        assert!(validate_events(&event, &[event.clone(), event.clone()]).is_err());
        let mut forged = event.clone();
        forged.data = "[DONE]".into();
        assert!(validate_events(&event, &[forged.clone()]).is_err());
        assert!(validate_events(&forged, &[event]).is_err());
    }

    #[test]
    fn terminal_event_names_and_finish_reasons_cannot_fan_out() {
        for event in [
            ResponseEvent {
                event: Some("response.completed".into()),
                id: None,
                data: r#"{"response":{"id":"r","status":"completed"}}"#.into(),
            },
            ResponseEvent {
                event: None,
                id: None,
                data: r#"{"choices":[{"index":0,"finish_reason":"stop","delta":{}}]}"#.into(),
            },
            ResponseEvent {
                event: None, id: None,
                data: r#"{"type":"response.output_text.delta","sequence_number":1,"delta":"text"}"#.into(),
            },
            ResponseEvent {
                event: None, id: None,
                data: r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"run","arguments":"{}"}}]}}]}"#.into(),
            },
        ] {
            assert!(validate_events(&event, std::slice::from_ref(&event)).is_ok());
            assert!(validate_events(&event, &[event.clone(), event.clone()]).is_err());
        }
    }
}
