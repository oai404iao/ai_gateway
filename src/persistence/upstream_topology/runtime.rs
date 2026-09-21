//! Canonical snapshot adapter.
//!
//! Resolves the joint capability topology into the backend-neutral
//! [`ControlPlaneRecords`] the runtime compiler consumes. This is the only
//! place that maps canonical owners (routing group, upstream access, logical
//! channel, channel capability, operation rule) onto the compiled snapshot
//! input, and it never consults the retired legacy configuration tables.
//!
//! Validation covers every non-deleted capability, access, group, logical
//! channel, and credential, including disabled, unreferenced, and unrouted
//! drafts, so an invalid graph can never be published as a silently narrower
//! snapshot. Errors carry only stable identifiers, never upstream URLs,
//! secrets, or wire model values.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use uuid::Uuid;

use super::super::upstream_credentials::CredentialRecord;
use super::super::{
    ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
    CredentialIdentity, ModelRecord, ModelRuleRecord, ModelRuleRouteCandidate,
    ModelRuleRoutingTier, ProxyRecord, RepositoryError,
};
use super::{
    ChannelCapabilityRecord, GrantOriginKind, LogicalChannelRecord, RoutingGroupRecord,
    UpstreamAccessRecord, UpstreamTopologyRecords,
};
use crate::domain::{ApiOperation, CapabilityTransport, ConnectorKind, CredentialTarget};

/// Minimal identity of one priced model's routing profile. The adapter needs
/// only the profile id, the owned model id, and the model enablement flag; the
/// priced metadata itself comes from the base model list.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct ModelRoutingProfileBinding {
    pub id: Uuid,
    pub model_id: Uuid,
    pub model_enabled: bool,
    pub model_deleted: bool,
}

/// Base control-plane rows that canonical rows do not own.
///
/// User status and flags are carried by each [`ApiKeyRecord`] (the loader joins
/// users), so no separate user list is required.
#[derive(Debug, Default)]
pub(crate) struct BaseControlPlaneRecords {
    pub api_keys: Vec<ApiKeyRecord>,
    pub models: Vec<ModelRecord>,
    pub proxies: Vec<ProxyRecord>,
    pub templates: Vec<ConfigTemplateRecord>,
}

/// Precise, secret-free canonical graph failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CanonicalGraphError {
    #[error("canonical graph has a duplicate {kind} id")]
    DuplicateId { kind: &'static str },
    #[error("canonical graph references a missing {kind}")]
    MissingReference { kind: &'static str },
    #[error("canonical graph references a deleted {kind}")]
    DeletedReference { kind: &'static str },
    #[error("canonical capability {capability_id} has settings invalid for its connector")]
    InvalidCapability { capability_id: Uuid },
    #[error("canonical {kind} {id} has an invalid name")]
    InvalidName { kind: &'static str, id: Uuid },
    #[error("canonical upstream access {access_id} has an invalid base URL")]
    InvalidBaseUrl { access_id: Uuid },
    #[error("canonical upstream access {access_id} has a non-positive timeout")]
    InvalidTimeout { access_id: Uuid },
    #[error("canonical upstream access {access_id} references a missing or disabled proxy")]
    InvalidProxy { access_id: Uuid },
    #[error("canonical logical channel {channel_id} references an unknown upstream credential")]
    UnknownCredential { channel_id: Uuid },
    #[error("canonical upstream credential kind conflicts with the channel connector")]
    CredentialKindMismatch,
    #[error("canonical credential {credential_id} is invalid")]
    InvalidCredential { credential_id: Uuid },
    #[error("canonical logical channel {channel_id} does not match its managed credential id")]
    CredentialIdentityMismatch { channel_id: Uuid },
    #[error("canonical credential {credential_id} is deleted while still bound")]
    DeletedCredential { credential_id: Uuid },
    #[error("canonical credential {credential_id} does not allow the bound access base URL")]
    CredentialScope { credential_id: Uuid },
    #[error("canonical routing group {group_id} is sharing-only but has non-Codex capabilities")]
    SharingOnlyRequiresCodex { group_id: Uuid },
    #[error("canonical operation rule {rule_id} has no model routing profile binding")]
    MissingProfile { rule_id: Uuid },
    #[error("canonical operation rule {rule_id} references a missing model")]
    MissingModel { rule_id: Uuid },
    #[error("canonical operation rule {rule_id} has inconsistent operation ownership")]
    InconsistentOperation { rule_id: Uuid },
    #[error("canonical model rule {rule_id} references an unknown routing capability")]
    UnknownRouteTarget { rule_id: Uuid },
    #[error("canonical api-key grant references an unknown capability")]
    UnknownGrantTarget,
}

impl From<CanonicalGraphError> for RepositoryError {
    fn from(_: CanonicalGraphError) -> Self {
        Self::Validation
    }
}

/// One live capability with its resolved canonical owners.
struct LiveCapability<'a> {
    capability: &'a ChannelCapabilityRecord,
    logical: &'a LogicalChannelRecord,
    access: &'a UpstreamAccessRecord,
    group: &'a RoutingGroupRecord,
    connector_kind: ConnectorKind,
    credential: Option<&'a CredentialRecord>,
    /// Admin-level enablement including owner switches and credential state.
    enabled: bool,
}

pub(crate) fn resolve_runtime(
    topology: &UpstreamTopologyRecords,
    base: BaseControlPlaneRecords,
    profiles: &[ModelRoutingProfileBinding],
    credentials: &[CredentialRecord],
) -> Result<ControlPlaneRecords, CanonicalGraphError> {
    let groups = index(&topology.routing_groups, "routing group", |group| group.id)?;
    let accesses = index(&topology.upstream_accesses, "upstream access", |access| {
        access.id
    })?;
    let logical = index(&topology.logical_channels, "logical channel", |channel| {
        channel.id
    })?;
    let capabilities = index(
        &topology.channel_capabilities,
        "channel capability",
        |capability| capability.id,
    )?;
    let credentials = index(credentials, "upstream credential", |record| record.id)?;
    let profiles = index(profiles, "model routing profile", |profile| profile.id)?;
    let models = index(&base.models, "model", |model| model.id)?;

    validate_credentials(&credentials)?;
    validate_accesses(&accesses, &base.proxies)?;
    validate_group_names(&groups)?;
    validate_logical_credentials(&logical, &accesses, &groups, &credentials)?;
    let live = resolve_capabilities(&capabilities, &logical, &accesses, &groups, &credentials)?;
    validate_sharing_only_groups(&groups, &live)?;

    let channels = build_channels(&live);
    let channel_ids = channels.iter().map(|channel| channel.id).collect();
    let model_rules = build_model_rules(topology, &profiles, &models, &live, &channel_ids)?;
    let known_capabilities = capabilities.keys().copied().collect::<HashSet<_>>();
    let api_keys = fold_grants(
        base.api_keys,
        &topology.api_key_grants,
        &live,
        &known_capabilities,
    )?;
    validate_policy_grants(&topology.policy_grants, &live, &known_capabilities)?;
    let routing_groups = build_groups(&groups);

    Ok(ControlPlaneRecords {
        api_keys,
        models: base.models,
        model_rules,
        groups: routing_groups,
        channels,
        proxies: base.proxies,
        templates: base.templates,
    })
}

fn index<'a, T>(
    rows: &'a [T],
    kind: &'static str,
    id: impl Fn(&T) -> Uuid,
) -> Result<HashMap<Uuid, &'a T>, CanonicalGraphError> {
    let mut index = HashMap::with_capacity(rows.len());
    for row in rows {
        if index.insert(id(row), row).is_some() {
            return Err(CanonicalGraphError::DuplicateId { kind });
        }
    }
    Ok(index)
}

fn is_live(deleted_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    deleted_at.is_none()
}

fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 100
}

fn validate_credentials(
    credentials: &HashMap<Uuid, &CredentialRecord>,
) -> Result<(), CanonicalGraphError> {
    let mut ordered = credentials.values().copied().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|credential| credential.id);
    for credential in ordered {
        credential
            .validate()
            .map_err(|_| CanonicalGraphError::InvalidCredential {
                credential_id: credential.id,
            })?;
    }
    Ok(())
}

/// Validates every non-deleted access, even one no logical channel references,
/// because an invalid draft must never publish as a narrower snapshot.
fn validate_accesses(
    accesses: &HashMap<Uuid, &UpstreamAccessRecord>,
    proxies: &[ProxyRecord],
) -> Result<(), CanonicalGraphError> {
    let enabled_proxies = proxies
        .iter()
        .filter(|proxy| proxy.enabled)
        .map(|proxy| proxy.id)
        .collect::<HashSet<_>>();
    let mut ordered = accesses.values().copied().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|access| access.id);
    for access in ordered {
        if !is_live(access.deleted_at) {
            continue;
        }
        if !valid_name(&access.name) {
            return Err(CanonicalGraphError::InvalidName {
                kind: "upstream access",
                id: access.id,
            });
        }
        if CredentialTarget::parse(&access.base_url).is_err() {
            return Err(CanonicalGraphError::InvalidBaseUrl {
                access_id: access.id,
            });
        }
        if [
            access.connect_timeout_ms,
            access.response_header_timeout_ms,
            access.stream_idle_timeout_ms,
        ]
        .into_iter()
        .flatten()
        .any(|value| value <= 0)
        {
            return Err(CanonicalGraphError::InvalidTimeout {
                access_id: access.id,
            });
        }
        if access
            .proxy_id
            .is_some_and(|id| !enabled_proxies.contains(&id))
        {
            return Err(CanonicalGraphError::InvalidProxy {
                access_id: access.id,
            });
        }
    }
    Ok(())
}

fn validate_group_names(
    groups: &HashMap<Uuid, &RoutingGroupRecord>,
) -> Result<(), CanonicalGraphError> {
    let mut ordered = groups.values().copied().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|group| group.id);
    for group in ordered {
        if is_live(group.deleted_at) && !valid_name(&group.name) {
            return Err(CanonicalGraphError::InvalidName {
                kind: "routing group",
                id: group.id,
            });
        }
    }
    Ok(())
}

/// Validates every non-deleted logical channel reference, including channels
/// that currently have no capability. A credential's target scope and kind
/// must hold for all live bindings even when they are disabled or unrouted.
fn validate_logical_credentials<'a>(
    logical: &'a HashMap<Uuid, &'a LogicalChannelRecord>,
    accesses: &'a HashMap<Uuid, &'a UpstreamAccessRecord>,
    groups: &'a HashMap<Uuid, &'a RoutingGroupRecord>,
    credentials: &'a HashMap<Uuid, &'a CredentialRecord>,
) -> Result<(), CanonicalGraphError> {
    for channel in logical.values() {
        if !is_live(channel.deleted_at) {
            continue;
        }
        if !valid_name(&channel.name) {
            return Err(CanonicalGraphError::InvalidName {
                kind: "logical channel",
                id: channel.id,
            });
        }
        let Some(access) = accesses.get(&channel.access_id).copied() else {
            return Err(CanonicalGraphError::MissingReference {
                kind: "upstream access",
            });
        };
        if !is_live(access.deleted_at) {
            return Err(CanonicalGraphError::DeletedReference {
                kind: "upstream access",
            });
        }
        let Some(group) = groups.get(&channel.group_id).copied() else {
            return Err(CanonicalGraphError::MissingReference {
                kind: "routing group",
            });
        };
        if !is_live(group.deleted_at) {
            return Err(CanonicalGraphError::DeletedReference {
                kind: "routing group",
            });
        }
        let credential = match channel.credential_id {
            Some(id) => Some(credentials.get(&id).copied().ok_or(
                CanonicalGraphError::UnknownCredential {
                    channel_id: channel.id,
                },
            )?),
            None => None,
        };
        resolve_credential(access, credential, &channel.id)?;
    }
    Ok(())
}

fn resolve_capabilities<'a>(
    capabilities: &'a HashMap<Uuid, &'a ChannelCapabilityRecord>,
    logical: &'a HashMap<Uuid, &'a LogicalChannelRecord>,
    accesses: &'a HashMap<Uuid, &'a UpstreamAccessRecord>,
    groups: &'a HashMap<Uuid, &'a RoutingGroupRecord>,
    credentials: &'a HashMap<Uuid, &'a CredentialRecord>,
) -> Result<Vec<LiveCapability<'a>>, CanonicalGraphError> {
    let mut live = Vec::new();
    let mut ordered = capabilities.values().copied().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|capability| capability.id);
    for capability in ordered {
        if !is_live(capability.deleted_at) {
            continue;
        }
        let Some(logical_channel) = logical.get(&capability.channel_id).copied() else {
            return Err(CanonicalGraphError::MissingReference {
                kind: "logical channel",
            });
        };
        if !is_live(logical_channel.deleted_at) {
            return Err(CanonicalGraphError::DeletedReference {
                kind: "logical channel",
            });
        }
        let Some(access) = accesses.get(&logical_channel.access_id).copied() else {
            return Err(CanonicalGraphError::MissingReference {
                kind: "upstream access",
            });
        };
        if !is_live(access.deleted_at) {
            return Err(CanonicalGraphError::DeletedReference {
                kind: "upstream access",
            });
        }
        let Some(group) = groups.get(&logical_channel.group_id).copied() else {
            return Err(CanonicalGraphError::MissingReference {
                kind: "routing group",
            });
        };
        if !is_live(group.deleted_at) {
            return Err(CanonicalGraphError::DeletedReference {
                kind: "routing group",
            });
        }
        capability
            .settings
            .validate(access.connector_kind)
            .map_err(|_| CanonicalGraphError::InvalidCapability {
                capability_id: capability.id,
            })?;

        let credential = match logical_channel.credential_id {
            Some(id) => Some(credentials.get(&id).copied().ok_or(
                CanonicalGraphError::UnknownCredential {
                    channel_id: logical_channel.id,
                },
            )?),
            None => None,
        };
        let credential_enabled = resolve_credential(access, credential, &logical_channel.id)?;
        live.push(LiveCapability {
            capability,
            logical: logical_channel,
            access,
            group,
            connector_kind: access.connector_kind,
            credential,
            enabled: logical_channel.enabled && access.enabled && credential_enabled,
        });
    }
    Ok(live)
}

/// Validates the credential/connector pairing and target scope. Returns whether
/// the credential permits an active binding.
fn resolve_credential(
    access: &UpstreamAccessRecord,
    credential: Option<&CredentialRecord>,
    channel_id: &Uuid,
) -> Result<bool, CanonicalGraphError> {
    let managed = access.connector_kind == ConnectorKind::CodexOauth;
    let Some(credential) = credential else {
        if managed {
            // An unauthenticated Codex access is unusable.
            return Err(CanonicalGraphError::UnknownCredential {
                channel_id: *channel_id,
            });
        }
        return Ok(true);
    };
    let provider_managed = credential.kind == "codex_oauth";
    if managed != provider_managed {
        return Err(CanonicalGraphError::CredentialKindMismatch);
    }
    if managed && credential.id != *channel_id {
        return Err(CanonicalGraphError::CredentialIdentityMismatch {
            channel_id: *channel_id,
        });
    }
    if credential.deleted_at.is_some() {
        return Err(CanonicalGraphError::DeletedCredential {
            credential_id: credential.id,
        });
    }
    if !managed {
        credential.allows_target(&access.base_url).map_err(|_| {
            CanonicalGraphError::CredentialScope {
                credential_id: credential.id,
            }
        })?;
    }
    Ok(credential.enabled)
}

fn validate_sharing_only_groups(
    groups: &HashMap<Uuid, &RoutingGroupRecord>,
    live: &[LiveCapability<'_>],
) -> Result<(), CanonicalGraphError> {
    for group in groups.values() {
        if !is_live(group.deleted_at) || !group.sharing_only {
            continue;
        }
        if live.iter().any(|capability| {
            capability.group.id == group.id
                && capability.connector_kind != ConnectorKind::CodexOauth
        }) {
            return Err(CanonicalGraphError::SharingOnlyRequiresCodex { group_id: group.id });
        }
    }
    Ok(())
}

fn build_channels(live: &[LiveCapability<'_>]) -> Vec<ChannelRecord> {
    live.iter()
        .map(|capability| {
            let settings = &capability.capability.settings;
            let managed = capability.connector_kind == ConnectorKind::CodexOauth;
            let (upstream_auth_kind, upstream_auth_header_name, upstream_api_key) =
                match capability.credential {
                    Some(credential) if !managed => (
                        credential.kind.clone(),
                        credential.header_name.clone(),
                        credential.secret.clone(),
                    ),
                    _ => ("none".to_owned(), None, None),
                };
            ChannelRecord {
                credential: capability.credential.map(|credential| CredentialIdentity {
                    id: credential.id,
                    revision: credential.revision,
                }),
                credential_binding_revision: capability.logical.binding_revision,
                id: capability.capability.id,
                channel_group_id: capability.group.id,
                api_format: settings.operation.api_format().as_str().to_owned(),
                logical_channel_id: capability.logical.id,
                access_id: capability.access.id,
                api_operation: Some(settings.operation),
                connector_kind: capability.connector_kind.as_str().to_owned(),
                request_compression: settings.request_compression.as_str().to_owned(),
                access_revision: capability.access.revision,
                capability_revision: capability.capability.revision,
                transports: settings.transports.clone(),
                name: capability.logical.name.clone(),
                base_url: capability.access.base_url.clone(),
                enabled: capability.enabled && settings.enabled,
                supports_websocket: settings
                    .transports
                    .contains(&CapabilityTransport::Websocket),
                supports_standalone_web_search: settings.operation
                    == ApiOperation::StandaloneWebSearch,
                auto_disabled: capability.capability.auto_disabled,
                auto_disable_allowed: settings.auto_disable_allowed,
                billing_multiplier: capability.capability.billing_multiplier,
                proxy_id: capability.access.proxy_id,
                config_template_id: capability.capability.config_template_id,
                override_document: capability.capability.override_document.clone(),
                connect_timeout_ms: capability.access.connect_timeout_ms,
                response_header_timeout_ms: capability.access.response_header_timeout_ms,
                stream_idle_timeout_ms: capability.access.stream_idle_timeout_ms,
                upstream_auth_kind,
                upstream_auth_header_name,
                upstream_api_key,
                available_models: settings.available_models.clone(),
                test_model: settings.test_model.clone(),
                test_pricing_model_id: settings.test_pricing_model_id,
            }
        })
        .collect()
}

fn build_groups(groups: &HashMap<Uuid, &RoutingGroupRecord>) -> Vec<ChannelGroupRecord> {
    let mut ordered = groups.values().copied().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|group| group.id);
    ordered
        .into_iter()
        .filter(|group| is_live(group.deleted_at))
        .map(|group| ChannelGroupRecord {
            id: group.id,
            name: group.name.clone(),
            // Canonical routing groups own no protocol metadata; each channel
            // capability owns its connector, compression, and operation.
            api_format: String::new(),
            connector_kind: String::new(),
            request_compression: String::new(),
            sharing_only: group.sharing_only,
            enabled: group.enabled,
        })
        .collect()
}

fn build_model_rules(
    topology: &UpstreamTopologyRecords,
    profiles: &HashMap<Uuid, &ModelRoutingProfileBinding>,
    models: &HashMap<Uuid, &ModelRecord>,
    live: &[LiveCapability<'_>],
    channel_ids: &HashSet<Uuid>,
) -> Result<Vec<ModelRuleRecord>, CanonicalGraphError> {
    let live_operations = live
        .iter()
        .map(|capability| {
            (
                capability.capability.id,
                capability.capability.settings.operation,
            )
        })
        .collect::<HashMap<_, _>>();
    let mut candidates_by_tier = HashMap::<Uuid, Vec<&super::OperationCandidateRecord>>::new();
    for candidate in &topology.operation_candidates {
        candidates_by_tier
            .entry(candidate.tier_id)
            .or_default()
            .push(candidate);
    }
    let mut tiers_by_rule = HashMap::<Uuid, Vec<&super::OperationTierRecord>>::new();
    for tier in &topology.operation_tiers {
        tiers_by_rule.entry(tier.rule_id).or_default().push(tier);
    }

    let mut rules = Vec::with_capacity(topology.operation_rules.len());
    for rule in &topology.operation_rules {
        let profile = profiles
            .get(&rule.model_routing_profile_id)
            .copied()
            .ok_or(CanonicalGraphError::MissingProfile { rule_id: rule.id })?;
        if profile.model_deleted {
            continue;
        }
        let model = models
            .get(&profile.model_id)
            .copied()
            .ok_or(CanonicalGraphError::MissingModel { rule_id: rule.id })?;
        let mut tiers = tiers_by_rule.remove(&rule.id).unwrap_or_default();
        tiers.sort_unstable_by_key(|tier| tier.priority);
        let mut routing_tiers = Vec::with_capacity(tiers.len());
        for tier in tiers {
            if tier.operation != rule.operation {
                return Err(CanonicalGraphError::InconsistentOperation { rule_id: rule.id });
            }
            let mut candidates = Vec::new();
            for candidate in candidates_by_tier.get(&tier.id).into_iter().flatten() {
                if candidate.operation != rule.operation {
                    return Err(CanonicalGraphError::InconsistentOperation { rule_id: rule.id });
                }
                if !channel_ids.contains(&candidate.capability_id)
                    || live_operations.get(&candidate.capability_id) != Some(&rule.operation)
                {
                    return Err(CanonicalGraphError::UnknownRouteTarget { rule_id: rule.id });
                }
                // The internal channel_id of a canonical candidate is the
                // capability UUID; there is no legacy channel lookup.
                candidates.push(ModelRuleRouteCandidate {
                    channel_id: candidate.capability_id,
                    upstream_model: candidate.upstream_model.clone(),
                    weight: candidate.weight,
                });
            }
            routing_tiers.push(ModelRuleRoutingTier {
                priority: tier.priority,
                selection_strategy: tier.strategy.clone(),
                candidates,
            });
        }
        rules.push(ModelRuleRecord {
            id: rule.id,
            client_model: model.source_model_id.clone(),
            api_format: rule.operation.api_format().as_str().to_owned(),
            api_operation: rule.operation,
            model_id: profile.model_id,
            model_enabled: profile.model_enabled,
            model_currency: model.currency.clone(),
            price_unit_tokens: model.price_unit_tokens,
            price_effective_at: model.price_effective_at,
            input_unit_price: model.input_unit_price,
            cached_input_unit_price: model.cached_input_unit_price,
            cache_write_unit_price: model.cache_write_unit_price,
            output_unit_price: model.output_unit_price,
            advanced_billing: model.advanced_billing.clone(),
            routing_tiers,
            enabled: rule.enabled,
        });
    }
    Ok(rules)
}

/// Folds only explicit canonical capability grants into `allowed_channel_ids`.
///
/// Group-origin grants are kept only while the capability still belongs to the
/// granting group, channel-origin grants only while the capability still
/// belongs to the granting logical channel, and no grant may widen the key's
/// own API-format permissions. Group ids never authorize canonical channels.
fn fold_grants(
    mut api_keys: Vec<ApiKeyRecord>,
    grants: &[super::ApiKeyCapabilityGrantRecord],
    live: &[LiveCapability<'_>],
    known_capabilities: &HashSet<Uuid>,
) -> Result<Vec<ApiKeyRecord>, CanonicalGraphError> {
    let capabilities = live
        .iter()
        .map(|capability| (capability.capability.id, capability))
        .collect::<HashMap<_, _>>();
    let mut grants_by_key = HashMap::<Uuid, Vec<&super::ApiKeyCapabilityGrantRecord>>::new();
    for grant in grants {
        grants_by_key
            .entry(grant.api_key_id)
            .or_default()
            .push(grant);
    }
    for key in &mut api_keys {
        let mut allowed = HashSet::new();
        for grant in grants_by_key.get(&key.id).into_iter().flatten() {
            let Some(capability) = capabilities.get(&grant.capability_id) else {
                // A tombstoned capability keeps its grant rows; it authorizes
                // nothing. A grant to a capability that never existed is a
                // graph inconsistency.
                if known_capabilities.contains(&grant.capability_id) {
                    continue;
                }
                return Err(CanonicalGraphError::UnknownGrantTarget);
            };
            if !grant_matches_origin(grant, capability) {
                continue;
            }
            if !key.allowed_api_formats.iter().any(|format| {
                format
                    == capability
                        .capability
                        .settings
                        .operation
                        .api_format()
                        .as_str()
            }) {
                continue;
            }
            allowed.insert(grant.capability_id);
        }
        let mut allowed = allowed.into_iter().collect::<Vec<_>>();
        allowed.sort_unstable();
        key.allowed_group_ids.clear();
        key.allowed_channel_ids = allowed;
    }
    Ok(api_keys)
}

fn grant_matches_origin(
    grant: &super::ApiKeyCapabilityGrantRecord,
    capability: &LiveCapability<'_>,
) -> bool {
    match grant.origin_kind {
        GrantOriginKind::Group => capability.group.id == grant.origin_id,
        GrantOriginKind::Channel => capability.logical.id == grant.origin_id,
        GrantOriginKind::Capability => capability.capability.id == grant.origin_id,
    }
}

fn validate_policy_grants(
    grants: &[super::ApiKeyPolicyCapabilityGrantRecord],
    live: &[LiveCapability<'_>],
    known_capabilities: &HashSet<Uuid>,
) -> Result<(), CanonicalGraphError> {
    let live_capabilities = live
        .iter()
        .map(|capability| capability.capability.id)
        .collect::<HashSet<_>>();
    if grants.iter().any(|grant| {
        !live_capabilities.contains(&grant.capability_id)
            && !known_capabilities.contains(&grant.capability_id)
    }) {
        return Err(CanonicalGraphError::UnknownGrantTarget);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use serde_json::json;

    use super::super::{
        ApiKeyCapabilityGrantRecord, OperationCandidateRecord, OperationRuleRecord,
        OperationTierRecord,
    };
    use super::*;

    fn at() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp")
    }

    fn group(id: u128, sharing_only: bool) -> RoutingGroupRecord {
        RoutingGroupRecord {
            id: Uuid::from_u128(id),
            name: format!("group-{id}"),
            enabled: true,
            sharing_only,
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn access(id: u128, connector: ConnectorKind, base_url: &str) -> UpstreamAccessRecord {
        UpstreamAccessRecord {
            id: Uuid::from_u128(id),
            name: format!("access-{id}"),
            connector_kind: connector,
            base_url: base_url.into(),
            proxy_id: None,
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            enabled: true,
            revision: Uuid::from_u128(id + 500),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn logical(
        id: u128,
        group_id: u128,
        access_id: u128,
        credential_id: Option<u128>,
    ) -> LogicalChannelRecord {
        LogicalChannelRecord {
            id: Uuid::from_u128(id),
            group_id: Uuid::from_u128(group_id),
            access_id: Uuid::from_u128(access_id),
            credential_id: credential_id.map(Uuid::from_u128),
            name: format!("channel-{id}"),
            enabled: true,
            binding_revision: Uuid::from_u128(id + 700),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn settings(
        operation: ApiOperation,
        transports: Vec<CapabilityTransport>,
    ) -> crate::domain::CapabilitySettings {
        crate::domain::CapabilitySettings {
            operation,
            transports,
            enabled: true,
            available_models: vec!["wire-model".into()],
            request_compression: crate::domain::RequestCompression::Default,
            test_model: None,
            test_pricing_model_id: None,
            auto_disable_allowed: false,
        }
    }

    fn capability(
        id: u128,
        channel_id: u128,
        settings: crate::domain::CapabilitySettings,
    ) -> ChannelCapabilityRecord {
        ChannelCapabilityRecord {
            id: Uuid::from_u128(id),
            channel_id: Uuid::from_u128(channel_id),
            settings,
            auto_disabled: false,
            auto_disable_reason: None,
            auto_disable_at: None,
            status_statistics_enabled: false,
            config_template_id: None,
            override_document: json!({}),
            billing_multiplier: Decimal::ONE,
            revision: Uuid::from_u128(id + 900),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn credential(id: u128, kind: &str, allowed: &[&str], deleted: bool) -> CredentialRecord {
        let codex = kind == "codex_oauth";
        CredentialRecord {
            id: Uuid::from_u128(id),
            name: format!("credential-{id}"),
            kind: kind.into(),
            header_name: None,
            secret: (!codex).then(|| "secret".into()),
            allowed_base_urls: if codex {
                Vec::new()
            } else {
                allowed.iter().map(|value| (*value).to_owned()).collect()
            },
            enabled: true,
            revision: Uuid::from_u128(id + 300),
            created_at: at(),
            updated_at: at(),
            deleted_at: deleted.then_some(at()),
        }
    }

    fn api_key(id: u128, formats: &[&str]) -> ApiKeyRecord {
        ApiKeyRecord {
            id: Uuid::from_u128(id),
            user_id: Uuid::from_u128(id + 10),
            user_status: "active".into(),
            user_websocket_enabled: false,
            user_filter_fast_mode: false,
            secret_value: format!("secret-{id}"),
            status: "active".into(),
            expires_at: None,
            allowed_api_formats: formats.iter().map(|value| (*value).to_owned()).collect(),
            permissions: vec!["proxy".into()],
            allowed_group_ids: vec![Uuid::from_u128(999)],
            allowed_channel_ids: vec![Uuid::from_u128(998)],
            requests_per_minute: None,
            max_concurrent_requests: None,
            quota_limit_amount: None,
            quota_used_amount: Decimal::ZERO,
        }
    }

    fn model(id: u128) -> ModelRecord {
        ModelRecord {
            id: Uuid::from_u128(id),
            source_model_id: "client-model".into(),
            currency: "USD".into(),
            price_unit_tokens: 1_000_000,
            price_effective_at: at(),
            input_unit_price: Decimal::ONE,
            cached_input_unit_price: Decimal::ZERO,
            cache_write_unit_price: Decimal::ZERO,
            output_unit_price: Decimal::ONE,
            advanced_billing: json!({}),
        }
    }

    fn rule(id: u128, profile_id: u128, operation: ApiOperation) -> OperationRuleRecord {
        OperationRuleRecord {
            id: Uuid::from_u128(id),
            model_routing_profile_id: Uuid::from_u128(profile_id),
            operation,
            enabled: true,
            created_at: at(),
            updated_at: at(),
        }
    }

    fn topology(capabilities: Vec<ChannelCapabilityRecord>) -> UpstreamTopologyRecords {
        UpstreamTopologyRecords {
            routing_groups: vec![group(1, false)],
            upstream_accesses: vec![access(
                2,
                ConnectorKind::OpenAiCompatible,
                "https://up.example",
            )],
            logical_channels: vec![logical(3, 1, 2, None)],
            channel_capabilities: capabilities,
            ..UpstreamTopologyRecords::default()
        }
    }

    fn base() -> BaseControlPlaneRecords {
        BaseControlPlaneRecords {
            api_keys: Vec::new(),
            models: vec![model(50)],
            proxies: Vec::new(),
            templates: Vec::new(),
        }
    }

    fn profile() -> ModelRoutingProfileBinding {
        ModelRoutingProfileBinding {
            id: Uuid::from_u128(60),
            model_id: Uuid::from_u128(50),
            model_enabled: true,
            model_deleted: false,
        }
    }

    #[test]
    fn each_capability_becomes_one_channel_with_owner_metadata() {
        let records = topology(vec![
            capability(
                100,
                3,
                settings(
                    ApiOperation::Responses,
                    vec![
                        CapabilityTransport::HttpJson,
                        CapabilityTransport::HttpSse,
                        CapabilityTransport::Websocket,
                    ],
                ),
            ),
            capability(
                101,
                3,
                settings(
                    ApiOperation::StandaloneWebSearch,
                    vec![CapabilityTransport::HttpJson],
                ),
            ),
        ]);
        let resolved = resolve_runtime(&records, base(), &[], &[]).expect("resolve");
        assert_eq!(resolved.channels.len(), 2);
        let mut ids = resolved
            .channels
            .iter()
            .map(|channel| channel.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, vec![Uuid::from_u128(100), Uuid::from_u128(101)]);

        let responses = resolved
            .channels
            .iter()
            .find(|channel| channel.id == Uuid::from_u128(100))
            .expect("responses channel");
        assert_eq!(responses.logical_channel_id, Uuid::from_u128(3));
        assert_eq!(responses.access_id, Uuid::from_u128(2));
        assert_eq!(responses.api_operation, Some(ApiOperation::Responses));
        assert_eq!(responses.connector_kind, "openai_compatible");
        assert_eq!(responses.request_compression, "default");
        assert_eq!(responses.access_revision, Uuid::from_u128(502));
        assert_eq!(responses.capability_revision, Uuid::from_u128(1000));
        assert!(responses.supports_websocket);
        assert!(!responses.supports_standalone_web_search);
        assert_eq!(responses.channel_group_id, Uuid::from_u128(1));

        let search = resolved
            .channels
            .iter()
            .find(|channel| channel.id == Uuid::from_u128(101))
            .expect("search channel");
        assert!(search.supports_standalone_web_search);
        assert!(!search.supports_websocket);
        assert_eq!(
            search.api_operation,
            Some(ApiOperation::StandaloneWebSearch)
        );
        assert_eq!(search.api_format, "open_ai_responses");
    }

    #[test]
    fn model_rules_use_capability_uuids_and_profile_model_bindings() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.operation_rules = vec![rule(200, 60, ApiOperation::Responses)];
        records.operation_tiers = vec![OperationTierRecord {
            id: Uuid::from_u128(201),
            rule_id: Uuid::from_u128(200),
            operation: ApiOperation::Responses,
            priority: 0,
            strategy: "weighted_round_robin".into(),
        }];
        records.operation_candidates = vec![OperationCandidateRecord {
            tier_id: Uuid::from_u128(201),
            operation: ApiOperation::Responses,
            capability_id: Uuid::from_u128(100),
            upstream_model: "wire-model".into(),
            weight: 2,
        }];
        let resolved = resolve_runtime(&records, base(), &[profile()], &[]).expect("resolve");
        let rule = resolved.model_rules.first().expect("rule");
        assert_eq!(rule.id, Uuid::from_u128(200));
        assert_eq!(rule.model_id, Uuid::from_u128(50));
        assert_eq!(rule.client_model, "client-model");
        assert_eq!(rule.api_operation, ApiOperation::Responses);
        assert_eq!(
            rule.routing_tiers[0].candidates[0].channel_id,
            Uuid::from_u128(100)
        );
        assert_eq!(
            rule.routing_tiers[0].candidates[0].upstream_model,
            "wire-model"
        );
    }

    #[test]
    fn grants_fold_only_matching_origins_and_key_formats() {
        let mut records = topology(vec![
            capability(
                100,
                3,
                settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
            ),
            capability(
                101,
                3,
                settings(
                    ApiOperation::ImagesGeneration,
                    vec![CapabilityTransport::HttpJson],
                ),
            ),
        ]);
        records.operation_rules = vec![rule(200, 60, ApiOperation::Responses)];
        records.operation_tiers = vec![OperationTierRecord {
            id: Uuid::from_u128(201),
            rule_id: Uuid::from_u128(200),
            operation: ApiOperation::Responses,
            priority: 0,
            strategy: "weighted_random".into(),
        }];
        records.operation_candidates = vec![OperationCandidateRecord {
            tier_id: Uuid::from_u128(201),
            operation: ApiOperation::Responses,
            capability_id: Uuid::from_u128(100),
            upstream_model: "wire-model".into(),
            weight: 1,
        }];
        records.api_key_grants = vec![
            ApiKeyCapabilityGrantRecord {
                api_key_id: Uuid::from_u128(300),
                capability_id: Uuid::from_u128(100),
                origin_kind: GrantOriginKind::Group,
                origin_id: Uuid::from_u128(1),
                created_at: at(),
            },
            ApiKeyCapabilityGrantRecord {
                api_key_id: Uuid::from_u128(300),
                capability_id: Uuid::from_u128(100),
                origin_kind: GrantOriginKind::Channel,
                origin_id: Uuid::from_u128(3),
                created_at: at(),
            },
            ApiKeyCapabilityGrantRecord {
                api_key_id: Uuid::from_u128(300),
                capability_id: Uuid::from_u128(100),
                origin_kind: GrantOriginKind::Capability,
                origin_id: Uuid::from_u128(100),
                created_at: at(),
            },
            // Origin no longer matches the capability's current parent.
            ApiKeyCapabilityGrantRecord {
                api_key_id: Uuid::from_u128(300),
                capability_id: Uuid::from_u128(100),
                origin_kind: GrantOriginKind::Group,
                origin_id: Uuid::from_u128(404),
                created_at: at(),
            },
            // Explicit grant the key's own format permissions do not cover.
            ApiKeyCapabilityGrantRecord {
                api_key_id: Uuid::from_u128(300),
                capability_id: Uuid::from_u128(101),
                origin_kind: GrantOriginKind::Capability,
                origin_id: Uuid::from_u128(101),
                created_at: at(),
            },
        ];
        let mut base = base();
        base.api_keys = vec![api_key(300, &["open_ai_responses"])];
        let resolved = resolve_runtime(&records, base, &[profile()], &[]).expect("resolve");
        let key = resolved.api_keys.first().expect("key");
        assert!(key.allowed_group_ids.is_empty());
        assert_eq!(key.allowed_channel_ids, vec![Uuid::from_u128(100)]);
    }

    #[test]
    fn grants_to_tombstoned_capabilities_are_dropped_and_unknown_targets_fail_closed() {
        let mut records = topology(vec![
            capability(
                100,
                3,
                settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
            ),
            capability(
                102,
                3,
                settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
            ),
        ]);
        records.channel_capabilities[1].deleted_at = Some(at());
        let grant = |capability_id: u128| ApiKeyCapabilityGrantRecord {
            api_key_id: Uuid::from_u128(300),
            capability_id: Uuid::from_u128(capability_id),
            origin_kind: GrantOriginKind::Capability,
            origin_id: Uuid::from_u128(capability_id),
            created_at: at(),
        };
        let mut tombstoned_base = base();
        tombstoned_base.api_keys = vec![api_key(300, &["open_ai_responses"])];

        records.api_key_grants = vec![grant(102)];
        let resolved = resolve_runtime(&records, tombstoned_base, &[], &[]).expect("resolve");
        assert!(resolved.api_keys[0].allowed_channel_ids.is_empty());

        let mut unknown_base = base();
        unknown_base.api_keys = vec![api_key(300, &["open_ai_responses"])];
        records.api_key_grants = vec![grant(999)];
        assert_eq!(
            resolve_runtime(&records, unknown_base, &[], &[]).unwrap_err(),
            CanonicalGraphError::UnknownGrantTarget
        );
    }

    #[test]
    fn codex_connector_requires_matching_managed_credential() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(
                ApiOperation::Responses,
                vec![CapabilityTransport::HttpSse, CapabilityTransport::Websocket],
            ),
        )]);
        records.upstream_accesses = vec![access(
            2,
            ConnectorKind::CodexOauth,
            "https://codex.example",
        )];
        records.logical_channels = vec![logical(3, 1, 2, Some(400))];
        let static_credential = credential(400, "bearer", &["https://codex.example"], false);
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[static_credential]).unwrap_err(),
            CanonicalGraphError::CredentialKindMismatch
        );

        let wrong_identity = credential(400, "codex_oauth", &[], false);
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[wrong_identity]).unwrap_err(),
            CanonicalGraphError::CredentialIdentityMismatch {
                channel_id: Uuid::from_u128(3)
            }
        );

        records.logical_channels = vec![logical(3, 1, 2, Some(3))];
        let deleted = credential(3, "codex_oauth", &[], true);
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[deleted]).unwrap_err(),
            CanonicalGraphError::DeletedCredential {
                credential_id: Uuid::from_u128(3)
            }
        );
    }

    #[test]
    fn static_credential_scope_is_checked_against_every_live_binding() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.logical_channels = vec![logical(3, 1, 2, Some(400))];
        let wrong_scope = credential(400, "bearer", &["https://other.example"], false);
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[wrong_scope]).unwrap_err(),
            CanonicalGraphError::CredentialScope {
                credential_id: Uuid::from_u128(400)
            }
        );
    }

    #[test]
    fn unreferenced_accesses_are_validated() {
        let mut records = topology(vec![]);
        records.upstream_accesses = vec![access(2, ConnectorKind::OpenAiCompatible, "not a url")];
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::InvalidBaseUrl {
                access_id: Uuid::from_u128(2)
            }
        );

        records.upstream_accesses = vec![access(
            2,
            ConnectorKind::OpenAiCompatible,
            "https://up.example",
        )];
        records.upstream_accesses[0].proxy_id = Some(Uuid::from_u128(77));
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::InvalidProxy {
                access_id: Uuid::from_u128(2)
            }
        );
    }

    #[test]
    fn disabled_credentials_gate_static_and_codex_channels() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.logical_channels = vec![logical(3, 1, 2, Some(400))];
        let mut disabled = credential(400, "bearer", &["https://up.example"], false);
        disabled.enabled = false;
        let resolved = resolve_runtime(&records, base(), &[], &[disabled]).expect("resolve");
        assert!(!resolved.channels[0].enabled);

        records.upstream_accesses = vec![access(
            2,
            ConnectorKind::CodexOauth,
            "https://codex.example",
        )];
        records.logical_channels = vec![logical(3, 1, 2, Some(3))];
        let mut disabled = credential(3, "codex_oauth", &[], false);
        disabled.enabled = false;
        let resolved = resolve_runtime(&records, base(), &[], &[disabled]).expect("resolve");
        assert!(!resolved.channels[0].enabled);
    }

    #[test]
    fn live_children_of_deleted_owners_fail_closed() {
        let mut records = topology(vec![]);
        records.logical_channels[0].enabled = false;
        records.routing_groups = vec![RoutingGroupRecord {
            deleted_at: Some(at()),
            ..group(1, false)
        }];
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::DeletedReference {
                kind: "routing group"
            }
        );

        let mut records = topology(vec![]);
        records.logical_channels[0].enabled = false;
        records.upstream_accesses[0].deleted_at = Some(at());
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::DeletedReference {
                kind: "upstream access"
            }
        );

        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.logical_channels[0].deleted_at = Some(at());
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::DeletedReference {
                kind: "logical channel"
            }
        );
    }

    #[test]
    fn unused_invalid_credentials_fail_closed() {
        let records = topology(vec![]);
        let mut invalid = credential(500, "bearer", &["https://up.example"], false);
        invalid.name = "  ".into();
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[invalid]).unwrap_err(),
            CanonicalGraphError::InvalidCredential {
                credential_id: Uuid::from_u128(500)
            }
        );
    }

    #[test]
    fn disabled_and_unrouted_capabilities_are_still_validated() {
        let mut invalid = settings(
            ApiOperation::ChatCompletions,
            vec![CapabilityTransport::Websocket],
        );
        invalid.enabled = false;
        let records = topology(vec![capability(100, 3, invalid)]);
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::InvalidCapability {
                capability_id: Uuid::from_u128(100)
            }
        );
    }

    #[test]
    fn missing_routing_profile_is_a_precise_error() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.operation_rules = vec![rule(200, 60, ApiOperation::Responses)];
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::MissingProfile {
                rule_id: Uuid::from_u128(200)
            }
        );
    }

    #[test]
    fn sharing_only_groups_reject_non_codex_capabilities() {
        let mut records = topology(vec![capability(
            100,
            3,
            settings(ApiOperation::Responses, vec![CapabilityTransport::HttpSse]),
        )]);
        records.routing_groups = vec![group(1, true)];
        assert_eq!(
            resolve_runtime(&records, base(), &[], &[]).unwrap_err(),
            CanonicalGraphError::SharingOnlyRequiresCodex {
                group_id: Uuid::from_u128(1)
            }
        );
    }
}
