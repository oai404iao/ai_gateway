//! Connector-constrained operations; transports are derived, never configured.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiOperation, ConnectorKind, RequestCompression};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityTransport {
    HttpJson,
    HttpSse,
    Websocket,
    Multipart,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySettings {
    pub operation: ApiOperation,
    pub enabled: bool,
    pub available_models: Vec<String>,
    pub request_compression: RequestCompression,
    pub test_model: Option<String>,
    pub test_pricing_model_id: Option<Uuid>,
    pub auto_disable_allowed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityError {
    #[error("operation is not implemented by this connector")]
    Operation,
    #[error("compression requires HTTP Responses")]
    Compression,
    #[error("invalid capability model catalogue")]
    Models,
    #[error("invalid capability probe configuration")]
    Probe,
}

impl CapabilitySettings {
    pub fn validate(&self, connector: ConnectorKind) -> Result<(), CapabilityError> {
        use ApiOperation as O;
        if connector == ConnectorKind::CodexOauth && self.operation == O::ChatCompletions {
            return Err(CapabilityError::Operation);
        }
        if self.request_compression.is_encoded() && self.operation != O::Responses {
            return Err(CapabilityError::Compression);
        }
        let mut models = HashSet::new();
        if self
            .available_models
            .iter()
            .any(|model| model.trim().is_empty() || !models.insert(model))
        {
            return Err(CapabilityError::Models);
        }
        if self.test_model.is_some() != self.test_pricing_model_id.is_some()
            || self.test_model.as_ref().is_some_and(|model| {
                connector != ConnectorKind::OpenAiCompatible
                    || !matches!(self.operation, O::ChatCompletions | O::Responses)
                    || !self.available_models.contains(model)
            })
        {
            return Err(CapabilityError::Probe);
        }
        Ok(())
    }
}

impl ApiOperation {
    pub const fn transports(self) -> &'static [CapabilityTransport] {
        use CapabilityTransport as T;
        match self {
            Self::ChatCompletions | Self::Responses => &[T::HttpJson, T::HttpSse],
            Self::ResponsesWebSocket => &[T::Websocket],
            Self::StandaloneWebSearch | Self::ImagesGeneration => &[T::HttpJson],
            Self::ImagesEdit => &[T::Multipart],
        }
    }
}
