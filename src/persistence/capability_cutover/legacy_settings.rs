//! Frozen pre-six-operation capability settings for historical upgrades.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{ApiOperation, CapabilityTransport, ConnectorKind, RequestCompression};

pub type Topology = crate::persistence::upstream_topology::UpstreamTopologyRecords<
    CapabilitySettings,
    crate::persistence::upstream_topology::ApiKeyCapabilityGrantRecord,
    crate::persistence::upstream_topology::ApiKeyPolicyCapabilityGrantRecord,
    RoutingGroupRecord,
    LogicalChannelRecord,
>;
pub type SixOperationTopology = crate::persistence::upstream_topology::UpstreamTopologyRecords<
    crate::domain::CapabilitySettings,
    crate::persistence::upstream_topology::ApiKeyCapabilityGrantRecord,
    crate::persistence::upstream_topology::ApiKeyPolicyCapabilityGrantRecord,
    RoutingGroupRecord,
    LogicalChannelRecord,
>;
pub type ChannelAuthorizationTopology =
    crate::persistence::upstream_topology::UpstreamTopologyRecords<
        crate::domain::CapabilitySettings,
        crate::persistence::upstream_topology::ApiKeyChannelGrantRecord,
        crate::persistence::upstream_topology::ApiKeyPolicyChannelGrantRecord,
        RoutingGroupRecord,
        LogicalChannelRecord,
    >;
pub type Capability =
    crate::persistence::upstream_topology::ChannelCapabilityRecord<CapabilitySettings>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingGroupRecord {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub sharing_only: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalChannelRecord {
    pub id: Uuid,
    pub group_id: Uuid,
    pub access_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub name: String,
    pub enabled: bool,
    pub binding_revision: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

pub fn operation_name(operation: ApiOperation) -> &'static str {
    match operation {
        ApiOperation::ChatCompletions => "chat_completions",
        ApiOperation::StandaloneWebSearch => "standalone_web_search",
        operation => operation.as_str(),
    }
}

pub fn connector_name(connector: ConnectorKind) -> &'static str {
    match connector {
        ConnectorKind::OpenAiCompatible => "openai_compatible",
        ConnectorKind::CodexOauth => "codex_oauth",
    }
}

pub fn decode<T: serde::de::DeserializeOwned>(row: &str) -> Result<T, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_str(row)?;
    normalize_operation(&mut value);
    if let Some(settings) = value.get_mut("settings") {
        normalize_operation(settings);
    }
    if let Some(connector) = value.get_mut("connector_kind") {
        match connector.as_str() {
            Some("openai_compatible") => *connector = "general".into(),
            Some("codex_oauth") => *connector = "codex".into(),
            _ => {}
        }
    }
    serde_json::from_value(value)
}

fn normalize_operation(value: &mut serde_json::Value) {
    if let Some(operation) = value.get_mut("operation") {
        match operation.as_str() {
            Some("chat_completions") => *operation = "chat_completion".into(),
            Some("standalone_web_search") => *operation = "web_search".into(),
            _ => {}
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySettings {
    pub operation: ApiOperation,
    pub transports: Vec<CapabilityTransport>,
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
    #[error("unsupported or duplicate operation transport")]
    Transport,
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
        use CapabilityTransport as T;

        if matches!(
            (connector, self.operation),
            (ConnectorKind::CodexOauth, O::ChatCompletions)
        ) {
            return Err(CapabilityError::Operation);
        }
        let supported: &[T] = match (connector, self.operation) {
            (_, O::ResponsesWebSocket) => return Err(CapabilityError::Operation),
            (_, O::ChatCompletions) => &[T::HttpJson, T::HttpSse],
            (ConnectorKind::OpenAiCompatible, O::Responses) => {
                &[T::HttpJson, T::HttpSse, T::Websocket]
            }
            (ConnectorKind::CodexOauth, O::Responses) => &[T::HttpSse, T::Websocket],
            (_, O::StandaloneWebSearch | O::ImagesGeneration) => &[T::HttpJson],
            (_, O::ImagesEdit) => &[T::Multipart],
        };
        let mut transports = HashSet::new();
        if self.transports.is_empty()
            || self
                .transports
                .iter()
                .any(|transport| !supported.contains(transport) || !transports.insert(*transport))
        {
            return Err(CapabilityError::Transport);
        }
        if self.request_compression.is_encoded()
            && (self.operation != O::Responses
                || !self
                    .transports
                    .iter()
                    .any(|t| matches!(t, T::HttpJson | T::HttpSse)))
        {
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
                    || !self.transports.contains(&T::HttpJson)
                    || !self.available_models.contains(model)
            })
        {
            return Err(CapabilityError::Probe);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(
        operation: ApiOperation,
        transports: Vec<CapabilityTransport>,
    ) -> CapabilitySettings {
        CapabilitySettings {
            operation,
            transports,
            enabled: false,
            available_models: vec![],
            request_compression: RequestCompression::Default,
            test_model: None,
            test_pricing_model_id: None,
            auto_disable_allowed: false,
        }
    }

    #[test]
    fn connectors_reject_unimplemented_operations_and_transport_combinations() {
        use ApiOperation as O;
        use CapabilityTransport as T;
        for operation in [
            O::ChatCompletions,
            O::Responses,
            O::StandaloneWebSearch,
            O::ImagesGeneration,
            O::ImagesEdit,
        ] {
            for connector in [ConnectorKind::OpenAiCompatible, ConnectorKind::CodexOauth] {
                for transport in [T::HttpJson, T::HttpSse, T::Websocket, T::Multipart] {
                    let expected = match (connector, operation, transport) {
                        (ConnectorKind::CodexOauth, O::ChatCompletions, _) => false,
                        (_, O::ChatCompletions, T::HttpJson | T::HttpSse)
                        | (ConnectorKind::OpenAiCompatible, O::Responses, T::HttpJson)
                        | (_, O::Responses, T::HttpSse | T::Websocket)
                        | (_, O::StandaloneWebSearch | O::ImagesGeneration, T::HttpJson)
                        | (_, O::ImagesEdit, T::Multipart) => true,
                        _ => false,
                    };
                    assert_eq!(
                        settings(operation, vec![transport])
                            .validate(connector)
                            .is_ok(),
                        expected,
                        "{connector:?} {operation:?} {transport:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn compression_and_probe_settings_are_operation_scoped() {
        let mut value = settings(
            ApiOperation::Responses,
            vec![CapabilityTransport::Websocket],
        );
        value.request_compression = RequestCompression::Zstd;
        assert_eq!(
            value.validate(ConnectorKind::CodexOauth),
            Err(CapabilityError::Compression)
        );
        value.transports.push(CapabilityTransport::HttpSse);
        assert!(value.validate(ConnectorKind::CodexOauth).is_ok());
        value.test_model = Some("wire".into());
        assert_eq!(
            value.validate(ConnectorKind::CodexOauth),
            Err(CapabilityError::Probe)
        );
        value.available_models.push("wire".into());
        value.test_pricing_model_id = Some(Uuid::new_v4());
        assert_eq!(
            value.validate(ConnectorKind::CodexOauth),
            Err(CapabilityError::Probe)
        );
        value.transports.push(CapabilityTransport::HttpJson);
        assert!(value.validate(ConnectorKind::OpenAiCompatible).is_ok());
    }

    #[test]
    fn empty_duplicate_and_cross_operation_transports_are_rejected() {
        for transports in [
            vec![],
            vec![CapabilityTransport::HttpJson, CapabilityTransport::HttpJson],
            vec![CapabilityTransport::Multipart],
        ] {
            assert_eq!(
                settings(ApiOperation::Responses, transports)
                    .validate(ConnectorKind::OpenAiCompatible),
                Err(CapabilityError::Transport)
            );
        }
    }

    #[test]
    fn search_retains_its_wire_catalogue_but_has_no_compression_or_probes() {
        let mut value = settings(
            ApiOperation::StandaloneWebSearch,
            vec![CapabilityTransport::HttpJson],
        );
        assert!(value.validate(ConnectorKind::CodexOauth).is_ok());
        value.available_models.push("wire".into());
        assert!(value.validate(ConnectorKind::CodexOauth).is_ok());
        assert!(value.validate(ConnectorKind::OpenAiCompatible).is_ok());
        value.request_compression = RequestCompression::Zstd;
        assert_eq!(
            value.validate(ConnectorKind::CodexOauth),
            Err(CapabilityError::Compression)
        );
        value.request_compression = RequestCompression::Default;
        value.test_model = Some("wire".into());
        value.test_pricing_model_id = Some(Uuid::new_v4());
        assert_eq!(
            value.validate(ConnectorKind::CodexOauth),
            Err(CapabilityError::Probe)
        );
    }

    #[test]
    fn image_probes_are_not_advertised_before_they_are_implemented() {
        for (operation, transport) in [
            (
                ApiOperation::ImagesGeneration,
                CapabilityTransport::HttpJson,
            ),
            (ApiOperation::ImagesEdit, CapabilityTransport::Multipart),
        ] {
            let mut value = settings(operation, vec![transport]);
            value.available_models.push("image".into());
            value.test_model = Some("image".into());
            value.test_pricing_model_id = Some(Uuid::new_v4());
            for connector in [ConnectorKind::OpenAiCompatible, ConnectorKind::CodexOauth] {
                assert_eq!(value.validate(connector), Err(CapabilityError::Probe));
            }
        }
    }

    #[test]
    fn catalogues_and_probe_pairs_are_validated_even_when_disabled() {
        let mut value = settings(ApiOperation::Responses, vec![CapabilityTransport::HttpJson]);
        for models in [vec!["".into()], vec!["m".into(), "m".into()]] {
            value.available_models = models;
            assert_eq!(
                value.validate(ConnectorKind::OpenAiCompatible),
                Err(CapabilityError::Models)
            );
        }
        value.available_models = vec!["m".into()];
        value.test_pricing_model_id = Some(Uuid::new_v4());
        assert_eq!(
            value.validate(ConnectorKind::OpenAiCompatible),
            Err(CapabilityError::Probe)
        );
        value.test_model = Some("not-in-catalogue".into());
        assert_eq!(
            value.validate(ConnectorKind::OpenAiCompatible),
            Err(CapabilityError::Probe)
        );
    }
}
