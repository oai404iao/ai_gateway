//! Versioned request-log encoding shared by the local spool and DB ingress.

use thiserror::Error;
use uuid::Uuid;

use crate::domain::RequestLogEvent;

pub(crate) const REQUEST_LOG_SCHEMA_VERSION: i16 = 1;

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
