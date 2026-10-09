//! Explicit protocol capabilities and bounded, request-local response adaptation.

use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ATTEMPT_DESCRIBE: &str = "attempt.describe/v1";
pub const RESPONSE_JSON: &str = "response.json/v1";
pub const RESPONSE_EVENT: &str = "response.event/v1";
pub const MAX_RESPONSE_JSON_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESPONSE_EVENT_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_STATE_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_OUTPUT_EVENTS: usize = 16;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorProtocol {
    NonStream,
    Sse,
    Websocket,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseMode {
    Passthrough,
    Json,
    Sse,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProtocolCapability {
    pub protocol: ConnectorProtocol,
    pub response: ResponseMode,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptCapabilities {
    pub preserves_affinity_on_failure: bool,
    pub successful_response_is_sse: bool,
    pub changes_request_body: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptDescriptor {
    pub capabilities: AttemptCapabilities,
    pub protocols: Vec<ProtocolCapability>,
}

impl AttemptDescriptor {
    pub fn validate_for_operation(&self, operation: &str) -> bool {
        let allowed: &[ConnectorProtocol] = match operation {
            "chat_completion" | "responses" => {
                &[ConnectorProtocol::NonStream, ConnectorProtocol::Sse]
            }
            "responses-ws" => &[ConnectorProtocol::Websocket],
            "web_search" | "images_generation" | "images_edit" => &[ConnectorProtocol::NonStream],
            _ => return false,
        };
        !self.protocols.is_empty()
            && self.protocols.len() <= allowed.len()
            && self.protocols.iter().enumerate().all(|(index, entry)| {
                allowed.contains(&entry.protocol)
                    && !self.protocols[..index]
                        .iter()
                        .any(|previous| previous.protocol == entry.protocol)
                    && match entry.response {
                        ResponseMode::Passthrough => true,
                        ResponseMode::Json => {
                            matches!(operation, "chat_completion" | "responses")
                                && entry.protocol == ConnectorProtocol::NonStream
                                && !self.capabilities.successful_response_is_sse
                        }
                        ResponseMode::Sse => {
                            matches!(operation, "chat_completion" | "responses")
                                && entry.protocol == ConnectorProtocol::Sse
                        }
                    }
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseJsonInput {
    pub operation: String,
    pub protocol: ConnectorProtocol,
    pub status: u16,
}

impl ResponseJsonInput {
    pub fn validate_bounds(&self, body: &[u8]) -> bool {
        self.protocol == ConnectorProtocol::NonStream
            && matches!(self.operation.as_str(), "chat_completion" | "responses")
            && body.len() <= MAX_RESPONSE_JSON_BYTES
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponsePhase {
    Event,
    Finish,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResponseEventInput {
    pub operation: String,
    pub protocol: ConnectorProtocol,
    pub status: u16,
    pub event: Option<String>,
    pub id: Option<String>,
    pub sequence: u64,
    pub state: Value,
    pub phase: ResponsePhase,
}

impl ResponseEventInput {
    pub fn validate_bounds(&self, body: &[u8]) -> bool {
        self.protocol == ConnectorProtocol::Sse
            && matches!(self.operation.as_str(), "chat_completion" | "responses")
            && body.len() <= MAX_RESPONSE_EVENT_BYTES
            && (self.phase != ResponsePhase::Finish || body.is_empty())
            && valid_field(&self.event)
            && valid_field(&self.id)
            && valid_state(&self.state)
            && serialized_within(self, crate::MAX_METADATA_BYTES)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResponseEvent {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResponseEventOutput {
    pub state: Value,
    pub events: Vec<ResponseEvent>,
}

impl ResponseEventOutput {
    pub fn validate_bounds(&self) -> bool {
        valid_state(&self.state)
            && self.events.len() <= MAX_RESPONSE_OUTPUT_EVENTS
            && self.events.iter().all(|event| {
                event.data.len() <= MAX_RESPONSE_EVENT_BYTES
                    && valid_field(&event.event)
                    && valid_field(&event.id)
            })
            && serialized_within(self, crate::MAX_METADATA_BYTES)
    }
}

fn valid_field(field: &Option<String>) -> bool {
    field.as_ref().is_none_or(|field| {
        field.len() <= MAX_RESPONSE_EVENT_BYTES && !field.contains(['\r', '\n', '\0'])
    })
}

fn valid_state(state: &Value) -> bool {
    state.is_object() && serialized_within(state, MAX_RESPONSE_STATE_BYTES)
}

// Counting through a bounded writer avoids allocating a second, potentially
// oversized serialized copy merely to decide whether a value is acceptable.
fn serialized_within(value: &impl Serialize, limit: usize) -> bool {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(io::Error::other("response limit exceeded"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(limit), value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn descriptor() -> AttemptDescriptor {
        AttemptDescriptor {
            capabilities: AttemptCapabilities {
                preserves_affinity_on_failure: false,
                successful_response_is_sse: false,
                changes_request_body: false,
            },
            protocols: vec![
                ProtocolCapability {
                    protocol: ConnectorProtocol::NonStream,
                    response: ResponseMode::Json,
                },
                ProtocolCapability {
                    protocol: ConnectorProtocol::Sse,
                    response: ResponseMode::Sse,
                },
            ],
        }
    }

    #[test]
    fn descriptors_roundtrip_and_reject_invalid_protocols() {
        let descriptor = descriptor();
        let encoded = serde_json::to_value(&descriptor).unwrap();
        assert_eq!(encoded["protocols"][0]["protocol"], "non_stream");
        assert_eq!(
            serde_json::from_value::<AttemptDescriptor>(encoded).unwrap(),
            descriptor
        );
        assert!(descriptor.validate_for_operation("chat_completion"));
        assert!(descriptor.validate_for_operation("responses"));
        assert!(!descriptor.validate_for_operation("web_search"));
        let mut invalid = descriptor.clone();
        invalid.capabilities.successful_response_is_sse = true;
        assert!(!invalid.validate_for_operation("responses"));
        invalid = descriptor.clone();
        invalid.protocols[1] = invalid.protocols[0].clone();
        assert!(!invalid.validate_for_operation("responses"));
        invalid.protocols.clear();
        assert!(!invalid.validate_for_operation("responses"));
        invalid.protocols.push(ProtocolCapability {
            protocol: ConnectorProtocol::Websocket,
            response: ResponseMode::Passthrough,
        });
        assert!(invalid.validate_for_operation("responses-ws"));
        assert!(!invalid.validate_for_operation("responses"));
        assert!(
            serde_json::from_value::<AttemptCapabilities>(json!({
                "preserves_affinity_on_failure":false,"successful_response_is_sse":false
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ProtocolCapability>(json!({
                "protocol":"sse","response":"raw_chunks"
            }))
            .is_err()
        );
    }

    #[test]
    fn event_metadata_roundtrips_and_is_bounded() {
        let mut input = ResponseEventInput {
            operation: "responses".into(),
            protocol: ConnectorProtocol::Sse,
            status: 200,
            event: Some("response.output_text.delta".into()),
            id: None,
            sequence: 0,
            state: json!({}),
            phase: ResponsePhase::Event,
        };
        assert_eq!(
            serde_json::from_value::<ResponseEventInput>(serde_json::to_value(&input).unwrap())
                .unwrap(),
            input
        );
        assert!(input.validate_bounds(&vec![b'x'; MAX_RESPONSE_EVENT_BYTES]));
        assert!(!input.validate_bounds(&vec![b'x'; MAX_RESPONSE_EVENT_BYTES + 1]));
        input.phase = ResponsePhase::Finish;
        assert!(input.validate_bounds(&[]));
        assert!(!input.validate_bounds(b"data"));
        input.state = json!({"s":"x".repeat(MAX_RESPONSE_STATE_BYTES - 8)});
        assert!(input.validate_bounds(&[]));
        input.state["s"] = json!("x".repeat(MAX_RESPONSE_STATE_BYTES - 7));
        assert!(!input.validate_bounds(&[]));
        input.state = Value::Null;
        assert!(!input.validate_bounds(&[]));
    }

    #[test]
    fn json_metadata_roundtrips_and_body_bounds_are_exact() {
        let mut input = ResponseJsonInput {
            operation: "chat_completion".into(),
            protocol: ConnectorProtocol::NonStream,
            status: 200,
        };
        assert_eq!(
            serde_json::from_slice::<ResponseJsonInput>(&serde_json::to_vec(&input).unwrap())
                .unwrap(),
            input
        );
        assert!(input.validate_bounds(&vec![b'x'; MAX_RESPONSE_JSON_BYTES]));
        assert!(!input.validate_bounds(&vec![b'x'; MAX_RESPONSE_JSON_BYTES + 1]));
        input.protocol = ConnectorProtocol::Sse;
        assert!(!input.validate_bounds(b"{}"));
        input.protocol = ConnectorProtocol::NonStream;
        input.operation = "images_generation".into();
        assert!(!input.validate_bounds(b"{}"));
    }

    #[test]
    fn output_limits_and_sse_framing_are_enforced() {
        let event = ResponseEvent {
            event: None,
            data: "hello".into(),
            id: None,
        };
        let mut output = ResponseEventOutput {
            state: json!({}),
            events: vec![event.clone(); MAX_RESPONSE_OUTPUT_EVENTS],
        };
        assert!(output.validate_bounds());
        assert_eq!(
            serde_json::from_slice::<ResponseEventOutput>(&serde_json::to_vec(&output).unwrap())
                .unwrap(),
            output
        );
        output.events.push(event);
        assert!(!output.validate_bounds());
        output.events.truncate(1);
        output.events[0].id = Some("id\ninjected".into());
        assert!(!output.validate_bounds());
        output.events[0].id = None;
        output.events[0].data = "x".repeat(MAX_RESPONSE_EVENT_BYTES + 1);
        assert!(!output.validate_bounds());
        output.events[0].data = "\"".repeat(MAX_RESPONSE_EVENT_BYTES / 2);
        assert!(!output.validate_bounds());
        assert_eq!(MAX_RESPONSE_JSON_BYTES, 8 * 1024 * 1024);
    }
}
