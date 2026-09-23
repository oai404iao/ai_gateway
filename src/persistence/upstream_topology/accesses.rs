//! Access writes preserve credential scope across every bound logical channel.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{UpstreamAccessInput, UpstreamAccessRecord, UpstreamTopologyRecords};
use crate::{
    domain::CredentialTarget,
    persistence::{MutationResult, RepositoryError},
};

fn validate_input(input: &UpstreamAccessInput) -> Result<(), RepositoryError> {
    if input.name.trim().is_empty()
        || input.name.chars().count() > 100
        || [
            input.connect_timeout_ms,
            input.response_header_timeout_ms,
            input.stream_idle_timeout_ms,
        ]
        .into_iter()
        .flatten()
        .any(|timeout| timeout <= 0)
    {
        return Err(RepositoryError::Validation);
    }
    CredentialTarget::parse(&input.base_url).map_err(|_| RepositoryError::Validation)?;
    Ok(())
}

fn check_version(
    topology: &UpstreamTopologyRecords,
    id: Uuid,
    input: &UpstreamAccessInput,
    expected: Option<DateTime<Utc>>,
) -> Result<(), RepositoryError> {
    validate_input(input)?;
    let previous = topology
        .upstream_accesses
        .iter()
        .find(|access| access.id == id);
    match (previous, expected) {
        (None, Some(_)) => Err(RepositoryError::NotFound),
        (Some(_), None) => Err(RepositoryError::Conflict),
        (Some(previous), Some(expected)) => {
            if previous.deleted_at.is_some() {
                return Err(RepositoryError::NotFound);
            }
            if previous.updated_at != expected {
                return Err(RepositoryError::Conflict);
            }
            if previous.connector_kind != input.connector_kind {
                return Err(RepositoryError::Validation);
            }
            Ok(())
        }
        (None, None) => Ok(()),
    }
}

fn audit(record: Option<&UpstreamAccessRecord>) -> serde_json::Value {
    record.map_or_else(
        || json!({}),
        |record| {
            json!({
                "id": record.id,
                "name": record.name,
                "connector_kind": record.connector_kind,
                "base_url": "[REDACTED]",
                "proxy_id": record.proxy_id,
                "connect_timeout_ms": record.connect_timeout_ms,
                "response_header_timeout_ms": record.response_header_timeout_ms,
                "stream_idle_timeout_ms": record.stream_idle_timeout_ms,
                "enabled": record.enabled,
                "revision": record.revision,
                "deleted_at": record.deleted_at,
            })
        },
    )
}

fn result(
    before: &UpstreamTopologyRecords,
    after: &UpstreamTopologyRecords,
    id: Uuid,
    creating: bool,
) -> Result<MutationResult, RepositoryError> {
    let previous = before
        .upstream_accesses
        .iter()
        .find(|access| access.id == id);
    let current = after
        .upstream_accesses
        .iter()
        .find(|access| access.id == id)
        .ok_or(RepositoryError::NotFound)?;
    Ok(MutationResult {
        id,
        object_type: "upstream_access",
        action: if creating { "create" } else { "update" },
        before_redacted: audit(previous),
        after_redacted: audit(Some(current)),
        created_secret: None,
        reason: None,
        updated_at: current.updated_at,
        correlation_id: None,
    })
}

/// Runs inside the coordinator's serializable transaction. Scope validation
/// includes disabled and unrouted bindings; the caller still owns the complete
/// settings/sharing validation, audit, commit, and snapshot publication.
pub async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: &UpstreamAccessInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM upstream_accesses WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let before = super::pg_load(transaction).await?;
    check_version(&before, id, input, expected)?;
    let revision = Uuid::new_v4();
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE upstream_accesses SET name=$2,base_url=$3,proxy_id=$4,
             connect_timeout_ms=$5,response_header_timeout_ms=$6,stream_idle_timeout_ms=$7,
             enabled=$8,revision=$9 WHERE id=$1 AND updated_at=$10 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(input.name.trim())
        .bind(&input.base_url)
        .bind(input.proxy_id)
        .bind(input.connect_timeout_ms)
        .bind(input.response_header_timeout_ms)
        .bind(input.stream_idle_timeout_ms)
        .bind(input.enabled)
        .bind(revision)
        .bind(expected)
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO upstream_accesses
             (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms,
              stream_idle_timeout_ms,enabled,revision)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(id).bind(input.name.trim()).bind(input.connector_kind.as_str()).bind(&input.base_url)
        .bind(input.proxy_id).bind(input.connect_timeout_ms).bind(input.response_header_timeout_ms)
        .bind(input.stream_idle_timeout_ms).bind(input.enabled).bind(revision)
        .execute(&mut **transaction).await?.rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    super::pg_load_control_plane(transaction).await?;
    let after = super::pg_load(transaction).await?;
    result(&before, &after, id, expected.is_none())
}

#[cfg(feature = "sqlite-backend")]
/// Runs inside the database owner's sole-writer transaction.
pub async fn sqlite_save(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    id: Uuid,
    input: &UpstreamAccessInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    let before = super::sqlite_load(transaction).await?;
    check_version(&before, id, input, expected)?;
    let revision = Uuid::new_v4();
    let changed = if let Some(expected) = expected {
        sqlx::query(
            "UPDATE upstream_accesses SET name=?,base_url=?,proxy_id=?,
             connect_timeout_ms=?,response_header_timeout_ms=?,stream_idle_timeout_ms=?,
             enabled=?,revision=?,updated_at=ag_now()
             WHERE id=? AND updated_at=? AND deleted_at IS NULL",
        )
        .bind(input.name.trim())
        .bind(&input.base_url)
        .bind(input.proxy_id.map(SqliteUuid))
        .bind(input.connect_timeout_ms)
        .bind(input.response_header_timeout_ms)
        .bind(input.stream_idle_timeout_ms)
        .bind(input.enabled)
        .bind(SqliteUuid(revision))
        .bind(SqliteUuid(id))
        .bind(SqliteTimestamp(expected))
        .execute(&mut **transaction)
        .await?
        .rows_affected()
    } else {
        sqlx::query(
            "INSERT INTO upstream_accesses
             (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms,
              stream_idle_timeout_ms,enabled,revision) VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(SqliteUuid(id)).bind(input.name.trim()).bind(input.connector_kind.as_str())
        .bind(&input.base_url).bind(input.proxy_id.map(SqliteUuid)).bind(input.connect_timeout_ms)
        .bind(input.response_header_timeout_ms).bind(input.stream_idle_timeout_ms)
        .bind(input.enabled).bind(SqliteUuid(revision))
        .execute(&mut **transaction).await?.rows_affected()
    };
    if changed != 1 {
        return Err(RepositoryError::Conflict);
    }
    super::sqlite_load_control_plane(transaction).await?;
    let after = super::sqlite_load(transaction).await?;
    result(&before, &after, id, expected.is_none())
}
