//! Shared credential contracts, scope enforcement, and PostgreSQL identity operations.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{MutationResult, RepositoryError};
use crate::domain::{ConnectorKind, CredentialTarget, UpstreamAuth};

pub(crate) mod codex;

#[derive(Clone, Copy, Debug)]
pub struct CredentialIdentity {
    pub id: Uuid,
    pub revision: Uuid,
}

#[derive(Clone, Deserialize)]
pub(crate) struct CredentialRecord {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub header_name: Option<String>,
    pub secret: Option<String>,
    pub allowed_base_urls: Vec<String>,
    pub enabled: bool,
    pub revision: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl fmt::Debug for CredentialRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRecord")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("secret", &"REDACTED")
            .finish()
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamCredentialInput {
    pub name: String,
    pub kind: String,
    pub header_name: Option<String>,
    #[serde(default, deserialize_with = "present_secret")]
    pub secret: Option<String>,
    pub allowed_base_urls: Vec<String>,
    pub enabled: bool,
}

fn present_secret<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Option<String>, D::Error> {
    String::deserialize(de).map(Some)
}

#[derive(Clone, Debug, Serialize)]
pub struct UpstreamCredentialView {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub connector_kind: ConnectorKind,
    pub header_name: Option<String>,
    pub allowed_base_urls: Vec<String>,
    pub enabled: bool,
    pub provider_managed: bool,
    pub channel_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct UpstreamCredentialDetail {
    #[serde(flatten)]
    pub credential: UpstreamCredentialView,
    pub secret: Option<String>,
}

pub(crate) type CredentialBinding = (Uuid, Option<Uuid>, Option<Uuid>, Uuid);

pub(crate) fn validate_legacy_auth(
    name: &str,
    base_url: &str,
    kind: &str,
    header: Option<&str>,
    secret: Option<&str>,
) -> bool {
    if UpstreamAuth::compile(kind, header, secret).is_err() {
        return false;
    }
    kind == "none"
        || (!name.trim().is_empty()
            && name.chars().count() <= 100
            && CredentialTarget::parse(base_url).is_ok())
}

impl CredentialRecord {
    pub(crate) fn validate(&self) -> Result<(), RepositoryError> {
        if self.deleted_at.is_some() {
            return Ok(());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 100 {
            return Err(RepositoryError::Validation);
        }
        if self.kind == "codex_oauth" {
            if self.secret.is_some() || self.header_name.is_some() {
                return Err(RepositoryError::Validation);
            }
        } else {
            UpstreamAuth::compile(
                &self.kind,
                self.header_name.as_deref(),
                self.secret.as_deref(),
            )
            .map_err(|_| RepositoryError::Validation)?;
        }
        if !matches!(self.kind.as_str(), "bearer" | "header" | "codex_oauth")
            || self.allowed_base_urls.is_empty()
        {
            return Err(RepositoryError::Validation);
        }
        let mut targets = std::collections::HashSet::new();
        for target in &self.allowed_base_urls {
            if !targets
                .insert(CredentialTarget::parse(target).map_err(|_| RepositoryError::Validation)?)
            {
                return Err(RepositoryError::Validation);
            }
        }
        Ok(())
    }

    pub(crate) fn allows_target(&self, target: &str) -> Result<(), RepositoryError> {
        let target = CredentialTarget::parse(target).map_err(|_| RepositoryError::Validation)?;
        if self
            .allowed_base_urls
            .iter()
            .any(|scope| CredentialTarget::parse(scope).is_ok_and(|scope| scope == target))
        {
            Ok(())
        } else {
            Err(RepositoryError::Validation)
        }
    }

    pub(crate) fn view(&self, bindings: &[CredentialBinding]) -> UpstreamCredentialView {
        let mut channel_ids = bindings
            .iter()
            .filter_map(|(channel, credential, _, _)| {
                (*credential == Some(self.id)).then_some(*channel)
            })
            .collect::<Vec<_>>();
        channel_ids.sort_unstable();
        UpstreamCredentialView {
            id: self.id,
            name: self.name.clone(),
            kind: self.kind.clone(),
            connector_kind: if self.kind == "codex_oauth" {
                ConnectorKind::CodexOauth
            } else {
                ConnectorKind::OpenAiCompatible
            },
            header_name: self.header_name.clone(),
            allowed_base_urls: self.allowed_base_urls.clone(),
            enabled: self.enabled,
            provider_managed: self.kind == "codex_oauth",
            channel_ids,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub(crate) fn audit(&self) -> Value {
        json!({
            "id": self.id, "name": self.name, "kind": self.kind,
            "header_name": self.header_name, "enabled": self.enabled,
            "credential_configured": self.secret.is_some(),
            "target_count": self.allowed_base_urls.len(),
            "updated_at": self.updated_at, "deleted_at": self.deleted_at,
        })
    }
}

pub(crate) fn prepare_record(
    id: Uuid,
    input: UpstreamCredentialInput,
    previous: Option<&CredentialRecord>,
    expected: Option<DateTime<Utc>>,
) -> Result<CredentialRecord, RepositoryError> {
    if let Some(previous) = previous {
        if previous.deleted_at.is_some() {
            return Err(RepositoryError::NotFound);
        }
        if Some(previous.updated_at) != expected {
            return Err(RepositoryError::Conflict);
        }
        if previous.kind != input.kind || previous.kind == "codex_oauth" {
            return Err(RepositoryError::Validation);
        }
    }
    if input.kind == "codex_oauth" {
        return Err(RepositoryError::Validation);
    }
    let now = Utc::now();
    let record = CredentialRecord {
        id,
        name: input.name,
        kind: input.kind,
        header_name: input.header_name.map(|name| name.to_ascii_lowercase()),
        secret: input
            .secret
            .or_else(|| previous.and_then(|record| record.secret.clone())),
        allowed_base_urls: input
            .allowed_base_urls
            .iter()
            .map(|value| {
                CredentialTarget::parse(value)
                    .map(|target| target.as_str().to_owned())
                    .map_err(|_| RepositoryError::Validation)
            })
            .collect::<Result<_, _>>()?,
        enabled: input.enabled,
        revision: Uuid::new_v4(),
        created_at: previous.map_or(now, |record| record.created_at),
        updated_at: now,
        deleted_at: None,
    };
    record.validate()?;
    Ok(record)
}

pub(crate) async fn pg_records(
    connection: &mut PgConnection,
) -> Result<Vec<CredentialRecord>, RepositoryError> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT row_to_json(c)::text FROM upstream_credentials c ORDER BY id",
    )
    .fetch_all(connection)
    .await?;
    rows.iter()
        .map(|row| serde_json::from_str(row).map_err(|_| RepositoryError::Validation))
        .collect()
}

pub(crate) async fn pg_bindings(
    connection: &mut PgConnection,
) -> Result<Vec<CredentialBinding>, RepositoryError> {
    // Live canonical logical channels own credential scope, including disabled
    // and unrouted drafts; the legacy physical channel table is no longer read.
    Ok(sqlx::query_as(
        "SELECT c.id,c.credential_id,NULL::uuid,c.binding_revision FROM upstream_channels c \
         WHERE c.deleted_at IS NULL ORDER BY c.id",
    )
    .fetch_all(connection)
    .await?)
}

pub(crate) async fn pg_save(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    input: UpstreamCredentialInput,
    expected: Option<DateTime<Utc>>,
) -> Result<MutationResult, RepositoryError> {
    let previous = if expected.is_some() {
        sqlx::query("SELECT id FROM upstream_credentials WHERE id=$1 FOR UPDATE")
            .bind(id)
            .execute(&mut **transaction)
            .await?;
        Some(
            pg_records(transaction)
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
        "SELECT access.base_url FROM upstream_channels channel \
         JOIN upstream_accesses access ON access.id=channel.access_id \
         WHERE channel.credential_id=$1 AND channel.deleted_at IS NULL",
    )
    .bind(id)
    .fetch_all(&mut **transaction)
    .await?
    {
        record.allows_target(&target)?;
    }
    let updated_at = if let Some(expected) = expected {
        sqlx::query_scalar(
            "UPDATE upstream_credentials SET name=$2,header_name=$3,secret=$4,allowed_base_urls=$5,
             enabled=$6,revision=$7 WHERE id=$1 AND updated_at=$8 RETURNING updated_at",
        )
        .bind(id)
        .bind(&record.name)
        .bind(&record.header_name)
        .bind(&record.secret)
        .bind(json!(record.allowed_base_urls))
        .bind(record.enabled)
        .bind(record.revision)
        .bind(expected)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(RepositoryError::Conflict)?
    } else {
        sqlx::query_scalar(
            "INSERT INTO upstream_credentials(id,name,kind,header_name,secret,allowed_base_urls,enabled,revision)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING updated_at")
            .bind(id).bind(&record.name).bind(&record.kind).bind(&record.header_name).bind(&record.secret)
            .bind(json!(record.allowed_base_urls)).bind(record.enabled).bind(record.revision)
            .fetch_one(&mut **transaction).await?
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

pub(crate) async fn pg_validate_binding(
    connection: &mut PgConnection,
    id: Option<Uuid>,
    target: &str,
) -> Result<(), RepositoryError> {
    let Some(id) = id else {
        return Ok(());
    };
    sqlx::query("SELECT id FROM upstream_credentials WHERE id=$1 FOR SHARE")
        .bind(id)
        .execute(&mut *connection)
        .await?;
    let record = pg_records(connection)
        .await?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or(RepositoryError::Validation)?;
    validate_binding(&record, target)
}

pub(crate) fn validate_binding(
    record: &CredentialRecord,
    target: &str,
) -> Result<(), RepositoryError> {
    if record.deleted_at.is_some() {
        return Err(RepositoryError::Validation);
    }
    record.validate()?;
    record.allows_target(target)
}

pub(crate) async fn pg_delete(
    transaction: &mut Transaction<'_, Postgres>,
    id: Uuid,
    expected: DateTime<Utc>,
) -> Result<MutationResult, RepositoryError> {
    sqlx::query("SELECT id FROM upstream_credentials WHERE id=$1 FOR UPDATE")
        .bind(id)
        .execute(&mut **transaction)
        .await?;
    let record = pg_records(transaction)
        .await?
        .into_iter()
        .find(|record| record.id == id && record.deleted_at.is_none())
        .ok_or(RepositoryError::NotFound)?;
    check_delete(&record, expected, &pg_bindings(transaction).await?)?;
    let updated_at = sqlx::query_scalar(
        "UPDATE upstream_credentials SET secret=NULL,enabled=false,deleted_at=now(),revision=$2
         WHERE id=$1 AND updated_at=$3 RETURNING updated_at",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(expected)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(RepositoryError::Conflict)?;
    Ok(deletion_result(record, updated_at))
}

pub(crate) fn check_delete(
    record: &CredentialRecord,
    expected: DateTime<Utc>,
    bindings: &[CredentialBinding],
) -> Result<(), RepositoryError> {
    if record.updated_at != expected {
        return Err(RepositoryError::Conflict);
    }
    if record.kind == "codex_oauth" {
        return Err(RepositoryError::Validation);
    }
    if bindings
        .iter()
        .any(|(_, credential, _, _)| *credential == Some(record.id))
    {
        return Err(RepositoryError::CredentialInUse);
    }
    Ok(())
}

pub(crate) fn deletion_result(
    record: CredentialRecord,
    updated_at: DateTime<Utc>,
) -> MutationResult {
    MutationResult {
        id: record.id,
        object_type: "upstream_credential",
        action: "delete",
        before_redacted: record.audit(),
        after_redacted: json!({"id":record.id,"deleted":true}),
        created_secret: None,
        reason: None,
        updated_at,
        correlation_id: None,
    }
}
