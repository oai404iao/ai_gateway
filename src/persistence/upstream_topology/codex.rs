//! Codex identity edits must not reset independently configured capabilities.

use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{domain::CredentialTarget, persistence::RepositoryError};

/// A per-credential proxy edit must not change another credential's shared
/// access. Copy its network defaults on write, retaining capability switches,
/// catalogues, health state, routes, grants, and all financial state.
pub(crate) async fn pg_reconfigure(
    transaction: &mut Transaction<'_, Postgres>,
    credential_id: Uuid,
    label: &str,
    base_url: Option<&str>,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    let (access_id, current_url, current_proxy): (Uuid, String, Option<Uuid>) = sqlx::query_as(
        "SELECT a.id,a.base_url,a.proxy_id FROM upstream_channels c
         JOIN upstream_accesses a ON a.id=c.access_id
         JOIN codex_oauth_credentials credential ON credential.channel_id=c.credential_id
         WHERE c.id=$1 AND c.credential_id=$1 AND c.deleted_at IS NULL
           AND credential.deleted_at IS NULL AND a.deleted_at IS NULL
         FOR UPDATE OF c,a",
    )
    .bind(credential_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::NotFound)?;
    let target = base_url.unwrap_or(&current_url);
    let target_identity =
        CredentialTarget::parse(target).map_err(|_| RepositoryError::Validation)?;
    let current_identity =
        CredentialTarget::parse(&current_url).map_err(|_| RepositoryError::Validation)?;
    let network_changed =
        target_identity.as_str() != current_identity.as_str() || current_proxy != proxy_id;
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM upstream_channels WHERE access_id=$1 AND id<>$2 AND deleted_at IS NULL)",
    ).bind(access_id).bind(credential_id).fetch_one(&mut **transaction).await?;
    let selected_access = if !network_changed {
        access_id
    } else if shared {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO upstream_accesses
             (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms,
              stream_idle_timeout_ms,enabled,revision)
             SELECT $2,$3,connector_kind,$4,$5,connect_timeout_ms,response_header_timeout_ms,
                    stream_idle_timeout_ms,enabled,$6 FROM upstream_accesses WHERE id=$1",
        ).bind(access_id).bind(id).bind(label.trim()).bind(target).bind(proxy_id).bind(Uuid::new_v4())
            .execute(&mut **transaction).await?;
        id
    } else {
        sqlx::query("UPDATE upstream_accesses SET base_url=$2,proxy_id=$3,revision=$4 WHERE id=$1")
            .bind(access_id)
            .bind(target)
            .bind(proxy_id)
            .bind(Uuid::new_v4())
            .execute(&mut **transaction)
            .await?;
        access_id
    };
    sqlx::query(
        "UPDATE upstream_channels SET name=$2,access_id=$3,
         binding_revision=CASE WHEN $5 THEN $4 ELSE binding_revision END WHERE id=$1",
    )
    .bind(credential_id)
    .bind(label.trim())
    .bind(selected_access)
    .bind(Uuid::new_v4())
    .bind(network_changed)
    .execute(&mut **transaction)
    .await?;
    sqlx::query("UPDATE upstream_credentials SET name=$2 WHERE id=$1 AND kind='codex_oauth' AND deleted_at IS NULL")
        .bind(credential_id).bind(label.trim()).execute(&mut **transaction).await?;
    Ok(())
}

#[cfg(feature = "sqlite-backend")]
pub(crate) async fn sqlite_reconfigure(
    transaction: &mut Transaction<'_, sqlx::Sqlite>,
    credential_id: Uuid,
    label: &str,
    base_url: Option<&str>,
    proxy_id: Option<Uuid>,
) -> Result<(), RepositoryError> {
    use crate::persistence::sqlite::SqliteUuid;
    let (access_id, current_url, current_proxy): (SqliteUuid, String, Option<SqliteUuid>) =
        sqlx::query_as(
            "SELECT a.id,a.base_url,a.proxy_id FROM upstream_channels c
         JOIN upstream_accesses a ON a.id=c.access_id
         JOIN codex_oauth_credentials credential ON credential.channel_id=c.credential_id
         WHERE c.id=? AND c.credential_id=c.id AND c.deleted_at IS NULL
           AND credential.deleted_at IS NULL AND a.deleted_at IS NULL",
        )
        .bind(SqliteUuid(credential_id))
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::NotFound)?;
    let target = base_url.unwrap_or(&current_url);
    let target_identity =
        CredentialTarget::parse(target).map_err(|_| RepositoryError::Validation)?;
    let current_identity =
        CredentialTarget::parse(&current_url).map_err(|_| RepositoryError::Validation)?;
    let network_changed = target_identity.as_str() != current_identity.as_str()
        || current_proxy.map(|id| id.0) != proxy_id;
    let shared: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM upstream_channels WHERE access_id=? AND id<>? AND deleted_at IS NULL)",
    ).bind(access_id).bind(SqliteUuid(credential_id)).fetch_one(&mut **transaction).await?;
    let selected_access = if !network_changed {
        access_id
    } else if shared {
        let id = SqliteUuid(Uuid::new_v4());
        sqlx::query(
            "INSERT INTO upstream_accesses
             (id,name,connector_kind,base_url,proxy_id,connect_timeout_ms,response_header_timeout_ms,
              stream_idle_timeout_ms,enabled,revision)
             SELECT ?,?,connector_kind,?,?,connect_timeout_ms,response_header_timeout_ms,
                    stream_idle_timeout_ms,enabled,? FROM upstream_accesses WHERE id=?",
        ).bind(id).bind(label.trim()).bind(target).bind(proxy_id.map(SqliteUuid))
            .bind(SqliteUuid(Uuid::new_v4())).bind(access_id).execute(&mut **transaction).await?;
        id
    } else {
        sqlx::query("UPDATE upstream_accesses SET base_url=?,proxy_id=?,revision=?,updated_at=ag_now() WHERE id=?")
            .bind(target).bind(proxy_id.map(SqliteUuid)).bind(SqliteUuid(Uuid::new_v4()))
            .bind(access_id).execute(&mut **transaction).await?;
        access_id
    };
    sqlx::query("UPDATE upstream_channels SET name=?,access_id=?,binding_revision=CASE WHEN ? THEN ? ELSE binding_revision END,updated_at=ag_now() WHERE id=?")
        .bind(label.trim()).bind(selected_access).bind(network_changed).bind(SqliteUuid(Uuid::new_v4()))
        .bind(SqliteUuid(credential_id)).execute(&mut **transaction).await?;
    sqlx::query("UPDATE upstream_credentials SET name=?,updated_at=ag_now() WHERE id=? AND kind='codex_oauth' AND deleted_at IS NULL")
        .bind(label.trim()).bind(SqliteUuid(credential_id)).execute(&mut **transaction).await?;
    Ok(())
}

#[cfg(all(test, feature = "sqlite-backend"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use super::*;
    use crate::persistence::{
        capability_cutover::io::sqlite_transfer,
        sqlite::{SqliteDatabase, SqliteUuid},
        upstream_topology::sqlite_load,
    };

    #[tokio::test]
    async fn reimport_keeps_capabilities_and_isolates_shared_access_from_pending_credentials() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let database = Arc::new(
            SqliteDatabase::open(&directory.path().join("codex.sqlite"))
                .await
                .unwrap(),
        );
        database.install_schema().await.unwrap();
        let mut transaction = database.begin_write().await.unwrap();
        let group_id = Uuid::new_v4();
        let pool_id = Uuid::new_v4();
        sqlx::query("INSERT INTO connector_pools(id,connector_kind) VALUES (?,'codex_oauth')")
            .bind(SqliteUuid(pool_id))
            .execute(&mut *transaction)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO channel_groups(id,name,api_format,connector_kind,enabled,connector_pool_id)
             VALUES (?,'Codex test','open_ai_responses','codex_oauth',1,?)",
        )
        .bind(SqliteUuid(group_id))
        .bind(SqliteUuid(pool_id))
        .execute(&mut *transaction)
        .await
        .unwrap();
        let credentials = [Uuid::new_v4(), Uuid::new_v4()];
        for (index, id) in credentials.iter().enumerate() {
            let label = format!("Credential {index}");
            sqlx::query(
                "INSERT INTO channels
                 (id,channel_group_id,api_format,name,base_url,enabled,upstream_auth_kind,
                  available_models,supports_websocket,supports_standalone_web_search)
                 VALUES (?,?,'open_ai_responses',?,'https://codex.test',1,'none','[\"wire\"]',1,1)",
            )
            .bind(SqliteUuid(*id))
            .bind(SqliteUuid(group_id))
            .bind(&label)
            .execute(&mut *transaction)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO codex_oauth_credentials
                 (channel_id,channel_group_id,connector_pool_id,label,account_id,id_token,access_token,refresh_token,last_refreshed_at)
                 VALUES (?,?,?,?,?,'synthetic-id-token','synthetic-access-token','synthetic-refresh-token',ag_now())",
            ).bind(SqliteUuid(*id)).bind(SqliteUuid(group_id)).bind(SqliteUuid(pool_id)).bind(&label).bind(format!("account-{index}"))
                .execute(&mut *transaction).await.unwrap();
        }
        sqlx::query(
            "INSERT INTO _gateway_codex_operations(credential_id,attempt_id,kind,generation)
             SELECT channel_id,?,'quota_reset',refresh_generation FROM codex_oauth_credentials WHERE channel_id=?",
        ).bind(SqliteUuid(Uuid::new_v4())).bind(SqliteUuid(credentials[1]))
            .execute(&mut *transaction).await.unwrap();
        sqlx::raw_sql(include_str!("../capability_cutover/sqlite-schema.sql"))
            .execute(&mut *transaction)
            .await
            .unwrap();
        sqlite_transfer(&mut transaction, chrono::Utc::now())
            .await
            .unwrap();
        let before = sqlite_load(&mut transaction).await.unwrap();
        let original_access = before
            .logical_channels
            .iter()
            .find(|channel| channel.id == credentials[0])
            .unwrap()
            .access_id;
        sqlx::query("UPDATE upstream_channels SET access_id=?,binding_revision=?,updated_at=ag_now() WHERE id=?")
            .bind(SqliteUuid(original_access)).bind(SqliteUuid(Uuid::new_v4())).bind(SqliteUuid(credentials[1]))
            .execute(&mut *transaction).await.unwrap();
        sqlx::raw_sql(include_str!(
            "../capability_cutover/sqlite-codex-guards.sql"
        ))
        .execute(&mut *transaction)
        .await
        .unwrap();
        sqlite_reconfigure(
            &mut transaction,
            credentials[0],
            "Rename only",
            Some("HTTPS://CODEX.TEST/"),
            None,
        )
        .await
        .unwrap();
        let renamed = sqlite_load(&mut transaction).await.unwrap();
        let original_channel = before
            .logical_channels
            .iter()
            .find(|channel| channel.id == credentials[0])
            .unwrap();
        let renamed_channel = renamed
            .logical_channels
            .iter()
            .find(|channel| channel.id == credentials[0])
            .unwrap();
        assert_eq!(renamed_channel.access_id, original_access);
        assert_eq!(
            renamed_channel.binding_revision,
            original_channel.binding_revision
        );
        assert_eq!(
            serde_json::to_value(&renamed.upstream_accesses).unwrap(),
            serde_json::to_value(&before.upstream_accesses).unwrap()
        );
        sqlite_reconfigure(
            &mut transaction,
            credentials[0],
            "Reimported",
            Some("https://new-codex.test"),
            None,
        )
        .await
        .unwrap();
        let after = sqlite_load(&mut transaction).await.unwrap();
        assert_eq!(
            serde_json::to_value(&before.channel_capabilities).unwrap(),
            serde_json::to_value(&after.channel_capabilities).unwrap(),
        );
        assert_eq!(
            before.operation_candidates.len(),
            after.operation_candidates.len()
        );
        assert_eq!(before.api_key_grants.len(), after.api_key_grants.len());
        let changed = after
            .logical_channels
            .iter()
            .find(|channel| channel.id == credentials[0])
            .unwrap();
        let unaffected = after
            .logical_channels
            .iter()
            .find(|channel| channel.id == credentials[1])
            .unwrap();
        assert_ne!(changed.access_id, original_access);
        assert_eq!(unaffected.access_id, original_access);
        assert_eq!(
            after
                .upstream_accesses
                .iter()
                .find(|access| access.id == original_access)
                .unwrap()
                .base_url,
            "https://codex.test"
        );
        assert_eq!(
            after
                .upstream_accesses
                .iter()
                .find(|access| access.id == changed.access_id)
                .unwrap()
                .base_url,
            "https://new-codex.test"
        );
        assert!(
            sqlite_reconfigure(&mut transaction, credentials[1], "Blocked", None, None)
                .await
                .is_err()
        );
        let error = sqlx::query(
            "UPDATE channel_capabilities SET enabled=0,updated_at=ag_now() WHERE channel_id=?",
        )
        .bind(SqliteUuid(credentials[1]))
        .execute(&mut *transaction)
        .await
        .unwrap_err();
        assert!(error.to_string().contains("codex_operation_pending"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM _gateway_codex_operations")
                .fetch_one(&mut *transaction)
                .await
                .unwrap(),
            1
        );
        let actor = Uuid::new_v4();
        sqlx::query("INSERT INTO users(id,email,display_name,role,status,password_hash) VALUES (?,'codex-audit@test.invalid','Audit','admin','active','test')")
            .bind(SqliteUuid(actor)).execute(&mut *transaction).await.unwrap();
        let expected: crate::persistence::sqlite::SqliteTimestamp =
            sqlx::query_scalar("SELECT updated_at FROM codex_oauth_credentials WHERE channel_id=?")
                .bind(SqliteUuid(credentials[0]))
                .fetch_one(&mut *transaction)
                .await
                .unwrap();
        transaction.commit().await.unwrap();
        let repository =
            crate::persistence::ControlPlaneRepository::from_sqlite(Arc::clone(&database));
        let change = repository
            .prepare_codex_credential_update(
                actor,
                credentials[0],
                crate::persistence::CodexCredentialUpdateInput {
                    label: "Audited label".into(),
                    enabled: true,
                    proxy_id: None,
                    quota_threshold_percent: 90,
                },
                expected.0,
            )
            .await
            .unwrap();
        let (mutations, _) = change.commit().await.unwrap();
        assert_eq!(
            mutations[0].after_redacted["access_id"],
            changed.access_id.to_string()
        );
        assert_eq!(mutations[0].after_redacted["base_url"], "[REDACTED]");
        assert_eq!(
            mutations[0].after_redacted["capabilities"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        drop(repository);
        database.close().await;
    }
}
