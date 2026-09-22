//! Fixed capability grants for explicit group/channel authorization commands.

use std::collections::{BTreeSet, HashSet};

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{GrantOriginKind, UpstreamTopologyRecords};
use crate::persistence::{
    RepositoryError,
    postgres_control_plane::{SelfApiKeyPolicy, SelfApiKeySharingAccess},
};

pub(crate) fn sharing_options(
    topology: &UpstreamTopologyRecords,
    options: &mut [crate::persistence::SelfApiKeySharingCredentialOption],
) {
    for option in options {
        option.channel_ids = topology
            .logical_channels
            .iter()
            .filter(|channel| {
                channel.credential_id == Some(option.credential_id) && channel.deleted_at.is_none()
            })
            .map(|channel| channel.id)
            .collect();
        option.api_formats = topology
            .channel_capabilities
            .iter()
            .filter(|cap| {
                option.channel_ids.contains(&cap.channel_id)
                    && cap.deleted_at.is_none()
                    && cap.settings.operation != crate::domain::ApiOperation::StandaloneWebSearch
            })
            .map(|cap| cap.settings.operation.api_format().as_str().to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
    }
}

pub(crate) fn options(
    topology: &UpstreamTopologyRecords,
    policy: Option<&SelfApiKeyPolicy>,
    sharing: &SelfApiKeySharingAccess,
) -> (
    Vec<crate::persistence::SelfApiKeyGroupOption>,
    Vec<crate::persistence::SelfApiKeyChannelOption>,
) {
    use crate::persistence::{SelfApiKeyChannelOption, SelfApiKeyGroupOption};
    let mut groups = Vec::new();
    let mut channels = Vec::new();
    for group in topology
        .routing_groups
        .iter()
        .filter(|g| g.deleted_at.is_none())
    {
        if let Ok(plan) = resolve(topology, &[group.id], &[], Some((policy, sharing)))
            && !plan.grants.is_empty()
        {
            groups.push(SelfApiKeyGroupOption {
                id: group.id,
                name: group.name.clone(),
                api_formats: plan.formats,
                enabled: group.enabled,
            });
        }
        for channel in topology.logical_channels.iter().filter(|c| {
            c.group_id == group.id
                && c.deleted_at.is_none()
                && !sharing.protected_channels.contains(&c.id)
        }) {
            if let Ok(plan) = resolve(topology, &[], &[channel.id], Some((policy, sharing)))
                && !plan.grants.is_empty()
            {
                channels.push(SelfApiKeyChannelOption {
                    id: channel.id,
                    channel_group_id: group.id,
                    channel_group_name: group.name.clone(),
                    channel_group_enabled: group.enabled,
                    api_formats: plan.formats,
                    name: channel.name.clone(),
                    enabled: channel.enabled,
                    auto_disabled: topology
                        .channel_capabilities
                        .iter()
                        .filter(|cap| {
                            plan.grants
                                .iter()
                                .any(|grant| grant.capability_id == cap.id)
                        })
                        .all(|cap| cap.auto_disabled),
                });
            }
        }
    }
    groups.sort_by(|a, b| (&a.name, a.id).cmp(&(&b.name, b.id)));
    channels.sort_by(|a, b| {
        (&a.channel_group_name, &a.name, a.id).cmp(&(&b.channel_group_name, &b.name, b.id))
    });
    (groups, channels)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Grant {
    pub capability_id: Uuid,
    pub group: bool,
    pub origin_id: Uuid,
}

pub(crate) struct AuthorizationPlan {
    pub grants: Vec<Grant>,
    pub formats: Vec<String>,
}

pub(crate) fn resolve(
    topology: &UpstreamTopologyRecords,
    groups: &[Uuid],
    channels: &[Uuid],
    self_service: Option<(Option<&SelfApiKeyPolicy>, &SelfApiKeySharingAccess)>,
) -> Result<AuthorizationPlan, RepositoryError> {
    let live_groups = topology
        .routing_groups
        .iter()
        .filter(|group| group.deleted_at.is_none())
        .map(|group| (group.id, group))
        .collect::<std::collections::HashMap<_, _>>();
    let live_channels = topology
        .logical_channels
        .iter()
        .filter(|channel| {
            channel.deleted_at.is_none() && live_groups.contains_key(&channel.group_id)
        })
        .map(|channel| (channel.id, channel))
        .collect::<std::collections::HashMap<_, _>>();
    if groups.iter().any(|id| !live_groups.contains_key(id))
        || channels.iter().any(|id| !live_channels.contains_key(id))
    {
        return Err(RepositoryError::ApiKeyTargetNotAllowed);
    }
    let mut grants = Vec::new();
    let mut formats = BTreeSet::new();
    for capability in topology
        .channel_capabilities
        .iter()
        .filter(|capability| capability.deleted_at.is_none())
    {
        let Some(channel) = live_channels.get(&capability.channel_id) else {
            continue;
        };
        let group = live_groups[&channel.group_id];
        let group_selected = groups.contains(&group.id);
        let channel_selected = channels.contains(&channel.id);
        if !group_selected && !channel_selected {
            continue;
        }
        let (allow_group, allow_channel) = if let Some((policy, sharing)) = self_service {
            let protected = sharing.protected_channels.contains(&channel.id);
            let owned = sharing.owned_channels.contains(&channel.id);
            let policy_allows = policy.is_some_and(|policy| {
                policy.enabled
                    && topology.policy_grants.iter().any(|grant| {
                        grant.policy_id == policy.id
                            && grant.capability_id == capability.id
                            && match grant.origin_kind {
                                GrantOriginKind::Group => grant.origin_id == group.id,
                                GrantOriginKind::Channel => grant.origin_id == channel.id,
                                GrantOriginKind::Capability => grant.origin_id == capability.id,
                            }
                    })
            });
            let ordinary = !protected && !group.sharing_only && policy_allows;
            let sharing_operation = owned
                && capability.settings.operation
                    != crate::domain::ApiOperation::StandaloneWebSearch;
            (ordinary, ordinary || sharing_operation)
        } else {
            (true, true)
        };
        if group_selected && allow_group {
            grants.push(Grant {
                capability_id: capability.id,
                group: true,
                origin_id: group.id,
            });
        }
        if channel_selected && allow_channel {
            grants.push(Grant {
                capability_id: capability.id,
                group: false,
                origin_id: channel.id,
            });
        }
        if (group_selected && allow_group) || (channel_selected && allow_channel) {
            formats.insert(
                capability
                    .settings
                    .operation
                    .api_format()
                    .as_str()
                    .to_owned(),
            );
        }
    }
    if let Some((policy, _)) = self_service
        && (groups
            .iter()
            .any(|id| !grants.iter().any(|g| g.group && g.origin_id == *id))
            || channels
                .iter()
                .any(|id| !grants.iter().any(|g| !g.group && g.origin_id == *id)))
    {
        return Err(match policy {
            None => RepositoryError::DefaultApiKeyPolicyRequired,
            Some(policy) if !policy.enabled => RepositoryError::DefaultApiKeyPolicyDisabled,
            Some(_) => RepositoryError::ApiKeyTargetNotAllowed,
        });
    }
    Ok(AuthorizationPlan {
        grants,
        formats: formats.into_iter().collect(),
    })
}

fn previous_targets(before: &Value, field: &str) -> Result<Vec<Uuid>, RepositoryError> {
    before.get(field).map_or(Ok(Vec::new()), |value| {
        serde_json::from_value(value.clone()).map_err(|_| RepositoryError::Validation)
    })
}

fn resolve_added(
    topology: &UpstreamTopologyRecords,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
) -> Result<AuthorizationPlan, RepositoryError> {
    let old_groups = previous_targets(before, "allowed_group_ids")?;
    let old_channels = previous_targets(before, "allowed_channel_ids")?;
    resolve(
        topology,
        &groups
            .iter()
            .copied()
            .filter(|id| !old_groups.contains(id))
            .collect::<Vec<_>>(),
        &channels
            .iter()
            .copied()
            .filter(|id| !old_channels.contains(id))
            .collect::<Vec<_>>(),
        None,
    )
}

// A new self-service format must not activate dormant admin grants on retained targets.
fn was_effective(topology: &UpstreamTopologyRecords, before: &Value, capability_id: Uuid) -> bool {
    topology.channel_capabilities.iter().any(|capability| {
        capability.id == capability_id
            && before["allowed_api_formats"]
                .as_array()
                .is_some_and(|formats| {
                    formats.iter().any(|format| {
                        format.as_str() == Some(capability.settings.operation.api_format().as_str())
                    })
                })
    })
}

/// Retained origins never expand, even when a different target is added. Empty
/// origins are retained too: an empty draft cannot later become an implicit grant.
fn reconcile(
    existing: Vec<Grant>,
    plan: AuthorizationPlan,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
) -> Result<Vec<Grant>, RepositoryError> {
    let old_groups = previous_targets(before, "allowed_group_ids")?;
    let old_channels = previous_targets(before, "allowed_channel_ids")?;
    let mut result = existing
        .into_iter()
        .filter(|grant| {
            if grant.group {
                groups.contains(&grant.origin_id)
            } else {
                channels.contains(&grant.origin_id)
            }
        })
        .collect::<HashSet<_>>();
    result.extend(plan.grants.into_iter().filter(|grant| {
        if grant.group {
            !old_groups.contains(&grant.origin_id)
        } else {
            !old_channels.contains(&grant.origin_id)
        }
    }));
    let mut result = result.into_iter().collect::<Vec<_>>();
    result.sort_by_key(|grant| (grant.group, grant.origin_id, grant.capability_id));
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn pg_write(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    policy: bool,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
    plan: Option<AuthorizationPlan>,
) -> Result<(), RepositoryError> {
    let topology = super::pg_load(transaction).await?;
    let self_service = plan.is_some();
    let plan = match plan {
        Some(plan) => plan,
        None => resolve_added(&topology, before, groups, channels)?,
    };
    let existing = if policy {
        topology
            .policy_grants
            .iter()
            .filter(|g| g.policy_id == id)
            .map(|g| (g.capability_id, g.origin_kind, g.origin_id))
            .collect::<Vec<_>>()
    } else {
        topology
            .api_key_grants
            .iter()
            .filter(|g| g.api_key_id == id)
            .map(|g| (g.capability_id, g.origin_kind, g.origin_id))
            .collect::<Vec<_>>()
    };
    if existing
        .iter()
        .any(|(_, kind, _)| *kind == GrantOriginKind::Capability)
    {
        return Err(RepositoryError::Validation);
    }
    let grants = reconcile(
        existing
            .into_iter()
            .filter(|(capability_id, _, _)| {
                !self_service || was_effective(&topology, before, *capability_id)
            })
            .map(|(capability_id, kind, origin_id)| Grant {
                capability_id,
                group: kind == GrantOriginKind::Group,
                origin_id,
            })
            .collect(),
        plan,
        before,
        groups,
        channels,
    )?;
    let (select, delete, insert) = if policy {
        (
            "SELECT capability_id,origin_kind,origin_id FROM api_key_policy_capability_grants WHERE policy_id=$1",
            "DELETE FROM api_key_policy_capability_grants WHERE policy_id=$1 AND capability_id=$2 AND origin_kind=$3 AND origin_id=$4",
            "INSERT INTO api_key_policy_capability_grants (policy_id,capability_id,origin_kind,origin_id) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        )
    } else {
        (
            "SELECT capability_id,origin_kind,origin_id FROM api_key_capability_grants WHERE api_key_id=$1",
            "DELETE FROM api_key_capability_grants WHERE api_key_id=$1 AND capability_id=$2 AND origin_kind=$3 AND origin_id=$4",
            "INSERT INTO api_key_capability_grants (api_key_id,capability_id,origin_kind,origin_id) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING",
        )
    };
    // Delete only revoked rows so non-authorization edits preserve grant facts.
    let rows = sqlx::query_as::<_, (Uuid, String, Uuid)>(select)
        .bind(id)
        .fetch_all(&mut **transaction)
        .await?;
    for (capability_id, kind, origin_id) in rows {
        if !grants.contains(&Grant {
            capability_id,
            group: kind == "group",
            origin_id,
        }) {
            sqlx::query(delete)
                .bind(id)
                .bind(capability_id)
                .bind(kind)
                .bind(origin_id)
                .execute(&mut **transaction)
                .await?;
        }
    }
    for grant in grants {
        sqlx::query(insert)
            .bind(id)
            .bind(grant.capability_id)
            .bind(if grant.group { "group" } else { "channel" })
            .bind(grant.origin_id)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(feature = "sqlite-backend")]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sqlite_write(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    policy: bool,
    before: &Value,
    groups: &[Uuid],
    channels: &[Uuid],
    plan: Option<AuthorizationPlan>,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let topology = super::sqlite_load(transaction).await?;
    let self_service = plan.is_some();
    let plan = match plan {
        Some(plan) => plan,
        None => resolve_added(&topology, before, groups, channels)?,
    };
    let existing = if policy {
        topology
            .policy_grants
            .iter()
            .filter(|g| g.policy_id == id)
            .map(|g| (g.capability_id, g.origin_kind, g.origin_id))
            .collect::<Vec<_>>()
    } else {
        topology
            .api_key_grants
            .iter()
            .filter(|g| g.api_key_id == id)
            .map(|g| (g.capability_id, g.origin_kind, g.origin_id))
            .collect::<Vec<_>>()
    };
    if existing
        .iter()
        .any(|(_, kind, _)| *kind == GrantOriginKind::Capability)
    {
        return Err(RepositoryError::Validation);
    }
    let grants = reconcile(
        existing
            .into_iter()
            .filter(|(capability_id, _, _)| {
                !self_service || was_effective(&topology, before, *capability_id)
            })
            .map(|(capability_id, kind, origin_id)| Grant {
                capability_id,
                group: kind == GrantOriginKind::Group,
                origin_id,
            })
            .collect(),
        plan,
        before,
        groups,
        channels,
    )?;
    let (select, delete, insert) = if policy {
        (
            "SELECT capability_id,origin_kind,origin_id FROM api_key_policy_capability_grants WHERE policy_id=?",
            "DELETE FROM api_key_policy_capability_grants WHERE policy_id=? AND capability_id=? AND origin_kind=? AND origin_id=?",
            "INSERT INTO api_key_policy_capability_grants (policy_id,capability_id,origin_kind,origin_id) VALUES (?,?,?,?) ON CONFLICT DO NOTHING",
        )
    } else {
        (
            "SELECT capability_id,origin_kind,origin_id FROM api_key_capability_grants WHERE api_key_id=?",
            "DELETE FROM api_key_capability_grants WHERE api_key_id=? AND capability_id=? AND origin_kind=? AND origin_id=?",
            "INSERT INTO api_key_capability_grants (api_key_id,capability_id,origin_kind,origin_id) VALUES (?,?,?,?) ON CONFLICT DO NOTHING",
        )
    };
    let rows = sqlx::query_as::<_, (SqliteUuid, String, SqliteUuid)>(select)
        .bind(SqliteUuid(id))
        .fetch_all(&mut **transaction)
        .await?;
    for (capability_id, kind, origin_id) in rows {
        if !grants.contains(&Grant {
            capability_id: capability_id.0,
            group: kind == "group",
            origin_id: origin_id.0,
        }) {
            sqlx::query(delete)
                .bind(SqliteUuid(id))
                .bind(capability_id)
                .bind(kind)
                .bind(origin_id)
                .execute(&mut **transaction)
                .await?;
        }
    }
    for grant in grants {
        sqlx::query(insert)
            .bind(SqliteUuid(id))
            .bind(SqliteUuid(grant.capability_id))
            .bind(if grant.group { "group" } else { "channel" })
            .bind(SqliteUuid(grant.origin_id))
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}
