//! Versioned request-log encoding shared by the local spool and DB ingress.

use thiserror::Error;
use uuid::Uuid;

use crate::domain::RequestLogEvent;

pub(crate) const REQUEST_LOG_SCHEMA_VERSION: i16 = 10;

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
        if self.schema_version != REQUEST_LOG_SCHEMA_VERSION {
            return Err(JournalCodecError::UnsupportedVersion {
                version: self.schema_version,
            });
        }
        let event: RequestLogEvent =
            serde_json::from_slice(&self.payload).map_err(JournalCodecError::Deserialize)?;
        if event.id != self.request_log_id {
            return Err(JournalCodecError::IdentifierMismatch);
        }
        Ok(event)
    }
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
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn journal_accepts_only_current_complete_terminal_records() {
        let id = Uuid::new_v4();
        let credential_id = Uuid::new_v4();
        let payload = json!({
            "id":id, "started_at":"2026-07-22T00:00:00Z", "completed_at":"2026-07-22T00:00:01Z",
            "user_id":Uuid::new_v4(), "api_key_id":Uuid::new_v4(), "request_source":"client",
            "api_format":"open_ai_responses", "api_operation":"responses-ws", "request_protocol":"websocket",
            "client_model":"model", "upstream_credential_id":credential_id, "fast_mode":false,
            "outcome":"succeeded", "response_status_code":200, "total_duration_ms":42, "error_summary":null
        });
        let record = EncodedRequestLog {
            schema_version: REQUEST_LOG_SCHEMA_VERSION,
            request_log_id: id,
            payload: serde_json::to_vec(&payload).unwrap(),
        };
        let event = record.decode().unwrap();
        assert_eq!(event.upstream_credential_id, Some(credential_id));
        let round_trip = EncodedRequestLog::encode(&event).unwrap();
        assert_eq!(
            round_trip.decode().unwrap().request_protocol,
            crate::domain::RequestProtocol::WebSocket
        );
        assert!(matches!(
            EncodedRequestLog {
                request_log_id: Uuid::new_v4(),
                ..record.clone()
            }
            .decode(),
            Err(JournalCodecError::IdentifierMismatch)
        ));
        for version in [1, 9, 11] {
            assert!(matches!(
                EncodedRequestLog {
                    schema_version: version,
                    ..record.clone()
                }
                .decode(),
                Err(JournalCodecError::UnsupportedVersion { .. })
            ));
        }
        for required in [
            "request_protocol",
            "api_operation",
            "upstream_credential_id",
            "error_summary",
        ] {
            let mut missing: Value = payload.clone();
            missing.as_object_mut().unwrap().remove(required);
            assert!(matches!(
                EncodedRequestLog {
                    payload: serde_json::to_vec(&missing).unwrap(),
                    ..record.clone()
                }
                .decode(),
                Err(JournalCodecError::Deserialize(_))
            ));
        }
    }
}
