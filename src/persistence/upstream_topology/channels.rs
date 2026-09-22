//! Logical-channel writes bind one access and at most one credential.
//!
//! A channel save or delete never creates capabilities, routes, grants, or a
//! credential, and never touches a provider-managed (Codex) channel: that
//! lifecycle belongs to the dedicated connector APIs. Scope, connector, and
//! target checks run for every non-deleted owner, including disabled ones, so a
//! disabled draft cannot hide a binding that the snapshot compiler would reject.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{LogicalChannelInput, LogicalChannelRecord, UpstreamTopologyRecords};
use crate::{
    domain::ConnectorKind,
    persistence::{MutationResult, RepositoryError},
};

fn validate_input(input: &LogicalChannelInput) -> Result<(), RepositoryError> {
    if input.name.trim().is_empty() || input.name.chars().count() > 100 {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

/// Structural graph checks that do not need credential rows. A missing or
/// tombstoned group/access is a routing dependency; a sharing-only group may
/// only own Codex channels, which generic writes must never create or rebind.
fn validate_replacement(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &LogicalChannelInput,
    expected: Option<DateTime<Utc>>,
) -> Result<(), RepositoryError> {
    validate_input(input)?;
    let existing = topology
        .logical_channels
        .iter()
        .find(|channel| channel.id == id);
    match (existing, expected) {
        (None, Some(_)) => return Err(RepositoryError::NotFound),
        (Some(_), None) => return Err(RepositoryError::Conflict),
        (Some(channel), Some(expected)) => {
            if channel.deleted_at.is_some() {
                return Err(RepositoryError::NotFound);
            }
            if channel.updated_at != expected {
                return Err(RepositoryError::Conflict);
            }
        }
        (None, None) => {}
    }
    let group = topology
        .routing_groups
        .iter()
        .find(|group| group.id == input.group_id && group.deleted_at.is_none())
        .ok_or(RepositoryError::RoutingDependencyInvalid)?;
    let access = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == input.access_id && access.deleted_at.is_none())
        .ok_or(RepositoryError::RoutingDependencyInvalid)?;
    if access.connector_kind == ConnectorKind::CodexOauth {
        return Err(RepositoryError::ProviderManagedResource);
    }
    if group.sharing_only {
        return Err(RepositoryError::Validation);
    }
    if existing.is_some_and(|channel| {
        topology
            .upstream_accesses
            .iter()
            .find(|access| access.id == channel.access_id)
            .is_some_and(|access| access.connector_kind == ConnectorKind::CodexOauth)
    }) {
        return Err(RepositoryError::ProviderManagedResource);
    }
    let name = input.name.trim();
    if topology.logical_channels.iter().any(|channel| {
        channel.id != id
            && channel.deleted_at.is_none()
            && channel.group_id == input.group_id
            && channel.name.trim() == name
    }) {
        return Err(RepositoryError::Conflict);
    }
    Ok(())
}

/// Every non-deleted capability under the channel is a routing target, whether
/// enabled or not; only a tombstoned capability releases the channel.
fn check_delete(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let channel = topology
        .logical_channels
        .iter()
        .find(|channel| channel.id == id && channel.deleted_at.is_none())
        .ok_or(RepositoryError::NotFound)?;
    if channel.updated_at != expected {
        return Err(RepositoryError::Conflict);
    }
    if topology
        .channel_capabilities
        .iter()
        .any(|capability| capability.channel_id == id && capability.deleted_at.is_none())
    {
        return Err(RepositoryError::RoutingDependencyInvalid);
    }
    Ok(())
}

fn audit(record: Option<&LogicalChannelRecord>) -> serde_json::Value {
    record.map_or_else(
        || json!({}),
        |record| {
            json!({
                "id": record.id,
                "group_id": record.group_id,
                "access_id": record.access_id,
                "credential_id": record.credential_id,
                "name": record.name,
                "enabled": record.enabled,
                "binding_revision": record.binding_revision,
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
    let previous = before
        .logical_channels
        .iter()
        .find(|channel| channel.id == id);
    let current = after
        .logical_channels
        .iter()
        .find(|channel| channel.id == id)
        .ok_or(RepositoryError::NotFound)?;
    Ok(MutationResult {
        id,
        object_type: "logical_channel",
        action,
        before_redacted: audit(previous),
        after_redacted: audit(Some(current)),
        created_secret: None,
        reason: None,
        updated_at: current.updated_at,
        correlation_id: None,
    })
}

/// Runs inside the coordinator's serializable transaction. The credential is
/// locked `FOR SHARE` and its exact target scope re-validated before the
/// binding changes; the caller still owns audit, commit, and snapshot
/// publication.
pub async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &LogicalChannelInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM upstream_channels WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    let base_url = before
        .upstream_accesses
        .iter()
        .find(|access| access.id == input.access_id)
        .map(|access| access.base_url.clone())
        .ok_or(RepositoryError::RoutingDependencyInvalid)?;
    crate::persistence::upstream_credentials::pg_validate_binding(
        transaction,
        input.credential_id,
        &base_url,
    )
    .await?;
    let binding_revision = Uuid::new_v4();
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE upstream_channels SET group_id=$2,access_id=$3,credential_id=$4,name=$5,
             enabled=$6,binding_revision=$7
             WHERE id=$1 AND updated_at=$8 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(input.group_id)
        .bind(input.access_id)
        .bind(input.credential_id)
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(binding_revision)
        .bind(expected)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO upstream_channels
             (id,group_id,access_id,credential_id,name,enabled,binding_revision)
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(id)
        .bind(input.group_id)
        .bind(input.access_id)
        .bind(input.credential_id)
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(binding_revision)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    if expected.is_none() {
        sqlx::query(
            "INSERT INTO channel_identity_registry
             (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id)
             SELECT c.id,c.name,c.created_at,c.id,NULL,NULL FROM upstream_channels c WHERE c.id=$1",
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
    input: &LogicalChannelInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    if let Some(credential_id) = input.credential_id {
        let base_url = before
            .upstream_accesses
            .iter()
            .find(|access| access.id == input.access_id)
            .map(|access| access.base_url.clone())
            .ok_or(RepositoryError::RoutingDependencyInvalid)?;
        let record = crate::persistence::sqlite::credential_records(transaction)
            .await?
            .into_iter()
            .find(|record| record.id == credential_id)
            .ok_or(RepositoryError::Validation)?;
        crate::persistence::upstream_credentials::validate_static_binding(&record, &base_url)?;
    }
    let binding_revision = Uuid::new_v4();
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE upstream_channels SET group_id=?,access_id=?,credential_id=?,name=?,
             enabled=?,binding_revision=?,updated_at=ag_now()
             WHERE id=? AND updated_at=? AND deleted_at IS NULL",
        )
        .bind(SqliteUuid(input.group_id))
        .bind(SqliteUuid(input.access_id))
        .bind(input.credential_id.map(SqliteUuid))
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(SqliteUuid(binding_revision))
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected))
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO upstream_channels
             (id,group_id,access_id,credential_id,name,enabled,binding_revision)
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(input.group_id))
        .bind(SqliteUuid(input.access_id))
        .bind(input.credential_id.map(SqliteUuid))
        .bind(input.name.trim())
        .bind(input.enabled)
        .bind(SqliteUuid(binding_revision))
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    if expected.is_none() {
        sqlx::query(
            "INSERT INTO channel_identity_registry
             (id,label,created_at,canonical_channel_id,codex_credential_id,capability_id)
             SELECT c.id,c.name,c.created_at,c.id,NULL,NULL FROM upstream_channels c WHERE c.id=?",
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

/// Runs inside the coordinator's serializable transaction. A channel that
/// still owns a non-tombstoned capability is a routing target and cannot be
/// deleted; nothing cascades and the row remains as history.
pub async fn pg_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM upstream_channels WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    check_delete(&before, id, expected)?;
    let changed = sqlx::query(
        "UPDATE upstream_channels SET enabled=false,deleted_at=now(),binding_revision=gen_random_uuid()
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
        "UPDATE upstream_channels SET enabled=0,deleted_at=ag_now(),
         binding_revision=ag_md5_uuid(hex(randomblob(32))),updated_at=ag_now()
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
    use crate::persistence::upstream_topology::{RoutingGroupRecord, UpstreamAccessRecord};

    fn at() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp")
    }

    fn input(group_id: u128, access_id: u128, name: &str) -> LogicalChannelInput {
        LogicalChannelInput {
            group_id: Uuid::from_u128(group_id),
            access_id: Uuid::from_u128(access_id),
            credential_id: None,
            name: name.into(),
            enabled: true,
        }
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

    fn channel(id: u128, group_id: u128, access_id: u128, name: &str) -> LogicalChannelRecord {
        LogicalChannelRecord {
            id: Uuid::from_u128(id),
            group_id: Uuid::from_u128(group_id),
            access_id: Uuid::from_u128(access_id),
            credential_id: None,
            name: name.into(),
            enabled: true,
            binding_revision: Uuid::from_u128(id + 900),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn capability(
        id: u128,
        channel_id: u128,
        deleted: bool,
    ) -> crate::persistence::ChannelCapabilityRecord {
        crate::persistence::ChannelCapabilityRecord {
            id: Uuid::from_u128(id),
            channel_id: Uuid::from_u128(channel_id),
            settings: crate::domain::CapabilitySettings {
                operation: crate::domain::ApiOperation::Responses,
                enabled: true,
                available_models: vec!["wire".into()],
                request_compression: crate::domain::RequestCompression::Default,
                test_model: None,
                test_pricing_model_id: None,
                auto_disable_allowed: false,
            },
            auto_disabled: false,
            auto_disable_reason: None,
            auto_disable_at: None,
            status_statistics_enabled: false,
            config_template_id: None,
            override_document: json!({}),
            billing_multiplier: rust_decimal::Decimal::ONE,
            revision: Uuid::from_u128(id + 700),
            created_at: at(),
            updated_at: at(),
            deleted_at: deleted.then_some(at()),
        }
    }

    fn topology() -> UpstreamTopologyRecords {
        UpstreamTopologyRecords {
            routing_groups: vec![group(1, false)],
            upstream_accesses: vec![access(2, ConnectorKind::OpenAiCompatible)],
            logical_channels: vec![channel(3, 1, 2, "channel")],
            ..Default::default()
        }
    }

    #[test]
    fn creation_requires_live_owners_and_cas_version() {
        let id = Uuid::from_u128(9);
        let topology = topology();
        assert!(validate_replacement(&topology, id, &input(1, 2, "new"), None).is_ok());
        assert!(matches!(
            validate_replacement(&topology, id, &input(1, 2, "new"), Some(at())),
            Err(RepositoryError::NotFound)
        ));
        assert!(matches!(
            validate_replacement(&topology, Uuid::from_u128(3), &input(1, 2, "channel"), None),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            validate_replacement(
                &topology,
                Uuid::from_u128(3),
                &input(1, 2, "channel"),
                Some(at() + chrono::Duration::seconds(1))
            ),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input(9, 2, "new"), None),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input(1, 9, "new"), None),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input(1, 2, "  "), None),
            Err(RepositoryError::Validation)
        ));
    }

    #[test]
    fn active_group_name_is_globally_unique_and_tombstones_release_it() {
        let topology = topology();
        assert!(matches!(
            validate_replacement(&topology, Uuid::from_u128(9), &input(1, 2, "channel"), None),
            Err(RepositoryError::Conflict)
        ));
        let mut released = topology.clone();
        released.logical_channels[0].deleted_at = Some(at());
        released.logical_channels[0].enabled = false;
        assert!(
            validate_replacement(&released, Uuid::from_u128(9), &input(1, 2, "channel"), None)
                .is_ok()
        );
    }

    #[test]
    fn provider_managed_and_sharing_only_bindings_are_rejected() {
        let id = Uuid::from_u128(3);
        let mut codex = topology();
        codex.upstream_accesses[0].connector_kind = ConnectorKind::CodexOauth;
        assert!(matches!(
            validate_replacement(&codex, id, &input(1, 2, "channel"), Some(at())),
            Err(RepositoryError::ProviderManagedResource)
        ));

        let mut sharing = topology();
        sharing.routing_groups[0].sharing_only = true;
        assert!(matches!(
            validate_replacement(&sharing, id, &input(1, 2, "channel"), Some(at())),
            Err(RepositoryError::Validation)
        ));

        let mut gone = topology();
        gone.upstream_accesses.clear();
        assert!(matches!(
            validate_replacement(&gone, id, &input(1, 2, "channel"), Some(at())),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));
    }

    #[test]
    fn delete_requires_version_and_releases_only_tombstoned_capabilities() {
        let id = Uuid::from_u128(3);
        let mut topology = topology();
        assert!(check_delete(&topology, id, at()).is_ok());
        assert!(matches!(
            check_delete(&topology, id, at() + chrono::Duration::seconds(1)),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            check_delete(&topology, Uuid::from_u128(9), at()),
            Err(RepositoryError::NotFound)
        ));

        topology.channel_capabilities = vec![capability(4, 3, false)];
        topology.channel_capabilities[0].settings.enabled = false;
        assert!(matches!(
            check_delete(&topology, id, at()),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));
        topology.channel_capabilities[0].deleted_at = Some(at());
        assert!(check_delete(&topology, id, at()).is_ok());
    }
}
