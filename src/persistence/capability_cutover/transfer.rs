//! Pure, backend-independent transfer of the persisted stage-1 configuration
//! into the canonical capability topology and immutable identity history.
//!
//! The transformer never touches the database and never carries authentication
//! material: static secrets, OAuth tokens, and refresh state stay in the
//! credential tables. It reuses [`CapabilityCutoverIndex`] for both operation
//! rule and target grant planning, and it fails closed whenever a legacy draft
//! cannot be represented without widening or silently dropping authority.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::legacy_settings::{
    Capability as ChannelCapabilityRecord, CapabilitySettings, LogicalChannelRecord,
    RoutingGroupRecord, Topology as UpstreamTopologyRecords, operation_name,
};
use super::{
    CapabilityCutoverError, CapabilityCutoverIndex, CapabilityGrantOrigin, LegacyCapabilityTarget,
    LegacyChannelTarget, LegacyGroupTarget,
};
use crate::domain::{
    ApiFormat, ApiOperation, CapabilityTransport, ConnectorKind, RequestCompression,
};
use crate::persistence::upstream_topology::{
    ApiKeyCapabilityGrantRecord, ApiKeyPolicyCapabilityGrantRecord, GrantOriginKind,
    OperationCandidateRecord, OperationRuleRecord, OperationTierRecord, UpstreamAccessRecord,
};
use crate::persistence::{ModelRoutingProfileBinding, ModelRuleRecord};

/// Raw persisted channel-group row needed by the transfer.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyGroup {
    pub id: Uuid,
    pub name: String,
    pub api_format: ApiFormat,
    pub connector_kind: ConnectorKind,
    pub connector_pool_id: Option<Uuid>,
    pub enabled: bool,
    pub sharing_only: bool,
    pub request_compression: RequestCompression,
    pub status_statistics_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Raw persisted channel row. `enabled` is the stored column, never the
/// credential-filtered projection the legacy control-plane loader produces.
/// It intentionally carries no authentication material.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyChannel {
    pub id: Uuid,
    pub channel_group_id: Uuid,
    pub api_format: ApiFormat,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub auto_disabled: bool,
    pub auto_disabled_reason: Option<String>,
    pub auto_disable_allowed: bool,
    pub billing_multiplier: Decimal,
    pub proxy_id: Option<Uuid>,
    pub config_template_id: Option<Uuid>,
    pub override_document: Value,
    pub connect_timeout_ms: Option<i32>,
    pub response_header_timeout_ms: Option<i32>,
    pub stream_idle_timeout_ms: Option<i32>,
    pub credential_id: Option<Uuid>,
    pub credential_binding_revision: Uuid,
    pub supports_websocket: bool,
    pub supports_standalone_web_search: bool,
    pub available_models: Vec<String>,
    pub test_model: Option<String>,
    pub test_pricing_model_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One Codex managed projection row (`codex_oauth_credential_channels`).
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyProjection {
    pub credential_id: Uuid,
    pub api_format: ApiFormat,
    pub channel_id: Uuid,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodexCredentialLifecycle {
    pub id: Uuid,
    pub enabled: bool,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// Persisted target arrays of one API key.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyKeyTargets {
    pub key_id: Uuid,
    pub allowed_api_formats: Vec<ApiFormat>,
    #[serde(default)]
    pub allowed_group_ids: Vec<Uuid>,
    #[serde(default)]
    pub allowed_channel_ids: Vec<Uuid>,
}

/// Persisted target arrays of one API-key policy.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPolicyTargets {
    pub policy_id: Uuid,
    #[serde(default)]
    pub allowed_group_ids: Vec<Uuid>,
    #[serde(default)]
    pub allowed_channel_ids: Vec<Uuid>,
}

/// Complete serde input of one transfer run. Model rules and routing profiles
/// are supplied separately as existing typed records.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityCutoverInput {
    pub groups: Vec<LegacyGroup>,
    pub channels: Vec<LegacyChannel>,
    #[serde(default)]
    pub projections: Vec<LegacyProjection>,
    #[serde(default)]
    pub codex_credentials: Vec<CodexCredentialLifecycle>,
    #[serde(default)]
    pub keys: Vec<LegacyKeyTargets>,
    #[serde(default)]
    pub policies: Vec<LegacyPolicyTargets>,
    pub rule_identities: Vec<LegacyRuleIdentity>,
    /// Single deterministic timestamp for rows the legacy schema does not
    /// timestamp (operation rules, tiers, grants) and the cutover revision seed.
    pub cutover_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRuleIdentity {
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleIdentityRegistryRecord {
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub canonical_rule_id: Option<Uuid>,
}

/// Read-only historical group identity. Never editable, authorizable, or
/// routable.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupIdentityRegistryRecord {
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub canonical_group_id: Option<Uuid>,
}

/// Read-only historical channel identity. Registry ids stay the original
/// dispatch identities: legacy physical channel UUIDs or replacement
/// capability UUIDs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelIdentityRegistryRecord {
    pub id: Uuid,
    pub label: String,
    pub created_at: DateTime<Utc>,
    pub canonical_channel_id: Option<Uuid>,
    pub codex_credential_id: Option<Uuid>,
    pub capability_id: Option<Uuid>,
}

/// Canonical rows plus the immutable identity registries that keep every
/// legacy UUID resolvable after the legacy tables are retired.
#[derive(Clone, Debug, Default)]
pub struct CapabilityCutoverTransfer {
    pub topology: UpstreamTopologyRecords,
    pub group_identity_registry: Vec<GroupIdentityRegistryRecord>,
    pub channel_identity_registry: Vec<ChannelIdentityRegistryRecord>,
    pub rule_identity_registry: Vec<RuleIdentityRegistryRecord>,
}

/// Fail-closed transfer errors carry only stable identifiers.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityCutoverTransferError {
    #[error("legacy capability topology is inconsistent")]
    InvalidTopology,
    #[error("legacy channel {channel_id} references unknown channel group {group_id}")]
    UnknownGroup { channel_id: Uuid, group_id: Uuid },
    #[error("legacy Codex channel {channel_id} has no matching identity projection")]
    InvalidCodexBinding { channel_id: Uuid },
    #[error("projected Codex credential {credential_id} has no lifecycle row")]
    MissingCredentialLifecycle { credential_id: Uuid },
    #[error("Codex connector pool {pool_id} is missing a Responses or Images projection")]
    IncompleteCodexPool { pool_id: Uuid },
    #[error("Codex connector pool {pool_id} projections disagree on network configuration")]
    ConflictingNetworkConfig { pool_id: Uuid },
    #[error("legacy capability {channel_id} for {operation:?} has unsupported settings")]
    InvalidCapability {
        channel_id: Uuid,
        operation: ApiOperation,
    },
    #[error("model rule {rule_id} has no routing profile for model {model_id}")]
    MissingProfile { rule_id: Uuid, model_id: Uuid },
    #[error("multiple routing profiles bind model {model_id}")]
    DuplicateProfile { model_id: Uuid },
    #[error("duplicate legacy {kind} id")]
    DuplicateId { kind: &'static str },
    #[error(transparent)]
    Routing(#[from] CapabilityCutoverError),
}

struct PlannedChannel<'a> {
    channel: &'a LegacyChannel,
    group: &'a LegacyGroup,
    connector: ConnectorKind,
    canonical_group: Uuid,
    logical_id: Uuid,
    capabilities: Vec<(ApiOperation, Uuid)>,
}

pub fn transfer(
    input: &CapabilityCutoverInput,
    model_rules: &[ModelRuleRecord],
    profiles: &[ModelRoutingProfileBinding],
) -> Result<CapabilityCutoverTransfer, CapabilityCutoverTransferError> {
    let cutover_at = input.cutover_at;
    let groups = group_index(&input.groups)?;
    let responses_by_pool = responses_group_by_pool(&input.groups)?;
    let projections = projection_index(&input.projections)?;
    let mut credentials = HashMap::new();
    for credential in &input.codex_credentials {
        if credentials.insert(credential.id, credential).is_some() {
            return Err(CapabilityCutoverTransferError::DuplicateId {
                kind: "codex credential",
            });
        }
    }

    let mut planned = Vec::with_capacity(input.channels.len());
    let mut seen_channels = HashSet::new();
    for channel in &input.channels {
        if !seen_channels.insert(channel.id) {
            return Err(CapabilityCutoverTransferError::DuplicateId { kind: "channel" });
        }
        let group = groups.get(&channel.channel_group_id).copied().ok_or(
            CapabilityCutoverTransferError::UnknownGroup {
                channel_id: channel.id,
                group_id: channel.channel_group_id,
            },
        )?;
        if group.api_format != channel.api_format {
            return Err(CapabilityCutoverTransferError::InvalidTopology);
        }
        let connector = group.connector_kind;
        let canonical_group = canonical_group(group, &responses_by_pool)?;
        let logical_id = match connector {
            ConnectorKind::OpenAiCompatible => channel.id,
            ConnectorKind::CodexOauth => {
                let credential_id = validate_projection(&projections, channel)?;
                let lifecycle = credentials.get(&credential_id).ok_or(
                    CapabilityCutoverTransferError::MissingCredentialLifecycle { credential_id },
                )?;
                if channel.credential_id != Some(credential_id)
                    && !(channel.credential_id.is_none()
                        && (channel.deleted_at.is_some() || lifecycle.deleted_at.is_some()))
                {
                    return Err(CapabilityCutoverTransferError::InvalidCodexBinding {
                        channel_id: channel.id,
                    });
                }
                credential_id
            }
        };
        planned.push(PlannedChannel {
            channel,
            group,
            connector,
            canonical_group,
            logical_id,
            capabilities: capability_set(channel),
        });
    }

    let mut members: BTreeMap<Uuid, Vec<&PlannedChannel<'_>>> = BTreeMap::new();
    for channel in &planned {
        members.entry(channel.logical_id).or_default().push(channel);
    }

    let mut routing_groups = build_routing_groups(&input.groups, &responses_by_pool)?;

    let mut upstream_accesses = Vec::new();
    let mut logical_channels = Vec::new();
    let mut logical_deleted = HashMap::new();
    for (logical_id, group_members) in &members {
        let first = group_members[0];
        let connector = first.connector;
        let (identity, network, deleted) = match connector {
            ConnectorKind::OpenAiCompatible => {
                (first.channel, first.channel, first.channel.deleted_at)
            }
            ConnectorKind::CodexOauth => {
                let lifecycle = credentials.get(logical_id).ok_or(
                    CapabilityCutoverTransferError::MissingCredentialLifecycle {
                        credential_id: *logical_id,
                    },
                )?;
                let pool_id = first
                    .group
                    .connector_pool_id
                    .ok_or(CapabilityCutoverTransferError::InvalidTopology)?;
                let responses = group_members
                    .iter()
                    .copied()
                    .find(|member| member.channel.api_format == ApiFormat::OpenAiResponses)
                    .ok_or(CapabilityCutoverTransferError::IncompleteCodexPool { pool_id })?;
                if responses.channel.id != *logical_id {
                    return Err(CapabilityCutoverTransferError::InvalidCodexBinding {
                        channel_id: responses.channel.id,
                    });
                }
                let live = group_members
                    .iter()
                    .copied()
                    .filter(|member| member.channel.deleted_at.is_none())
                    .collect::<Vec<_>>();
                if let Some(base) = live.first()
                    && lifecycle.deleted_at.is_none()
                {
                    for other in &live[1..] {
                        if !same_network(base.channel, other.channel) {
                            return Err(CapabilityCutoverTransferError::ConflictingNetworkConfig {
                                pool_id,
                            });
                        }
                    }
                }
                let network = live.first().copied().unwrap_or(responses);
                (
                    responses.channel,
                    network.channel,
                    match (responses.channel.deleted_at, lifecycle.deleted_at) {
                        (Some(channel), Some(credential)) => Some(channel.min(credential)),
                        (channel, credential) => channel.or(credential),
                    },
                )
            }
        };

        logical_deleted.insert(*logical_id, deleted);
        let access = access_id(*logical_id);
        upstream_accesses.push(UpstreamAccessRecord {
            id: access,
            name: identity.name.clone(),
            connector_kind: connector,
            base_url: if deleted.is_some() {
                "https://deleted.invalid".into()
            } else {
                network.base_url.clone()
            },
            proxy_id: deleted.is_none().then_some(network.proxy_id).flatten(),
            connect_timeout_ms: deleted
                .is_none()
                .then_some(network.connect_timeout_ms)
                .flatten(),
            response_header_timeout_ms: deleted
                .is_none()
                .then_some(network.response_header_timeout_ms)
                .flatten(),
            stream_idle_timeout_ms: deleted
                .is_none()
                .then_some(network.stream_idle_timeout_ms)
                .flatten(),
            // Access enablement is independent of credential state; a live
            // access stays enabled even when its credential is disabled.
            enabled: deleted.is_none(),
            revision: access_revision(access),
            created_at: network.created_at,
            updated_at: network.updated_at,
            deleted_at: deleted,
        });
        logical_channels.push(LogicalChannelRecord {
            id: *logical_id,
            group_id: first.canonical_group,
            access_id: access,
            credential_id: if deleted.is_some() {
                None
            } else {
                match connector {
                    ConnectorKind::OpenAiCompatible => identity.credential_id,
                    ConnectorKind::CodexOauth => Some(*logical_id),
                }
            },
            name: identity.name.clone(),
            enabled: match connector {
                ConnectorKind::OpenAiCompatible => identity.enabled,
                // Codex enablement is expressed per capability so the group and
                // credential gates never collapse into the logical channel.
                ConnectorKind::CodexOauth => true,
            } && deleted.is_none(),
            binding_revision: binding_revision(*logical_id),
            created_at: identity.created_at,
            updated_at: identity.updated_at,
            deleted_at: deleted,
        });
    }

    let mut channel_capabilities = Vec::new();
    let mut capability_meta = HashMap::new();
    for channel in &planned {
        let logical_deleted_at = *logical_deleted
            .get(&channel.logical_id)
            .expect("logical channel planned");
        for (operation, capability_id) in &channel.capabilities {
            let deleted = channel.channel.deleted_at.or(logical_deleted_at);
            let enabled = match channel.connector {
                ConnectorKind::OpenAiCompatible => true,
                ConnectorKind::CodexOauth => channel.channel.enabled && channel.group.enabled,
            } && deleted.is_none();
            let probe = if channel.connector == ConnectorKind::OpenAiCompatible
                && matches!(
                    operation,
                    ApiOperation::ChatCompletions | ApiOperation::Responses
                ) {
                (
                    channel.channel.test_model.clone(),
                    channel.channel.test_pricing_model_id,
                )
            } else {
                (None, None)
            };
            let settings = CapabilitySettings {
                operation: *operation,
                transports: capability_transports(
                    channel.connector,
                    *operation,
                    channel.channel.supports_websocket,
                ),
                enabled,
                available_models: channel.channel.available_models.clone(),
                request_compression: if *operation == ApiOperation::Responses {
                    channel.group.request_compression
                } else {
                    RequestCompression::Default
                },
                test_model: probe.0,
                test_pricing_model_id: probe.1,
                auto_disable_allowed: channel.channel.auto_disable_allowed,
            };
            settings.validate(channel.connector).map_err(|_| {
                CapabilityCutoverTransferError::InvalidCapability {
                    channel_id: channel.channel.id,
                    operation: *operation,
                }
            })?;
            channel_capabilities.push(ChannelCapabilityRecord {
                id: *capability_id,
                channel_id: channel.logical_id,
                settings,
                auto_disabled: channel.channel.auto_disabled && deleted.is_none(),
                auto_disable_reason: if deleted.is_none() {
                    channel.channel.auto_disabled_reason.clone()
                } else {
                    None
                },
                auto_disable_at: None,
                status_statistics_enabled: channel.group.status_statistics_enabled,
                config_template_id: channel.channel.config_template_id,
                override_document: channel.channel.override_document.clone(),
                billing_multiplier: channel.channel.billing_multiplier,
                revision: capability_revision(*capability_id),
                created_at: channel.channel.created_at,
                updated_at: channel.channel.updated_at,
                deleted_at: deleted,
            });
            capability_meta.insert(*capability_id, (channel.logical_id, channel.connector));
        }
    }

    let full_index = capability_index(&input.groups, &responses_by_pool, &planned)?;
    let live_capabilities = channel_capabilities
        .iter()
        .filter_map(|capability| capability.deleted_at.is_none().then_some(capability.id))
        .collect::<HashSet<_>>();
    let mut rule_plans = full_index.rewrite_rules(model_rules)?;
    for rule in &mut rule_plans {
        for tier in &mut rule.routing_tiers {
            tier.candidates
                .retain(|candidate| live_capabilities.contains(&candidate.capability_id));
        }
        rule.routing_tiers
            .retain(|tier| !tier.candidates.is_empty());
        if rule.routing_tiers.is_empty() {
            rule.enabled = false;
        }
    }

    let profiles_by_model = profile_index(profiles)?;
    let mut operation_rules = Vec::new();
    let mut operation_tiers = Vec::new();
    let mut operation_candidates = Vec::new();
    let mut rule_identities = BTreeMap::new();
    for identity in &input.rule_identities {
        if rule_identities
            .insert(
                identity.id,
                RuleIdentityRegistryRecord {
                    id: identity.id,
                    label: identity.label.clone(),
                    created_at: identity.created_at,
                    canonical_rule_id: None,
                },
            )
            .is_some()
        {
            return Err(CapabilityCutoverTransferError::DuplicateId {
                kind: "rule identity",
            });
        }
    }
    for plan in rule_plans {
        let profile = profiles_by_model.get(&plan.model_id).copied().ok_or(
            CapabilityCutoverTransferError::MissingProfile {
                rule_id: plan.legacy_rule_id,
                model_id: plan.model_id,
            },
        )?;
        operation_rules.push(OperationRuleRecord {
            id: plan.id,
            model_routing_profile_id: profile,
            operation: plan.operation,
            enabled: plan.enabled,
            created_at: cutover_at,
            updated_at: cutover_at,
        });
        let legacy = model_rules
            .iter()
            .find(|rule| rule.id == plan.legacy_rule_id)
            .ok_or(CapabilityCutoverTransferError::InvalidTopology)?;
        let identity =
            rule_identities
                .entry(plan.id)
                .or_insert_with(|| RuleIdentityRegistryRecord {
                    id: plan.id,
                    label: legacy.client_model.clone(),
                    created_at: cutover_at,
                    canonical_rule_id: None,
                });
        identity.canonical_rule_id = Some(plan.id);
        for tier in &plan.routing_tiers {
            let tier_id = tier_id(plan.id, tier.priority);
            operation_tiers.push(OperationTierRecord {
                id: tier_id,
                rule_id: plan.id,
                operation: plan.operation,
                priority: tier.priority,
                strategy: tier.selection_strategy.clone(),
            });
            for candidate in &tier.candidates {
                operation_candidates.push(OperationCandidateRecord {
                    tier_id,
                    operation: plan.operation,
                    capability_id: candidate.capability_id,
                    upstream_model: candidate.upstream_model.clone(),
                    weight: candidate.weight,
                });
            }
        }
    }

    let mut api_key_grants = Vec::new();
    for key in &input.keys {
        let grants = full_index.rewrite_target_grants(
            &key.allowed_group_ids,
            &key.allowed_channel_ids,
            Some(&key.allowed_api_formats),
        )?;
        for grant in grants {
            let (origin_kind, origin_id) = grant_origin(grant.origin);
            api_key_grants.push(ApiKeyCapabilityGrantRecord {
                api_key_id: key.key_id,
                capability_id: grant.capability_id,
                origin_kind,
                origin_id,
                created_at: cutover_at,
            });
        }
    }
    let mut policy_grants = Vec::new();
    for policy in &input.policies {
        let grants = full_index.rewrite_target_grants(
            &policy.allowed_group_ids,
            &policy.allowed_channel_ids,
            None,
        )?;
        for grant in grants {
            let (origin_kind, origin_id) = grant_origin(grant.origin);
            policy_grants.push(ApiKeyPolicyCapabilityGrantRecord {
                policy_id: policy.policy_id,
                capability_id: grant.capability_id,
                origin_kind,
                origin_id,
                created_at: cutover_at,
            });
        }
    }

    let mut group_identity_registry = Vec::with_capacity(input.groups.len());
    for group in &input.groups {
        group_identity_registry.push(GroupIdentityRegistryRecord {
            id: group.id,
            label: group.name.clone(),
            created_at: group.created_at,
            canonical_group_id: Some(canonical_group(group, &responses_by_pool)?),
        });
    }
    group_identity_registry.sort_by_key(|row| row.id);

    let logical_lookup = logical_channels
        .iter()
        .map(|channel| (channel.id, channel))
        .collect::<HashMap<_, _>>();
    let mut channel_identity_registry = Vec::new();
    for channel in &planned {
        // A legacy channel that maps to more than one operation cannot link to
        // a single capability without guessing the replacement.
        let capability_id = match channel.capabilities.as_slice() {
            [(_, id)] => Some(*id),
            _ => None,
        };
        channel_identity_registry.push(ChannelIdentityRegistryRecord {
            id: channel.channel.id,
            label: channel.channel.name.clone(),
            created_at: channel.channel.created_at,
            canonical_channel_id: Some(channel.logical_id),
            codex_credential_id: (channel.connector == ConnectorKind::CodexOauth)
                .then_some(channel.logical_id),
            capability_id,
        });
    }
    for capability in &channel_capabilities {
        let (logical_id, connector) = capability_meta[&capability.id];
        let logical = logical_lookup[&logical_id];
        channel_identity_registry.push(ChannelIdentityRegistryRecord {
            id: capability.id,
            label: logical.name.clone(),
            created_at: logical.created_at,
            canonical_channel_id: Some(logical_id),
            codex_credential_id: (connector == ConnectorKind::CodexOauth).then_some(logical_id),
            capability_id: Some(capability.id),
        });
    }
    channel_identity_registry.sort_by_key(|row| row.id);

    routing_groups.sort_by_key(|record| record.id);
    upstream_accesses.sort_by_key(|record| record.id);
    logical_channels.sort_by_key(|record| record.id);
    channel_capabilities.sort_by_key(|record| record.id);
    operation_rules.sort_by_key(|record| record.id);
    operation_tiers.sort_by_key(|record| record.id);
    operation_candidates.sort_by_key(|record| {
        (
            record.tier_id,
            record.capability_id,
            record.upstream_model.clone(),
        )
    });
    api_key_grants.sort_by_key(|record| {
        (
            record.api_key_id,
            record.capability_id,
            origin_kind_rank(record.origin_kind),
            record.origin_id,
        )
    });
    policy_grants.sort_by_key(|record| {
        (
            record.policy_id,
            record.capability_id,
            origin_kind_rank(record.origin_kind),
            record.origin_id,
        )
    });

    Ok(CapabilityCutoverTransfer {
        rule_identity_registry: rule_identities.into_values().collect(),
        topology: UpstreamTopologyRecords {
            routing_groups,
            upstream_accesses,
            logical_channels,
            channel_capabilities,
            operation_rules,
            operation_tiers,
            operation_candidates,
            api_key_grants,
            policy_grants,
        },
        group_identity_registry,
        channel_identity_registry,
    })
}

fn group_index(
    groups: &[LegacyGroup],
) -> Result<HashMap<Uuid, &LegacyGroup>, CapabilityCutoverTransferError> {
    let mut index = HashMap::with_capacity(groups.len());
    for group in groups {
        if index.insert(group.id, group).is_some() {
            return Err(CapabilityCutoverTransferError::DuplicateId { kind: "group" });
        }
    }
    Ok(index)
}

fn responses_group_by_pool(
    groups: &[LegacyGroup],
) -> Result<HashMap<Uuid, Uuid>, CapabilityCutoverTransferError> {
    let mut responses = HashMap::new();
    for group in groups {
        if group.connector_kind != ConnectorKind::CodexOauth {
            continue;
        }
        let pool = group
            .connector_pool_id
            .ok_or(CapabilityCutoverTransferError::InvalidTopology)?;
        if group.api_format == ApiFormat::OpenAiResponses
            && responses.insert(pool, group.id).is_some()
        {
            return Err(CapabilityCutoverTransferError::InvalidTopology);
        }
    }
    Ok(responses)
}

fn canonical_group(
    group: &LegacyGroup,
    responses_by_pool: &HashMap<Uuid, Uuid>,
) -> Result<Uuid, CapabilityCutoverTransferError> {
    match group.connector_kind {
        ConnectorKind::OpenAiCompatible => Ok(group.id),
        ConnectorKind::CodexOauth => {
            let pool = group
                .connector_pool_id
                .ok_or(CapabilityCutoverTransferError::InvalidTopology)?;
            responses_by_pool
                .get(&pool)
                .copied()
                .ok_or(CapabilityCutoverTransferError::IncompleteCodexPool { pool_id: pool })
        }
    }
}

fn build_routing_groups(
    groups: &[LegacyGroup],
    responses_by_pool: &HashMap<Uuid, Uuid>,
) -> Result<Vec<RoutingGroupRecord>, CapabilityCutoverTransferError> {
    let mut chosen: HashMap<Uuid, &LegacyGroup> = HashMap::new();
    for group in groups {
        let id = canonical_group(group, responses_by_pool)?;
        match chosen.get(&id) {
            Some(existing) if existing.api_format == ApiFormat::OpenAiResponses => {}
            _ => {
                chosen.insert(id, group);
            }
        }
    }
    let mut records = chosen
        .into_iter()
        .map(|(id, group)| {
            let deleted = group.deleted_at;
            let enabled = match group.connector_kind {
                ConnectorKind::OpenAiCompatible => group.enabled,
                // The Codex pair's group gate moves down to each capability.
                ConnectorKind::CodexOauth => true,
            } && deleted.is_none();
            RoutingGroupRecord {
                id,
                name: group.name.clone(),
                enabled,
                sharing_only: group.sharing_only,
                created_at: group.created_at,
                updated_at: group.updated_at,
                deleted_at: deleted,
            }
        })
        .collect::<Vec<_>>();
    records.sort_by_key(|record| record.id);
    Ok(records)
}

fn capability_index(
    groups: &[LegacyGroup],
    responses_by_pool: &HashMap<Uuid, Uuid>,
    planned: &[PlannedChannel<'_>],
) -> Result<CapabilityCutoverIndex, CapabilityCutoverTransferError> {
    let mut index_groups = Vec::with_capacity(groups.len());
    for group in groups {
        index_groups.push(LegacyGroupTarget {
            id: group.id,
            management_group_id: canonical_group(group, responses_by_pool)?,
            api_format: group.api_format,
        });
    }
    let mut index_channels = Vec::new();
    for channel in planned {
        index_channels.push(LegacyChannelTarget {
            id: channel.channel.id,
            group_id: channel.channel.channel_group_id,
            logical_channel_id: channel.logical_id,
            api_format: channel.channel.api_format,
            capabilities: channel
                .capabilities
                .iter()
                .map(|(operation, id)| LegacyCapabilityTarget {
                    id: *id,
                    operation: *operation,
                })
                .collect(),
        });
    }
    Ok(CapabilityCutoverIndex::new(index_groups, index_channels)?)
}

fn projection_index(
    projections: &[LegacyProjection],
) -> Result<HashMap<(Uuid, ApiFormat), Uuid>, CapabilityCutoverTransferError> {
    let mut index = HashMap::with_capacity(projections.len());
    let mut channel_ids = HashSet::new();
    for projection in projections {
        if !channel_ids.insert(projection.channel_id)
            || index
                .insert(
                    (projection.credential_id, projection.api_format),
                    projection.channel_id,
                )
                .is_some()
        {
            return Err(CapabilityCutoverTransferError::DuplicateId { kind: "projection" });
        }
    }
    Ok(index)
}

fn validate_projection(
    projections: &HashMap<(Uuid, ApiFormat), Uuid>,
    channel: &LegacyChannel,
) -> Result<Uuid, CapabilityCutoverTransferError> {
    projections
        .iter()
        .find_map(|((credential_id, format), channel_id)| {
            (*channel_id == channel.id && *format == channel.api_format).then_some(*credential_id)
        })
        .ok_or(CapabilityCutoverTransferError::InvalidCodexBinding {
            channel_id: channel.id,
        })
}

fn same_network(left: &LegacyChannel, right: &LegacyChannel) -> bool {
    left.base_url == right.base_url
        && left.proxy_id == right.proxy_id
        && left.connect_timeout_ms == right.connect_timeout_ms
        && left.response_header_timeout_ms == right.response_header_timeout_ms
        && left.stream_idle_timeout_ms == right.stream_idle_timeout_ms
}

fn capability_set(channel: &LegacyChannel) -> Vec<(ApiOperation, Uuid)> {
    let mut operations = match channel.api_format {
        ApiFormat::OpenAiChatCompletions => vec![ApiOperation::ChatCompletions],
        ApiFormat::OpenAiResponses => vec![ApiOperation::Responses],
        ApiFormat::OpenAiImages => vec![ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit],
    };
    if channel.api_format == ApiFormat::OpenAiResponses && channel.supports_standalone_web_search {
        operations.push(ApiOperation::StandaloneWebSearch);
    }
    operations
        .into_iter()
        .map(|operation| (operation, capability_id(channel.id, operation)))
        .collect()
}

fn capability_transports(
    connector: ConnectorKind,
    operation: ApiOperation,
    websocket: bool,
) -> Vec<CapabilityTransport> {
    match operation {
        ApiOperation::ResponsesWebSocket => vec![CapabilityTransport::Websocket],
        ApiOperation::ChatCompletions => {
            vec![CapabilityTransport::HttpJson, CapabilityTransport::HttpSse]
        }
        ApiOperation::Responses => {
            let mut transports = match connector {
                ConnectorKind::OpenAiCompatible => {
                    vec![CapabilityTransport::HttpJson, CapabilityTransport::HttpSse]
                }
                ConnectorKind::CodexOauth => vec![CapabilityTransport::HttpSse],
            };
            if websocket {
                transports.push(CapabilityTransport::Websocket);
            }
            transports
        }
        ApiOperation::StandaloneWebSearch | ApiOperation::ImagesGeneration => {
            vec![CapabilityTransport::HttpJson]
        }
        ApiOperation::ImagesEdit => vec![CapabilityTransport::Multipart],
    }
}

fn origin_kind_rank(kind: GrantOriginKind) -> u8 {
    match kind {
        GrantOriginKind::Group => 0,
        GrantOriginKind::Channel => 1,
        GrantOriginKind::Capability => 2,
    }
}

fn grant_origin(origin: CapabilityGrantOrigin) -> (GrantOriginKind, Uuid) {
    match origin {
        CapabilityGrantOrigin::Group(id) => (GrantOriginKind::Group, id),
        CapabilityGrantOrigin::Channel(id) => (GrantOriginKind::Channel, id),
    }
}

fn profile_index(
    profiles: &[ModelRoutingProfileBinding],
) -> Result<HashMap<Uuid, Uuid>, CapabilityCutoverTransferError> {
    let mut index = HashMap::with_capacity(profiles.len());
    for profile in profiles {
        if index.insert(profile.model_id, profile.id).is_some() {
            return Err(CapabilityCutoverTransferError::DuplicateProfile {
                model_id: profile.model_id,
            });
        }
    }
    Ok(index)
}

const CAPABILITY_NAMESPACE: &[u8] = b"ai-gateway:capability-cutover:capability:v1:";
const ACCESS_NAMESPACE: &[u8] = b"ai-gateway:capability-cutover:access:v1:";
const BINDING_NAMESPACE: &[u8] = b"ai-gateway:capability-cutover:binding:v1:";
const ACCESS_REVISION_NAMESPACE: &[u8] = b"ai-gateway:capability-cutover:access-revision:v1:";
const CAPABILITY_REVISION_NAMESPACE: &[u8] =
    b"ai-gateway:capability-cutover:capability-revision:v1:";
const TIER_NAMESPACE: &[u8] = b"ai-gateway:capability-cutover:tier:v1:";

fn namespace_uuid(namespace: &[u8], id: Uuid, label: &str) -> Uuid {
    let mut hash = Sha256::new();
    hash.update(namespace);
    hash.update(id.as_bytes());
    hash.update(label.as_bytes());
    let mut bytes: [u8; 16] = hash.finalize()[..16].try_into().expect("SHA-256 length");
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn capability_id(channel_id: Uuid, operation: ApiOperation) -> Uuid {
    namespace_uuid(CAPABILITY_NAMESPACE, channel_id, operation_name(operation))
}

fn access_id(logical_id: Uuid) -> Uuid {
    namespace_uuid(ACCESS_NAMESPACE, logical_id, "")
}

fn binding_revision(logical_id: Uuid) -> Uuid {
    namespace_uuid(BINDING_NAMESPACE, logical_id, "")
}

fn access_revision(access_id: Uuid) -> Uuid {
    namespace_uuid(ACCESS_REVISION_NAMESPACE, access_id, "")
}

fn capability_revision(capability_id: Uuid) -> Uuid {
    namespace_uuid(CAPABILITY_REVISION_NAMESPACE, capability_id, "")
}

fn tier_id(rule_id: Uuid, priority: i32) -> Uuid {
    namespace_uuid(TIER_NAMESPACE, rule_id, &priority.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::persistence::{ModelRuleRouteCandidate, ModelRuleRoutingTier};

    fn id(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn group(value: u128, format: ApiFormat) -> LegacyGroup {
        LegacyGroup {
            id: id(value),
            name: format!("group-{value}"),
            api_format: format,
            connector_kind: ConnectorKind::OpenAiCompatible,
            connector_pool_id: None,
            enabled: true,
            sharing_only: false,
            request_compression: RequestCompression::Default,
            status_statistics_enabled: false,
            created_at: ts(1),
            updated_at: ts(2),
            deleted_at: None,
        }
    }

    fn channel(value: u128, group_id: u128, format: ApiFormat, enabled: bool) -> LegacyChannel {
        LegacyChannel {
            id: id(value),
            channel_group_id: id(group_id),
            api_format: format,
            name: format!("channel-{value}"),
            base_url: "https://up.example".to_owned(),
            enabled,
            auto_disabled: false,
            auto_disabled_reason: None,
            auto_disable_allowed: false,
            billing_multiplier: Decimal::ONE,
            proxy_id: None,
            config_template_id: None,
            override_document: json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            credential_id: None,
            credential_binding_revision: Uuid::nil(),
            supports_websocket: false,
            supports_standalone_web_search: false,
            available_models: Vec::new(),
            test_model: None,
            test_pricing_model_id: None,
            created_at: ts(1),
            updated_at: ts(2),
            deleted_at: None,
        }
    }

    fn input(groups: Vec<LegacyGroup>, channels: Vec<LegacyChannel>) -> CapabilityCutoverInput {
        let codex_credentials = channels
            .iter()
            .filter_map(|channel| {
                (channel.api_format == ApiFormat::OpenAiResponses
                    && groups.iter().any(|group| {
                        group.id == channel.channel_group_id
                            && group.connector_kind == ConnectorKind::CodexOauth
                    }))
                .then_some(CodexCredentialLifecycle {
                    id: channel.id,
                    enabled: true,
                    deleted_at: None,
                })
            })
            .collect();
        CapabilityCutoverInput {
            groups,
            channels,
            projections: Vec::new(),
            codex_credentials,
            keys: Vec::new(),
            policies: Vec::new(),
            rule_identities: Vec::new(),
            cutover_at: ts(10),
        }
    }

    fn codex_groups(pool: u128, images_enabled: bool) -> Vec<LegacyGroup> {
        let mut responses = group(1, ApiFormat::OpenAiResponses);
        responses.connector_kind = ConnectorKind::CodexOauth;
        responses.connector_pool_id = Some(id(pool));
        let mut images = group(2, ApiFormat::OpenAiImages);
        images.connector_kind = ConnectorKind::CodexOauth;
        images.connector_pool_id = Some(id(pool));
        images.enabled = images_enabled;
        vec![responses, images]
    }

    fn codex_channels(
        responses_enabled: bool,
        images_enabled: bool,
        images_url: &str,
    ) -> Vec<LegacyChannel> {
        let mut responses = channel(10, 1, ApiFormat::OpenAiResponses, responses_enabled);
        responses.credential_id = Some(id(10));
        responses.supports_websocket = true;
        let mut images = channel(20, 2, ApiFormat::OpenAiImages, images_enabled);
        images.credential_id = Some(id(10));
        images.base_url = images_url.to_owned();
        vec![responses, images]
    }

    fn codex_projections() -> Vec<LegacyProjection> {
        vec![
            LegacyProjection {
                credential_id: id(10),
                api_format: ApiFormat::OpenAiResponses,
                channel_id: id(10),
            },
            LegacyProjection {
                credential_id: id(10),
                api_format: ApiFormat::OpenAiImages,
                channel_id: id(20),
            },
        ]
    }

    #[test]
    fn credential_tombstone_retires_live_codex_shells_and_keeps_history() {
        use crate::persistence::upstream_credentials::CredentialRecord;
        use crate::persistence::upstream_topology::{BaseControlPlaneRecords, resolve_runtime};

        let mut request = input(
            codex_groups(500, false),
            codex_channels(true, true, "https://up.example"),
        );
        request.projections = codex_projections();
        request.codex_credentials[0].deleted_at = Some(ts(7));
        request.codex_credentials[0].enabled = false;
        request.channels[0].proxy_id = Some(id(700));
        request.channels[0].auto_disabled = true;
        request.channels[0].auto_disabled_reason = Some("stale".into());
        let output = transfer(&request, &[], &[]).unwrap();
        let logical = &output.topology.logical_channels[0];
        assert_eq!(logical.deleted_at, Some(ts(7)));
        assert_eq!(logical.credential_id, None);
        assert!(!logical.enabled);
        let access = &output.topology.upstream_accesses[0];
        assert_eq!(access.deleted_at, Some(ts(7)));
        assert_eq!(access.proxy_id, None);
        for capability in &output.topology.channel_capabilities {
            assert_eq!(capability.deleted_at, Some(ts(7)));
            assert!(!capability.settings.enabled);
            assert!(!capability.auto_disabled);
            assert!(capability.auto_disable_reason.is_none());
        }
        for old_id in [id(10), id(20)] {
            let historical = output
                .channel_identity_registry
                .iter()
                .find(|row| row.id == old_id)
                .unwrap();
            assert_eq!(historical.codex_credential_id, Some(id(10)));
            assert_eq!(historical.canonical_channel_id, Some(id(10)));
        }
        let credential = CredentialRecord {
            id: id(10),
            name: "retired".into(),
            kind: "codex_oauth".into(),
            header_name: None,
            secret: None,
            allowed_base_urls: vec![],
            enabled: false,
            revision: id(410),
            created_at: ts(1),
            updated_at: ts(7),
            deleted_at: Some(ts(7)),
        };
        let records = resolve_runtime(
            &super::super::credential_ownership::upgrade(
                &super::super::channel_authorization::upgrade(
                    &super::super::operation_split::upgrade(&output.topology).unwrap(),
                )
                .unwrap(),
            ),
            BaseControlPlaneRecords::default(),
            &[],
            &[credential],
        )
        .unwrap();
        assert!(records.channels.is_empty());
    }

    #[test]
    fn disabled_credential_does_not_change_capability_admin_switches() {
        let mut request = input(
            codex_groups(500, false),
            codex_channels(true, true, "https://up.example"),
        );
        request.projections = codex_projections();
        request.codex_credentials[0].enabled = false;
        let output = transfer(&request, &[], &[]).unwrap();
        assert!(output.topology.logical_channels[0].enabled);
        assert!(
            capability(&output, 10, ApiOperation::Responses)
                .settings
                .enabled
        );
        assert!(
            !capability(&output, 20, ApiOperation::ImagesGeneration)
                .settings
                .enabled
        );
        request.codex_credentials.clear();
        assert!(matches!(
            transfer(&request, &[], &[]),
            Err(CapabilityCutoverTransferError::MissingCredentialLifecycle { .. })
        ));
    }

    #[test]
    fn retired_targets_are_removed_without_erasing_rule_identity_or_live_weights() {
        let mut dead = channel(10, 1, ApiFormat::OpenAiChatCompletions, false);
        dead.deleted_at = Some(ts(7));
        let live = channel(20, 1, ApiFormat::OpenAiChatCompletions, true);
        let request = input(
            vec![group(1, ApiFormat::OpenAiChatCompletions)],
            vec![dead, live],
        );
        let original = model_rule(
            ApiFormat::OpenAiChatCompletions,
            vec![tier(
                0,
                vec![candidate(10, "wire", 2), candidate(20, "wire", 7)],
            )],
        );
        let output = transfer(
            &request,
            std::slice::from_ref(&original),
            &[profile(2000, 1001)],
        )
        .unwrap();
        assert_eq!(output.topology.operation_rules[0].id, original.id);
        assert_eq!(output.topology.operation_candidates.len(), 1);
        assert_eq!(output.topology.operation_candidates[0].weight, 7);
        let mut broken = original;
        broken.routing_tiers[0].candidates[0].channel_id = id(404);
        assert!(transfer(&request, &[broken], &[profile(2000, 1001)]).is_err());
    }

    #[test]
    fn retired_rule_history_survives_without_a_live_profile() {
        let mut request = input(Vec::new(), Vec::new());
        request.rule_identities.push(LegacyRuleIdentity {
            id: id(50),
            label: "retired-model".into(),
            created_at: ts(1),
        });
        let output = transfer(&request, &[], &[]).unwrap();
        assert!(output.topology.operation_rules.is_empty());
        let history = &output.rule_identity_registry[0];
        assert_eq!(history.id, id(50));
        assert_eq!(history.created_at, ts(1));
        assert_eq!(history.label, "retired-model");
        assert_eq!(history.canonical_rule_id, None);
        request
            .rule_identities
            .push(request.rule_identities[0].clone());
        assert!(matches!(
            transfer(&request, &[], &[]),
            Err(CapabilityCutoverTransferError::DuplicateId {
                kind: "rule identity"
            })
        ));
    }

    fn candidate(channel: u128, model: &str, weight: i32) -> ModelRuleRouteCandidate {
        ModelRuleRouteCandidate {
            channel_id: id(channel),
            upstream_model: model.into(),
            weight,
        }
    }

    fn tier(priority: i32, candidates: Vec<ModelRuleRouteCandidate>) -> ModelRuleRoutingTier {
        ModelRuleRoutingTier {
            priority,
            selection_strategy: "weighted_round_robin".into(),
            candidates,
        }
    }

    fn model_rule(format: ApiFormat, routing_tiers: Vec<ModelRuleRoutingTier>) -> ModelRuleRecord {
        ModelRuleRecord {
            id: id(1000),
            client_model: "client-model".into(),
            api_format: format.as_str().into(),
            api_operation: ApiOperation::legacy_default(format),
            model_id: id(1001),
            model_enabled: true,
            model_currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: ts(1),
            input_unit_price: Decimal::ONE,
            cached_input_unit_price: Decimal::ZERO,
            cache_write_unit_price: Decimal::ZERO,
            output_unit_price: Decimal::ONE,
            advanced_billing: json!({}),
            routing_tiers,
            enabled: true,
        }
    }

    fn profile(profile_id: u128, model_id: u128) -> ModelRoutingProfileBinding {
        ModelRoutingProfileBinding {
            id: id(profile_id),
            model_id: id(model_id),
            model_enabled: true,
            model_deleted: false,
        }
    }

    fn capability(
        output: &CapabilityCutoverTransfer,
        channel: u128,
        operation: ApiOperation,
    ) -> &ChannelCapabilityRecord {
        let expected = capability_id(id(channel), operation);
        output
            .topology
            .channel_capabilities
            .iter()
            .find(|record| record.id == expected)
            .expect("capability")
    }

    #[test]
    fn codex_responses_stay_enabled_while_images_start_disabled() {
        let mut request = input(
            codex_groups(500, false),
            codex_channels(true, false, "https://up.example"),
        );
        request.projections = codex_projections();
        let output = transfer(&request, &[], &[]).expect("transfer");

        assert!(
            capability(&output, 10, ApiOperation::Responses)
                .settings
                .enabled
        );
        for operation in [ApiOperation::ImagesGeneration, ApiOperation::ImagesEdit] {
            assert!(!capability(&output, 20, operation).settings.enabled);
        }
        let logical = output
            .topology
            .logical_channels
            .iter()
            .find(|record| record.id == id(10))
            .expect("logical channel");
        assert!(logical.enabled);
        assert_eq!(logical.credential_id, Some(id(10)));
        let master = output
            .topology
            .routing_groups
            .iter()
            .find(|record| record.id == id(1))
            .expect("responses group");
        assert!(master.enabled);
        assert!(
            !output
                .topology
                .routing_groups
                .iter()
                .any(|record| record.id == id(2))
        );
    }

    #[test]
    fn codex_channel_and_group_gates_are_not_baked_into_the_logical_channel() {
        let mut request = input(
            codex_groups(500, false),
            codex_channels(false, false, "https://up.example"),
        );
        request.projections = codex_projections();
        let output = transfer(&request, &[], &[]).expect("transfer");

        let logical = output
            .topology
            .logical_channels
            .iter()
            .find(|record| record.id == id(10))
            .expect("logical channel");
        assert!(
            logical.enabled,
            "credential/channel state must not disable the logical channel"
        );
        assert!(
            !capability(&output, 10, ApiOperation::Responses)
                .settings
                .enabled
        );
    }

    #[test]
    fn ordinary_and_codex_alias_uuids_are_preserved() {
        let ordinary = transfer(
            &input(
                vec![group(1, ApiFormat::OpenAiResponses)],
                vec![channel(10, 1, ApiFormat::OpenAiResponses, true)],
            ),
            &[],
            &[],
        )
        .expect("ordinary transfer");
        let logical = ordinary
            .topology
            .logical_channels
            .iter()
            .find(|record| record.id == id(10))
            .expect("ordinary logical");
        assert_eq!(logical.access_id, access_id(id(10)));
        assert!(
            ordinary
                .channel_identity_registry
                .iter()
                .find(|record| record.id == id(10))
                .is_some_and(|record| record.canonical_channel_id == Some(id(10)))
        );

        let mut codex = input(
            codex_groups(500, false),
            codex_channels(true, false, "https://up.example"),
        );
        codex.projections = codex_projections();
        let codex = transfer(&codex, &[], &[]).expect("codex transfer");
        assert!(
            codex
                .topology
                .logical_channels
                .iter()
                .any(|record| record.id == id(10))
        );
        assert!(
            !codex
                .topology
                .logical_channels
                .iter()
                .any(|record| record.id == id(20))
        );
        let images = codex
            .channel_identity_registry
            .iter()
            .find(|record| record.id == id(20))
            .expect("images history");
        assert_eq!(images.canonical_channel_id, Some(id(10)));
        assert_eq!(images.codex_credential_id, Some(id(10)));
        assert_eq!(images.capability_id, None);
    }

    #[test]
    fn weights_operation_rules_and_key_grants_are_preserved() {
        let mut request = input(
            vec![group(1, ApiFormat::OpenAiResponses)],
            vec![channel(10, 1, ApiFormat::OpenAiResponses, true)],
        );
        request.keys = vec![LegacyKeyTargets {
            key_id: id(300),
            allowed_api_formats: vec![ApiFormat::OpenAiResponses],
            allowed_group_ids: vec![id(1)],
            allowed_channel_ids: Vec::new(),
        }];
        let rule = model_rule(
            ApiFormat::OpenAiResponses,
            vec![
                tier(
                    0,
                    vec![candidate(10, "wire-a", 2), candidate(10, "wire-b", 3)],
                ),
                tier(10, vec![candidate(10, "wire-a", 5)]),
            ],
        );
        let output = transfer(
            &request,
            std::slice::from_ref(&rule),
            &[profile(2000, 1001)],
        )
        .expect("transfer");

        assert_eq!(output.topology.operation_rules.len(), 1);
        assert_eq!(output.topology.operation_rules[0].id, rule.id);
        assert_eq!(
            output.topology.operation_rules[0].model_routing_profile_id,
            id(2000)
        );
        let mut weights = output
            .topology
            .operation_candidates
            .iter()
            .map(|record| (record.upstream_model.as_str(), record.weight))
            .collect::<Vec<_>>();
        weights.sort_unstable();
        assert_eq!(weights, vec![("wire-a", 2), ("wire-a", 5), ("wire-b", 3)]);
        assert!(
            output.topology.operation_candidates.iter().all(
                |record| record.capability_id == capability_id(id(10), ApiOperation::Responses)
            )
        );

        assert_eq!(output.topology.api_key_grants.len(), 1);
        let grant = &output.topology.api_key_grants[0];
        assert_eq!(grant.api_key_id, id(300));
        assert_eq!(
            grant.capability_id,
            capability_id(id(10), ApiOperation::Responses)
        );
        assert_eq!(grant.origin_kind, GrantOriginKind::Group);
        assert_eq!(grant.origin_id, id(1));
    }

    #[test]
    fn wire_catalogue_and_probes_are_operation_scoped() {
        let mut responses = channel(10, 1, ApiFormat::OpenAiResponses, true);
        responses.supports_standalone_web_search = true;
        responses.supports_websocket = true;
        responses.available_models = vec!["wire".into()];
        responses.test_model = Some("wire".into());
        responses.test_pricing_model_id = Some(id(700));
        let output = transfer(
            &input(vec![group(1, ApiFormat::OpenAiResponses)], vec![responses]),
            &[],
            &[],
        )
        .expect("transfer");

        let responses = capability(&output, 10, ApiOperation::Responses);
        assert_eq!(responses.settings.available_models, vec!["wire".to_owned()]);
        assert_eq!(responses.settings.test_model.as_deref(), Some("wire"));
        assert_eq!(responses.settings.test_pricing_model_id, Some(id(700)));
        assert!(
            responses
                .settings
                .transports
                .contains(&CapabilityTransport::Websocket)
        );

        let search = capability(&output, 10, ApiOperation::StandaloneWebSearch);
        assert_eq!(search.settings.available_models, vec!["wire".to_owned()]);
        assert_eq!(
            search.settings.transports,
            vec![CapabilityTransport::HttpJson]
        );
        assert_eq!(search.settings.test_model, None);
        assert_eq!(search.settings.test_pricing_model_id, None);
    }

    #[test]
    fn conflicting_codex_network_configuration_is_rejected() {
        let mut request = input(
            codex_groups(500, false),
            codex_channels(true, false, "https://other.example"),
        );
        request.projections = codex_projections();
        assert_eq!(
            transfer(&request, &[], &[]).unwrap_err(),
            CapabilityCutoverTransferError::ConflictingNetworkConfig { pool_id: id(500) }
        );
    }

    #[test]
    fn deleted_legacy_state_stays_retired_and_disabled() {
        let mut deleted_group = group(1, ApiFormat::OpenAiResponses);
        deleted_group.enabled = false;
        deleted_group.deleted_at = Some(ts(6));
        let mut deleted_channel = channel(10, 1, ApiFormat::OpenAiResponses, false);
        deleted_channel.deleted_at = Some(ts(5));
        let output = transfer(&input(vec![deleted_group], vec![deleted_channel]), &[], &[])
            .expect("transfer");

        let group = &output.topology.routing_groups[0];
        assert!(!group.enabled);
        assert_eq!(group.deleted_at, Some(ts(6)));
        let logical = &output.topology.logical_channels[0];
        assert!(!logical.enabled);
        assert_eq!(logical.deleted_at, Some(ts(5)));
        let access = &output.topology.upstream_accesses[0];
        assert!(!access.enabled);
        assert_eq!(access.deleted_at, Some(ts(5)));
        let capability = &output.topology.channel_capabilities[0];
        assert!(!capability.settings.enabled);
        assert_eq!(capability.deleted_at, Some(ts(5)));
    }

    #[test]
    fn history_registers_every_legacy_and_replacement_uuid() {
        let mut responses = channel(10, 1, ApiFormat::OpenAiResponses, true);
        responses.supports_standalone_web_search = true;
        let output = transfer(
            &input(vec![group(1, ApiFormat::OpenAiResponses)], vec![responses]),
            &[],
            &[],
        )
        .expect("transfer");

        assert_eq!(output.group_identity_registry.len(), 1);
        assert_eq!(
            output.group_identity_registry[0].canonical_group_id,
            Some(id(1))
        );
        let legacy = output
            .channel_identity_registry
            .iter()
            .find(|record| record.id == id(10))
            .expect("legacy channel history");
        assert_eq!(
            legacy.capability_id, None,
            "multi-operation link must be null"
        );
        assert_eq!(legacy.canonical_channel_id, Some(id(10)));
        for operation in [ApiOperation::Responses, ApiOperation::StandaloneWebSearch] {
            let replacement = capability_id(id(10), operation);
            let record = output
                .channel_identity_registry
                .iter()
                .find(|row| row.id == replacement)
                .expect("capability history");
            assert_eq!(record.capability_id, Some(replacement));
            assert_eq!(record.canonical_channel_id, Some(id(10)));
            assert_eq!(record.codex_credential_id, None);
        }
    }
}
