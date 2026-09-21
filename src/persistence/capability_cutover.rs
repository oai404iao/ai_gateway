//! Pure, backend-independent route and target-grant planning for the joint capability cutover.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::ModelRuleRecord;
use crate::domain::{ApiFormat, ApiOperation, SelectionStrategy};

#[derive(Clone, Debug)]
pub struct LegacyGroupTarget {
    pub id: Uuid,
    pub management_group_id: Uuid,
    pub api_format: ApiFormat,
}

#[derive(Clone, Debug)]
pub struct LegacyCapabilityTarget {
    pub id: Uuid,
    pub operation: ApiOperation,
}

#[derive(Clone, Debug)]
pub struct LegacyChannelTarget {
    pub id: Uuid,
    pub group_id: Uuid,
    pub logical_channel_id: Uuid,
    pub api_format: ApiFormat,
    pub capabilities: Vec<LegacyCapabilityTarget>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRouteCandidate {
    pub capability_id: Uuid,
    pub upstream_model: String,
    pub weight: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRoutingTier {
    pub priority: i32,
    pub selection_strategy: String,
    pub candidates: Vec<CapabilityRouteCandidate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationRulePlan {
    pub id: Uuid,
    pub legacy_rule_id: Uuid,
    pub model_id: Uuid,
    pub operation: ApiOperation,
    pub enabled: bool,
    pub routing_tiers: Vec<CapabilityRoutingTier>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CapabilityGrantOrigin {
    Group(Uuid),
    Channel(Uuid),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CapabilityTargetGrant {
    pub capability_id: Uuid,
    pub origin: CapabilityGrantOrigin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityCutoverError {
    #[error("invalid legacy capability topology at {id}")]
    InvalidTopology { id: Uuid },
    #[error("invalid legacy rule {rule_id}")]
    InvalidRule { rule_id: Uuid },
    #[error("legacy rule {rule_id} references incompatible channel {channel_id}")]
    InvalidRouteTarget { rule_id: Uuid, channel_id: Uuid },
    #[error("legacy authorization references unknown target {id}")]
    UnknownGrantTarget { id: Uuid },
}

/// A migration-only index. Its mappings must never become a runtime routing fallback.
pub struct CapabilityCutoverIndex {
    groups: HashMap<Uuid, LegacyGroupTarget>,
    channels: HashMap<Uuid, LegacyChannelTarget>,
}

impl CapabilityCutoverIndex {
    pub fn new(
        groups: Vec<LegacyGroupTarget>,
        channels: Vec<LegacyChannelTarget>,
    ) -> Result<Self, CapabilityCutoverError> {
        let mut group_index = HashMap::new();
        for group in groups {
            let id = group.id;
            if group_index.insert(id, group).is_some() {
                return Err(CapabilityCutoverError::InvalidTopology { id });
            }
        }
        let mut channel_index = HashMap::new();
        let mut capability_ids = HashSet::new();
        let mut logical_groups = HashMap::new();
        let mut logical_operations = HashSet::new();
        for channel in channels {
            let id = channel.id;
            let invalid = || CapabilityCutoverError::InvalidTopology { id };
            let group = group_index.get(&channel.group_id).ok_or_else(invalid)?;
            if group.api_format != channel.api_format {
                return Err(invalid());
            }
            if let Some(previous) =
                logical_groups.insert(channel.logical_channel_id, group.management_group_id)
                && previous != group.management_group_id
            {
                return Err(invalid());
            }
            let mut operations = HashSet::new();
            for capability in &channel.capabilities {
                if capability.operation.api_format() != channel.api_format
                    || !operations.insert(capability.operation)
                    || !capability_ids.insert(capability.id)
                    || !logical_operations
                        .insert((channel.logical_channel_id, capability.operation))
                {
                    return Err(invalid());
                }
            }
            if required_operations(channel.api_format)
                .iter()
                .any(|operation| !operations.contains(operation))
                || channel_index.insert(id, channel).is_some()
            {
                return Err(invalid());
            }
        }
        Ok(Self {
            groups: group_index,
            channels: channel_index,
        })
    }

    pub fn rewrite_rules(
        &self,
        rules: &[ModelRuleRecord],
    ) -> Result<Vec<OperationRulePlan>, CapabilityCutoverError> {
        let mut output = Vec::new();
        let mut identities = HashSet::new();
        let mut model_operations = HashSet::new();
        for rule in rules {
            let invalid = || CapabilityCutoverError::InvalidRule { rule_id: rule.id };
            let format = ApiFormat::parse(&rule.api_format).ok_or_else(invalid)?;
            self.validate_rule(rule, format)?;
            let mut operations = required_operations(format).to_vec();
            if format == ApiFormat::OpenAiResponses
                && rule
                    .routing_tiers
                    .iter()
                    .flat_map(|tier| &tier.candidates)
                    .any(|candidate| {
                        self.channels[&candidate.channel_id]
                            .capabilities
                            .iter()
                            .any(|capability| {
                                capability.operation == ApiOperation::StandaloneWebSearch
                            })
                    })
            {
                operations.push(ApiOperation::StandaloneWebSearch);
            }
            for operation in operations {
                let id = operation_rule_id(rule.id, format, operation);
                if !identities.insert(id) || !model_operations.insert((rule.model_id, operation)) {
                    return Err(invalid());
                }
                let mut routing_tiers = Vec::new();
                for tier in &rule.routing_tiers {
                    let mut candidates = Vec::new();
                    let mut targets = HashSet::new();
                    for candidate in &tier.candidates {
                        let channel = &self.channels[&candidate.channel_id];
                        let Some(capability) = channel
                            .capabilities
                            .iter()
                            .find(|capability| capability.operation == operation)
                        else {
                            continue;
                        };
                        let upstream_model = candidate.upstream_model.clone();
                        if !targets.insert((capability.id, upstream_model.clone())) {
                            return Err(invalid());
                        }
                        candidates.push(CapabilityRouteCandidate {
                            capability_id: capability.id,
                            upstream_model,
                            weight: candidate.weight,
                        });
                    }
                    if !candidates.is_empty() {
                        routing_tiers.push(CapabilityRoutingTier {
                            priority: tier.priority,
                            selection_strategy: tier.selection_strategy.clone(),
                            candidates,
                        });
                    }
                }
                output.push(OperationRulePlan {
                    id,
                    legacy_rule_id: rule.id,
                    model_id: rule.model_id,
                    operation,
                    enabled: rule.enabled,
                    routing_tiers,
                });
            }
        }
        output.sort_by_key(|rule| rule.id);
        Ok(output)
    }

    /// Rewrites target grants only. User/key status, sharing seats, format/transport
    /// permissions and admission remain independent runtime checks.
    ///
    /// `formats=None` is for a Policy's target ceiling, not an API key with no formats.
    pub fn rewrite_target_grants(
        &self,
        group_ids: &[Uuid],
        channel_ids: &[Uuid],
        formats: Option<&[ApiFormat]>,
    ) -> Result<Vec<CapabilityTargetGrant>, CapabilityCutoverError> {
        let mut output = HashSet::new();
        for id in group_ids {
            let group = self
                .groups
                .get(id)
                .ok_or(CapabilityCutoverError::UnknownGrantTarget { id: *id })?;
            for channel in self
                .channels
                .values()
                .filter(|channel| channel.group_id == *id)
            {
                add_grants(
                    &mut output,
                    channel,
                    formats,
                    CapabilityGrantOrigin::Group(group.management_group_id),
                );
            }
        }
        for id in channel_ids {
            let channel = self
                .channels
                .get(id)
                .ok_or(CapabilityCutoverError::UnknownGrantTarget { id: *id })?;
            add_grants(
                &mut output,
                channel,
                formats,
                CapabilityGrantOrigin::Channel(channel.logical_channel_id),
            );
        }
        let mut output = output.into_iter().collect::<Vec<_>>();
        output.sort_by_key(|grant| {
            let (kind, id) = match grant.origin {
                CapabilityGrantOrigin::Group(id) => (0, id),
                CapabilityGrantOrigin::Channel(id) => (1, id),
            };
            (grant.capability_id, kind, id)
        });
        Ok(output)
    }

    fn validate_rule(
        &self,
        rule: &ModelRuleRecord,
        format: ApiFormat,
    ) -> Result<(), CapabilityCutoverError> {
        let invalid = || CapabilityCutoverError::InvalidRule { rule_id: rule.id };
        if rule.client_model.trim().is_empty() || (rule.enabled && rule.routing_tiers.is_empty()) {
            return Err(invalid());
        }
        let mut priorities = HashSet::new();
        for tier in &rule.routing_tiers {
            if tier.priority < 0
                || !priorities.insert(tier.priority)
                || SelectionStrategy::parse(&tier.selection_strategy).is_none()
                || tier.candidates.is_empty()
            {
                return Err(invalid());
            }
            let mut targets = HashSet::new();
            for candidate in &tier.candidates {
                if candidate.weight <= 0
                    || candidate.upstream_model.trim().is_empty()
                    || candidate.upstream_model.chars().count() > 300
                    || !targets.insert((candidate.channel_id, &candidate.upstream_model))
                {
                    return Err(invalid());
                }
                if !self
                    .channels
                    .get(&candidate.channel_id)
                    .is_some_and(|channel| channel.api_format == format)
                {
                    return Err(CapabilityCutoverError::InvalidRouteTarget {
                        rule_id: rule.id,
                        channel_id: candidate.channel_id,
                    });
                }
            }
        }
        Ok(())
    }
}

fn required_operations(format: ApiFormat) -> &'static [ApiOperation] {
    match format {
        ApiFormat::OpenAiChatCompletions => &[ApiOperation::ChatCompletions],
        ApiFormat::OpenAiResponses => &[ApiOperation::Responses],
        ApiFormat::OpenAiImages => &[ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
    }
}

fn operation_rule_id(legacy_id: Uuid, format: ApiFormat, operation: ApiOperation) -> Uuid {
    if operation == ApiOperation::legacy_default(format) {
        return legacy_id;
    }
    let mut hash = Sha256::new();
    hash.update(b"ai-gateway:operation-rule:v1:");
    hash.update(legacy_id.as_bytes());
    hash.update(operation.as_str().as_bytes());
    let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().expect("SHA-256 length");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn add_grants(
    output: &mut HashSet<CapabilityTargetGrant>,
    channel: &LegacyChannelTarget,
    formats: Option<&[ApiFormat]>,
    origin: CapabilityGrantOrigin,
) {
    if formats.is_some_and(|formats| !formats.contains(&channel.api_format)) {
        return;
    }
    output.extend(
        channel
            .capabilities
            .iter()
            .map(|capability| CapabilityTargetGrant {
                capability_id: capability.id,
                origin,
            }),
    );
}

#[cfg(test)]
mod tests;
