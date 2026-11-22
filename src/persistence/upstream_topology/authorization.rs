//! Group selectors and direct channel grants for Key and issuance Policy writes.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{GrantOriginKind, LogicalChannelRecord, UpstreamTopologyRecords};
use crate::persistence::{
    RepositoryError, SelfApiKeyChannelOption, SelfApiKeyGroupOption,
    postgres_control_plane::SelfApiKeyPolicy,
};

struct PolicyScope<'a> {
    groups: &'a [Uuid],
    direct_channels: HashSet<Uuid>,
}

impl<'a> PolicyScope<'a> {
    fn load(
        topology: &UpstreamTopologyRecords,
        policy: Option<&'a SelfApiKeyPolicy>,
    ) -> Result<Self, RepositoryError> {
        let policy = policy.ok_or(RepositoryError::DefaultApiKeyPolicyRequired)?;
        if !policy.enabled {
            return Err(RepositoryError::DefaultApiKeyPolicyDisabled);
        }
        Ok(Self {
            groups: &policy.allowed_group_ids,
            direct_channels: topology
                .policy_grants
                .iter()
                .filter(|grant| {
                    grant.policy_id == policy.id
                        && grant.origin_kind == GrantOriginKind::Channel
                        && grant.origin_id == grant.channel_id
                })
                .map(|grant| grant.channel_id)
                .collect(),
        })
    }

    fn allows_channel(&self, channel: &LogicalChannelRecord) -> bool {
        self.groups.contains(&channel.group_id) || self.direct_channels.contains(&channel.id)
    }
}

pub(crate) fn options(
    topology: &UpstreamTopologyRecords,
    policy: Option<&SelfApiKeyPolicy>,
) -> (Vec<SelfApiKeyGroupOption>, Vec<SelfApiKeyChannelOption>) {
    let Ok(scope) = PolicyScope::load(topology, policy) else {
        return (Vec::new(), Vec::new());
    };
    let mut members = HashMap::new();
    for channel in topology
        .logical_channels
        .iter()
        .filter(|c| c.deleted_at.is_none())
    {
        if scope.allows_channel(channel) {
            members
                .entry(channel.group_id)
                .or_insert_with(Vec::new)
                .push(channel);
        }
    }
    let mut capabilities = HashMap::new();
    for cap in topology
        .channel_capabilities
        .iter()
        .filter(|c| c.deleted_at.is_none())
    {
        capabilities
            .entry(cap.channel_id)
            .or_insert_with(Vec::new)
            .push(cap);
    }
    let mut groups = Vec::new();
    let mut channels = Vec::new();
    for group in topology
        .routing_groups
        .iter()
        .filter(|g| g.deleted_at.is_none())
    {
        let mut group_formats = BTreeSet::new();
        for channel in members.remove(&group.id).unwrap_or_default() {
            let caps = capabilities.remove(&channel.id).unwrap_or_default();
            let formats: BTreeSet<_> = caps
                .iter()
                .map(|cap| cap.settings.operation.api_format().as_str().to_owned())
                .collect();
            group_formats.extend(formats.iter().cloned());
            channels.push(SelfApiKeyChannelOption {
                id: channel.id,
                channel_group_id: group.id,
                channel_group_name: group.name.clone(),
                channel_group_enabled: group.enabled,
                api_formats: formats.into_iter().collect(),
                name: channel.name.clone(),
                enabled: channel.enabled,
                auto_disabled: !caps.is_empty() && caps.iter().all(|cap| cap.auto_disabled),
            });
        }
        if scope.groups.contains(&group.id) {
            groups.push(SelfApiKeyGroupOption {
                id: group.id,
                name: group.name.clone(),
                api_formats: group_formats.into_iter().collect(),
                enabled: group.enabled,
            });
        }
    }
    groups.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
    channels.sort_by(|a, b| {
        (&a.channel_group_name, &a.name, a.id).cmp(&(&b.channel_group_name, &b.name, b.id))
    });
    (groups, channels)
}

#[derive(Clone, Copy)]
pub(crate) enum GrantOwner<'a> {
    AdminKey,
    SelfKey(Option<&'a SelfApiKeyPolicy>),
    Policy,
}

struct GrantChanges {
    added: Vec<Uuid>,
    removed: Vec<(&'static str, Uuid)>,
}

fn previous_targets(before: &Value, field: &str) -> Result<Vec<Uuid>, RepositoryError> {
    before.get(field).map_or(Ok(Vec::new()), |value| {
        serde_json::from_value(value.clone()).map_err(|_| RepositoryError::Validation)
    })
}

fn prepare(
    topology: &UpstreamTopologyRecords,
    owner: GrantOwner<'_>,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
) -> Result<GrantChanges, RepositoryError> {
    let old_groups = previous_targets(before, "allowed_group_ids")?;
    let old_channels = previous_targets(before, "allowed_channel_ids")?;
    let live_groups: HashSet<_> = topology
        .routing_groups
        .iter()
        .filter(|g| g.deleted_at.is_none())
        .map(|g| g.id)
        .collect();
    let live_channels: HashMap<_, _> = topology
        .logical_channels
        .iter()
        .filter(|c| c.deleted_at.is_none() && live_groups.contains(&c.group_id))
        .map(|c| (c.id, c))
        .collect();
    let scope = match owner {
        GrantOwner::SelfKey(policy) => Some(PolicyScope::load(topology, policy)?),
        GrantOwner::AdminKey | GrantOwner::Policy => None,
    };
    // A changed self-service scope must fit today's policy. Administrators may
    // retain retired selectors on an existing Key or Policy.
    for group in groups
        .iter()
        .filter(|g| scope.is_some() || !old_groups.contains(g))
    {
        if !live_groups.contains(group) || scope.as_ref().is_some_and(|s| !s.groups.contains(group))
        {
            return Err(RepositoryError::ApiKeyTargetNotAllowed);
        }
    }
    for id in channels
        .iter()
        .filter(|c| scope.is_some() || !old_channels.contains(c))
    {
        let channel = live_channels
            .get(id)
            .ok_or(RepositoryError::ApiKeyTargetNotAllowed)?;
        if scope.as_ref().is_some_and(|s| !s.allows_channel(channel)) {
            return Err(RepositoryError::ApiKeyTargetNotAllowed);
        }
    }
    // Legacy direct selectors without a grant stay dormant until explicitly
    // removed and re-added. Group selectors never need per-member rows.
    Ok(GrantChanges {
        added: channels
            .iter()
            .copied()
            .filter(|id| !old_channels.contains(id))
            .collect(),
        removed: old_channels
            .into_iter()
            .filter(|id| !channels.contains(id))
            .map(|id| ("channel", id))
            // Revoke any legacy member grants along with their group origin.
            .chain(
                old_groups
                    .into_iter()
                    .filter(|id| !groups.contains(id))
                    .map(|id| ("group", id)),
            )
            .collect(),
    })
}

pub(crate) async fn pg_write(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    owner: GrantOwner<'_>,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
) -> Result<(), RepositoryError> {
    let topology = super::pg_load(transaction).await?;
    let changes = prepare(&topology, owner, before, groups, channels)?;
    let (delete, insert) = match owner {
        GrantOwner::Policy => (
            "DELETE FROM api_key_policy_channel_grants WHERE policy_id=$1 AND origin_kind=$2 AND origin_id=$3",
            "INSERT INTO api_key_policy_channel_grants (policy_id,channel_id,origin_kind,origin_id) VALUES ($1,$2,'channel',$2) ON CONFLICT DO NOTHING",
        ),
        GrantOwner::SelfKey(_) | GrantOwner::AdminKey => (
            "DELETE FROM api_key_channel_grants WHERE api_key_id=$1 AND origin_kind=$2 AND origin_id=$3",
            "INSERT INTO api_key_channel_grants (api_key_id,channel_id,origin_kind,origin_id) VALUES ($1,$2,'channel',$2) ON CONFLICT DO NOTHING",
        ),
    };
    for (kind, origin) in changes.removed {
        sqlx::query(delete)
            .bind(id)
            .bind(kind)
            .bind(origin)
            .execute(&mut **transaction)
            .await?;
    }
    for channel in changes.added {
        sqlx::query(insert)
            .bind(id)
            .bind(channel)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_write(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    owner: GrantOwner<'_>,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let topology = super::sqlite_load(transaction).await?;
    let changes = prepare(&topology, owner, before, groups, channels)?;
    let (delete, insert) = match owner {
        GrantOwner::Policy => (
            "DELETE FROM api_key_policy_channel_grants WHERE policy_id=?1 AND origin_kind=?2 AND origin_id=?3",
            "INSERT INTO api_key_policy_channel_grants (policy_id,channel_id,origin_kind,origin_id) VALUES (?1,?2,'channel',?2) ON CONFLICT DO NOTHING",
        ),
        GrantOwner::SelfKey(_) | GrantOwner::AdminKey => (
            "DELETE FROM api_key_channel_grants WHERE api_key_id=?1 AND origin_kind=?2 AND origin_id=?3",
            "INSERT INTO api_key_channel_grants (api_key_id,channel_id,origin_kind,origin_id) VALUES (?1,?2,'channel',?2) ON CONFLICT DO NOTHING",
        ),
    };
    for (kind, origin) in changes.removed {
        sqlx::query(delete)
            .bind(SqliteUuid(id))
            .bind(kind)
            .bind(SqliteUuid(origin))
            .execute(&mut **transaction)
            .await?;
    }
    for channel in changes.added {
        sqlx::query(insert)
            .bind(SqliteUuid(id))
            .bind(SqliteUuid(channel))
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
