//! Control-plane backend dispatch and opaque prepared changes.
//! SQLite Codex operations remain fail-closed until S5; production configuration is still PG-only.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::AutomaticDisableTrigger;
use crate::domain::codex_sharing::SharingGroup;

use super::{
    ChannelBatchUpdateInput, ChannelDeletionImpact, CodexCredentialBatchInput,
    CodexCredentialCreate, CodexCredentialExportBundle, CodexCredentialExportInput,
    CodexCredentialRecord, CodexCredentialUpdateInput, CodexCredentialView, CodexOauthFlowRecord,
    CodexOauthStartInput, CodexQuotaReset, CodexQuotaResetOutcome, CodexQuotaUpdate,
    CodexQuotaWindowHistory, CodexRefresh, ConsoleApiKey, ConsoleAuditLog,
    ControlPlaneChannelDetail, ControlPlaneConfigTemplateDetail, ControlPlaneLists,
    ControlPlaneMutation, ControlPlaneRecords, MutationResult, PostgresControlPlaneRepository,
    ProxyRecord, RepositoryError, RuntimeConfigRecords, SelfApiKeyCreate, SelfApiKeyOptions,
    SelfApiKeyUpdate, SelfCodexQuotaCredentialView, SelfCodexQuotaWindowHistory, SyncedModelInput,
    SystemProbeIdentity, SystemSettingsInput, SystemSettingsView, UserBatchUpdateInput,
    UserSettingsInput, UserSettingsView, control_plane_write::PostgresPreparedControlPlaneChange,
};

#[cfg(feature = "sqlite-backend")]
use std::sync::Arc;

#[cfg(feature = "sqlite-backend")]
use super::sqlite::{
    SqliteControlPlaneRepository, SqliteDatabase, SqlitePreparedControlPlaneChange,
};

enum Backend {
    Postgres(PostgresControlPlaneRepository),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqliteControlPlaneRepository),
}

impl Clone for Backend {
    fn clone(&self) -> Self {
        match self {
            Self::Postgres(repository) => Self::Postgres(repository.clone()),
            #[cfg(feature = "sqlite-backend")]
            Self::Sqlite(repository) => Self::Sqlite(repository.clone()),
        }
    }
}

/// One validated, uncommitted control-plane change.
///
/// The variant stays private: callers can only read the complete pending
/// candidate with [`Self::runtime_records`] or make it durable with
/// [`Self::commit`], and dropping or rolling it back discards every pending
/// write together with its audit attribution.
pub struct PreparedControlPlaneChange<'a> {
    variant: PreparedControlPlaneChangeVariant<'a>,
}

enum PreparedControlPlaneChangeVariant<'a> {
    Postgres(PostgresPreparedControlPlaneChange<'a>),
    #[cfg(feature = "sqlite-backend")]
    Sqlite(SqlitePreparedControlPlaneChange),
}

impl<'a> PreparedControlPlaneChange<'a> {
    fn from_postgres(change: PostgresPreparedControlPlaneChange<'a>) -> Self {
        Self {
            variant: PreparedControlPlaneChangeVariant::Postgres(change),
        }
    }

    #[cfg(feature = "sqlite-backend")]
    pub(crate) fn from_sqlite(change: SqlitePreparedControlPlaneChange) -> Self {
        Self {
            variant: PreparedControlPlaneChangeVariant::Sqlite(change),
        }
    }

    pub async fn runtime_records(&mut self) -> Result<RuntimeConfigRecords, RepositoryError> {
        match &mut self.variant {
            PreparedControlPlaneChangeVariant::Postgres(change) => change.runtime_records().await,
            #[cfg(feature = "sqlite-backend")]
            PreparedControlPlaneChangeVariant::Sqlite(change) => change.runtime_records().await,
        }
    }

    pub async fn commit(self) -> Result<(Vec<MutationResult>, Uuid), RepositoryError> {
        match self.variant {
            PreparedControlPlaneChangeVariant::Postgres(change) => change.commit().await,
            #[cfg(feature = "sqlite-backend")]
            PreparedControlPlaneChangeVariant::Sqlite(change) => change.commit().await,
        }
    }

    /// Discards the pending change without writing its audit rows.
    pub async fn rollback(self) -> Result<(), RepositoryError> {
        match self.variant {
            PreparedControlPlaneChangeVariant::Postgres(change) => change.rollback().await,
            #[cfg(feature = "sqlite-backend")]
            PreparedControlPlaneChangeVariant::Sqlite(change) => change.rollback().await,
        }
    }
}

/// Control-plane consistency reads, prepared business changes, and Codex
/// credential operations.
#[derive(Clone)]
pub struct ControlPlaneRepository {
    backend: Backend,
}

impl ControlPlaneRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self {
            backend: Backend::Postgres(PostgresControlPlaneRepository::new(pool)),
        }
    }

    /// Builds a development repository over a file-backed SQLite database.
    ///
    /// The server composition root never selects this constructor; it exists for
    /// backend contract tests and future SQLite enablement in S6.
    #[cfg(feature = "sqlite-backend")]
    #[must_use]
    pub fn from_sqlite(database: Arc<SqliteDatabase>) -> Self {
        Self {
            backend: Backend::Sqlite(SqliteControlPlaneRepository::new(database)),
        }
    }

    /// Reads one stored proxy by id.
    pub(crate) async fn proxy_record(
        &self,
        id: Uuid,
    ) -> Result<Option<ProxyRecord>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.proxy_record(id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.proxy_record(id).await,
        }
    }

    /// Selects the PostgreSQL implementation for operations the current SQLite
    /// slice does not implement.
    ///
    /// This is the single explicit PostgreSQL-only accessor: it converts the
    /// selected SQLite backend into a typed internal failure so a missing
    /// implementation can never look like a successful empty result.
    #[cfg_attr(not(feature = "sqlite-backend"), allow(dead_code, unused_variables))]
    fn postgres(
        &self,
        operation: &'static str,
    ) -> Result<&PostgresControlPlaneRepository, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => Ok(repository),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(_) => Err(super::backend::UnsupportedBackendOperation::new(
                super::backend::BackendKind::Sqlite,
                operation,
            )
            .into()),
        }
    }

    pub async fn ensure_system_probe_identity(
        &self,
    ) -> Result<SystemProbeIdentity, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.ensure_system_probe_identity().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.ensure_system_probe_identity().await,
        }
    }

    pub async fn ensure_system_settings(
        &self,
        input: SystemSettingsInput,
    ) -> Result<(), RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.ensure_system_settings(input).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.ensure_system_settings(input).await,
        }
    }

    pub async fn load(&self) -> Result<ControlPlaneRecords, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.load().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.load().await,
        }
    }

    pub async fn load_runtime(&self) -> Result<RuntimeConfigRecords, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.load_runtime().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.load_runtime().await,
        }
    }

    pub async fn system_settings(&self) -> Result<SystemSettingsView, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.system_settings().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.system_settings().await,
        }
    }

    pub async fn user_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<UserSettingsView>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.user_settings(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.user_settings(user_id).await,
        }
    }

    pub async fn control_plane_lists(&self) -> Result<ControlPlaneLists, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.control_plane_lists().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.control_plane_lists().await,
        }
    }

    pub async fn control_plane_channel_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneChannelDetail>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.control_plane_channel_detail(id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.control_plane_channel_detail(id).await,
        }
    }

    pub async fn channel_group_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.channel_group_deletion_impact(id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.channel_group_deletion_impact(id).await,
        }
    }

    pub async fn channel_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.channel_deletion_impact(id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.channel_deletion_impact(id).await,
        }
    }

    pub async fn control_plane_config_template_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneConfigTemplateDetail>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => {
                repository.control_plane_config_template_detail(id).await
            }
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => {
                repository.control_plane_config_template_detail(id).await
            }
        }
    }

    pub async fn audit_logs(&self, limit: i64) -> Result<Vec<ConsoleAuditLog>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.audit_logs(limit).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.audit_logs(limit).await,
        }
    }

    pub async fn own_api_keys(&self, user_id: Uuid) -> Result<Vec<ConsoleApiKey>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.own_api_keys(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.own_api_keys(user_id).await,
        }
    }

    pub async fn own_api_key(
        &self,
        user_id: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleApiKey>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.own_api_key(user_id, id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.own_api_key(user_id, id).await,
        }
    }

    pub async fn own_api_key_options(
        &self,
        user_id: Uuid,
    ) -> Result<SelfApiKeyOptions, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.own_api_key_options(user_id).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.own_api_key_options(user_id).await,
        }
    }

    pub async fn model_source_ids(&self) -> Result<Vec<String>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.model_source_ids().await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.model_source_ids().await,
        }
    }

    pub async fn verify_active_admin(&self, actor: Uuid) -> Result<(), RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository.verify_active_admin(actor).await,
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository.verify_active_admin(actor).await,
        }
    }

    pub async fn prepare_manual_reload(
        &self,
        actor: Uuid,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_manual_reload(actor)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_manual_reload(actor)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_user_settings(
        &self,
        user_id: Uuid,
        input: UserSettingsInput,
    ) -> Result<(UserSettingsView, PreparedControlPlaneChange<'_>), RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_user_settings(user_id, input)
                .await
                .map(|(settings, change)| {
                    (settings, PreparedControlPlaneChange::from_postgres(change))
                }),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_user_settings(user_id, input)
                .await
                .map(|(settings, change)| {
                    (settings, PreparedControlPlaneChange::from_sqlite(change))
                }),
        }
    }

    pub async fn prepare_mutation(
        &self,
        actor: Uuid,
        mutation: ControlPlaneMutation,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_mutation(actor, mutation)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_mutation(actor, mutation)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_channels_batch(
        &self,
        actor: Uuid,
        input: ChannelBatchUpdateInput,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_channels_batch(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_channels_batch(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_users_batch(
        &self,
        actor: Uuid,
        input: UserBatchUpdateInput,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_users_batch(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_users_batch(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_own_api_key_create(
        &self,
        actor: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_own_api_key_create(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_own_api_key_create(actor, input)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_own_api_key_update(
        &self,
        actor: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_own_api_key_update(actor, id, input, expected_updated_at)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_own_api_key_update(actor, id, input, expected_updated_at)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_own_api_key_revoke(
        &self,
        actor: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_own_api_key_revoke(actor, id, reason)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_own_api_key_revoke(actor, id, reason)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_own_api_key_delete(
        &self,
        actor: Uuid,
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_own_api_key_delete(actor, id, expected_updated_at)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_own_api_key_delete(actor, id, expected_updated_at)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_catalog_models(
        &self,
        actor: Uuid,
        inputs: Vec<SyncedModelInput>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_catalog_models(actor, inputs)
                .await
                .map(PreparedControlPlaneChange::from_postgres),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_catalog_models(actor, inputs)
                .await
                .map(PreparedControlPlaneChange::from_sqlite),
        }
    }

    pub async fn prepare_channel_disable(
        &self,
        channel_id: Uuid,
        trigger: &AutomaticDisableTrigger,
    ) -> Result<Option<PreparedControlPlaneChange<'_>>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_channel_disable(channel_id, trigger)
                .await
                .map(|change| change.map(PreparedControlPlaneChange::from_postgres)),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_channel_disable(channel_id, trigger)
                .await
                .map(|change| change.map(PreparedControlPlaneChange::from_sqlite)),
        }
    }

    pub async fn prepare_channel_recovery(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<PreparedControlPlaneChange<'_>>, RepositoryError> {
        match &self.backend {
            Backend::Postgres(repository) => repository
                .prepare_channel_recovery(channel_id)
                .await
                .map(|change| change.map(PreparedControlPlaneChange::from_postgres)),
            #[cfg(feature = "sqlite-backend")]
            Backend::Sqlite(repository) => repository
                .prepare_channel_recovery(channel_id)
                .await
                .map(|change| change.map(PreparedControlPlaneChange::from_sqlite)),
        }
    }
    pub async fn prepare_codex_credential_create(
        &self,
        actor: Uuid,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        self.postgres("prepare_codex_credential_create")?
            .prepare_codex_credential_create(actor, input, oauth_flow_id)
            .await
            .map(PreparedControlPlaneChange::from_postgres)
    }

    pub async fn prepare_codex_credential_update(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        self.postgres("prepare_codex_credential_update")?
            .prepare_codex_credential_update(actor, channel_id, input, expected_updated_at)
            .await
            .map(PreparedControlPlaneChange::from_postgres)
    }

    pub async fn prepare_codex_credential_delete(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        self.postgres("prepare_codex_credential_delete")?
            .prepare_codex_credential_delete(actor, channel_id, expected_updated_at)
            .await
            .map(PreparedControlPlaneChange::from_postgres)
    }

    pub async fn prepare_codex_credentials_batch(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<PreparedControlPlaneChange<'_>, RepositoryError> {
        self.postgres("prepare_codex_credentials_batch")?
            .prepare_codex_credentials_batch(actor, channel_group_id, input)
            .await
            .map(PreparedControlPlaneChange::from_postgres)
    }

    pub async fn codex_credentials(
        &self,
        channel_group_id: Uuid,
    ) -> Result<Vec<CodexCredentialView>, RepositoryError> {
        self.postgres("codex_credentials")?
            .codex_credentials(channel_group_id)
            .await
    }

    pub async fn codex_credential_view(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialView>, RepositoryError> {
        self.postgres("codex_credential_view")?
            .codex_credential_view(channel_id)
            .await
    }

    pub async fn codex_quota_window_history(
        &self,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<CodexQuotaWindowHistory, RepositoryError> {
        self.postgres("codex_quota_window_history")?
            .codex_quota_window_history(channel_id, limit_per_window)
            .await
    }

    pub async fn self_codex_quota_credentials(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<SelfCodexQuotaCredentialView>, RepositoryError> {
        self.postgres("self_codex_quota_credentials")?
            .self_codex_quota_credentials(user_id)
            .await
    }

    pub async fn self_codex_quota_window_history(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<SelfCodexQuotaWindowHistory, RepositoryError> {
        self.postgres("self_codex_quota_window_history")?
            .self_codex_quota_window_history(user_id, channel_id, limit_per_window)
            .await
    }

    pub async fn codex_credential(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialRecord>, RepositoryError> {
        self.postgres("codex_credential")?
            .codex_credential(channel_id)
            .await
    }

    pub async fn load_codex_credentials(
        &self,
    ) -> Result<Vec<CodexCredentialRecord>, RepositoryError> {
        self.postgres("load_codex_credentials")?
            .load_codex_credentials()
            .await
    }

    pub async fn set_codex_user_id_if_missing(
        &self,
        channel_id: Uuid,
        user_id: &str,
    ) -> Result<bool, RepositoryError> {
        self.postgres("set_codex_user_id_if_missing")?
            .set_codex_user_id_if_missing(channel_id, user_id)
            .await
    }

    pub async fn export_codex_credentials(
        &self,
        channel_group_id: Uuid,
        input: CodexCredentialExportInput,
    ) -> Result<CodexCredentialExportBundle, RepositoryError> {
        self.postgres("export_codex_credentials")?
            .export_codex_credentials(channel_group_id, input)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_codex_oauth_flow(
        &self,
        actor_user_id: Uuid,
        channel_group_id: Uuid,
        input: CodexOauthStartInput,
        redirect_uri: String,
        state_hash: Vec<u8>,
        code_verifier: String,
        expires_at: DateTime<Utc>,
    ) -> Result<CodexOauthFlowRecord, RepositoryError> {
        self.postgres("create_codex_oauth_flow")?
            .create_codex_oauth_flow(
                actor_user_id,
                channel_group_id,
                input,
                redirect_uri,
                state_hash,
                code_verifier,
                expires_at,
            )
            .await
    }

    pub async fn codex_oauth_flow(
        &self,
        id: Uuid,
        actor_user_id: Uuid,
    ) -> Result<Option<CodexOauthFlowRecord>, RepositoryError> {
        self.postgres("codex_oauth_flow")?
            .codex_oauth_flow(id, actor_user_id)
            .await
    }

    pub async fn persist_codex_quota(
        &self,
        channel_id: Uuid,
        quota: CodexQuotaUpdate,
    ) -> Result<(), RepositoryError> {
        self.postgres("persist_codex_quota")?
            .persist_codex_quota(channel_id, quota)
            .await
    }

    pub async fn record_codex_quota_reset(
        &self,
        actor_user_id: Uuid,
        channel_id: Uuid,
        event_id: Uuid,
        requested_at: DateTime<Utc>,
        outcome: CodexQuotaResetOutcome,
        windows_reset: i32,
    ) -> Result<Uuid, RepositoryError> {
        self.postgres("record_codex_quota_reset")?
            .record_codex_quota_reset(
                actor_user_id,
                channel_id,
                event_id,
                requested_at,
                outcome,
                windows_reset,
            )
            .await
    }

    pub async fn mark_codex_credential_error(
        &self,
        channel_id: Uuid,
        permanent: bool,
        code: &str,
        summary: &str,
    ) -> Result<(), RepositoryError> {
        self.postgres("mark_codex_credential_error")?
            .mark_codex_credential_error(channel_id, permanent, code, summary)
            .await
    }

    pub async fn cleanup_codex_oauth_flows(&self) -> Result<u64, RepositoryError> {
        self.postgres("cleanup_codex_oauth_flows")?
            .cleanup_codex_oauth_flows()
            .await
    }

    pub async fn claim_sharing_ledger(
        &self,
        ledger_id: Uuid,
    ) -> Result<sqlx::PgConnection, RepositoryError> {
        self.postgres("claim_sharing_ledger")?
            .claim_sharing_ledger(ledger_id)
            .await
    }

    pub async fn sharing_groups(
        &self,
        user: Option<Uuid>,
    ) -> Result<Vec<SharingGroup>, RepositoryError> {
        self.postgres("sharing_groups")?.sharing_groups(user).await
    }

    pub async fn lock_codex_refresh(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<(CodexCredentialRecord, CodexRefresh<'_>)>, RepositoryError> {
        self.postgres("lock_codex_refresh")?
            .lock_codex_refresh(channel_id)
            .await
    }

    pub async fn lock_codex_quota_reset(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<(CodexCredentialRecord, CodexQuotaReset<'_>)>, RepositoryError> {
        self.postgres("lock_codex_quota_reset")?
            .lock_codex_quota_reset(channel_id)
            .await
    }
}
