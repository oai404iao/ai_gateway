//! Versioned request-log encoding shared by the local spool and DB ingress.

use thiserror::Error;
use uuid::Uuid;

use crate::domain::{RequestLogEvent, RequestProtocol};

pub(crate) const REQUEST_LOG_SCHEMA_VERSION: i16 = 3;

#[derive(Clone, Debug)]
pub(crate) struct EncodedRequestLog {
    pub request_log_id: Uuid,
    pub schema_version: i16,
    pub payload: Vec<u8>,
}

impl EncodedRequestLog {
    pub(crate) fn encode(event: &RequestLogEvent) -> Result<Self, JournalCodecError> {
        Ok(Self {
            request_log_id: event.id,
            schema_version: REQUEST_LOG_SCHEMA_VERSION,
            payload: serde_json::to_vec(event).map_err(JournalCodecError::Serialize)?,
        })
    }

    pub(crate) fn decode(&self) -> Result<RequestLogEvent, JournalCodecError> {
        let event = match self.schema_version {
            2 => decode_v2(&self.payload)?,
            REQUEST_LOG_SCHEMA_VERSION => serde_json::from_slice::<RequestLogEvent>(&self.payload)
                .map_err(JournalCodecError::Deserialize)?,
            version => return Err(JournalCodecError::UnsupportedVersion { version }),
        };
        if event.id != self.request_log_id {
            return Err(JournalCodecError::IdentifierMismatch);
        }
        Ok(event)
    }
}

fn decode_v2(payload: &[u8]) -> Result<RequestLogEvent, JournalCodecError> {
    #[derive(serde::Deserialize)]
    struct ProtocolProbe {
        streamed: bool,
    }

    let probe =
        serde_json::from_slice::<ProtocolProbe>(payload).map_err(JournalCodecError::Deserialize)?;
    let mut value = serde_json::from_slice::<serde_json::Value>(payload)
        .map_err(JournalCodecError::Deserialize)?;
    let object = value
        .as_object_mut()
        .expect("a successfully decoded request-log probe is an object");
    object.insert(
        "request_protocol".into(),
        serde_json::Value::String(
            RequestProtocol::from_http_streamed(probe.streamed)
                .as_str()
                .into(),
        ),
    );
    serde_json::from_value(value).map_err(JournalCodecError::Deserialize)
}

#[derive(Debug, Error)]
pub(crate) enum JournalCodecError {
    #[error("request-log journal serialization failed")]
    Serialize(#[source] serde_json::Error),
    #[error("request-log journal deserialization failed")]
    Deserialize(#[source] serde_json::Error),
    #[error("request-log journal schema version {version} is unsupported")]
    UnsupportedVersion { version: i16 },
    #[error("request-log journal identifier does not match its payload")]
    IdentifierMismatch,
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{EncodedRequestLog, JournalCodecError};
    use crate::domain::RequestProtocol;

    #[test]
    fn rejects_prior_schema_versions() {
        let error = EncodedRequestLog {
            request_log_id: Uuid::new_v4(),
            schema_version: 1,
            payload: Vec::new(),
        }
        .decode()
        .unwrap_err();

        assert!(matches!(
            error,
            JournalCodecError::UnsupportedVersion { version: 1 }
        ));
    }

    #[test]
    fn decodes_v2_payloads_with_an_inferred_protocol() {
        let id = Uuid::new_v4();
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": id,
            "started_at": "2026-07-22T00:00:00Z",
            "completed_at": "2026-07-22T00:00:01Z",
            "user_id": Uuid::new_v4(),
            "api_key_id": Uuid::new_v4(),
            "request_source": "client",
            "api_format": "open_ai_chat_completions",
            "client_model": "model",
            "upstream_model": "upstream-model",
            "model_rule_id": null,
            "channel_group_id": null,
            "channel_id": null,
            "model_id": null,
            "outcome": "cancelled",
            "response_status_code": 200,
            "streamed": true,
            "ttft_ms": 1,
            "total_duration_ms": 2,
            "billing": null,
            "error_code": "client_cancelled",
            "error_summary": null
        }))
        .unwrap();
        let event = EncodedRequestLog {
            request_log_id: id,
            schema_version: 2,
            payload,
        }
        .decode()
        .unwrap();

        assert_eq!(event.request_protocol, RequestProtocol::Sse);
    }

    #[test]
    fn rejects_current_schema_payloads_without_a_request_protocol() {
        let id = Uuid::new_v4();
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": id,
            "started_at": "2026-07-22T00:00:00Z",
            "completed_at": "2026-07-22T00:00:01Z",
            "user_id": Uuid::new_v4(),
            "api_key_id": Uuid::new_v4(),
            "request_source": "client",
            "api_format": "open_ai_chat_completions",
            "client_model": "model",
            "upstream_model": "upstream-model",
            "model_rule_id": null,
            "channel_group_id": null,
            "channel_id": null,
            "model_id": null,
            "outcome": "cancelled",
            "response_status_code": 200,
            "streamed": true,
            "ttft_ms": 1,
            "total_duration_ms": 2,
            "billing": null,
            "error_code": "client_cancelled",
            "error_summary": null
        }))
        .unwrap();
        let error = EncodedRequestLog {
            request_log_id: id,
            schema_version: super::REQUEST_LOG_SCHEMA_VERSION,
            payload,
        }
        .decode()
        .unwrap_err();

        assert!(matches!(error, JournalCodecError::Deserialize(_)));
    }

    #[test]
    fn rejects_current_schema_payloads_without_an_error_summary() {
        let id = Uuid::new_v4();
        let payload = serde_json::to_vec(&serde_json::json!({
            "id": id,
            "started_at": "2026-07-22T00:00:00Z",
            "completed_at": "2026-07-22T00:00:01Z",
            "user_id": Uuid::new_v4(),
            "api_key_id": Uuid::new_v4(),
            "request_source": "client",
            "api_format": "open_ai_chat_completions",
            "request_protocol": "sse",
            "client_model": "model",
            "upstream_model": "upstream-model",
            "model_rule_id": null,
            "channel_group_id": null,
            "channel_id": null,
            "model_id": null,
            "outcome": "cancelled",
            "response_status_code": 200,
            "streamed": true,
            "ttft_ms": 1,
            "total_duration_ms": 2,
            "billing": null,
            "error_code": "client_cancelled"
        }))
        .unwrap();
        let error = EncodedRequestLog {
            request_log_id: id,
            schema_version: super::REQUEST_LOG_SCHEMA_VERSION,
            payload,
        }
        .decode()
        .unwrap_err();

        assert!(matches!(error, JournalCodecError::Deserialize(_)));
    }
}
