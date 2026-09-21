//! Routing-group writes own organization and the sharing switch only.
//!
//! A group save or delete never creates channels, capabilities, gates, or
//! routes; those remain explicit writes on their own owners. Creation records a
//! read-only historical identity so a retired group UUID stays resolvable.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{RoutingGroupInput, RoutingGroupRecord, UpstreamTopologyRecords};
use crate::{
    domain::ConnectorKind,
    persistence::{MutationResult, RepositoryError},
};

fn validate_input(input: &RoutingGroupInput) -> Result<(), RepositoryError> {
    if input.name.trim().is_empty() || input.name.chars().count() > 100 {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

/// Enabling sharing-only is the one unsafe group edit: it subjects every member
/// channel to the Codex sharing ledger. Require each live member channel to be
/// Codex, including disabled and unrouted drafts, so the switch can never widen
/// sharing to an ordinary connector. The switch grants nothing by itself.
fn validate_sharing_only(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &RoutingGroupInput,
) -> Result<(), RepositoryError> {
    if !input.sharing_only {
        return Ok(());
    }
    for channel in topology
        .logical_channels
        .iter()
        .filter(|channel| channel.group_id == id && channel.deleted_at.is_none())
    {
        let access = topology
            .upstream_accesses
            .iter()
            .find(|access| access.id == channel.access_id)
            .ok_or(RepositoryError::Validation)?;
        if access.connector_kind != ConnectorKind::CodexOauth {
            return Err(RepositoryError::Validation);
        }
    }
    Ok(())
}

fn validate_replacement(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &RoutingGroupInput,
    expected: Option<DateTime<Utc>>,
) -> Result<(), RepositoryError> {
    validate_input(input)?;
    let existing = topology.routing_groups.iter().find(|group| group.id == id);
    match (existing, expected) {
        (None, Some(_)) => return Err(RepositoryError::NotFound),
        (Some(_), None) => return Err(RepositoryError::Conflict),
        (Some(group), Some(expected)) => {
            if group.deleted_at.is_some() {
                return Err(RepositoryError::NotFound);
            }
            if group.updated_at != expected {
                return Err(RepositoryError::Conflict);
            }
        }
        (None, None) => {}
    }
    let name = input.name.trim();
    if topology
        .routing_groups
        .iter()
        .any(|group| group.id != id && group.deleted_at.is_none() && group.name.trim() == name)
    {
        return Err(RepositoryError::Conflict);
    }
    validate_sharing_only(topology, id, input)
}

/// Any live member channel blocks deletion, even when disabled or unrouted;
/// only a soft-deleted channel releases the group.
fn check_delete(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let group = topology
        .routing_groups
        .iter()
        .find(|group| group.id == id && group.deleted_at.is_none())
        .ok_or(RepositoryError::NotFound)?;
    if group.updated_at != expected {
        return Err(RepositoryError::Conflict);
    }
    if topology
        .logical_channels
        .iter()
        .any(|channel| channel.group_id == id && channel.deleted_at.is_none())
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn audit(record: Option<&RoutingGroupRecord>) -> serde_json::Value {
    record.map_or_else(
        || json!({}),
        |record| {
            json!({
                "id": record.id,
                "name": record.name,
                "enabled": record.enabled,
                "sharing_only": record.sharing_only,
                "created_at": record.created_at,
                "updated_at": record.updated_at,
                "deleted_at": record.deleted_at,
            })
        },
    )
}

fn result(
    before: &UpstreamTopologyRecords,
    after: &UpstreamTopologyRecords,
    id: Uuid,
    action: &'static str,
) -> Result<MutationResult, RepositoryError> {
    let previous = before.routing_groups.iter().find(|group| group.id == id);
    let current = after
        .routing_groups
        .iter()
        .find(|group| group.id == id)
        .ok_or(RepositoryError::NotFound)?;
    Ok(MutationResult {
        id,
        object_type: "routing_group",
        action,
        before_redacted: audit(previous),
        after_redacted: audit(Some(current)),
        created_secret: None,
        reason: None,
        updated_at: current.updated_at,
        correlation_id: None,
    })
}

/// Runs inside the coordinator's serializable transaction. The caller still
/// owns audit, commit, and snapshot publication.
pub async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &RoutingGroupInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM routing_groups WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE routing_groups SET name=$2,enabled=$3,sharing_only=$4
             WHERE id=$1 AND updated_at=$5 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(input.sharing_only)
        .bind(expected)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO routing_groups (id,name,enabled,sharing_only) VALUES ($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(input.sharing_only)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    if expected.is_none() {
        sqlx::query(
            "INSERT INTO group_identity_registry (id,label,created_at,canonical_group_id)
             SELECT g.id,g.name,g.created_at,g.id FROM routing_groups g WHERE g.id=$1",
        )
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    }
    super::pg_load_control_plane(transaction).await?;
    let after = super::pg_load(transaction).await?;
    result(
        &before,
        &after,
        id,
        if expected.is_none() {
            "create"
        } else {
            "update"
        },
    )
}

#[cfg(feature = "sqlite-backend")]
/// Runs inside the database owner's sole-writer transaction.
pub async fn sqlite_save(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    input: &RoutingGroupInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE routing_groups SET name=?,enabled=?,sharing_only=?,updated_at=ag_now()
             WHERE id=? AND updated_at=? AND deleted_at IS NULL",
        )
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(input.sharing_only)
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected))
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query("INSERT INTO routing_groups (id,name,enabled,sharing_only) VALUES (?,?,?,?)")
            .bind(SqliteUuid(id))
            .bind(input.name.trim())
            .bind(input.enabled)
            .bind(input.sharing_only)
            .execute(&mut **transaction)
            .await?
            .rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    if expected.is_none() {
        sqlx::query(
            "INSERT INTO group_identity_registry (id,label,created_at,canonical_group_id)
             SELECT g.id,g.name,g.created_at,g.id FROM routing_groups g WHERE g.id=?",
        )
        .bind(SqliteUuid(id))
        .execute(&mut **transaction)
        .await?;
    }
    super::sqlite_load_control_plane(transaction).await?;
    let after = super::sqlite_load(transaction).await?;
    result(
        &before,
        &after,
        id,
        if expected.is_none() {
            "create"
        } else {
            "update"
        },
    )
}

/// Runs inside the coordinator's serializable transaction. Dependent channels
/// must already be soft-deleted; nothing cascades.
pub async fn pg_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM routing_groups WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    check_delete(&before, id, expected)?;
    let changed = sqlx::query(
        "UPDATE routing_groups SET enabled=false,deleted_at=now()
         WHERE id=$1 AND updated_at=$2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(expected)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    super::pg_load_control_plane(transaction).await?;
    let after = super::pg_load(transaction).await?;
    result(&before, &after, id, "delete")
}

#[cfg(feature = "sqlite-backend")]
/// Runs inside the database owner's sole-writer transaction.
pub async fn sqlite_delete(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    check_delete(&before, id, expected)?;
    let changed = sqlx::query(
        "UPDATE routing_groups SET enabled=0,deleted_at=ag_now(),updated_at=ag_now()
         WHERE id=? AND updated_at=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(id))
    .bind(SqliteTimestamp(expected))
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    super::sqlite_load_control_plane(transaction).await?;
    let after = super::sqlite_load(transaction).await?;
    result(&before, &after, id, "delete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::upstream_topology::{LogicalChannelRecord, UpstreamAccessRecord};

    fn at() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp")
    }

    fn input(name: &str) -> RoutingGroupInput {
        RoutingGroupInput {
            name: name.into(),
            enabled: true,
            sharing_only: false,
        }
    }

    fn group(id: u128, name: &str) -> RoutingGroupRecord {
        RoutingGroupRecord {
            id: Uuid::from_u128(id),
            name: name.into(),
            enabled: true,
            sharing_only: false,
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn access(id: u128, connector: ConnectorKind) -> UpstreamAccessRecord {
        UpstreamAccessRecord {
            id: Uuid::from_u128(id),
            name: format!("access-{id}"),
            connector_kind: connector,
            base_url: "https://upstream.example".into(),
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

    fn channel(id: u128, group_id: u128, access_id: u128) -> LogicalChannelRecord {
        LogicalChannelRecord {
            id: Uuid::from_u128(id),
            group_id: Uuid::from_u128(group_id),
            access_id: Uuid::from_u128(access_id),
            credential_id: None,
            name: format!("channel-{id}"),
            enabled: true,
            binding_revision: Uuid::from_u128(id + 900),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    #[test]
    fn replacement_cas_and_active_name_are_enforced() {
        let id = Uuid::from_u128(1);
        let mut topology = UpstreamTopologyRecords::default();
        assert!(validate_replacement(&topology, id, &input("group"), None).is_ok());
        assert!(matches!(
            validate_replacement(&topology, id, &input("group"), Some(at())),
            Err(RepositoryError::NotFound)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input("  "), None),
            Err(RepositoryError::Validation)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input(&"x".repeat(101)), None),
            Err(RepositoryError::Validation)
        ));

        topology.routing_groups = vec![group(1, "group")];
        assert!(matches!(
            validate_replacement(&topology, id, &input("renamed"), None),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            validate_replacement(
                &topology,
                id,
                &input("renamed"),
                Some(at() - chrono::Duration::seconds(1))
            ),
            Err(RepositoryError::Conflict)
        ));
        assert!(validate_replacement(&topology, id, &input("renamed"), Some(at())).is_ok());
        let mut deleted = topology.clone();
        deleted.routing_groups[0].deleted_at = Some(at());
        deleted.routing_groups[0].enabled = false;
        assert!(matches!(
            validate_replacement(&deleted, id, &input("group"), Some(at())),
            Err(RepositoryError::NotFound)
        ));

        topology.routing_groups.push(group(2, "taken"));
        assert!(matches!(
            validate_replacement(&topology, id, &input("taken"), Some(at())),
            Err(RepositoryError::Conflict)
        ));
        let mut released = topology.clone();
        released.routing_groups[1].deleted_at = Some(at());
        released.routing_groups[1].enabled = false;
        assert!(validate_replacement(&released, id, &input("taken"), Some(at())).is_ok());
    }

    #[test]
    fn sharing_only_requires_every_live_member_to_be_codex() {
        let id = Uuid::from_u128(1);
        let mut topology = UpstreamTopologyRecords {
            routing_groups: vec![group(1, "group")],
            upstream_accesses: vec![access(2, ConnectorKind::CodexOauth)],
            logical_channels: vec![channel(3, 1, 2)],
            ..Default::default()
        };

        let mut sharing = input("group");
        sharing.sharing_only = true;
        assert!(validate_replacement(&topology, id, &sharing, Some(at())).is_ok());

        topology
            .upstream_accesses
            .push(access(4, ConnectorKind::OpenAiCompatible));
        topology.logical_channels.push(channel(5, 1, 4));
        assert!(matches!(
            validate_replacement(&topology, id, &sharing, Some(at())),
            Err(RepositoryError::Validation)
        ));
        assert!(validate_replacement(&topology, id, &input("group"), Some(at())).is_ok());

        topology.logical_channels[1].deleted_at = Some(at());
        assert!(validate_replacement(&topology, id, &sharing, Some(at())).is_ok());

        topology.logical_channels.clear();
        topology.logical_channels.push(channel(6, 1, 99));
        assert!(matches!(
            validate_replacement(&topology, id, &sharing, Some(at())),
            Err(RepositoryError::Validation)
        ));
    }

    #[test]
    fn delete_checks_version_and_live_members_only() {
        let id = Uuid::from_u128(1);
        let mut topology = UpstreamTopologyRecords {
            routing_groups: vec![group(1, "group")],
            ..Default::default()
        };
        assert!(check_delete(&topology, id, at()).is_ok());
        assert!(matches!(
            check_delete(&topology, id, at() - chrono::Duration::seconds(1)),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            check_delete(&topology, Uuid::from_u128(9), at()),
            Err(RepositoryError::NotFound)
        ));

        topology.upstream_accesses = vec![access(2, ConnectorKind::OpenAiCompatible)];
        topology.logical_channels = vec![channel(3, 1, 2)];
        assert!(matches!(
            check_delete(&topology, id, at()),
            Err(RepositoryError::Validation)
        ));
        topology.logical_channels[0].enabled = false;
        assert!(matches!(
            check_delete(&topology, id, at()),
            Err(RepositoryError::Validation)
        ));
        topology.logical_channels[0].deleted_at = Some(at());
        assert!(check_delete(&topology, id, at()).is_ok());
    }
}
