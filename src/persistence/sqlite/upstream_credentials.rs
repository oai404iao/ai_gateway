//! SQLite credential operations share validation and audit contracts with PostgreSQL.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{Sqlite, SqliteConnection, Transaction};
use uuid::Uuid;

use super::{SqliteTimestamp, SqliteUuid};
use crate::persistence::{
    MutationResult, RepositoryError, UpstreamCredentialInput,
    upstream_credentials::{
        CredentialBinding, CredentialRecord, check_delete, deletion_result, prepare_record,
    },
};

pub(super) async fn records(
    connection: &mut SqliteConnection,
) -> Result<Vec<CredentialRecord>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT json_object('id',id,'name',name,'kind',kind,'header_name',header_name,'secret',secret,
         'allowed_base_urls',json(allowed_base_urls),'enabled',json(CASE enabled WHEN 1 THEN 'true' ELSE 'false' END),
         'revision',revision,'created_at',created_at,'updated_at',updated_at,'deleted_at',deleted_at)
         FROM upstream_credentials ORDER BY id")
        .fetch_all(connection).await?;
    rows.iter()
        .map(|row| serde_json::from_str(row).map_err(|_| RepositoryError::Validation))
        .collect()
}

pub(super) async fn bindings(
    connection: &mut SqliteConnection,
) -> Result<Vec<CredentialBinding>, RepositoryError> {
    let rows = sqlx::query_as::<
        _,
        (
            SqliteUuid,
            Option<SqliteUuid>,
            Option<SqliteUuid>,
            SqliteUuid,
        ),
    >(
        "SELECT c.id,c.credential_id,p.credential_id,c.credential_binding_revision FROM channels c
         LEFT JOIN codex_oauth_credential_channels p ON p.channel_id=c.id
         WHERE c.deleted_at IS NULL ORDER BY c.id",
    )
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(channel, credential, projection, revision)| {
            (
                channel.0,
                credential.map(|id| id.0),
                projection.map(|id| id.0),
                revision.0,
            )
        })
        .collect())
}

pub(super) async fn save(
    transaction: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    input: UpstreamCredentialInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let previous = if expected.is_some() {
        Some(
            records(transaction)
                .await?
                .into_iter()
                .find(|record| record.id == id)
                .ok_or(RepositoryError::NotFound)?,
        )
    } else {
        None
    };
    let record = prepare_record(id, input, previous.as_ref(), expected)?;
    for target in sqlx::query_scalar::<_, String>(
        "SELECT base_url FROM channels WHERE credential_id=? AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(id))
    .fetch_all(&mut **transaction)
    .await?
    {
        record.allows_target(&target)?;
    }
    let updated_at = if let Some(expected) = expected {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "UPDATE upstream_credentials SET name=?,header_name=?,secret=?,allowed_base_urls=?,
             enabled=?,revision=?,updated_at=ag_now() WHERE id=? AND updated_at=? RETURNING updated_at")
            .bind(&record.name).bind(&record.header_name).bind(&record.secret)
            .bind(json!(record.allowed_base_urls).to_string()).bind(record.enabled)
            .bind(SqliteUuid(record.revision)).bind(SqliteUuid(id)).bind(SqliteTimestamp(expected))
            .fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?.0
    } else {
        sqlx::query_scalar::<_, SqliteTimestamp>(
            "INSERT INTO upstream_credentials(id,name,kind,header_name,secret,allowed_base_urls,enabled,revision)
             VALUES(?,?,?,?,?,?,?,?) RETURNING updated_at")
            .bind(SqliteUuid(id)).bind(&record.name).bind(&record.kind).bind(&record.header_name)
            .bind(&record.secret).bind(json!(record.allowed_base_urls).to_string()).bind(record.enabled)
            .bind(SqliteUuid(record.revision)).fetch_one(&mut **transaction).await?.0
    };
    Ok(MutationResult {
        id,
        object_type: "upstream_credential",
        action: if previous.is_some() {
            "update"
        } else {
            "create"
        },
        before_redacted: previous.as_ref().map_or(json!({}), CredentialRecord::audit),
        after_redacted: CredentialRecord {
            updated_at,
            ..record
        }
        .audit(),
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    })
}

pub(super) async fn validate_binding(
    connection: &mut SqliteConnection,
    id: Option<Uuid>,
    target: &str,
) -> Result<(), RepositoryError> {
    let Some(id) = id else {
        return Ok(());
    };
    let record = records(connection)
        .await?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or(RepositoryError::Validation)?;
    crate::persistence::upstream_credentials::validate_static_binding(&record, target)
}

pub(super) async fn delete(
    transaction: &mut Transaction<'_, Sqlite>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    let record = records(transaction)
        .await?
        .into_iter()
        .find(|record| record.id == id && record.deleted_at.is_none())
        .ok_or(RepositoryError::NotFound)?;
    check_delete(&record, expected, &bindings(transaction).await?)?;
    let updated_at = sqlx::query_scalar::<_, SqliteTimestamp>(
        "UPDATE upstream_credentials SET secret=NULL,enabled=0,deleted_at=ag_now(),revision=?,updated_at=ag_now()
         WHERE id=? AND updated_at=? RETURNING updated_at")
        .bind(SqliteUuid(Uuid::new_v4())).bind(SqliteUuid(id)).bind(SqliteTimestamp(expected))
        .fetch_optional(&mut **transaction).await?.ok_or(RepositoryError::Conflict)?.0;
    Ok(deletion_result(record, updated_at))
}
