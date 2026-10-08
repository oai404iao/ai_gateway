//! Backend-neutral plugin inventory, desired generations, settings and install jobs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{MutationResult, RepositoryError};

#[derive(Clone, Debug, Default)]
pub struct PluginRuntimeRecords {
    pub artifacts: Vec<PluginArtifactRecord>,
    pub states: Vec<PluginStateRecord>,
    pub settings: Vec<PluginSettingsRecord>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginArtifactRecord {
    pub plugin_id: String,
    pub digest: String,
    pub version: String,
    pub manifest: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginArtifactInput {
    pub plugin_id: String,
    pub digest: String,
    pub version: String,
    pub manifest: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginStateRecord {
    pub plugin_id: String,
    pub enabled: bool,
    pub artifact_digest: Option<String>,
    pub revision: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginSettingsRecord {
    pub plugin_id: String,
    pub schema_version: i32,
    pub values: Value,
    pub revision: i64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSettingsInput {
    pub schema_version: i32,
    pub values: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSaveInput {
    pub enabled: bool,
    pub artifact_digest: Option<String>,
    pub settings: Option<PluginSettingsInput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginInstallJob {
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub operation: String,
    pub status: String,
    pub plugin_id: Option<String>,
    pub artifact_digest: Option<String>,
    pub error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct PluginJobCompletion {
    pub plugin_id: Option<String>,
    pub artifact_digest: Option<String>,
    pub error_code: Option<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != "general"
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn validate_settings(settings: &PluginSettingsInput) -> Result<(), RepositoryError> {
    if settings.schema_version <= 0
        || !settings.values.is_object()
        || serde_json::to_vec(&settings.values)
            .map_err(|_| RepositoryError::Validation)?
            .len()
            > 65_536
    {
        return Err(RepositoryError::Validation);
    }
    Ok(())
}

fn mutation(plugin_id: &str, action: &'static str, before: Value, after: Value) -> MutationResult {
    let digest = Sha256::digest(format!("ai-gateway/plugin/{plugin_id}").as_bytes());
    MutationResult {
        id: Uuid::from_bytes(digest[..16].try_into().expect("SHA-256 contains 16 bytes")),
        object_type: "plugin",
        action,
        before_redacted: before,
        after_redacted: after,
        created_secret: None,
        reason: None,
        updated_at: Utc::now(),
        correlation_id: None,
    }
}

// The statements and CAS rules are identical on both backends; only parameter,
// UUID and timestamp encodings differ.
macro_rules! repository {
    () => {
        use sqlx::{Row, types::Json};

        fn artifact(row: &DbRow) -> Result<PluginArtifactRecord, RepositoryError> {
            Ok(PluginArtifactRecord {
                plugin_id: row.try_get("plugin_id")?,
                digest: row.try_get("digest")?,
                version: row.try_get("version")?,
                manifest: row.try_get::<Json<Value>, _>("manifest")?.0,
                created_at: decode_time(row.try_get("created_at")?),
            })
        }

        fn state(row: &DbRow) -> Result<PluginStateRecord, RepositoryError> {
            Ok(PluginStateRecord {
                plugin_id: row.try_get("plugin_id")?,
                enabled: row.try_get("enabled")?,
                artifact_digest: row.try_get("artifact_digest")?,
                revision: row.try_get("revision")?,
                updated_at: decode_time(row.try_get("updated_at")?),
            })
        }

        fn settings(row: &DbRow) -> Result<PluginSettingsRecord, RepositoryError> {
            Ok(PluginSettingsRecord {
                plugin_id: row.try_get("plugin_id")?,
                schema_version: row.try_get("schema_version")?,
                values: row.try_get::<Json<Value>, _>("settings_value")?.0,
                revision: row.try_get("revision")?,
                updated_at: decode_time(row.try_get("updated_at")?),
            })
        }

        fn job(row: &DbRow) -> Result<PluginInstallJob, RepositoryError> {
            Ok(PluginInstallJob {
                id: decode_uuid(row.try_get("id")?),
                actor_user_id: decode_uuid(row.try_get("actor_user_id")?),
                operation: row.try_get("operation")?,
                status: row.try_get("status")?,
                plugin_id: row.try_get("plugin_id")?,
                artifact_digest: row.try_get("artifact_digest")?,
                error_code: row.try_get("error_code")?,
                created_at: decode_time(row.try_get("created_at")?),
                updated_at: decode_time(row.try_get("updated_at")?),
            })
        }

        pub(crate) async fn load(connection: &mut DbConnection) -> Result<PluginRuntimeRecords, RepositoryError> {
            let artifacts = sqlx::query("SELECT * FROM plugin_artifacts WHERE deleted_at IS NULL ORDER BY plugin_id,digest")
                .fetch_all(&mut *connection).await?.iter().map(artifact).collect::<Result<_, _>>()?;
            let states = sqlx::query("SELECT * FROM plugin_states ORDER BY plugin_id")
                .fetch_all(&mut *connection).await?.iter().map(state).collect::<Result<_, _>>()?;
            let settings = sqlx::query("SELECT * FROM plugin_settings ORDER BY plugin_id")
                .fetch_all(&mut *connection).await?.iter().map(settings).collect::<Result<_, _>>()?;
            Ok(PluginRuntimeRecords { artifacts, states, settings })
        }

        async fn audit(connection: &mut DbConnection, id: &str) -> Result<Value, RepositoryError> {
            let state = sqlx::query(sql("SELECT * FROM plugin_states WHERE plugin_id=$1"))
                .bind(id).fetch_optional(&mut *connection).await?.as_ref().map(state).transpose()?;
            let settings = sqlx::query(sql("SELECT * FROM plugin_settings WHERE plugin_id=$1"))
                .bind(id).fetch_optional(&mut *connection).await?.as_ref().map(settings).transpose()?;
            Ok(json!({
                "plugin_id": id,
                "state": state,
                "settings": settings.map(|s| json!({
                    "schema_version":s.schema_version, "revision":s.revision,
                    "fields":s.values.as_object().map(|v|v.keys().collect::<Vec<_>>())
                }))
            }))
        }

        pub(crate) async fn register(connection: &mut DbConnection, input: PluginArtifactInput) -> Result<MutationResult, RepositoryError> {
            if !valid_id(&input.plugin_id) || !valid_digest(&input.digest)
                || input.version.is_empty() || input.version.len() > 128
                || !input.manifest.is_object()
                || serde_json::to_vec(&input.manifest).map_err(|_|RepositoryError::Validation)?.len() > 65_536
            {
                return Err(RepositoryError::Validation);
            }
            let before = audit(connection, &input.plugin_id).await?;
            sqlx::query(sql("INSERT INTO plugin_artifacts (plugin_id,digest,version,manifest) VALUES ($1,$2,$3,$4) ON CONFLICT (plugin_id,digest) DO NOTHING"))
                .bind(&input.plugin_id).bind(&input.digest).bind(&input.version).bind(Json(&input.manifest))
                .execute(&mut *connection).await?;
            let stored = sqlx::query(sql("SELECT * FROM plugin_artifacts WHERE plugin_id=$1 AND digest=$2"))
                .bind(&input.plugin_id).bind(&input.digest).fetch_one(&mut *connection).await?;
            let stored = artifact(&stored)?;
            if stored.version != input.version || stored.manifest != input.manifest {
                return Err(RepositoryError::Conflict);
            }
            sqlx::query(sql("UPDATE plugin_artifacts SET deleted_at=NULL WHERE plugin_id=$1 AND digest=$2"))
                .bind(&input.plugin_id).bind(&input.digest).execute(&mut *connection).await?;
            sqlx::query(sql("INSERT INTO plugin_states (plugin_id) VALUES ($1) ON CONFLICT (plugin_id) DO NOTHING"))
                .bind(&input.plugin_id).execute(&mut *connection).await?;
            Ok(mutation(&input.plugin_id, "install", before, json!({"plugin_id":input.plugin_id,"digest":input.digest,"version":input.version})))
        }

        pub(crate) async fn discover(connection: &mut DbConnection, input: PluginArtifactInput) -> Result<(), RepositoryError> {
            let exists: bool = sqlx::query_scalar(sql("SELECT EXISTS(SELECT 1 FROM plugin_artifacts WHERE plugin_id=$1 AND digest=$2)"))
                .bind(&input.plugin_id).bind(&input.digest).fetch_one(&mut *connection).await?;
            if exists { return Ok(()); }
            let result = register(connection, input).await?;
            sqlx::query(sql("INSERT INTO audit_logs (id,actor_type,action,object_type,object_id,before_redacted,after_redacted) VALUES ($1,'system','discover','plugin',$2,$3,$4)"))
                .bind(db_uuid(Uuid::new_v4())).bind(db_uuid(result.id)).bind(Json(result.before_redacted)).bind(Json(result.after_redacted))
                .execute(connection).await?;
            Ok(())
        }

        pub(crate) async fn save(connection: &mut DbConnection, id: &str, input: PluginSaveInput, expected: i64) -> Result<MutationResult, RepositoryError> {
            if !valid_id(id) || expected < 0 || expected == i64::MAX
                || input.artifact_digest.as_deref().is_some_and(|v|!valid_digest(v))
                || (input.enabled && input.artifact_digest.is_none())
            {
                return Err(RepositoryError::Validation);
            }
            if let Some(settings) = &input.settings { validate_settings(settings)?; }
            if let Some(digest) = &input.artifact_digest {
                let exists: bool = sqlx::query_scalar(sql("SELECT EXISTS(SELECT 1 FROM plugin_artifacts WHERE plugin_id=$1 AND digest=$2 AND deleted_at IS NULL)"))
                    .bind(id).bind(digest).fetch_one(&mut *connection).await?;
                if !exists { return Err(RepositoryError::Validation); }
            }
            let before = audit(connection, id).await?;
            let now = time(Utc::now());
            let changed = if expected == 0 {
                sqlx::query(sql("INSERT INTO plugin_states (plugin_id,enabled,artifact_digest,updated_at) VALUES ($1,$2,$3,$4) ON CONFLICT (plugin_id) DO NOTHING"))
                    .bind(id).bind(input.enabled).bind(&input.artifact_digest).bind(now)
                    .execute(&mut *connection).await?.rows_affected()
            } else {
                sqlx::query(sql("UPDATE plugin_states SET enabled=$2,artifact_digest=$3,updated_at=$4,revision=revision+1 WHERE plugin_id=$1 AND revision=$5"))
                    .bind(id).bind(input.enabled).bind(&input.artifact_digest).bind(now).bind(expected)
                    .execute(&mut *connection).await?.rows_affected()
            };
            if changed != 1 { return Err(RepositoryError::Conflict); }
            if let Some(settings) = input.settings {
                sqlx::query(sql("INSERT INTO plugin_settings (plugin_id,schema_version,settings_value,updated_at) VALUES ($1,$2,$3,$4) ON CONFLICT (plugin_id) DO UPDATE SET schema_version=excluded.schema_version,settings_value=excluded.settings_value,updated_at=excluded.updated_at,revision=plugin_settings.revision+1"))
                    .bind(id).bind(settings.schema_version).bind(Json(settings.values)).bind(time(Utc::now()))
                    .execute(&mut *connection).await?;
            }
            let after = audit(connection, id).await?;
            Ok(mutation(id, "update", before, after))
        }

        pub(crate) async fn delete_artifact(connection: &mut DbConnection, id: &str, digest: &str, expected: i64) -> Result<MutationResult, RepositoryError> {
            if !valid_id(id) || !valid_digest(digest) || expected <= 0 || expected == i64::MAX {
                return Err(RepositoryError::Validation);
            }
            let before = audit(connection, id).await?;
            let changed = sqlx::query(sql("UPDATE plugin_states SET revision=revision+1,updated_at=$4 WHERE plugin_id=$1 AND revision=$2 AND (artifact_digest IS NULL OR artifact_digest<>$3)"))
                .bind(id).bind(expected).bind(digest).bind(time(Utc::now())).execute(&mut *connection).await?.rows_affected();
            if changed != 1 { return Err(RepositoryError::Conflict); }
            let deleted = sqlx::query(sql("UPDATE plugin_artifacts SET deleted_at=$3 WHERE plugin_id=$1 AND digest=$2 AND deleted_at IS NULL"))
                .bind(id).bind(digest).bind(time(Utc::now())).execute(&mut *connection).await?.rows_affected();
            if deleted != 1 { return Err(RepositoryError::NotFound); }
            Ok(mutation(id, "delete_artifact", before, json!({"plugin_id":id,"digest":digest})))
        }

        async fn require_admin(connection: &mut DbConnection, actor: Uuid) -> Result<(), RepositoryError> {
            let exists: bool = sqlx::query_scalar(sql("SELECT EXISTS(SELECT 1 FROM users WHERE id=$1 AND role='admin' AND status='active' AND deleted_at IS NULL)"))
                .bind(db_uuid(actor)).fetch_one(&mut *connection).await?;
            if !exists { return Err(RepositoryError::InvalidActor); }
            Ok(())
        }

        pub(crate) async fn get_job(connection: &mut DbConnection, id: Uuid) -> Result<Option<PluginInstallJob>, RepositoryError> {
            sqlx::query(sql("SELECT * FROM plugin_install_jobs WHERE id=$1")).bind(db_uuid(id))
                .fetch_optional(connection).await?.as_ref().map(job).transpose()
        }

        async fn audit_job(connection: &mut DbConnection, actor: Option<Uuid>, job: &PluginInstallJob, action: &str) -> Result<(), RepositoryError> {
            let actor_type = if actor.is_some() {"user"} else {"system"};
            let role = actor.map(|_|"admin");
            sqlx::query(sql("INSERT INTO audit_logs (id,actor_user_id,actor_type,actor_role,action,object_type,object_id,before_redacted,after_redacted) VALUES ($1,$2,$3,$4,$5,'plugin_install_job',$6,$7,$8)"))
                .bind(db_uuid(Uuid::new_v4())).bind(actor.map(db_uuid)).bind(actor_type).bind(role).bind(action).bind(db_uuid(job.id))
                .bind(Json(json!({}))).bind(Json(json!({"operation":job.operation,"status":job.status,"plugin_id":job.plugin_id,"artifact_digest":job.artifact_digest,"error_code":job.error_code})))
                .execute(connection).await?;
            Ok(())
        }

        pub(crate) async fn create_job(connection: &mut DbConnection, actor: Uuid, id: Uuid, operation: &str) -> Result<PluginInstallJob, RepositoryError> {
            require_admin(connection, actor).await?;
            if !matches!(operation, "install" | "discover") { return Err(RepositoryError::Validation); }
            sqlx::query(sql("INSERT INTO plugin_install_jobs (id,actor_user_id,operation,status) VALUES ($1,$2,$3,'queued')"))
                .bind(db_uuid(id)).bind(db_uuid(actor)).bind(operation).execute(&mut *connection).await?;
            let job = get_job(connection, id).await?.ok_or(RepositoryError::NotFound)?;
            audit_job(connection, Some(actor), &job, "create").await?;
            Ok(job)
        }

        pub(crate) async fn claim_job(connection: &mut DbConnection, actor: Uuid, id: Uuid) -> Result<PluginInstallJob, RepositoryError> {
            require_admin(connection, actor).await?;
            let changed = sqlx::query(sql("UPDATE plugin_install_jobs SET status='running',updated_at=$3 WHERE id=$1 AND actor_user_id=$2 AND status='queued'"))
                .bind(db_uuid(id)).bind(db_uuid(actor)).bind(time(Utc::now())).execute(&mut *connection).await?.rows_affected();
            if changed != 1 { return Err(RepositoryError::Conflict); }
            let job = get_job(connection, id).await?.ok_or(RepositoryError::NotFound)?;
            audit_job(connection, Some(actor), &job, "claim").await?;
            Ok(job)
        }

        pub(crate) async fn finish_job(connection: &mut DbConnection, actor: Uuid, id: Uuid, result: PluginJobCompletion) -> Result<PluginInstallJob, RepositoryError> {
            require_admin(connection, actor).await?;
            if result.plugin_id.as_deref().is_some_and(|id|!valid_id(id))
                || result.artifact_digest.as_deref().is_some_and(|d|!valid_digest(d))
                || result.error_code.as_deref().is_some_and(|e|e.is_empty() || e.len()>64 || !e.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'_'))
            { return Err(RepositoryError::Validation); }
            let current = get_job(connection, id).await?.ok_or(RepositoryError::NotFound)?;
            if current.operation == "install" && result.error_code.is_none()
                && (result.plugin_id.is_none() || result.artifact_digest.is_none()) {
                return Err(RepositoryError::Validation);
            }
            if result.error_code.is_none() {
                if let (Some(plugin_id), Some(digest)) = (&result.plugin_id, &result.artifact_digest) {
                    let exists: bool = sqlx::query_scalar(sql("SELECT EXISTS(SELECT 1 FROM plugin_artifacts WHERE plugin_id=$1 AND digest=$2 AND deleted_at IS NULL)"))
                        .bind(plugin_id).bind(digest).fetch_one(&mut *connection).await?;
                    if !exists { return Err(RepositoryError::Validation); }
                }
            }
            let status = if result.error_code.is_some() {"failed"} else {"succeeded"};
            let changed = sqlx::query(sql("UPDATE plugin_install_jobs SET status=$3,plugin_id=$4,artifact_digest=$5,error_code=$6,updated_at=$7 WHERE id=$1 AND actor_user_id=$2 AND status='running'"))
                .bind(db_uuid(id)).bind(db_uuid(actor)).bind(status).bind(result.plugin_id).bind(result.artifact_digest).bind(result.error_code).bind(time(Utc::now()))
                .execute(&mut *connection).await?.rows_affected();
            if changed != 1 { return Err(RepositoryError::Conflict); }
            let job = get_job(connection, id).await?.ok_or(RepositoryError::NotFound)?;
            audit_job(connection, Some(actor), &job, "complete").await?;
            Ok(job)
        }

        pub(crate) async fn interrupt_jobs(connection: &mut DbConnection) -> Result<u64, RepositoryError> {
            let jobs = sqlx::query(sql("UPDATE plugin_install_jobs SET status='failed',error_code='interrupted',updated_at=$1 WHERE status IN ('queued','running') RETURNING *"))
                .bind(time(Utc::now())).fetch_all(&mut *connection).await?;
            for row in &jobs {
                audit_job(connection, None, &job(row)?, "interrupt").await?;
            }
            Ok(jobs.len() as u64)
        }

        pub(crate) async fn fail_job(connection: &mut DbConnection, id: Uuid, code: &str) -> Result<PluginInstallJob, RepositoryError> {
            if code.is_empty() || code.len()>64 || !code.bytes().all(|b|b.is_ascii_lowercase()||b.is_ascii_digit()||b==b'_') {
                return Err(RepositoryError::Validation);
            }
            let changed = sqlx::query(sql("UPDATE plugin_install_jobs SET status='failed',error_code=$2,updated_at=$3 WHERE id=$1 AND status IN ('queued','running')"))
                .bind(db_uuid(id)).bind(code).bind(time(Utc::now()))
                .execute(&mut *connection).await?.rows_affected();
            if changed != 1 { return Err(RepositoryError::Conflict); }
            let job = get_job(connection, id).await?.ok_or(RepositoryError::NotFound)?;
            audit_job(connection, None, &job, "fail").await?;
            Ok(job)
        }
    };
}

pub(crate) mod postgres {
    use super::*;
    type DbConnection = sqlx::PgConnection;
    type DbRow = sqlx::postgres::PgRow;
    fn sql(value: &'static str) -> &'static str {
        value
    }
    fn time(value: DateTime<Utc>) -> DateTime<Utc> {
        value
    }
    fn decode_time(value: DateTime<Utc>) -> DateTime<Utc> {
        value
    }
    fn db_uuid(value: Uuid) -> Uuid {
        value
    }
    fn decode_uuid(value: Uuid) -> Uuid {
        value
    }
    repository!();
}

#[cfg(feature = "sqlite-backend")]
pub(crate) mod sqlite {
    use super::*;
    use crate::persistence::sqlite::{SqliteTimestamp, SqliteUuid};
    type DbConnection = sqlx::SqliteConnection;
    type DbRow = sqlx::sqlite::SqliteRow;
    fn sql(value: &'static str) -> sqlx::AssertSqlSafe<String> {
        sqlx::AssertSqlSafe(value.replace('$', "?"))
    }
    fn time(value: DateTime<Utc>) -> SqliteTimestamp {
        SqliteTimestamp(value)
    }
    fn decode_time(value: SqliteTimestamp) -> DateTime<Utc> {
        value.0
    }
    fn db_uuid(value: Uuid) -> SqliteUuid {
        SqliteUuid(value)
    }
    fn decode_uuid(value: SqliteUuid) -> Uuid {
        value.0
    }
    repository!();
}
