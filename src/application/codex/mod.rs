mod attempt;
mod protocol;
mod runtime;

use std::{collections::HashMap, sync::Arc};

use chrono::{DateTime, Utc};
use futures_util::{StreamExt, stream};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::{
    domain::{ApiFormat, ChannelTimeoutPolicy, CompiledChannelUpstreamPolicy},
    persistence::{
        CodexCredentialBatchInput, CodexCredentialCreate, CodexCredentialExportBundle,
        CodexCredentialExportInput, CodexCredentialImportInput, CodexCredentialRecord,
        CodexCredentialUpdateInput, CodexCredentialView, CodexOauthStartInput,
        CodexQuotaResetOutcome, CodexQuotaWindowHistory, CodexTokenRefreshUpdate,
        ControlPlaneRepository, MutationResult, RepositoryError, RepositoryTransaction,
        SelfCodexQuotaCredentialView, SelfCodexQuotaWindowHistory,
    },
    runtime_config::RuntimeConfig,
    transforms::TransformPlan,
    upstream::{ResolvedUpstreamPolicy, UpstreamClientError, UpstreamClientRegistry},
};

use super::{CodexCredentialBatchResult, ControlPlaneCoordinator, ControlPlaneError};
use protocol::{
    CodexEndpoints, build_authorize_url, consume_quota_reset_credit, exchange_code, fetch_models,
    fetch_quota, generate_oauth_state, generate_pkce, parse_callback_url, parse_identity,
    parse_jwt_expiration, refresh_tokens, state_hash, state_matches,
};

pub(crate) use attempt::{CodexAttemptError, PreparedCodexAttempt};
pub use protocol::{CODEX_CLIENT_VERSION, CODEX_ORIGINATOR, codex_user_agent};
pub use runtime::{CodexCredentialRuntime, CodexCredentialUnavailable, CompiledCodexCredential};

const OAUTH_FLOW_TTL: chrono::Duration = chrono::Duration::minutes(15);
const ACCESS_TOKEN_REFRESH_WINDOW: chrono::Duration = chrono::Duration::minutes(5);
const REFRESH_FALLBACK_AGE: chrono::Duration = chrono::Duration::days(8);
const QUOTA_REFRESH_INTERVAL: chrono::Duration = chrono::Duration::minutes(15);
const MAINTENANCE_CONCURRENCY: usize = 8;

type CredentialRefreshLocks = Arc<Mutex<HashMap<Uuid, Arc<Mutex<()>>>>>;
type CredentialQuotaLocks = Arc<Mutex<HashMap<Uuid, Arc<Mutex<()>>>>>;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexOauthCompleteInput {
    pub callback_url: String,
}

#[derive(Clone, Serialize)]
pub struct CodexOauthStartResponse {
    pub flow_id: Uuid,
    pub authorization_url: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Clone, Serialize)]
pub struct CodexQuotaResetResponse {
    pub outcome: CodexQuotaResetOutcome,
    pub windows_reset: i32,
    pub quota_refreshed: bool,
    pub correlation_id: Uuid,
}

#[derive(Clone)]
pub struct CodexConnectorService {
    repository: ControlPlaneRepository,
    coordinator: ControlPlaneCoordinator,
    runtime_config: Arc<RuntimeConfig>,
    upstream_clients: Arc<UpstreamClientRegistry>,
    credentials: CodexCredentialRuntime,
    endpoints: Arc<CodexEndpoints>,
    refresh_locks: CredentialRefreshLocks,
    quota_locks: CredentialQuotaLocks,
}

impl CodexConnectorService {
    pub async fn new(
        repository: ControlPlaneRepository,
        coordinator: ControlPlaneCoordinator,
        runtime_config: Arc<RuntimeConfig>,
        upstream_clients: Arc<UpstreamClientRegistry>,
    ) -> Result<Self, CodexConnectorError> {
        Self::new_with_endpoints(
            repository,
            coordinator,
            runtime_config,
            upstream_clients,
            CodexEndpoints::default(),
        )
        .await
    }

    pub(crate) async fn new_with_endpoints(
        repository: ControlPlaneRepository,
        coordinator: ControlPlaneCoordinator,
        runtime_config: Arc<RuntimeConfig>,
        upstream_clients: Arc<UpstreamClientRegistry>,
        endpoints: CodexEndpoints,
    ) -> Result<Self, CodexConnectorError> {
        let service = Self {
            repository,
            coordinator,
            runtime_config,
            upstream_clients,
            credentials: CodexCredentialRuntime::new(),
            endpoints: Arc::new(endpoints),
            refresh_locks: Arc::new(Mutex::new(HashMap::new())),
            quota_locks: Arc::new(Mutex::new(HashMap::new())),
        };
        service.backfill_missing_user_ids().await?;
        service.reload_runtime().await?;
        Ok(service)
    }

    #[must_use]
    pub fn runtime(&self) -> CodexCredentialRuntime {
        self.credentials.clone()
    }

    pub async fn list_credentials(
        &self,
        channel_group_id: Uuid,
    ) -> Result<Vec<CodexCredentialView>, CodexConnectorError> {
        Ok(self.repository.codex_credentials(channel_group_id).await?)
    }

    pub async fn credential(
        &self,
        channel_id: Uuid,
    ) -> Result<Option<CodexCredentialView>, CodexConnectorError> {
        Ok(self.repository.codex_credential_view(channel_id).await?)
    }

    pub async fn quota_window_history(
        &self,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<CodexQuotaWindowHistory, CodexConnectorError> {
        Ok(self
            .repository
            .codex_quota_window_history(channel_id, limit_per_window)
            .await?)
    }

    pub async fn self_quota_credentials(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<SelfCodexQuotaCredentialView>, CodexConnectorError> {
        Ok(self
            .repository
            .self_codex_quota_credentials(user_id)
            .await?)
    }

    pub async fn self_quota_window_history(
        &self,
        user_id: Uuid,
        channel_id: Uuid,
        limit_per_window: i64,
    ) -> Result<SelfCodexQuotaWindowHistory, CodexConnectorError> {
        Ok(self
            .repository
            .self_codex_quota_window_history(user_id, channel_id, limit_per_window)
            .await?)
    }

    pub async fn export_credentials(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialExportInput,
    ) -> Result<CodexCredentialExportBundle, CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        let bundle = self
            .repository
            .export_codex_credentials(channel_group_id, input)
            .await?;
        tracing::warn!(
            actor_user_id = %actor,
            %channel_group_id,
            credential_count = bundle.credentials.len(),
            proxy_count = bundle.proxies.len(),
            "Codex credentials exported"
        );
        Ok(bundle)
    }

    pub async fn start_oauth(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexOauthStartInput,
    ) -> Result<CodexOauthStartResponse, CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        let pkce = generate_pkce();
        let state = generate_oauth_state();
        let authorization_url = build_authorize_url(&self.endpoints, &pkce, &state)?;
        let expires_at = Utc::now() + OAUTH_FLOW_TTL;
        let flow = self
            .repository
            .create_codex_oauth_flow(
                actor,
                channel_group_id,
                input,
                protocol::CODEX_OAUTH_REDIRECT_URI.to_owned(),
                state_hash(&state).to_vec(),
                pkce.verifier,
                expires_at,
            )
            .await?;
        Ok(CodexOauthStartResponse {
            flow_id: flow.id,
            authorization_url,
            expires_at,
        })
    }

    pub async fn complete_oauth(
        &self,
        actor: Uuid,
        flow_id: Uuid,
        input: CodexOauthCompleteInput,
    ) -> Result<MutationResult, CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        let flow = self
            .repository
            .codex_oauth_flow(flow_id, actor)
            .await?
            .ok_or(CodexConnectorError::OauthFlowExpired)?;
        let callback = parse_callback_url(&input.callback_url)?;
        if !state_matches(&flow.state_hash, &callback.state) {
            return Err(CodexConnectorError::OauthStateMismatch);
        }
        let (client, policy) = self.client_for_proxy(flow.proxy_id)?;
        let tokens = exchange_code(
            &client,
            &self.endpoints,
            &callback.code,
            &flow.code_verifier,
            policy.timeouts().response_header(),
            policy.timeouts().stream_idle(),
        )
        .await?;
        let create = self
            .prepare_credential(
                flow.channel_group_id,
                flow.label,
                true,
                flow.proxy_id,
                flow.weight,
                flow.quota_threshold_percent,
                Some(tokens.id_token),
                tokens.access_token,
                tokens.refresh_token,
                None,
                None,
                &client,
                policy,
            )
            .await?;
        let result = self
            .coordinator
            .create_codex_credential(actor, create, Some(flow.id))
            .await?;
        self.reload_runtime().await?;
        Ok(result)
    }

    pub async fn import_credential(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialImportInput,
    ) -> Result<MutationResult, CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        let (client, policy) = self.client_for_proxy(input.proxy_id)?;
        let create = self
            .prepare_credential(
                channel_group_id,
                input.label,
                input.enabled,
                input.proxy_id,
                input.weight,
                input.quota_threshold_percent,
                input.id_token,
                input.access_token,
                input.refresh_token,
                input.account_id,
                input.user_id,
                &client,
                policy,
            )
            .await?;
        let result = self
            .coordinator
            .create_codex_credential(actor, create, None)
            .await?;
        self.reload_runtime().await?;
        Ok(result)
    }

    pub async fn update_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        input: CodexCredentialUpdateInput,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, CodexConnectorError> {
        let result = self
            .coordinator
            .update_codex_credential(actor, channel_id, input, expected_updated_at)
            .await?;
        self.reload_runtime().await?;
        Ok(result)
    }

    pub async fn delete_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
        expected_updated_at: DateTime<Utc>,
    ) -> Result<MutationResult, CodexConnectorError> {
        let result = self
            .coordinator
            .delete_codex_credential(actor, channel_id, expected_updated_at)
            .await?;
        self.reload_runtime().await?;
        Ok(result)
    }

    pub async fn update_credentials_batch(
        &self,
        actor: Uuid,
        channel_group_id: Uuid,
        input: CodexCredentialBatchInput,
    ) -> Result<CodexCredentialBatchResult, CodexConnectorError> {
        let result = self
            .coordinator
            .update_codex_credentials_batch(actor, channel_group_id, input)
            .await?;
        self.reload_runtime().await?;
        Ok(result)
    }

    pub async fn refresh_credential(
        &self,
        actor: Uuid,
        channel_id: Uuid,
    ) -> Result<(), CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        self.refresh_credential_system(channel_id).await
    }

    pub async fn refresh_quota(
        &self,
        actor: Uuid,
        channel_id: Uuid,
    ) -> Result<(), CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        self.refresh_quota_system(channel_id).await
    }

    pub async fn reset_quota(
        &self,
        actor: Uuid,
        channel_id: Uuid,
    ) -> Result<CodexQuotaResetResponse, CodexConnectorError> {
        self.coordinator.verify_active_admin(actor).await?;
        let quota_lock = self.quota_lock(channel_id).await;
        let _guard = quota_lock.lock().await;
        let mut transaction = self.repository.begin_codex_quota_reset().await?;
        let record = self
            .repository
            .codex_credential_for_update(&mut transaction, channel_id)
            .await?
            .ok_or(CodexConnectorError::CredentialNotFound)?;
        validate_quota_credential(&record)?;
        let (client, policy) = self.client_for_proxy(record.proxy_id)?;
        let requested_at = Utc::now();
        let redeem_request_id = Uuid::new_v4();
        let reset = consume_quota_reset_credit(
            &client,
            &self.endpoints,
            &record.access_token,
            record.account_id.as_deref(),
            record.is_fedramp,
            &redeem_request_id.to_string(),
            policy.timeouts().response_header(),
            policy.timeouts().stream_idle(),
        )
        .await?;
        let correlation_id = self
            .repository
            .record_codex_quota_reset_transaction(
                &mut transaction,
                actor,
                channel_id,
                redeem_request_id,
                requested_at,
                reset.outcome,
                reset.windows_reset,
                record.quota_reset_credits_available,
            )
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        let quota_refreshed = match self.refresh_quota_locked(record).await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    %channel_id,
                    %correlation_id,
                    code = error.code(),
                    "Codex quota reset completed but the follow-up quota refresh failed"
                );
                false
            }
        };
        Ok(CodexQuotaResetResponse {
            outcome: reset.outcome,
            windows_reset: reset.windows_reset,
            quota_refreshed,
            correlation_id,
        })
    }

    pub async fn run_maintenance(&self) -> Result<(), CodexConnectorError> {
        let records = self.repository.load_codex_credentials().await?;
        self.credentials.replace(records.clone());
        let service = self.clone();
        stream::iter(records)
            .for_each_concurrent(MAINTENANCE_CONCURRENCY, move |record| {
                let service = service.clone();
                async move {
                    service.maintain_credential(record).await;
                }
            })
            .await;
        let _ = self.repository.cleanup_codex_oauth_flows().await?;
        Ok(())
    }

    async fn maintain_credential(&self, record: CodexCredentialRecord) {
        if !record.enabled || record.runtime_status == "disabled" || record.reauth_required {
            return;
        }
        if refresh_due(&record)
            && let Err(error) = self
                .refresh_credential_if_generation(record.channel_id, record.refresh_generation)
                .await
        {
            tracing::warn!(
                channel_id = %record.channel_id,
                code = error.code(),
                "Codex OAuth credential refresh failed"
            );
        }
        if quota_due(&record)
            && let Err(error) = self.refresh_quota_system(record.channel_id).await
        {
            tracing::warn!(
                channel_id = %record.channel_id,
                code = error.code(),
                "Codex quota refresh failed"
            );
        }
    }

    pub async fn report_unauthorized(&self, channel_id: Uuid, observed_generation: i64) {
        if let Err(error) = self
            .refresh_credential_if_generation(channel_id, observed_generation)
            .await
        {
            tracing::warn!(
                channel_id = %channel_id,
                code = error.code(),
                "unable to recover Codex credential after an upstream 401"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_credential(
        &self,
        channel_group_id: Uuid,
        label: String,
        enabled: bool,
        proxy_id: Option<Uuid>,
        weight: i32,
        quota_threshold_percent: i16,
        id_token: Option<String>,
        access_token: String,
        refresh_token: String,
        supplied_account_id: Option<String>,
        supplied_user_id: Option<String>,
        client: &Client,
        policy: ResolvedUpstreamPolicy,
    ) -> Result<CodexCredentialCreate, CodexConnectorError> {
        let id_token = id_token
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let access_token = access_token.trim().to_owned();
        let refresh_token = refresh_token.trim().to_owned();
        if access_token.is_empty() || refresh_token.is_empty() {
            return Err(CodexConnectorError::InvalidCredential);
        }
        let identity = parse_identity(id_token.as_deref().unwrap_or(&access_token))?;
        let account_id = resolve_account_id(identity.account_id, supplied_account_id)?;
        let user_id = resolve_user_id(identity.user_id, supplied_user_id)?;
        if account_id.is_none() && user_id.is_none() {
            return Err(CodexConnectorError::InvalidCredential);
        }
        let access_token_expires_at = parse_jwt_expiration(&access_token)?;
        let models = fetch_models(
            client,
            &self.endpoints,
            &access_token,
            account_id.as_deref(),
            identity.is_fedramp,
            policy.timeouts().response_header(),
            policy.timeouts().stream_idle(),
        )
        .await?;
        let quota = match fetch_quota(
            client,
            &self.endpoints,
            &access_token,
            account_id.as_deref(),
            identity.is_fedramp,
            policy.timeouts().response_header(),
            policy.timeouts().stream_idle(),
        )
        .await
        {
            Ok(quota) => Some(quota),
            Err(error) => {
                tracing::warn!(
                    code = error.code(),
                    "Codex credential validated but initial quota fetch failed"
                );
                None
            }
        };
        Ok(CodexCredentialCreate {
            channel_group_id,
            label,
            enabled,
            proxy_id,
            weight,
            quota_threshold_percent,
            base_url: self.endpoints.responses_base_url.to_string(),
            email: identity.email,
            account_id,
            user_id,
            plan_type: identity.plan_type,
            is_fedramp: identity.is_fedramp,
            id_token: id_token.unwrap_or_else(|| access_token.clone()),
            access_token,
            refresh_token,
            access_token_expires_at,
            available_models: models,
            quota,
        })
    }

    async fn refresh_credential_system(&self, channel_id: Uuid) -> Result<(), CodexConnectorError> {
        let refresh_lock = self.refresh_lock(channel_id).await;
        let _guard = refresh_lock.lock().await;
        self.refresh_credential_locked(channel_id, None).await
    }

    async fn refresh_credential_if_generation(
        &self,
        channel_id: Uuid,
        observed_generation: i64,
    ) -> Result<(), CodexConnectorError> {
        let refresh_lock = self.refresh_lock(channel_id).await;
        let _guard = refresh_lock.lock().await;
        self.refresh_credential_locked(channel_id, Some(observed_generation))
            .await
    }

    async fn refresh_credential_locked(
        &self,
        channel_id: Uuid,
        observed_generation: Option<i64>,
    ) -> Result<(), CodexConnectorError> {
        let mut transaction = self.repository.begin_codex_refresh().await?;
        let record = self
            .repository
            .codex_credential_for_update(&mut transaction, channel_id)
            .await?
            .ok_or(CodexConnectorError::CredentialNotFound)?;
        if observed_generation.is_some_and(|generation| generation != record.refresh_generation) {
            transaction.commit().await.map_err(RepositoryError::from)?;
            return Ok(());
        }
        if !record.enabled || record.runtime_status == "disabled" {
            return Err(CodexConnectorError::CredentialDisabled);
        }
        if record.reauth_required && observed_generation.is_some() {
            return Err(CodexConnectorError::CredentialReauthenticationRequired);
        }
        let (client, policy) = self.client_for_proxy(record.proxy_id)?;
        let refreshed = match refresh_tokens(
            &client,
            &self.endpoints,
            &record.refresh_token,
            policy.timeouts().response_header(),
            policy.timeouts().stream_idle(),
        )
        .await
        {
            Ok(refreshed) => refreshed,
            Err(error) => {
                self.commit_refresh_failure(
                    transaction,
                    channel_id,
                    error.permanent_refresh_failure(),
                    &error,
                )
                .await?;
                return Err(error);
            }
        };

        let identity = match refreshed
            .id_token
            .as_deref()
            .map(parse_identity)
            .transpose()
        {
            Ok(identity) => identity,
            Err(error) => {
                self.commit_refresh_failure(transaction, channel_id, true, &error)
                    .await?;
                return Err(error);
            }
        };
        if identity
            .as_ref()
            .and_then(|identity| identity.account_id.as_deref())
            .zip(record.account_id.as_deref())
            .is_some_and(|(account_id, current_account_id)| account_id != current_account_id)
            || identity
                .as_ref()
                .and_then(|identity| identity.user_id.as_deref())
                .zip(record.user_id.as_deref())
                .is_some_and(|(user_id, current_user_id)| user_id != current_user_id)
        {
            let error = CodexConnectorError::AccountChanged;
            self.commit_refresh_failure(transaction, channel_id, true, &error)
                .await?;
            return Err(error);
        }
        let access_token_expires_at = match refreshed.access_token.as_deref() {
            Some(access_token) => match parse_jwt_expiration(access_token) {
                Ok(expires_at) => expires_at,
                Err(error) => {
                    self.commit_refresh_failure(transaction, channel_id, true, &error)
                        .await?;
                    return Err(error);
                }
            },
            None => record.access_token_expires_at,
        };
        let updated = self
            .repository
            .persist_codex_token_refresh_transaction(
                &mut transaction,
                channel_id,
                CodexTokenRefreshUpdate {
                    expected_generation: record.refresh_generation,
                    id_token: refreshed.id_token,
                    access_token: refreshed.access_token,
                    refresh_token: refreshed.refresh_token,
                    email: identity
                        .as_ref()
                        .and_then(|identity| identity.email.clone()),
                    account_id: identity
                        .as_ref()
                        .and_then(|identity| identity.account_id.clone()),
                    user_id: identity
                        .as_ref()
                        .and_then(|identity| identity.user_id.clone()),
                    plan_type: identity
                        .as_ref()
                        .and_then(|identity| identity.plan_type.clone()),
                    is_fedramp: identity.as_ref().map(|identity| identity.is_fedramp),
                    access_token_expires_at,
                    refreshed_at: Utc::now(),
                },
            )
            .await?;
        if !updated {
            return Err(CodexConnectorError::Repository(RepositoryError::Conflict));
        }
        transaction.commit().await.map_err(RepositoryError::from)?;
        tracing::info!(%channel_id, "Codex OAuth credential refreshed");
        self.reload_runtime().await?;
        Ok(())
    }

    async fn refresh_lock(&self, channel_id: Uuid) -> Arc<Mutex<()>> {
        let mut locks = self.refresh_locks.lock().await;
        Arc::clone(
            locks
                .entry(channel_id)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn quota_lock(&self, channel_id: Uuid) -> Arc<Mutex<()>> {
        let mut locks = self.quota_locks.lock().await;
        Arc::clone(
            locks
                .entry(channel_id)
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn commit_refresh_failure(
        &self,
        mut transaction: RepositoryTransaction<'_>,
        channel_id: Uuid,
        permanent: bool,
        error: &CodexConnectorError,
    ) -> Result<(), CodexConnectorError> {
        self.repository
            .mark_codex_credential_error_transaction(
                &mut transaction,
                channel_id,
                permanent,
                error.code(),
                error.safe_summary(),
            )
            .await?;
        transaction.commit().await.map_err(RepositoryError::from)?;
        self.reload_runtime().await
    }

    async fn refresh_quota_system(&self, channel_id: Uuid) -> Result<(), CodexConnectorError> {
        let quota_lock = self.quota_lock(channel_id).await;
        let _guard = quota_lock.lock().await;
        let record = self
            .repository
            .codex_credential(channel_id)
            .await?
            .ok_or(CodexConnectorError::CredentialNotFound)?;
        self.refresh_quota_locked(record).await
    }

    async fn refresh_quota_locked(
        &self,
        mut record: CodexCredentialRecord,
    ) -> Result<(), CodexConnectorError> {
        validate_quota_credential(&record)?;
        let channel_id = record.channel_id;
        let mut refreshed_after_unauthorized = false;
        loop {
            let (client, policy) = self.client_for_proxy(record.proxy_id)?;
            match fetch_quota(
                &client,
                &self.endpoints,
                &record.access_token,
                record.account_id.as_deref(),
                record.is_fedramp,
                policy.timeouts().response_header(),
                policy.timeouts().stream_idle(),
            )
            .await
            {
                Ok(quota) => {
                    self.repository
                        .persist_codex_quota(channel_id, quota)
                        .await?;
                    self.reload_runtime().await?;
                    return Ok(());
                }
                Err(CodexConnectorError::CodexBackendStatus(401))
                    if !refreshed_after_unauthorized =>
                {
                    self.refresh_credential_if_generation(channel_id, record.refresh_generation)
                        .await?;
                    record = self
                        .repository
                        .codex_credential(channel_id)
                        .await?
                        .ok_or(CodexConnectorError::CredentialNotFound)?;
                    refreshed_after_unauthorized = true;
                }
                Err(error) => {
                    self.repository
                        .mark_codex_credential_error(
                            channel_id,
                            false,
                            error.code(),
                            error.safe_summary(),
                        )
                        .await?;
                    self.reload_runtime().await?;
                    return Err(error);
                }
            }
        }
    }

    fn client_for_proxy(
        &self,
        proxy_id: Option<Uuid>,
    ) -> Result<(Client, ResolvedUpstreamPolicy), CodexConnectorError> {
        let snapshot = self.runtime_config.snapshot();
        let proxy = proxy_id
            .map(|id| snapshot.proxy(id).ok_or(CodexConnectorError::InvalidProxy))
            .transpose()?;
        let api_format = ApiFormat::OpenAiResponses;
        let noop = TransformPlan::noop(api_format);
        let policy = CompiledChannelUpstreamPolicy::new_with_default_connect_timeout(
            proxy,
            None,
            noop.clone(),
            noop,
            ChannelTimeoutPolicy::default(),
            snapshot.system_settings().upstream_timeouts().connect(),
        );
        let resolved = ResolvedUpstreamPolicy::try_resolve(
            &snapshot.system_settings().upstream_timeouts(),
            &policy,
        )
        .map_err(|_| CodexConnectorError::InvalidProxy)?;
        let client = self.upstream_clients.client_for(&policy, resolved)?;
        Ok((client, resolved))
    }

    async fn reload_runtime(&self) -> Result<(), CodexConnectorError> {
        self.credentials
            .replace(self.repository.load_codex_credentials().await?);
        Ok(())
    }

    async fn backfill_missing_user_ids(&self) -> Result<(), CodexConnectorError> {
        for record in self.repository.load_codex_credentials().await? {
            if record.user_id.is_some() {
                continue;
            }
            let user_id = [&record.id_token, &record.access_token]
                .into_iter()
                .filter_map(|token| parse_identity(token).ok())
                .filter(|identity| {
                    identity
                        .account_id
                        .as_deref()
                        .is_none_or(|account_id| record.account_id.as_deref() == Some(account_id))
                })
                .find_map(|identity| identity.user_id);
            let Some(user_id) = user_id else {
                continue;
            };
            let _ = self
                .repository
                .set_codex_user_id_if_missing(record.channel_id, &user_id)
                .await?;
        }
        Ok(())
    }
}

fn resolve_account_id(
    token_account_id: Option<String>,
    supplied_account_id: Option<String>,
) -> Result<Option<String>, CodexConnectorError> {
    let token_account_id = token_account_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let supplied_account_id = supplied_account_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    match (token_account_id, supplied_account_id) {
        (Some(token), Some(supplied)) if token != supplied => {
            Err(CodexConnectorError::AccountChanged)
        }
        (Some(token), _) => Ok(Some(token)),
        (None, supplied) => Ok(supplied),
    }
}

fn resolve_user_id(
    token_user_id: Option<String>,
    supplied_user_id: Option<String>,
) -> Result<Option<String>, CodexConnectorError> {
    let token_user_id = token_user_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let supplied_user_id = supplied_user_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    match (token_user_id, supplied_user_id) {
        (Some(token), Some(supplied)) if token != supplied => {
            Err(CodexConnectorError::AccountChanged)
        }
        (Some(token), _) => Ok(Some(token)),
        (None, supplied) => Ok(supplied),
    }
}

fn refresh_due(record: &CodexCredentialRecord) -> bool {
    match record.access_token_expires_at {
        Some(expires_at) => expires_at <= Utc::now() + ACCESS_TOKEN_REFRESH_WINDOW,
        None => record.last_refreshed_at <= Utc::now() - REFRESH_FALLBACK_AGE,
    }
}

fn quota_due(record: &CodexCredentialRecord) -> bool {
    quota_due_at(record.quota_checked_at, Utc::now())
}

fn quota_due_at(quota_checked_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    quota_checked_at.is_none_or(|checked_at| checked_at <= now - QUOTA_REFRESH_INTERVAL)
}

fn validate_quota_credential(record: &CodexCredentialRecord) -> Result<(), CodexConnectorError> {
    if !record.enabled || record.runtime_status == "disabled" {
        return Err(CodexConnectorError::CredentialDisabled);
    }
    if record.reauth_required {
        return Err(CodexConnectorError::CredentialReauthenticationRequired);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum CodexConnectorError {
    #[error("Codex persistence operation failed")]
    Repository(#[from] RepositoryError),
    #[error("Codex control-plane operation failed")]
    ControlPlane(#[from] ControlPlaneError),
    #[error("Codex upstream client is unavailable")]
    UpstreamClient(#[from] UpstreamClientError),
    #[error("invalid Codex endpoint configuration")]
    InvalidEndpoint,
    #[error("invalid Codex proxy configuration")]
    InvalidProxy,
    #[error("invalid OAuth callback URL")]
    InvalidCallback,
    #[error("OAuth authorization was denied")]
    OauthDenied,
    #[error("OAuth flow expired or was already completed")]
    OauthFlowExpired,
    #[error("OAuth callback state did not match")]
    OauthStateMismatch,
    #[error("Codex credential is invalid")]
    InvalidCredential,
    #[error("Codex workspace or member changed while refreshing the credential")]
    AccountChanged,
    #[error("invalid JWT")]
    InvalidJwt,
    #[error("invalid OAuth token response")]
    InvalidTokenResponse,
    #[error("OAuth token endpoint returned HTTP {0}")]
    TokenEndpointStatus(u16),
    #[error("refresh token is no longer valid")]
    RefreshTokenInvalid,
    #[error("Codex backend returned HTTP {0}")]
    CodexBackendStatus(u16),
    #[error("Codex models response is invalid")]
    InvalidModelsResponse,
    #[error("Codex account returned no supported models")]
    NoModels,
    #[error("Codex quota response is invalid")]
    InvalidQuotaResponse,
    #[error("Codex upstream request timed out")]
    UpstreamTimeout,
    #[error("Codex upstream request failed")]
    UpstreamUnavailable,
    #[error("Codex upstream response is too large")]
    UpstreamResponseTooLarge,
    #[error("Codex credential was not found")]
    CredentialNotFound,
    #[error("Codex credential is disabled")]
    CredentialDisabled,
    #[error("Codex credential requires reauthentication")]
    CredentialReauthenticationRequired,
}

impl CodexConnectorError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Repository(_) | Self::ControlPlane(_) => "codex_persistence_failed",
            Self::UpstreamClient(_) | Self::InvalidProxy => "codex_network_policy_invalid",
            Self::InvalidEndpoint => "codex_endpoint_invalid",
            Self::InvalidCallback => "codex_oauth_callback_invalid",
            Self::OauthDenied => "codex_oauth_denied",
            Self::OauthFlowExpired => "codex_oauth_flow_expired",
            Self::OauthStateMismatch => "codex_oauth_state_mismatch",
            Self::InvalidCredential | Self::InvalidJwt | Self::InvalidTokenResponse => {
                "codex_credential_invalid"
            }
            Self::AccountChanged => "codex_account_changed",
            Self::TokenEndpointStatus(_) => "codex_token_exchange_failed",
            Self::RefreshTokenInvalid => "codex_refresh_token_invalid",
            Self::CodexBackendStatus(_) => "codex_backend_http_error",
            Self::InvalidModelsResponse | Self::NoModels => "codex_models_invalid",
            Self::InvalidQuotaResponse => "codex_quota_invalid",
            Self::UpstreamTimeout => "codex_upstream_timeout",
            Self::UpstreamUnavailable => "codex_upstream_unavailable",
            Self::UpstreamResponseTooLarge => "codex_upstream_response_too_large",
            Self::CredentialNotFound => "codex_credential_not_found",
            Self::CredentialDisabled => "codex_credential_disabled",
            Self::CredentialReauthenticationRequired => {
                "codex_credential_reauthentication_required"
            }
        }
    }

    #[must_use]
    pub const fn safe_summary(&self) -> &'static str {
        match self {
            Self::RefreshTokenInvalid => "The Codex refresh token requires a new login.",
            Self::UpstreamTimeout => "The Codex upstream request timed out.",
            Self::UpstreamUnavailable => "The Codex upstream request failed.",
            Self::CodexBackendStatus(_) => "The Codex backend returned an error.",
            Self::InvalidQuotaResponse => "The Codex quota response was invalid.",
            Self::InvalidModelsResponse | Self::NoModels => {
                "The Codex model catalog response was invalid."
            }
            Self::CredentialReauthenticationRequired => {
                "The Codex credential requires a new authorization."
            }
            _ => "The Codex credential operation failed.",
        }
    }

    #[must_use]
    pub const fn permanent_refresh_failure(&self) -> bool {
        matches!(self, Self::RefreshTokenInvalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_is_optional_but_conflicting_overrides_are_rejected() {
        assert_eq!(resolve_account_id(None, None).unwrap(), None);
        assert_eq!(
            resolve_account_id(None, Some("manual-account".into())).unwrap(),
            Some("manual-account".into())
        );
        assert!(matches!(
            resolve_account_id(
                Some("token-account".into()),
                Some("different-account".into())
            ),
            Err(CodexConnectorError::AccountChanged)
        ));
    }

    #[test]
    fn quota_refresh_becomes_due_every_fifteen_minutes() {
        let now = DateTime::parse_from_rfc3339("2026-08-08T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(quota_due_at(None, now));
        assert!(!quota_due_at(
            Some(now - chrono::Duration::minutes(14)),
            now
        ));
        assert!(quota_due_at(Some(now - chrono::Duration::minutes(15)), now));
    }
}
