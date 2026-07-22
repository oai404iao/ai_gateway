//! Versioned request-log encoding shared by the local spool and DB ingress.

use thiserror::Error;
use uuid::Uuid;

use crate::domain::RequestLogEvent;

pub(crate) const REQUEST_LOG_SCHEMA_VERSION: i16 = 2;

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
        let event = serde_json::from_slice::<RequestLogEvent>(&self.payload)
            .map_err(JournalCodecError::Deserialize)?;
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
    use uuid::Uuid;

    use super::{EncodedRequestLog, JournalCodecError};

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
