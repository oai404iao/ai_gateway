//! Serialized control-plane publication for reloads and management writes.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    domain::AutomaticDisableTrigger,
    persistence::{
        ApiHostsView, ChannelBatchUpdateInput, CodexCredentialBatchInput, CodexCredentialCreate,
        CodexCredentialUpdateInput, ConsoleApiKey, ConsoleAuditLog, ControlPlaneChannelDetail,
        ControlPlaneConfigTemplateDetail, ControlPlaneLists, ControlPlaneMutation,
        ControlPlaneRepository, MutationResult, RepositoryError, SelfApiKeyCreate,
        SelfApiKeyOptions, SelfApiKeyUpdate, SyncedModelInput, SystemSettingsView,
        UserBatchUpdateInput, UserSettingsInput, UserSettingsView,
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
            repository: self.repository.clone(),
            runtime: Arc::clone(&self.runtime),
            routing,
            serial: Arc::clone(&self.serial),
            upstream_client_cleanup: self.upstream_client_cleanup.clone(),
        }
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let next = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&next)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_manual_reload_audit(&mut transaction, actor, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(next);
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
        let mut transaction = self.repository.begin_serializable().await?;
        let settings = self
            .repository
            .update_user_settings(&mut transaction, user_id, input)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .apply_control_plane_mutation(&mut transaction, mutation)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        tracing::info!(%correlation_id, object_type = result.object_type, action = result.action, "management mutation committed");
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn update_channels_batch(
        &self,
        actor: Uuid,
        input: ChannelBatchUpdateInput,
    ) -> Result<ChannelBatchUpdateResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let mutations = self
            .repository
            .update_channels_batch(&mut transaction, input)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        for mutation in &mutations {
            self.repository
                .insert_audit(&mut transaction, actor, mutation, correlation_id)
                .await?;
        }
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .insert_codex_credential(&mut transaction, input, oauth_flow_id)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn update_codex_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .update_codex_credential(&mut transaction, channel_id, input, expected_updated_at)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn delete_codex_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .delete_codex_credential(&mut transaction, channel_id, expected_updated_at)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn update_codex_credentials_batch(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<CodexCredentialBatchResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let mutations = self
            .repository
            .update_codex_credentials_batch(&mut transaction, channel_group_id, input)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        for mutation in &mutations {
            self.repository
                .insert_audit(&mut transaction, actor, mutation, correlation_id)
                .await?;
        }
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let mutations = self
            .repository
            .update_users_batch(&mut transaction, actor, input)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        for mutation in &mutations {
            self.repository
                .insert_audit(&mut transaction, actor, mutation, correlation_id)
                .await?;
        }
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_user_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .create_own_api_key(&mut transaction, actor, input)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_self_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn update_own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_user_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .update_own_api_key(&mut transaction, actor, id, input, expected_updated_at)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_self_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
    }

    pub async fn revoke_own_api_key(
        &self,
        actor: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<MutationResult, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_user_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let result = self
            .repository
            .revoke_own_api_key(&mut transaction, actor, id, reason)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_self_audit(&mut transaction, actor, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        Ok(MutationResult {
            correlation_id: Some(correlation_id),
            ..result
        })
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
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let mutations = self
            .repository
            .apply_catalog_models(&mut transaction, inputs)
            .await?;
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        for mutation in &mutations {
            self.repository
                .insert_audit(&mut transaction, actor, mutation, correlation_id)
                .await?;
        }
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        let active = self
            .repository
            .active_admin_exists(&mut transaction, actor)
            .await?;
        transaction
            .rollback()
            .await
            .map_err(RepositoryError::from)?;
        if active {
            Ok(())
        } else {
            Err(ControlPlaneError::InvalidActor)
        }
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
        let mut transaction = self.repository.begin_serializable().await?;
        let Some(result) = self
            .repository
            .automatically_disable_channel(&mut transaction, channel_id, &trigger)
            .await?
        else {
            transaction.commit().await.map_err(RepositoryError::from)?;
            return Ok(false);
        };
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_system_audit(&mut transaction, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
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
        let mut transaction = self.repository.begin_serializable().await?;
        let Some(result) = self
            .repository
            .automatically_recover_channel(&mut transaction, channel_id)
            .await?
        else {
            transaction.commit().await.map_err(RepositoryError::from)?;
            return Ok(false);
        };
        let candidate = self.compile_transaction(&mut transaction).await?;
        self.validate_candidate(&candidate)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_system_audit(&mut transaction, &result, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(candidate);
        tracing::info!(
            %correlation_id,
            channel_id = %channel_id,
            action = result.action,
            "channel automation transition committed"
        );
        Ok(true)
    }

    fn publish(&self, next: Arc<crate::domain::CompiledRuntimeConfig>) {
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

    async fn compile_transaction(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Arc<crate::domain::CompiledRuntimeConfig>, ControlPlaneError> {
        Ok(Arc::new(compile_runtime_config(
            ControlPlaneRepository::load_runtime_transaction(transaction).await?,
        )?))
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
    Repository(#[from] RepositoryError),
    #[error("candidate configuration is invalid")]
    Compile(#[from] ConfigError),
    #[error("Console actor is not an active administrator")]
    InvalidActor,
}
