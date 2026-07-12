//! Serialized control-plane publication for reloads and management writes.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    persistence::{
        AdminLists, AdminMutation, ControlPlaneRepository, MutationResult, RepositoryError,
    },
    routing::RoutingRuntime,
    runtime_config::UpstreamConfig,
    runtime_config::{ConfigError, RuntimeConfig, compile_control_plane},
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
    upstream_defaults: UpstreamConfig,
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
        upstream_defaults: UpstreamConfig,
    ) -> Self {
        Self {
            repository,
            runtime,
            routing,
            serial: Arc::new(Mutex::new(())),
            upstream_defaults,
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
        upstream_defaults: UpstreamConfig,
    ) -> Result<Self, UpstreamClientError> {
        Self::new(repository, runtime, routing, upstream_defaults)
            .with_upstream_registry(upstream_clients)
    }
    /// Adds a shared upstream client registry and establishes its initial
    /// active-key set from the current runtime snapshot.
    pub fn with_upstream_registry(
        mut self,
        upstream_clients: Arc<UpstreamClientRegistry>,
    ) -> Result<Self, UpstreamClientError> {
        upstream_clients.reconcile(&self.runtime.snapshot(), &self.upstream_defaults)?;
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
            upstream_defaults: self.upstream_defaults.clone(),
            upstream_client_cleanup: self.upstream_client_cleanup.clone(),
        }
    }

    pub async fn reload(&self) -> Result<(), ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let next = Arc::new(compile_control_plane(self.repository.load().await?)?);
        self.validate_candidate(&next)?;
        self.publish(next);
        Ok(())
    }

    pub async fn manual_reload(&self, actor: Uuid) -> Result<Uuid, ControlPlaneError> {
        let _guard = self.serial.lock().await;
        let mut transaction = self.repository.begin_serializable().await?;
        if !self
            .repository
            .active_user_exists(&mut transaction, actor)
            .await?
        {
            return Err(ControlPlaneError::InvalidActor);
        }
        let next = Arc::new(compile_control_plane(
            ControlPlaneRepository::load_transaction(&mut transaction).await?,
        )?);
        self.validate_candidate(&next)?;
        let correlation_id = Uuid::new_v4();
        self.repository
            .insert_manual_reload_audit(&mut transaction, actor, correlation_id)
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.publish(next);
        Ok(correlation_id)
    }

    pub async fn lists(&self) -> Result<AdminLists, ControlPlaneError> {
        Ok(self.repository.admin_lists().await?)
    }

    pub async fn mutate(
        &self,
        actor: Uuid,
        mutation: AdminMutation,
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
            .apply_admin_mutation(&mut transaction, mutation)
            .await?;
        let candidate = Arc::new(compile_control_plane(
            ControlPlaneRepository::load_transaction(&mut transaction).await?,
        )?);
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

    pub async fn verify_active_actor(&self, actor: Uuid) -> Result<(), ControlPlaneError> {
        let mut transaction = self.repository.begin_serializable().await?;
        let active = self
            .repository
            .active_user_exists(&mut transaction, actor)
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

    fn publish(&self, next: Arc<crate::domain::CompiledRuntimeConfig>) {
        if let Some(cleanup) = &self.upstream_client_cleanup {
            // The candidate was validated before this point, so a failure here
            // cannot be an invalid policy; retain the existing availability
            // handling for a poisoned shared registry.
            if let Err(error) = cleanup.registry.reconcile(&next, &self.upstream_defaults) {
                tracing::warn!(%error, "upstream client registry reconciliation failed before configuration publication");
            }
        }
        self.routing.reconcile(&next);
        self.runtime.replace_snapshot(next);
    }

    fn validate_candidate(
        &self,
        candidate: &crate::domain::CompiledRuntimeConfig,
    ) -> Result<(), ControlPlaneError> {
        validate_snapshot_upstream_policies(candidate, &self.upstream_defaults).map_err(|_| {
            ConfigError::Compile("invalid resolved upstream timeout policy".into()).into()
        })
    }
}

#[derive(Debug, Error)]
pub enum ControlPlaneError {
    #[error("control-plane database operation failed")]
    Repository(#[from] RepositoryError),
    #[error("candidate configuration is invalid")]
    Compile(#[from] ConfigError),
    #[error("configured admin actor is not active")]
    InvalidActor,
}
