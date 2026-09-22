//! Backend-independent planning for the six-operation schema upgrade.
//!
//! This planner does not publish configuration or mutate storage. Its input is
//! the complete pre-upgrade topology, including disabled rows and tombstones.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub mod storage;

use super::legacy_settings::{
    Capability as ChannelCapabilityRecord, Topology as UpstreamTopologyRecords,
};
use crate::domain::{ApiOperation, CapabilityTransport, ConnectorKind};
use crate::persistence::upstream_topology::{
    ApiKeyCapabilityGrantRecord, ApiKeyPolicyCapabilityGrantRecord, GrantOriginKind,
    OperationCandidateRecord,
};

/// Frozen destination spellings for this migration, independent of later API changes.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Operation {
    #[serde(rename = "chat_completion")]
    ChatCompletion,
    #[serde(rename = "responses")]
    Responses,
    #[serde(rename = "responses-ws")]
    ResponsesWs,
    #[serde(rename = "web_search")]
    WebSearch,
    #[serde(rename = "images_edit")]
    ImagesEdit,
    #[serde(rename = "images_generation")]
    ImagesGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Connector {
    General,
    Codex,
}

#[derive(Clone, Debug, Serialize)]
pub struct Access {
    pub id: Uuid,
    pub connector: Connector,
}

#[derive(Clone, Debug, Serialize)]
pub struct Capability {
    pub source_id: Uuid,
    pub id: Uuid,
    pub operation: Operation,
}

impl Capability {
    /// WS siblings cannot inherit HTTP compression or scheduled probe settings.
    pub fn clear_http_settings(&self) -> bool {
        self.operation == Operation::ResponsesWs
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Rule {
    pub source_id: Uuid,
    pub id: Uuid,
    pub operation: Operation,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tier {
    pub source_id: Uuid,
    pub id: Uuid,
    pub rule_id: Uuid,
    pub operation: Operation,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub tier_id: Uuid,
    pub capability_id: Uuid,
    pub operation: Operation,
    pub upstream_model: String,
    pub weight: i32,
}

/// Copy all unmentioned row metadata from `source_id`, including enabled state,
/// health, transforms, prices and timestamps. Only WS capabilities clear HTTP
/// settings. Register new capability/rule identities without rewriting history;
/// any identity collision must abort the caller's transaction.
#[derive(Clone, Debug, Serialize)]
pub struct Plan {
    pub accesses: Vec<Access>,
    pub capabilities: Vec<Capability>,
    pub rules: Vec<Rule>,
    pub tiers: Vec<Tier>,
    pub candidates: Vec<Candidate>,
    pub api_key_grants: Vec<ApiKeyCapabilityGrantRecord>,
    pub policy_grants: Vec<ApiKeyPolicyCapabilityGrantRecord>,
}

impl From<Operation> for ApiOperation {
    fn from(operation: Operation) -> Self {
        match operation {
            Operation::ChatCompletion => Self::ChatCompletions,
            Operation::Responses => Self::Responses,
            Operation::ResponsesWs => Self::ResponsesWebSocket,
            Operation::WebSearch => Self::StandaloneWebSearch,
            Operation::ImagesEdit => Self::ImagesEdit,
            Operation::ImagesGeneration => Self::ImagesGeneration,
        }
    }
}

/// Materializes the same planned rows in memory, allowing database applicators
/// and historical fixtures to verify the complete upgrade without a fallback
/// in the serving path.
pub fn upgrade(
    input: &UpstreamTopologyRecords,
) -> Result<crate::persistence::upstream_topology::UpstreamTopologyRecords, SplitError> {
    use crate::persistence::upstream_topology as current;
    let plan = plan(input)?;
    let capabilities: HashMap<_, _> = input
        .channel_capabilities
        .iter()
        .map(|row| (row.id, row))
        .collect();
    let rules: HashMap<_, _> = input
        .operation_rules
        .iter()
        .map(|row| (row.id, row))
        .collect();
    let tiers: HashMap<_, _> = input
        .operation_tiers
        .iter()
        .map(|row| (row.id, row))
        .collect();
    Ok(current::UpstreamTopologyRecords {
        routing_groups: input.routing_groups.clone(),
        upstream_accesses: input.upstream_accesses.clone(),
        logical_channels: input.logical_channels.clone(),
        channel_capabilities: plan
            .capabilities
            .iter()
            .map(|projection| {
                let source = capabilities[&projection.source_id];
                let settings = &source.settings;
                current::ChannelCapabilityRecord {
                    id: projection.id,
                    channel_id: source.channel_id,
                    settings: crate::domain::CapabilitySettings {
                        operation: projection.operation.into(),
                        enabled: settings.enabled,
                        available_models: settings.available_models.clone(),
                        request_compression: if projection.clear_http_settings() {
                            crate::domain::RequestCompression::Default
                        } else {
                            settings.request_compression
                        },
                        test_model: if projection.clear_http_settings() {
                            None
                        } else {
                            settings.test_model.clone()
                        },
                        test_pricing_model_id: if projection.clear_http_settings() {
                            None
                        } else {
                            settings.test_pricing_model_id
                        },
                        auto_disable_allowed: settings.auto_disable_allowed,
                    },
                    auto_disabled: source.auto_disabled,
                    auto_disable_reason: source.auto_disable_reason.clone(),
                    auto_disable_at: source.auto_disable_at,
                    status_statistics_enabled: source.status_statistics_enabled,
                    config_template_id: source.config_template_id,
                    override_document: source.override_document.clone(),
                    billing_multiplier: source.billing_multiplier,
                    revision: source.revision,
                    created_at: source.created_at,
                    updated_at: source.updated_at,
                    deleted_at: source.deleted_at,
                }
            })
            .collect(),
        operation_rules: plan
            .rules
            .iter()
            .map(|projection| current::OperationRuleRecord {
                id: projection.id,
                operation: projection.operation.into(),
                ..rules[&projection.source_id].clone()
            })
            .collect(),
        operation_tiers: plan
            .tiers
            .iter()
            .map(|projection| current::OperationTierRecord {
                id: projection.id,
                rule_id: projection.rule_id,
                operation: projection.operation.into(),
                ..tiers[&projection.source_id].clone()
            })
            .collect(),
        operation_candidates: plan
            .candidates
            .into_iter()
            .map(|row| current::OperationCandidateRecord {
                tier_id: row.tier_id,
                capability_id: row.capability_id,
                operation: row.operation.into(),
                upstream_model: row.upstream_model,
                weight: row.weight,
            })
            .collect(),
        api_key_grants: plan.api_key_grants,
        policy_grants: plan.policy_grants,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SplitError {
    #[error("operation upgrade found duplicate identities or operation scopes")]
    Duplicate,
    #[error("operation upgrade found an invalid capability")]
    Capability,
    #[error("operation upgrade found an invalid route graph")]
    Route,
    #[error("operation upgrade found invalid fixed-grant provenance")]
    Grant,
    #[error("operation upgrade generated an existing identity")]
    IdentityCollision,
}

pub fn plan(input: &UpstreamTopologyRecords) -> Result<Plan, SplitError> {
    let accesses = unique(input.upstream_accesses.iter().map(|row| (row.id, row)))?;
    let channels = unique(input.logical_channels.iter().map(|row| (row.id, row)))?;
    let capabilities = unique(input.channel_capabilities.iter().map(|row| (row.id, row)))?;
    let rules = unique(input.operation_rules.iter().map(|row| (row.id, row)))?;
    let tiers = unique(input.operation_tiers.iter().map(|row| (row.id, row)))?;
    let mut channel_operations = HashSet::new();
    let mut profile_operations = HashSet::new();
    let mut rule_priorities = HashSet::new();
    let mut candidate_pairs = HashSet::new();
    let mut capability_ids: HashSet<_> = capabilities.keys().copied().collect();
    let mut rule_ids: HashSet<_> = rules.keys().copied().collect();
    let mut tier_ids: HashSet<_> = tiers.keys().copied().collect();
    let mut projected_tiers = HashSet::new();
    let mut plan = Plan {
        accesses: accesses
            .values()
            .map(|access| Access {
                id: access.id,
                connector: match access.connector_kind {
                    ConnectorKind::OpenAiCompatible => Connector::General,
                    ConnectorKind::CodexOauth => Connector::Codex,
                },
            })
            .collect(),
        capabilities: Vec::new(),
        rules: Vec::new(),
        tiers: Vec::new(),
        candidates: Vec::new(),
        api_key_grants: Vec::new(),
        policy_grants: Vec::new(),
    };
    let mut projections = HashMap::new();
    for capability in capabilities.values() {
        if !channel_operations.insert((capability.channel_id, capability.settings.operation)) {
            return Err(SplitError::Duplicate);
        }
        let channel = channels
            .get(&capability.channel_id)
            .ok_or(SplitError::Capability)?;
        let access = accesses
            .get(&channel.access_id)
            .ok_or(SplitError::Capability)?;
        capability
            .settings
            .validate(access.connector_kind)
            .map_err(|_| SplitError::Capability)?;
        let mut projected = Vec::new();
        for (index, operation) in capability_operations(capability).into_iter().enumerate() {
            let id = if index == 0 {
                capability.id
            } else {
                sibling_id(b"capability", capability.id, &mut capability_ids)?
            };
            projected.push((operation, id));
            plan.capabilities.push(Capability {
                source_id: capability.id,
                id,
                operation,
            });
        }
        projections.insert(capability.id, projected);
    }

    let mut candidates_by_tier: HashMap<Uuid, Vec<&OperationCandidateRecord>> = HashMap::new();
    for candidate in &input.operation_candidates {
        let tier = tiers.get(&candidate.tier_id).ok_or(SplitError::Route)?;
        let capability = capabilities
            .get(&candidate.capability_id)
            .ok_or(SplitError::Route)?;
        if candidate.operation != tier.operation
            || candidate.operation != capability.settings.operation
            || candidate.weight <= 0
            || candidate.upstream_model.trim().is_empty()
            || !candidate_pairs.insert((
                candidate.tier_id,
                candidate.capability_id,
                &candidate.upstream_model,
            ))
        {
            return Err(SplitError::Route);
        }
        candidates_by_tier
            .entry(tier.id)
            .or_default()
            .push(candidate);
    }
    let mut tiers_by_rule = HashMap::new();
    for tier in tiers.values() {
        let rule = rules.get(&tier.rule_id).ok_or(SplitError::Route)?;
        if tier.operation != rule.operation
            || tier.priority < 0
            || !matches!(
                tier.strategy.as_str(),
                "weighted_random" | "weighted_round_robin"
            )
            || !rule_priorities.insert((tier.rule_id, tier.priority))
            || !candidates_by_tier.contains_key(&tier.id)
        {
            return Err(SplitError::Route);
        }
        tiers_by_rule
            .entry(rule.id)
            .or_insert_with(Vec::new)
            .push(*tier);
    }
    for rule in rules.values() {
        if !profile_operations.insert((rule.model_routing_profile_id, rule.operation)) {
            return Err(SplitError::Duplicate);
        }
        let source_tiers = tiers_by_rule.get(&rule.id).cloned().unwrap_or_default();
        let mut operations = BTreeSet::new();
        for tier in &source_tiers {
            for candidate in &candidates_by_tier[&tier.id] {
                operations.extend(
                    projections[&candidate.capability_id]
                        .iter()
                        .map(|(op, _)| *op),
                );
            }
        }
        if operations.is_empty() {
            if rule.enabled {
                return Err(SplitError::Route);
            }
            operations.insert(default_operation(rule.operation));
        }
        for (index, operation) in operations.into_iter().enumerate() {
            let id = if index == 0 {
                rule.id
            } else {
                sibling_id(b"rule", rule.id, &mut rule_ids)?
            };
            plan.rules.push(Rule {
                source_id: rule.id,
                id,
                operation,
            });
            for tier in &source_tiers {
                let candidates: Vec<_> = candidates_by_tier[&tier.id]
                    .iter()
                    .filter_map(|candidate| {
                        projections[&candidate.capability_id]
                            .iter()
                            .find(|(op, _)| *op == operation)
                            .map(|(_, capability_id)| (candidate, *capability_id))
                    })
                    .collect();
                if candidates.is_empty() {
                    continue;
                }
                let tier_id = if projected_tiers.insert(tier.id) {
                    tier.id
                } else {
                    sibling_id(b"tier", tier.id, &mut tier_ids)?
                };
                plan.tiers.push(Tier {
                    source_id: tier.id,
                    id: tier_id,
                    rule_id: id,
                    operation,
                });
                plan.candidates
                    .extend(
                        candidates
                            .into_iter()
                            .map(|(source, capability_id)| Candidate {
                                tier_id,
                                capability_id,
                                operation,
                                upstream_model: source.upstream_model.clone(),
                                weight: source.weight,
                            }),
                    );
            }
        }
    }

    let resolve_origin = |capability_id, kind, origin_id| {
        let capability = capabilities.get(&capability_id).ok_or(SplitError::Grant)?;
        // Moved channels can retain dormant group-origin grants. Preserve that
        // provenance rather than rejecting it or granting the channel's new group.
        if kind == GrantOriginKind::Capability && origin_id != capability.id {
            return Err(SplitError::Grant);
        }
        Ok(&projections[&capability_id])
    };
    let mut key_grants = HashSet::new();
    for grant in &input.api_key_grants {
        for (_, capability_id) in
            resolve_origin(grant.capability_id, grant.origin_kind, grant.origin_id)?
        {
            let mut projected = grant.clone();
            projected.capability_id = *capability_id;
            if grant.origin_kind == GrantOriginKind::Capability {
                projected.origin_id = *capability_id;
            }
            if !key_grants.insert((
                projected.api_key_id,
                *capability_id,
                origin_tag(projected.origin_kind),
                projected.origin_id,
            )) {
                return Err(SplitError::Duplicate);
            }
            plan.api_key_grants.push(projected);
        }
    }
    let mut policy_grants = HashSet::new();
    for grant in &input.policy_grants {
        for (_, capability_id) in
            resolve_origin(grant.capability_id, grant.origin_kind, grant.origin_id)?
        {
            let mut projected = grant.clone();
            projected.capability_id = *capability_id;
            if grant.origin_kind == GrantOriginKind::Capability {
                projected.origin_id = *capability_id;
            }
            if !policy_grants.insert((
                projected.policy_id,
                *capability_id,
                origin_tag(projected.origin_kind),
                projected.origin_id,
            )) {
                return Err(SplitError::Duplicate);
            }
            plan.policy_grants.push(projected);
        }
    }
    plan.candidates.sort_by(|a, b| {
        (a.tier_id, a.capability_id, &a.upstream_model).cmp(&(
            b.tier_id,
            b.capability_id,
            &b.upstream_model,
        ))
    });
    plan.api_key_grants.sort_by_key(|row| {
        (
            row.api_key_id,
            row.capability_id,
            origin_tag(row.origin_kind),
            row.origin_id,
        )
    });
    plan.policy_grants.sort_by_key(|row| {
        (
            row.policy_id,
            row.capability_id,
            origin_tag(row.origin_kind),
            row.origin_id,
        )
    });
    Ok(plan)
}

fn unique<'a, T>(
    rows: impl Iterator<Item = (Uuid, &'a T)>,
) -> Result<BTreeMap<Uuid, &'a T>, SplitError> {
    let mut index = BTreeMap::new();
    for (id, row) in rows {
        if index.insert(id, row).is_some() {
            return Err(SplitError::Duplicate);
        }
    }
    Ok(index)
}

fn default_operation(operation: ApiOperation) -> Operation {
    match operation {
        ApiOperation::ChatCompletions => Operation::ChatCompletion,
        ApiOperation::Responses => Operation::Responses,
        ApiOperation::ResponsesWebSocket => Operation::ResponsesWs,
        ApiOperation::StandaloneWebSearch => Operation::WebSearch,
        ApiOperation::ImagesEdit => Operation::ImagesEdit,
        ApiOperation::ImagesGeneration => Operation::ImagesGeneration,
    }
}

fn capability_operations(capability: &ChannelCapabilityRecord) -> Vec<Operation> {
    if capability.settings.operation != ApiOperation::Responses {
        return vec![default_operation(capability.settings.operation)];
    }
    let transports = &capability.settings.transports;
    let mut operations = Vec::new();
    if transports.iter().any(|transport| {
        matches!(
            transport,
            CapabilityTransport::HttpJson | CapabilityTransport::HttpSse
        )
    }) {
        operations.push(Operation::Responses);
    }
    if transports.contains(&CapabilityTransport::Websocket) {
        operations.push(Operation::ResponsesWs);
    }
    operations
}

fn origin_tag(kind: GrantOriginKind) -> u8 {
    match kind {
        GrantOriginKind::Group => 0,
        GrantOriginKind::Channel => 1,
        GrantOriginKind::Capability => 2,
    }
}

fn sibling_id(kind: &[u8], source_id: Uuid, ids: &mut HashSet<Uuid>) -> Result<Uuid, SplitError> {
    let mut hash = Sha256::new();
    hash.update(b"ai-gateway:operation-split:v1:");
    hash.update(kind);
    hash.update(source_id.as_bytes());
    let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().expect("SHA-256 length");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let id = Uuid::from_bytes(bytes);
    if !ids.insert(id) {
        return Err(SplitError::IdentityCollision);
    }
    Ok(id)
}

#[cfg(test)]
mod tests;
