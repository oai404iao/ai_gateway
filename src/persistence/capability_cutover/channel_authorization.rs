//! Expected logical-channel projection of the frozen capability grants.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::persistence::RepositoryError;
use crate::persistence::upstream_topology::{
    ApiKeyChannelGrantRecord, ApiKeyPolicyChannelGrantRecord, GrantOriginKind,
    UpstreamTopologyRecords,
};

pub fn upgrade(
    old: &super::legacy_settings::SixOperationTopology,
) -> Result<super::legacy_settings::ChannelAuthorizationTopology, RepositoryError> {
    let capabilities: HashMap<_, _> = old
        .channel_capabilities
        .iter()
        .map(|capability| (capability.id, capability.channel_id))
        .collect();
    let project = |owner: Uuid, capability: Uuid, kind: GrantOriginKind, origin: Uuid| {
        let channel = *capabilities
            .get(&capability)
            .ok_or(RepositoryError::Validation)?;
        if kind == GrantOriginKind::Capability && origin != capability {
            return Err(RepositoryError::Validation);
        }
        Ok((
            owner,
            channel,
            kind == GrantOriginKind::Group,
            if kind == GrantOriginKind::Capability {
                channel
            } else {
                origin
            },
        ))
    };
    let mut keys = BTreeMap::<_, DateTime<Utc>>::new();
    for grant in &old.api_key_grants {
        let identity = project(
            grant.api_key_id,
            grant.capability_id,
            grant.origin_kind,
            grant.origin_id,
        )?;
        keys.entry(identity)
            .and_modify(|at| *at = (*at).min(grant.created_at))
            .or_insert(grant.created_at);
    }
    let mut policies = BTreeMap::<_, DateTime<Utc>>::new();
    for grant in &old.policy_grants {
        let identity = project(
            grant.policy_id,
            grant.capability_id,
            grant.origin_kind,
            grant.origin_id,
        )?;
        policies
            .entry(identity)
            .and_modify(|at| *at = (*at).min(grant.created_at))
            .or_insert(grant.created_at);
    }
    Ok(UpstreamTopologyRecords {
        routing_groups: old.routing_groups.clone(),
        upstream_accesses: old.upstream_accesses.clone(),
        logical_channels: old.logical_channels.clone(),
        channel_capabilities: old.channel_capabilities.clone(),
        operation_rules: old.operation_rules.clone(),
        operation_tiers: old.operation_tiers.clone(),
        operation_candidates: old.operation_candidates.clone(),
        api_key_grants: keys
            .into_iter()
            .map(|((api_key_id, channel_id, group, origin_id), created_at)| {
                ApiKeyChannelGrantRecord {
                    api_key_id,
                    channel_id,
                    origin_kind: if group {
                        GrantOriginKind::Group
                    } else {
                        GrantOriginKind::Channel
                    },
                    origin_id,
                    created_at,
                }
            })
            .collect(),
        policy_grants: policies
            .into_iter()
            .map(|((policy_id, channel_id, group, origin_id), created_at)| {
                ApiKeyPolicyChannelGrantRecord {
                    policy_id,
                    channel_id,
                    origin_kind: if group {
                        GrantOriginKind::Group
                    } else {
                        GrantOriginKind::Channel
                    },
                    origin_id,
                    created_at,
                }
            })
            .collect(),
    })
}
