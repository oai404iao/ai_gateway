//! Prepared control-plane changes: validation precedes atomic audit and commit.

use crate::domain::AutomaticDisableTrigger;
use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::persistence::*;

enum Audit {
    Admin(Uuid),
    SelfService(Uuid),
    System,
    Reload(Uuid),
    None,
}

/// Dropping a prepared change rolls it back. Callers must validate its complete
/// runtime records before committing, then publish only after commit succeeds.
pub(super) struct PostgresPreparedControlPlaneChange<'a> {
    repository: &'a PostgresControlPlaneRepository,
    transaction: Transaction<'a, Postgres>,
    mutations: Vec<MutationResult>,
    audit: Audit,
}

impl PostgresPreparedControlPlaneChange<'_> {
    /// Reads the complete pending candidate through the open transaction.
    pub async fn runtime_records(&mut self) -> Result<RuntimeConfigRecords, RepositoryError> {
        PostgresControlPlaneRepository::load_runtime_transaction(&mut self.transaction).await
    }

    pub async fn commit(mut self) -> Result<(Vec<MutationResult>, Uuid), RepositoryError> {
        let correlation_id = Uuid::new_v4();
        for mutation in &self.mutations {
            match self.audit {
                Audit::Admin(actor) => {
                    self.repository
                        .insert_audit(&mut self.transaction, actor, mutation, correlation_id)
                        .await?
                }
                Audit::SelfService(actor) => {
                    self.repository
                        .insert_self_audit(&mut self.transaction, actor, mutation, correlation_id)
                        .await?
                }
                Audit::System => {
                    self.repository
                        .insert_system_audit(&mut self.transaction, mutation, correlation_id)
                        .await?
                }
                Audit::Reload(_) | Audit::None => {}
            }
        }
        if let Audit::Reload(actor) = self.audit {
            self.repository
                .insert_manual_reload_audit(&mut self.transaction, actor, correlation_id)
                .await?;
        }
        self.transaction.commit().await?;
        for mutation in &mut self.mutations {
            mutation.correlation_id = Some(correlation_id);
        }
        Ok((self.mutations, correlation_id))
    }

    /// Discards the pending change. Equivalent to dropping it, but it reports
    /// a rollback failure instead of leaving it to connection teardown.
    pub async fn rollback(self) -> Result<(), RepositoryError> {
        self.transaction.rollback().await?;
        Ok(())
    }
}

impl PostgresControlPlaneRepository {
    fn prepared<'a>(
        &'a self,
        transaction: Transaction<'a, Postgres>,
        mutations: Vec<MutationResult>,
        audit: Audit,
    ) -> PostgresPreparedControlPlaneChange<'a> {
        PostgresPreparedControlPlaneChange {
            repository: self,
            transaction,
            mutations,
            audit,
        }
    }

    async fn admin_write(&self, actor: Uuid) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        if !self.active_admin_exists(&mut transaction, actor).await? {
            return Err(RepositoryError::InvalidActor);
        }
        Ok(transaction)
    }

    async fn self_service_write(
        &self,
        actor: Uuid,
    ) -> Result<Transaction<'_, Postgres>, RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        if !self.active_user_exists(&mut transaction, actor).await? {
            return Err(RepositoryError::InvalidActor);
        }
        Ok(transaction)
    }

    pub async fn verify_active_admin(&self, actor: Uuid) -> Result<(), RepositoryError> {
        self.admin_write(actor).await?.rollback().await?;
        Ok(())
    }

    pub async fn prepare_manual_reload(
        &self,
        actor: Uuid,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let transaction = self.admin_write(actor).await?;
        Ok(self.prepared(transaction, Vec::new(), Audit::Reload(actor)))
    }

    pub async fn prepare_user_settings(
        &self,
        user_id: Uuid,
        input: UserSettingsInput,
    ) -> Result<(UserSettingsView, PostgresPreparedControlPlaneChange<'_>), RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        let settings = self
            .update_user_settings(&mut transaction, user_id, input)
            .await?
            .ok_or(RepositoryError::NotFound)?;
        Ok((
            settings,
            self.prepared(transaction, Vec::new(), Audit::None),
        ))
    }

    pub async fn prepare_mutation(
        &self,
        actor: Uuid,
        mutation: ControlPlaneMutation,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let result = self
            .apply_control_plane_mutation(&mut transaction, mutation)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::Admin(actor)))
    }

    pub async fn prepare_model_onboarding(
        &self,
        actor: Uuid,
        input: ModelOnboardingInput<crate::models_dev::ModelsDevModel>,
    ) -> Result<(Uuid, PostgresPreparedControlPlaneChange<'_>), RepositoryError> {
        let mut tx = self.admin_write(actor).await?;
        let mut results = Vec::new();
        let model_id = match input.model {
            OnboardingModel::Existing { id, updated_at } => {
                let current: Option<DateTime<Utc>> = sqlx::query_scalar(
                    "SELECT updated_at FROM models WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
                if current != Some(updated_at) {
                    return Err(RepositoryError::Conflict);
                }
                id
            }
            OnboardingModel::Create(model) => {
                let result = self
                    .apply_control_plane_mutation(
                        &mut tx,
                        ControlPlaneMutation::CreateModel(*model),
                    )
                    .await
                    .map_err(StorageError::create_only)?;
                let id = result.id;
                results.push(result);
                id
            }
            OnboardingModel::Catalog(model) => {
                let result =
                    super::postgres_control_plane::import_model(&mut tx, model, Utc::now()).await?;
                let id = result.id;
                results.push(result);
                id
            }
        };
        let profile: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM model_routing_profiles WHERE model_id=$1 FOR UPDATE",
        )
        .bind(model_id)
        .fetch_optional(&mut *tx)
        .await?;
        let profile_id = if let Some(id) = profile {
            id
        } else {
            let result = self
                .apply_control_plane_mutation(
                    &mut tx,
                    ControlPlaneMutation::CreateRule(ModelRuleCreateInput { model_id }),
                )
                .await?;
            let id = result.id;
            results.push(result);
            id
        };
        // Even a target already present in its tier is version-guarded until commit.
        sqlx::query("SELECT id FROM channel_capabilities WHERE id=ANY($1) ORDER BY id FOR UPDATE")
            .bind(super::model_onboarding::capability_ids(&input.operations))
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT id FROM model_operation_rules WHERE model_routing_profile_id=$1 ORDER BY id FOR UPDATE")
            .bind(profile_id).execute(&mut *tx).await?;
        let topology = pg_load(&mut tx).await?;
        let additions =
            super::model_onboarding::route_mutations(profile_id, input.operations, &topology)?;
        for (id, (mut version, names)) in additions.registrations {
            for name in names {
                let input = super::model_onboarding::register_model(id, version, name);
                let updates = self.update_channels_batch(&mut tx, input).await?;
                if let Some(last) = updates.last() {
                    version = last.updated_at;
                }
                results.extend(updates);
            }
        }
        for rule in additions.rules {
            results.push(super::upstream_topology::rules::pg_append(&mut tx, rule).await?);
        }
        Ok((model_id, self.prepared(tx, results, Audit::Admin(actor))))
    }

    pub async fn prepare_channels_batch(
        &self,
        actor: Uuid,
        input: ChannelBatchUpdateInput,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = self.update_channels_batch(&mut transaction, input).await?;
        Ok(self.prepared(transaction, results, Audit::Admin(actor)))
    }

    pub async fn prepare_codex_credential_create(
        &self,
        actor: Uuid,
        input: CodexCredentialCreate,
        oauth_flow_id: Option<Uuid>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let result = self
            .insert_codex_credential(&mut transaction, input, oauth_flow_id)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::Admin(actor)))
    }

    pub async fn prepare_codex_credential_update(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let result = self
            .update_codex_credential(&mut transaction, channel_id, input, expected_updated_at)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::Admin(actor)))
    }

    pub async fn prepare_codex_credential_delete(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let result = self
            .delete_codex_credential(&mut transaction, channel_id, expected_updated_at)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::Admin(actor)))
    }

    pub async fn prepare_codex_credentials_batch(
        &self,
        actor: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = self
            .update_codex_credentials_batch(&mut transaction, input)
            .await?;
        Ok(self.prepared(transaction, results, Audit::Admin(actor)))
    }

    pub async fn prepare_users_batch(
        &self,
        actor: Uuid,
        input: UserBatchUpdateInput,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = self
            .update_users_batch(&mut transaction, actor, input)
            .await?;
        Ok(self.prepared(transaction, results, Audit::Admin(actor)))
    }

    pub async fn prepare_own_api_key_create(
        &self,
        actor: Uuid,
        input: SelfApiKeyCreate,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = self
            .create_own_api_key(&mut transaction, actor, input)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_update(
        &self,
        actor: Uuid,
        id: Uuid,
        input: SelfApiKeyUpdate,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = self
            .update_own_api_key(&mut transaction, actor, id, input, expected_updated_at)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_revoke(
        &self,
        actor: Uuid,
        id: Uuid,
        reason: String,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = self
            .revoke_own_api_key(&mut transaction, actor, id, reason)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::SelfService(actor)))
    }

    pub async fn prepare_own_api_key_delete(
        &self,
        actor: Uuid,
        id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.self_service_write(actor).await?;
        let result = self
            .delete_own_api_key(&mut transaction, actor, id, expected_updated_at)
            .await?;
        Ok(self.prepared(transaction, vec![result], Audit::SelfService(actor)))
    }

    pub async fn prepare_catalog_models(
        &self,
        actor: Uuid,
        inputs: Vec<SyncedModelInput>,
    ) -> Result<PostgresPreparedControlPlaneChange<'_>, RepositoryError> {
        let mut transaction = self.admin_write(actor).await?;
        let results = self.apply_catalog_models(&mut transaction, inputs).await?;
        Ok(self.prepared(transaction, results, Audit::Admin(actor)))
    }

    pub async fn prepare_channel_disable(
        &self,
        channel_id: Uuid,
        trigger: &AutomaticDisableTrigger,
    ) -> Result<Option<PostgresPreparedControlPlaneChange<'_>>, RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        let Some(result) = self
            .automatically_disable_channel(&mut transaction, channel_id, trigger)
            .await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        Ok(Some(self.prepared(
            transaction,
            vec![result],
            Audit::System,
        )))
    }

    pub async fn prepare_channel_recovery(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<PostgresPreparedControlPlaneChange<'_>>, RepositoryError> {
        let mut transaction = self.begin_serializable().await?;
        let Some(result) = self
            .automatically_recover_channel(&mut transaction, channel_id)
            .await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };
        Ok(Some(self.prepared(
            transaction,
            vec![result],
            Audit::System,
        )))
    }
}
