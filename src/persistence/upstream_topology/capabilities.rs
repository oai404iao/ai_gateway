//! Channel-capability writes own the operation, catalogue,
//! transforms, compression, and probe configuration of one logical channel.
//!
//! Writes never create grants or route candidates. Existing logical-channel
//! grants already cover new capabilities; routes remain explicit.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{ChannelCapabilityInput, ChannelCapabilityRecord, UpstreamTopologyRecords};
use crate::persistence::{MutationResult, RepositoryError};

fn validate_replacement(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &ChannelCapabilityInput,
    expected: Option<DateTime<Utc>>,
) -> Result<(), RepositoryError> {
    if !input.override_document.is_object() || input.billing_multiplier.is_sign_negative() {
        return Err(RepositoryError::Validation);
    }
    let existing = topology
        .channel_capabilities
        .iter()
        .find(|capability| capability.id == id);
    match (existing, expected) {
        (None, Some(_)) => return Err(RepositoryError::NotFound),
        (Some(_), None) => return Err(RepositoryError::Conflict),
        (Some(capability), Some(expected)) => {
            if capability.deleted_at.is_some() {
                return Err(RepositoryError::NotFound);
            }
            if capability.updated_at != expected {
                return Err(RepositoryError::Conflict);
            }
            if capability.channel_id != input.channel_id
                || capability.settings.operation != input.settings.operation
            {
                return Err(RepositoryError::Validation);
            }
        }
        (None, None) => {}
    }
    let channel = topology
        .logical_channels
        .iter()
        .find(|channel| channel.id == input.channel_id && channel.deleted_at.is_none())
        .ok_or(RepositoryError::RoutingDependencyInvalid)?;
    let access = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == channel.access_id && access.deleted_at.is_none())
        .ok_or(RepositoryError::RoutingDependencyInvalid)?;
    input
        .settings
        .validate(access.connector_kind)
        .map_err(|_| RepositoryError::Validation)?;
    if topology.channel_capabilities.iter().any(|capability| {
        capability.id != id
            && capability.channel_id == input.channel_id
            && capability.settings.operation == input.settings.operation
    }) {
        return Err(RepositoryError::Conflict);
    }
    Ok(())
}

fn check_delete(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    let capability = topology
        .channel_capabilities
        .iter()
        .find(|capability| capability.id == id && capability.deleted_at.is_none())
        .ok_or(RepositoryError::NotFound)?;
    if capability.updated_at != expected {
        return Err(RepositoryError::Conflict);
    }
    Ok(())
}

fn affected_rules(topology: &UpstreamTopologyRecords, id: Uuid) -> Vec<Uuid> {
    let tiers = topology
        .operation_candidates
        .iter()
        .filter(|candidate| candidate.capability_id == id)
        .map(|candidate| candidate.tier_id)
        .collect::<std::collections::HashSet<_>>();
    topology
        .operation_tiers
        .iter()
        .filter(|tier| tiers.contains(&tier.id))
        .map(|tier| tier.rule_id)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

async fn pg_validate_references(
    connection: &mut sqlx::PgConnection,
    input: &ChannelCapabilityInput,
) -> Result<(), RepositoryError> {
    if let Some(template_id) = input.config_template_id {
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM config_templates WHERE id=$1 AND enabled)",
        )
        .bind(template_id)
        .fetch_one(&mut *connection)
        .await?;
        if !live {
            return Err(RepositoryError::Validation);
        }
    }
    if let Some(model_id) = input.settings.test_pricing_model_id {
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=$1 AND deleted_at IS NULL)",
        )
        .bind(model_id)
        .fetch_one(&mut *connection)
        .await?;
        if !live {
            return Err(RepositoryError::Validation);
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
async fn sqlite_validate_references(
    connection: &mut sqlx::SqliteConnection,
    input: &ChannelCapabilityInput,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    if let Some(template_id) = input.config_template_id {
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM config_templates WHERE id=? AND enabled=1)",
        )
        .bind(SqliteUuid(template_id))
        .fetch_one(&mut *connection)
        .await?;
        if !live {
            return Err(RepositoryError::Validation);
        }
    }
    if let Some(model_id) = input.settings.test_pricing_model_id {
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM models WHERE id=? AND deleted_at IS NULL)",
        )
        .bind(SqliteUuid(model_id))
        .fetch_one(&mut *connection)
        .await?;
        if !live {
            return Err(RepositoryError::Validation);
        }
    }
    Ok(())
}

fn audit(record: Option<&ChannelCapabilityRecord>) -> serde_json::Value {
    record.map_or_else(
        || json!({}),
        |record| {
            json!({
                "id": record.id,
                "channel_id": record.channel_id,
                "settings": record.settings,
                "auto_disabled": record.auto_disabled,
                "auto_disable_reason": record.auto_disable_reason,
                "auto_disable_at": record.auto_disable_at,
                "status_statistics_enabled": record.status_statistics_enabled,
                "config_template_id": record.config_template_id,
                "billing_multiplier": record.billing_multiplier,
                "revision": record.revision,
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
        .channel_capabilities
        .iter()
        .find(|capability| capability.id == id);
    let current = after
        .channel_capabilities
        .iter()
        .find(|capability| capability.id == id)
        .ok_or(RepositoryError::NotFound)?;
    let mut after_redacted = audit(Some(current));
    if action == "delete" {
        after_redacted["detached_routes"] = json!(
            affected_rules(before, id)
                .into_iter()
                .map(|rule_id| {
                    let disabled = after
                        .operation_rules
                        .iter()
                        .any(|rule| rule.id == rule_id && !rule.enabled);
                    json!({ "rule_id": rule_id, "disabled": disabled })
                })
                .collect::<Vec<_>>()
        );
    }
    Ok(MutationResult {
        id,
        object_type: "channel_capability",
        action,
        before_redacted: audit(previous),
        after_redacted,
        created_secret: None,
        reason: None,
        updated_at: current.updated_at,
        correlation_id: None,
    })
}

/// Runs inside the coordinator's serializable transaction. The owning channel
/// row is locked so concurrent capability writes for one channel serialize; the
/// caller still owns audit, commit, and snapshot publication.
pub async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &ChannelCapabilityInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM upstream_channels WHERE id=$1 FOR UPDATE")
        .bind(input.channel_id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    pg_validate_references(transaction, input).await?;
    let revision = Uuid::new_v4();
    let settings = &input.settings;
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE channel_capabilities SET operation=$2,enabled=$3,
             available_models=$4,request_compression=$5,test_model=$6,test_pricing_model_id=$7,
             auto_disable_allowed=$8,status_statistics_enabled=$9,config_template_id=$10,
             override_document=$11,billing_multiplier=$12,revision=$13
             WHERE id=$1 AND updated_at=$14 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(settings.operation.as_str())
        .bind(settings.enabled)
        .bind(&settings.available_models)
        .bind(settings.request_compression.as_str())
        .bind(settings.test_model.as_deref())
        .bind(settings.test_pricing_model_id)
        .bind(settings.auto_disable_allowed)
        .bind(input.status_statistics_enabled)
        .bind(input.config_template_id)
        .bind(&input.override_document)
        .bind(input.billing_multiplier)
        .bind(revision)
        .bind(expected)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO channel_capabilities
             (id,channel_id,operation,enabled,available_models,request_compression,
              test_model,test_pricing_model_id,auto_disable_allowed,status_statistics_enabled,
              config_template_id,override_document,billing_multiplier,revision)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(id)
        .bind(input.channel_id)
        .bind(settings.operation.as_str())
        .bind(settings.enabled)
        .bind(&settings.available_models)
        .bind(settings.request_compression.as_str())
        .bind(settings.test_model.as_deref())
        .bind(settings.test_pricing_model_id)
        .bind(settings.auto_disable_allowed)
        .bind(input.status_statistics_enabled)
        .bind(input.config_template_id)
        .bind(&input.override_document)
        .bind(input.billing_multiplier)
        .bind(revision)
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
             SELECT cap.id,c.name,cap.created_at,c.id,NULL,cap.id
             FROM channel_capabilities cap JOIN upstream_channels c ON c.id=cap.channel_id
             WHERE cap.id=$1",
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
    input: &ChannelCapabilityInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    validate_replacement(&before, id, input, expected)?;
    sqlite_validate_references(transaction, input).await?;
    let revision = Uuid::new_v4();
    let settings = &input.settings;
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE channel_capabilities SET operation=?,enabled=?,
             available_models=?,request_compression=?,test_model=?,test_pricing_model_id=?,
             auto_disable_allowed=?,status_statistics_enabled=?,config_template_id=?,
             override_document=?,billing_multiplier=?,revision=?,updated_at=ag_now()
             WHERE id=? AND updated_at=? AND deleted_at IS NULL",
        )
        .bind(settings.operation.as_str())
        .bind(settings.enabled)
        .bind(sqlx::types::Json(&settings.available_models))
        .bind(settings.request_compression.as_str())
        .bind(settings.test_model.as_deref())
        .bind(settings.test_pricing_model_id.map(SqliteUuid))
        .bind(settings.auto_disable_allowed)
        .bind(input.status_statistics_enabled)
        .bind(input.config_template_id.map(SqliteUuid))
        .bind(sqlx::types::Json(&input.override_document))
        .bind(input.billing_multiplier.to_string())
        .bind(SqliteUuid(revision))
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected))
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO channel_capabilities
             (id,channel_id,operation,enabled,available_models,request_compression,
              test_model,test_pricing_model_id,auto_disable_allowed,status_statistics_enabled,
              config_template_id,override_document,billing_multiplier,revision)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(SqliteUuid(id))
        .bind(SqliteUuid(input.channel_id))
        .bind(settings.operation.as_str())
        .bind(settings.enabled)
        .bind(sqlx::types::Json(&settings.available_models))
        .bind(settings.request_compression.as_str())
        .bind(settings.test_model.as_deref())
        .bind(settings.test_pricing_model_id.map(SqliteUuid))
        .bind(settings.auto_disable_allowed)
        .bind(input.status_statistics_enabled)
        .bind(input.config_template_id.map(SqliteUuid))
        .bind(sqlx::types::Json(&input.override_document))
        .bind(input.billing_multiplier.to_string())
        .bind(SqliteUuid(revision))
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
             SELECT cap.id,c.name,cap.created_at,c.id,NULL,cap.id
             FROM channel_capabilities cap JOIN upstream_channels c ON c.id=cap.channel_id
             WHERE cap.id=?",
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

/// Candidate withdrawal, empty-tier removal, rule version changes and the
/// capability tombstone commit together in the coordinator's transaction.
pub async fn pg_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM channel_capabilities WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    check_delete(&before, id, expected)?;
    let rules = affected_rules(&before, id);
    sqlx::query("DELETE FROM model_capability_candidates WHERE capability_id=$1")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "DELETE FROM model_capability_tiers t WHERE t.rule_id=ANY($1)
        AND NOT EXISTS(SELECT 1 FROM model_capability_candidates c WHERE c.tier_id=t.id)",
    )
    .bind(&rules)
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "UPDATE model_operation_rules r SET enabled=r.enabled AND EXISTS(
        SELECT 1 FROM model_capability_tiers t WHERE t.rule_id=r.id) WHERE r.id=ANY($1)",
    )
    .bind(&rules)
    .execute(&mut **transaction)
    .await?;
    let changed = sqlx::query(
        "UPDATE channel_capabilities SET enabled=false,auto_disabled=false,
         auto_disable_reason=NULL,auto_disable_at=NULL,deleted_at=now(),revision=gen_random_uuid()
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
    let rules = affected_rules(&before, id);
    sqlx::query("DELETE FROM model_capability_candidates WHERE capability_id=?")
        .bind(SqliteUuid(id))
        .execute(&mut **transaction)
        .await?;
    for rule_id in rules {
        sqlx::query("DELETE FROM model_capability_tiers WHERE rule_id=?
            AND NOT EXISTS(SELECT 1 FROM model_capability_candidates c WHERE c.tier_id=model_capability_tiers.id)")
            .bind(SqliteUuid(rule_id)).execute(&mut **transaction).await?;
        sqlx::query(
            "UPDATE model_operation_rules SET enabled=enabled AND EXISTS(
            SELECT 1 FROM model_capability_tiers t WHERE t.rule_id=model_operation_rules.id),
            updated_at=ag_now() WHERE id=?",
        )
        .bind(SqliteUuid(rule_id))
        .execute(&mut **transaction)
        .await?;
    }
    let changed = sqlx::query(
        "UPDATE channel_capabilities SET enabled=0,auto_disabled=0,auto_disable_reason=NULL,
         auto_disable_at=NULL,deleted_at=ag_now(),revision=ag_md5_uuid(hex(randomblob(32))),
         updated_at=ag_now()
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
    use rust_decimal::Decimal;
    use serde_json::json;

    use super::*;
    use crate::domain::{ApiOperation, CapabilitySettings, ConnectorKind, RequestCompression};
    use crate::persistence::upstream_topology::{
        OperationCandidateRecord, RoutingGroupRecord, UpstreamAccessRecord,
    };

    fn at() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("timestamp")
    }

    fn settings() -> CapabilitySettings {
        CapabilitySettings {
            operation: ApiOperation::Responses,
            enabled: true,
            available_models: vec!["wire".into()],
            request_compression: RequestCompression::Default,
            test_model: None,
            test_pricing_model_id: None,
            auto_disable_allowed: false,
        }
    }

    fn input(channel_id: u128) -> ChannelCapabilityInput {
        ChannelCapabilityInput {
            channel_id: Uuid::from_u128(channel_id),
            settings: settings(),
            status_statistics_enabled: false,
            config_template_id: None,
            override_document: json!({}),
            billing_multiplier: Decimal::ONE,
        }
    }

    fn group(id: u128) -> RoutingGroupRecord {
        RoutingGroupRecord {
            id: Uuid::from_u128(id),
            name: format!("group-{id}"),
            enabled: true,
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

    fn channel(id: u128, access_id: u128) -> crate::persistence::LogicalChannelRecord {
        crate::persistence::LogicalChannelRecord {
            id: Uuid::from_u128(id),
            group_id: Uuid::from_u128(1),
            access_id: Uuid::from_u128(access_id),
            credential_id: None,
            name: format!("channel-{id}"),
            enabled: true,
            sharing_only: false,
            binding_revision: Uuid::from_u128(id + 600),
            created_at: at(),
            updated_at: at(),
            deleted_at: None,
        }
    }

    fn capability(id: u128, channel_id: u128, deleted: bool) -> ChannelCapabilityRecord {
        ChannelCapabilityRecord {
            id: Uuid::from_u128(id),
            channel_id: Uuid::from_u128(channel_id),
            settings: settings(),
            auto_disabled: false,
            auto_disable_reason: None,
            auto_disable_at: None,
            status_statistics_enabled: false,
            config_template_id: None,
            override_document: json!({}),
            billing_multiplier: Decimal::ONE,
            revision: Uuid::from_u128(id + 700),
            created_at: at(),
            updated_at: at(),
            deleted_at: deleted.then_some(at()),
        }
    }

    fn topology() -> UpstreamTopologyRecords {
        UpstreamTopologyRecords {
            routing_groups: vec![group(1)],
            upstream_accesses: vec![access(2, ConnectorKind::OpenAiCompatible)],
            logical_channels: vec![channel(3, 2)],
            ..Default::default()
        }
    }

    #[test]
    fn creation_validates_channel_connector_settings_and_pair_uniqueness() {
        let topology = topology();
        let id = Uuid::from_u128(4);
        assert!(validate_replacement(&topology, id, &input(3), None).is_ok());
        assert!(matches!(
            validate_replacement(&topology, id, &input(3), Some(at())),
            Err(RepositoryError::NotFound)
        ));
        assert!(matches!(
            validate_replacement(&topology, id, &input(9), None),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));

        let mut invalid = input(3);
        invalid.settings.available_models = vec![String::new()];
        assert!(matches!(
            validate_replacement(&topology, id, &invalid, None),
            Err(RepositoryError::Validation)
        ));

        let mut bad_override = input(3);
        bad_override.override_document = json!([]);
        assert!(matches!(
            validate_replacement(&topology, id, &bad_override, None),
            Err(RepositoryError::Validation)
        ));
        let mut negative = input(3);
        negative.billing_multiplier = Decimal::NEGATIVE_ONE;
        assert!(matches!(
            validate_replacement(&topology, id, &negative, None),
            Err(RepositoryError::Validation)
        ));

        let mut taken = topology.clone();
        taken.channel_capabilities = vec![capability(4, 3, false), capability(5, 3, true)];
        assert!(matches!(
            validate_replacement(&taken, Uuid::from_u128(6), &input(3), None),
            Err(RepositoryError::Conflict)
        ));
    }

    #[test]
    fn codex_capabilities_are_editable_but_channel_rebinding_is_rejected() {
        let id = Uuid::from_u128(4);
        let mut codex = topology();
        codex.upstream_accesses[0].connector_kind = ConnectorKind::CodexOauth;
        assert!(validate_replacement(&codex, id, &input(3), None).is_ok());
        let mut unsupported = input(3);
        unsupported.settings.operation = ApiOperation::ChatCompletions;
        assert!(matches!(
            validate_replacement(&codex, id, &unsupported, None),
            Err(RepositoryError::Validation)
        ));

        let mut topology = topology();
        assert!(matches!(
            validate_replacement(&topology, id, &input(9), None),
            Err(RepositoryError::RoutingDependencyInvalid)
        ));
        topology.channel_capabilities = vec![capability(4, 3, false)];
        assert!(matches!(
            validate_replacement(&topology, id, &input(9), Some(at())),
            Err(RepositoryError::Validation)
        ));
        assert!(matches!(
            validate_replacement(
                &topology,
                id,
                &input(3),
                Some(at() + chrono::Duration::seconds(1))
            ),
            Err(RepositoryError::Conflict)
        ));
    }

    #[test]
    fn routed_operation_change_is_refused() {
        let topology = UpstreamTopologyRecords {
            routing_groups: vec![group(1)],
            upstream_accesses: vec![access(2, ConnectorKind::OpenAiCompatible)],
            logical_channels: vec![channel(3, 2)],
            channel_capabilities: vec![capability(4, 3, false)],
            operation_candidates: vec![OperationCandidateRecord {
                tier_id: Uuid::from_u128(8),
                operation: ApiOperation::Responses,
                capability_id: Uuid::from_u128(4),
                upstream_model: "wire".into(),
                weight: 1,
            }],
            ..Default::default()
        };
        let mut input = input(3);
        input.settings.operation = ApiOperation::ChatCompletions;
        assert!(matches!(
            validate_replacement(&topology, Uuid::from_u128(4), &input, Some(at())),
            Err(RepositoryError::Validation)
        ));
    }

    #[test]
    fn unrouted_operation_identity_cannot_be_repurposed() {
        let mut topology = topology();
        let mut current = capability(4, 3, false);
        current.settings.operation = ApiOperation::ImagesGeneration;
        topology.channel_capabilities.push(current);
        let mut replacement = input(3);
        replacement.settings.operation = ApiOperation::ImagesEdit;
        assert!(matches!(
            validate_replacement(&topology, Uuid::from_u128(4), &replacement, Some(at())),
            Err(RepositoryError::Validation)
        ));
    }

    #[test]
    fn codex_images_can_be_enabled_without_replacing_their_identity() {
        let mut topology = topology();
        topology.upstream_accesses[0].connector_kind = ConnectorKind::CodexOauth;
        let mut current = capability(4, 3, false);
        current.settings.operation = ApiOperation::ImagesGeneration;
        current.settings.enabled = false;
        let mut replacement = input(3);
        replacement.settings = current.settings.clone();
        replacement.settings.enabled = true;
        topology.channel_capabilities.push(current);
        assert!(
            validate_replacement(&topology, Uuid::from_u128(4), &replacement, Some(at())).is_ok()
        );
    }

    #[test]
    fn audit_omits_transform_material() {
        let mut record = capability(4, 3, false);
        record.override_document = json!({"sensitive": "must-not-appear"});
        let value = audit(Some(&record));
        assert!(value.get("override_document").is_none());
        assert!(!value.to_string().contains("must-not-appear"));
    }

    #[test]
    fn delete_accepts_route_dependencies_for_all_connectors_but_rejects_stale_versions() {
        let id = Uuid::from_u128(4);
        let mut topology = topology();
        topology.channel_capabilities = vec![capability(4, 3, false)];
        assert!(check_delete(&topology, id, at()).is_ok());
        assert!(matches!(
            check_delete(&topology, id, at() + chrono::Duration::seconds(1)),
            Err(RepositoryError::Conflict)
        ));
        assert!(matches!(
            check_delete(&topology, Uuid::from_u128(9), at()),
            Err(RepositoryError::NotFound)
        ));

        topology.operation_candidates = vec![OperationCandidateRecord {
            tier_id: Uuid::from_u128(8),
            operation: ApiOperation::Responses,
            capability_id: id,
            upstream_model: "wire".into(),
            weight: 1,
        }];
        assert!(check_delete(&topology, id, at()).is_ok());

        topology.operation_candidates.clear();
        topology.upstream_accesses[0].connector_kind = ConnectorKind::CodexOauth;
        assert!(check_delete(&topology, id, at()).is_ok());
    }
}
