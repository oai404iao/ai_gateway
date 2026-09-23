//! Codex authentication identity lifecycle, independent of channel topology.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::CredentialTarget, persistence::RepositoryError};

pub(crate) struct CodexIdentityCreate<'a> {
    pub credential_id: Uuid,
    pub label: &'a str,
    pub base_url: &'a str,
    pub enabled: bool,
}

pub(crate) async fn pg_create(
    transaction: &mut Transaction<'_, Postgres>,
    input: CodexIdentityCreate<'_>,
) -> Result<(), RepositoryError> {
    let target =
        CredentialTarget::parse(input.base_url).map_err(|_| RepositoryError::Validation)?;
    sqlx::query(
        "INSERT INTO upstream_credentials(id,name,kind,enabled,allowed_base_urls)
         VALUES ($1,$2,'codex_oauth',$3,$4)",
    )
    .bind(input.credential_id)
    .bind(input.label.trim())
    .bind(input.enabled)
    .bind(sqlx::types::Json(vec![target.as_str()]))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_create(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    input: CodexIdentityCreate<'_>,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let target =
        CredentialTarget::parse(input.base_url).map_err(|_| RepositoryError::Validation)?;
    sqlx::query(
        "INSERT INTO upstream_credentials(id,name,kind,enabled,allowed_base_urls)
         VALUES (?,?,'codex_oauth',?,?)",
    )
    .bind(SqliteUuid(input.credential_id))
    .bind(input.label.trim())
    .bind(input.enabled)
    .bind(sqlx::types::Json(vec![target.as_str()]))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn pg_reconfigure(
    transaction: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
    label: &str,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    sqlx::query("UPDATE upstream_credentials SET name=$2 WHERE id=$1 AND deleted_at IS NULL")
        .bind(credential_id)
        .bind(label.trim())
        .execute(&mut **transaction)
        .await?;
    sqlx::query(
        "UPDATE codex_oauth_credentials SET proxy_id=$2 WHERE channel_id=$1 AND deleted_at IS NULL",
    )
    .bind(credential_id)
    .bind(proxy_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_reconfigure(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    credential_id: Uuid,
    label: &str,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    sqlx::query("UPDATE upstream_credentials SET name=?,updated_at=ag_now() WHERE id=? AND deleted_at IS NULL")
        .bind(label.trim()).bind(SqliteUuid(credential_id)).execute(&mut **transaction).await?;
    sqlx::query("UPDATE codex_oauth_credentials SET proxy_id=?,updated_at=ag_now() WHERE channel_id=? AND deleted_at IS NULL")
        .bind(proxy_id.map(SqliteUuid)).bind(SqliteUuid(credential_id)).execute(&mut **transaction).await?;
    Ok(())
}

pub(crate) async fn pg_update_credential_lifecycle(
    transaction: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
    enabled: bool,
    rotate: bool,
) -> Result<(), RepositoryError> {
    let changed = sqlx::query(
        "UPDATE upstream_credentials SET enabled=$2,
         revision=CASE WHEN $3 OR enabled IS DISTINCT FROM $2 THEN gen_random_uuid() ELSE revision END
         WHERE id=$1 AND kind='codex_oauth' AND deleted_at IS NULL",
    )
    .bind(credential_id).bind(enabled).bind(rotate)
    .execute(&mut **transaction).await?.rows_affected();
    if changed != 1 {
        return Err(RepositoryError::NotFound);
    }
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_update_credential_lifecycle(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    credential_id: Uuid,
    enabled: bool,
    rotate: bool,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let changed = sqlx::query(
        "UPDATE upstream_credentials SET enabled=?2,updated_at=ag_now(),
         revision=CASE WHEN ?3 OR enabled IS NOT ?2 THEN ag_md5_uuid(hex(randomblob(32))) ELSE revision END
         WHERE id=?1 AND kind='codex_oauth' AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(credential_id)).bind(enabled).bind(rotate)
    .execute(&mut **transaction).await?.rows_affected();
    if changed != 1 {
        return Err(RepositoryError::NotFound);
    }
    Ok(())
}

pub(crate) async fn pg_delete(
    transaction: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
) -> Result<(), RepositoryError> {
    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM upstream_channels WHERE credential_id=$1 AND deleted_at IS NULL)",
    ).bind(credential_id).fetch_one(&mut **transaction).await?;
    if bound {
        return Err(RepositoryError::RoutingDependencyInvalid);
    }
    sqlx::query(
        "UPDATE upstream_credentials SET enabled=false,deleted_at=now(),revision=gen_random_uuid()
         WHERE id=$1 AND kind='codex_oauth' AND deleted_at IS NULL",
    )
    .bind(credential_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_delete(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    credential_id: Uuid,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let bound: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM upstream_channels WHERE credential_id=? AND deleted_at IS NULL)",
    ).bind(SqliteUuid(credential_id)).fetch_one(&mut **transaction).await?;
    if bound {
        return Err(RepositoryError::RoutingDependencyInvalid);
    }
    sqlx::query(
        "UPDATE upstream_credentials SET enabled=0,deleted_at=ag_now(),updated_at=ag_now(),
         revision=ag_md5_uuid(hex(randomblob(32)))
         WHERE id=? AND kind='codex_oauth' AND deleted_at IS NULL",
    )
    .bind(SqliteUuid(credential_id))
    .execute(&mut **transaction)
    .await?;
    Ok(())
}
