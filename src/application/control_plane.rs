//! Serialized control-plane publication for reloads and management writes.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    domain::AutomaticDisableTrigger,
    persistence::{
        ApiHostsView, ChannelBatchUpdateInput, ChannelDeletionImpact, CodexCredentialBatchInput,
        CodexCredentialCreate, CodexCredentialUpdateInput, ConsoleApiKey, ConsoleAuditLog,
        ControlPlaneChannelDetail, ControlPlaneConfigTemplateDetail, ControlPlaneLists,
        ControlPlaneMutation, ControlPlaneRepository, MutationResult, PreparedControlPlaneChange,
        RepositoryError, SelfApiKeyCreate, SelfApiKeyOptions, SelfApiKeyUpdate, SyncedModelInput,
        SystemSettingsView, UserBatchUpdateInput, UserSettingsInput, UserSettingsView,
    },
    routing::{
        PassiveHealthPolicy, RoutingRuntime, SessionAffinityCacheClearResult,
        SessionAffinityCacheSnapshot,
    },
    runtime_config::{ConfigError, RuntimeConfig, compile_runtime_config},
    upstream::{UpstreamClientError, UpstreamClientRegistry, validate_snapshot_upstream_policies},
};

/// The single process gate prevents a periodic read from publishing an older
/// snapshot over a just-committed management mutation.
#[derive(Clone)]
pub struct ControlPlaneCoordinator {
    sharing: crate::codex_sharing::SharingRuntime,
    repository: ControlPlaneRepository,
    runtime: Arc<RuntimeConfig>,
    routing: RoutingRuntime,
    serial: Arc<Mutex<()>>,
    upstream_client_cleanup: Option<UpstreamClientCleanup>,
}

#[derive(Clone)]
struct UpstreamClientCleanup {
    registry: Arc<UpstreamClientRegistry>,
}

impl ControlPlaneCoordinator {
    #[must_use]
    pub fn new(
        repository: ControlPlaneRepository,
        runtime: Arc<RuntimeConfig>,
        routing: RoutingRuntime,
    ) -> Self {
        Self {
            sharing: Default::default(),
            repository,
            runtime,
            routing,
            serial: Arc::new(Mutex::new(())),
            upstream_client_cleanup: None,
        }
    }
    /// Creates a coordinator which reconciles the process-shared upstream
    /// client registry before each snapshot publication.
    pub fn new_with_upstream_registry(
        repository: ControlPlaneRepository,
        runtime: Arc<RuntimeConfig>,
        routing: RoutingRuntime,
        upstream_clients: Arc<UpstreamClientRegistry>,
    ) -> Result<Self, UpstreamClientError> {
        Self::new(repository, runtime, routing).with_upstream_registry(upstream_clients)
    }
    /// Adds a shared upstream client registry and establishes its initial
    /// active-key set from the current runtime snapshot.
    pub fn with_upstream_registry(
        mut self,
        upstream_clients: Arc<UpstreamClientRegistry>,
    ) -> Result<Self, UpstreamClientError> {
        upstream_clients.reconcile(&self.runtime.snapshot())?;
        self.upstream_client_cleanup = Some(UpstreamClientCleanup {
            registry: upstream_clients,
        });
        Ok(self)
    }
    #[must_use]
    pub fn with_routing(&self, routing: RoutingRuntime) -> Self {
        Self {
            sharing: self.sharing.clone(),
            repository: self.repository.clone(),
            runtime: Arc::clone(&self.runtime),
            routing,
            serial: Arc::clone(&self.serial),
            upstream_client_cleanup: self.upstream_client_cleanup.clone(),
        }
    }

    pub fn with_sharing_runtime(mut self, sharing: crate::codex_sharing::SharingRuntime) -> Self {
        sharing.publish(self.runtime.snapshot().sharing());
        self.sharing = sharing;
        self
    }

    pub fn sharing_runtime(&self) -> &crate::codex_sharing::SharingRuntime {
        &self.sharing
    }

    pub async fn sharing_groups(
        &self,
        user: Option<Uuid>,
    ) -> Result<Vec<crate::domain::codex_sharing::SharingGroup>, ControlPlaneError> {
        Ok(self.repository.sharing_groups(user).await?)
    }

    pub async fn reload(&self) -> Result<(), ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let next = Arc::new(compile_runtime_config(
            self.repository.load_runtime().await?,
        )?);
        self.validate_candidate(&next)?;
        self.publish(next);
        Ok(())
    }

    pub async fn manual_reload(&self, actor: Uuid) -> Result<Uuid, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self.repository.prepare_manual_reload(actor).await?;
        let (_, correlation_id) = self.commit_change(change).await?;
        Ok(correlation_id)
    }

    pub async fn lists(&self) -> Result<ControlPlaneLists, ControlPlaneError> {
        Ok(self.repository.control_plane_lists().await?)
    }

    pub async fn channel_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneChannelDetail>, ControlPlaneError> {
        Ok(self.repository.control_plane_channel_detail(id).await?)
    }

    pub async fn channel_group_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, ControlPlaneError> {
        Ok(self.repository.channel_group_deletion_impact(id).await?)
    }

    pub async fn channel_deletion_impact(
        &self,
        id: Uuid,
    ) -> Result<ChannelDeletionImpact, ControlPlaneError> {
        Ok(self.repository.channel_deletion_impact(id).await?)
    }

    pub async fn config_template_detail(
        &self,
        id: Uuid,
    ) -> Result<Option<ControlPlaneConfigTemplateDetail>, ControlPlaneError> {
        Ok(self
            .repository
            .control_plane_config_template_detail(id)
            .await?)
    }

    pub async fn system_settings(&self) -> Result<SystemSettingsView, ControlPlaneError> {
        Ok(self.repository.system_settings().await?)
    }

    pub async fn user_settings(
        &self,
        user_id: Uuid,
    ) -> Result<Option<UserSettingsView>, ControlPlaneError> {
        Ok(self.repository.user_settings(user_id).await?)
    }

    pub async fn update_user_settings(
        &self,
        user_id: Uuid,
        input: UserSettingsInput,
    ) -> Result<UserSettingsView, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let (settings, change) = self
            .repository
            .prepare_user_settings(user_id, input)
            .await?;
        self.commit_change(change).await?;
        tracing::info!(
            %user_id,
            websocket_enabled = settings.websocket_enabled,
            "user settings updated"
        );
        Ok(settings)
    }

    #[must_use]
    pub fn session_affinity_cache(&self) -> SessionAffinityCacheSnapshot {
        self.routing.session_affinity_cache_snapshot()
    }

    #[must_use]
    pub fn clear_session_affinity_cache(
        &self,
        rule_name: Option<&str>,
    ) -> Option<SessionAffinityCacheClearResult> {
        self.routing.clear_session_affinity_cache(rule_name)
    }

    pub async fn api_hosts(&self) -> Result<ApiHostsView, ControlPlaneError> {
        Ok(ApiHostsView {
            api_hosts: self.repository.system_settings().await?.settings.api_hosts,
        })
    }

    pub async fn mutate(
        &self,
        actor: Uuid,
        mutation: ControlPlaneMutation,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        if matches!(&mutation, ControlPlaneMutation::SaveCodexSharing { input, .. }
            if input.enabled && !self.sharing.available())
        {
            self.verify_active_admin(actor).await?;
            return Err(RepositoryError::Validation.into());
        }
        let change = self.repository.prepare_mutation(actor, mutation).await?;
        let result = self.commit_mutation(change).await?;
        let correlation_id = result
            .correlation_id
            .expect("committed mutation has audit id");
        tracing::info!(%correlation_id, object_type = result.object_type, action = result.action, "management mutation committed");
        Ok(result)
    }

    pub async fn update_channels_batch(
        &self,
        actor: Uuid,
        input: ChannelBatchUpdateInput,
    ) -> Result<ChannelBatchUpdateResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self.repository.prepare_channels_batch(actor, input).await?;
        let (mutations, correlation_id) = self.commit_change(change).await?;
        tracing::info!(
            %correlation_id,
            channel_count = mutations.len(),
            "channel batch update committed"
        );
        Ok(ChannelBatchUpdateResult {
            updated_ids: mutations.into_iter().map(|mutation| mutation.id).collect(),
            correlation_id,
        })
    }

    pub async fn create_codex_credential(
        &self,
        actor: Uuid,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_codex_credential_create(actor, input, oauth_flow_id)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn update_codex_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_codex_credential_update(actor, channel_id, input, expected_updated_at)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn delete_codex_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_codex_credential_delete(actor, channel_id, expected_updated_at)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn update_codex_credentials_batch(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<CodexCredentialBatchResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_codex_credentials_batch(actor, channel_group_id, input)
            .await?;
        let (mutations, correlation_id) = self.commit_change(change).await?;
        Ok(CodexCredentialBatchResult {
            updated_ids: mutations.into_iter().map(|mutation| mutation.id).collect(),
            correlation_id,
        })
    }

    pub async fn update_users_batch(
        &self,
        actor: Uuid,
        input: UserBatchUpdateInput,
    ) -> Result<UserBatchUpdateResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self.repository.prepare_users_batch(actor, input).await?;
        let (mutations, correlation_id) = self.commit_change(change).await?;
        tracing::info!(
            %correlation_id,
            user_count = mutations.len(),
            "user batch update committed"
        );
        Ok(UserBatchUpdateResult {
            updated_ids: mutations.into_iter().map(|mutation| mutation.id).collect(),
            correlation_id,
        })
    }

    pub async fn own_api_keys(&self, actor: Uuid) -> Result<Vec<ConsoleApiKey>, ControlPlaneError> {
        Ok(self.repository.own_api_keys(actor).await?)
    }

    pub async fn own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
    ) -> Result<Option<ConsoleApiKey>, ControlPlaneError> {
        Ok(self.repository.own_api_key(actor, id).await?)
    }

    pub async fn own_api_key_options(
        &self,
        actor: Uuid,
    ) -> Result<SelfApiKeyOptions, ControlPlaneError> {
        Ok(self.repository.own_api_key_options(actor).await?)
    }

    pub async fn audit_logs(&self, limit: i64) -> Result<Vec<ConsoleAuditLog>, ControlPlaneError> {
        Ok(self.repository.audit_logs(limit).await?)
    }

    pub async fn create_own_api_key(
        &self,
        actor: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_own_api_key_create(actor, input)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn update_own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_own_api_key_update(actor, id, input, expected_updated_at)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn revoke_own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_own_api_key_revoke(actor, id, reason)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn delete_own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_own_api_key_delete(actor, id, expected_updated_at)
            .await?;
        self.commit_mutation(change).await
    }

    pub async fn model_source_ids(&self) -> Result<Vec<String>, ControlPlaneError> {
        Ok(self.repository.model_source_ids().await?)
    }

    /// Applies a bounded, already validated external catalog selection and
    /// publishes it with one audit correlation id. A selected existing source
    /// model receives a new models.dev price snapshot; a new source model is
    /// imported. The catalog is fetched before entering this method, so a slow
    /// external dependency never holds the control-plane serialization gate or
    /// a database transaction.
    pub async fn apply_catalog_models(
        &self,
        actor: Uuid,
        inputs: Vec<SyncedModelInput>,
    ) -> Result<ModelSyncResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let change = self
            .repository
            .prepare_catalog_models(actor, inputs)
            .await?;
        let (mutations, correlation_id) = self.commit_change(change).await?;
        tracing::info!(
            %correlation_id,
            model_count = mutations.len(),
            "models.dev catalog changes committed"
        );
        Ok(ModelSyncResult {
            model_count: mutations.len(),
            imported_count: mutations
                .iter()
                .filter(|mutation| mutation.action == "import")
                .count(),
            updated_count: mutations
                .iter()
                .filter(|mutation| mutation.action == "price_sync")
                .count(),
            correlation_id,
        })
    }

    pub async fn verify_active_admin(&self, actor: Uuid) -> Result<(), ControlPlaneError> {
        Ok(self.repository.verify_active_admin(actor).await?)
    }

    /// Transitions one eligible channel into durable temporary disablement.
    /// Both the persisted global policy and the channel opt-in flag are
    /// rechecked transactionally, so a stale background event cannot override
    /// a just-saved administrator setting.
    pub async fn automatically_disable_channel(
        &self,
        channel_id: Uuid,
        trigger: AutomaticDisableTrigger,
    ) -> Result<bool, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let Some(change) = self
            .repository
            .prepare_channel_disable(channel_id, &trigger)
            .await?
        else {
            return Ok(false);
        };
        let result = self.commit_mutation(change).await?;
        let correlation_id = result
            .correlation_id
            .expect("committed mutation has audit id");
        tracing::info!(
            %correlation_id,
            channel_id = %channel_id,
            action = result.action,
            "channel automation transition committed"
        );
        Ok(true)
    }

    /// Clears a temporary disable after a successful periodic test. Automatic
    /// recovery is rechecked from the persisted system policy in the same
    /// transaction as the state transition.
    pub async fn automatically_recover_channel(
        &self,
        channel_id: Uuid,
    ) -> Result<bool, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let Some(change) = self.repository.prepare_channel_recovery(channel_id).await? else {
            return Ok(false);
        };
        let result = self.commit_mutation(change).await?;
        let correlation_id = result
            .correlation_id
            .expect("committed mutation has audit id");
        tracing::info!(
            %correlation_id,
            channel_id = %channel_id,
            action = result.action,
            "channel automation transition committed"
        );
        Ok(true)
    }

    fn publish(&self, next: Arc<crate::domain::CompiledRuntimeConfig>) {
        self.sharing.publish(next.sharing());
        if let Some(cleanup) = &self.upstream_client_cleanup {
            // The candidate was validated before this point, so a failure here
            // cannot be an invalid policy; retain the existing availability
            // handling for a poisoned shared registry.
            if let Err(error) = cleanup.registry.reconcile(&next) {
                tracing::warn!(%error, "upstream client registry reconciliation failed before configuration publication");
            }
        }
        let passive_health = next.system_settings().passive_health();
        self.routing.update_policy(PassiveHealthPolicy {
            connection_failure_threshold: passive_health.connection_failure_threshold(),
            cooldown: passive_health.cooldown(),
        });
        self.routing.reconcile(&next);
        self.runtime.replace_snapshot(next);
    }

    fn validate_candidate(
        &self,
        candidate: &crate::domain::CompiledRuntimeConfig,
    ) -> Result<(), ControlPlaneError> {
        validate_snapshot_upstream_policies(candidate).map_err(|_| {
            ConfigError::Compile("invalid resolved upstream timeout policy".into()).into()
        })
    }

    async fn commit_mutation(
        &self,
        change: PreparedControlPlaneChange<'_>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let (mut results, _) = self.commit_change(change).await?;
        Ok(results.pop().expect("single mutation produces one result"))
    }

    async fn commit_change(
        &self,
        mut change: PreparedControlPlaneChange<'_>,
    ) -> Result<(Vec<MutationResult>, Uuid), ControlPlaneError> {
        let candidate = Arc::new(compile_runtime_config(change.runtime_records().await?)?);
        self.validate_candidate(&candidate)?;
        let result = change.commit().await?;
        self.publish(candidate);
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ModelSyncResult {
    pub model_count: usize,
    pub imported_count: usize,
    pub updated_count: usize,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct ChannelBatchUpdateResult {
    pub updated_ids: Vec<Uuid>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct CodexCredentialBatchResult {
    pub updated_ids: Vec<Uuid>,
    pub correlation_id: Uuid,
}

#[derive(Clone, Debug)]
pub struct UserBatchUpdateResult {
    pub updated_ids: Vec<Uuid>,
    pub correlation_id: Uuid,
}

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("control-plane database operation failed")]
    Repository(#[source] RepositoryError),
    #[error("candidate configuration is invalid")]
    Compile(#[from] ConfigError),
    #[error("Console actor is not an active administrator")]
    InvalidActor,
}

impl From<RepositoryError> for ControlPlaneError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::InvalidActor => Self::InvalidActor,
            error => Self::Repository(error),
        }
    }
}
