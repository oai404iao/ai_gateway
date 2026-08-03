//! JWT-authenticated Console API for self-service and role-gated control-plane work.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
};
use uuid::Uuid;

use crate::{
    application::{
        AuthError, ChannelModelDiscoveryError, ChannelModelDiscoveryInput,
        ChannelModelDiscoveryResponse, ChannelModelDiscoveryService, CodexConnectorError,
        CodexConnectorService, CodexOauthCompleteInput, CodexOauthStartResponse,
        CodexQuotaResetResponse, ConsoleAuthService, ControlPlaneCoordinator, ControlPlaneError,
        IssuedInvitation, IssuedRegistrationInvitationCode, IssuedSession, IssuedTemporaryPassword,
        ModelImportRequest, ModelSyncError, ModelSyncPreview, ModelSyncPreviewRequest,
        ModelSyncResponse, ModelSyncService, ProxyTestError, ProxyTestInput, ProxyTestResponse,
        ProxyTestService, RegistrationInvitationCodeCreateInput,
        RegistrationInvitationCodeUpdateInput, SelfRegistrationInput, SystemLoadReport,
        SystemMetricsService,
    },
    domain::{ConsolePrincipal, UserRole},
    persistence::{
        ApiKeyCreate, ApiKeyPolicyInput, ApiKeyUpdate, ChannelBatchUpdateInput, ChannelCreateInput,
        ChannelGroupInput, ChannelInput, ChannelRecoverInput, ChannelStatusWindow,
        CodexCredentialBatchInput, CodexCredentialExportBundle, CodexCredentialExportInput,
        CodexCredentialImportInput, CodexCredentialUpdateInput, CodexCredentialView,
        CodexOauthStartInput, CodexQuotaWindowHistory, ConfigTemplateCreateInput,
        ConfigTemplateInput, ConsoleApiKey, ControlPlaneMutation, CostStatisticsFilter,
        InviteUserInput, ModelInput, ModelRuleInput, ProxyCreateInput, ProxyInput,
        RequestLogFilter, RequestLogRepository, SelfApiKeyCreate, SelfApiKeyUpdate,
        SelfCodexQuotaCredentialView, SelfCodexQuotaWindowHistory, SpendLeaderboardFilter,
        SpendLeaderboardPeriod, StatisticsGranularity, SystemSettingsInput, UserBatchUpdateInput,
        UserGroupInput, UserInput, UserSettingsInput, UserUpdateInput,
    },
    runtime_config::ConfigError,
};

const REFRESH_COOKIE_NAME: &str = "__Host-ai_gateway_refresh";

#[derive(Clone)]
pub struct ConsoleState {
    pub coordinator: ControlPlaneCoordinator,
    pub codex_connector: CodexConnectorService,
    pub channel_models: ChannelModelDiscoveryService,
    pub proxy_tests: ProxyTestService,
    pub model_sync: ModelSyncService,
    pub auth: ConsoleAuthService,
    pub request_logs: RequestLogRepository,
    pub system_metrics: SystemMetricsService,
    pub console_body_bytes: usize,
    pub auth_body_bytes: usize,
    pub allowed_origins: Vec<String>,
}

/// Builds the dedicated Console listener. `admin` is only a user role; no
/// `/admin` namespace or static process-wide bearer credential exists here.
pub fn router(state: ConsoleState) -> Router {
    let auth_routes = Router::new()
        .route("/console/v1/auth/login", post(login))
        .route("/console/v1/auth/register", post(register))
        .route("/console/v1/auth/refresh", post(refresh))
        .route(
            "/console/v1/auth/activate-invitation",
            post(activate_invitation),
        )
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(state.auth_body_bytes));

    let self_routes = Router::new()
        .route("/console/v1/me", get(get_me).patch(update_me))
        .route(
            "/console/v1/me/settings",
            get(get_user_settings).put(update_user_settings),
        )
        .route("/console/v1/me/password", post(change_password))
        .route(
            "/console/v1/me/sessions",
            get(list_sessions).delete(revoke_other_sessions),
        )
        .route(
            "/console/v1/me/sessions/{id}",
            axum::routing::delete(revoke_session),
        )
        .route(
            "/console/v1/me/api-keys",
            get(list_own_api_keys).post(create_own_api_key),
        )
        .route(
            "/console/v1/me/api-key-options",
            get(get_own_api_key_options),
        )
        .route("/console/v1/me/api-hosts", get(get_api_hosts))
        .route(
            "/console/v1/me/api-keys/{id}",
            get(get_own_api_key).put(update_own_api_key),
        )
        .route(
            "/console/v1/me/api-keys/{id}/revoke",
            post(revoke_own_api_key),
        )
        .route("/console/v1/me/request-logs", get(list_own_request_logs))
        .route("/console/v1/me/request-logs/{id}", get(get_own_request_log))
        .route("/console/v1/me/usage", get(get_own_usage))
        .route("/console/v1/me/codex-quotas", get(list_own_codex_quotas))
        .route(
            "/console/v1/me/codex-quotas/{id}/windows",
            get(get_own_codex_quota_window_history),
        );

    let statistics_routes = Router::new()
        .route(
            "/console/v1/statistics/channel-status",
            get(get_channel_status),
        )
        .route("/console/v1/statistics/costs", get(get_own_cost_statistics))
        .route(
            "/console/v1/statistics/spend-leaderboard",
            get(get_spend_leaderboard),
        );

    let control_routes = Router::new()
        .route("/console/v1/users", get(list_users).post(invite_user))
        .route("/console/v1/users/batch", post(update_users_batch))
        .route(
            "/console/v1/users/{id}/invitation",
            post(reissue_user_invitation),
        )
        .route(
            "/console/v1/users/{id}/temporary-password",
            post(issue_user_temporary_password),
        )
        .route(
            "/console/v1/users/{id}",
            get(get_user)
                .put(replace_user)
                .patch(update_user)
                .delete(delete_user),
        )
        .route(
            "/console/v1/user-groups",
            get(list_user_groups).post(create_user_group),
        )
        .route(
            "/console/v1/user-groups/{id}",
            get(get_user_group)
                .put(update_user_group)
                .delete(delete_user_group),
        )
        .route(
            "/console/v1/registration-invitation-codes",
            get(list_registration_invitation_codes).post(create_registration_invitation_code),
        )
        .route(
            "/console/v1/registration-invitation-codes/{id}",
            get(get_registration_invitation_code).put(update_registration_invitation_code),
        )
        .route(
            "/console/v1/api-key-policies",
            get(list_api_key_policies).post(create_api_key_policy),
        )
        .route(
            "/console/v1/api-key-policies/{id}",
            get(get_api_key_policy).put(update_api_key_policy),
        )
        .route("/console/v1/models", get(list_models).post(create_model))
        .route(
            "/console/v1/catalog/models/sync/preview",
            post(preview_models_sync),
        )
        .route("/console/v1/catalog/models/import", post(import_models))
        .route("/console/v1/models/{id}", get(get_model).put(update_model))
        .route(
            "/console/v1/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route(
            "/console/v1/api-keys/{id}",
            get(get_api_key).put(update_api_key),
        )
        .route("/console/v1/api-keys/{id}/revoke", post(revoke_api_key))
        .route(
            "/console/v1/routing/channel-groups",
            get(list_groups).post(create_group),
        )
        .route(
            "/console/v1/routing/channel-groups/{id}",
            get(get_group).put(update_group),
        )
        .route(
            "/console/v1/routing/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/console/v1/routing/channels/models/discover",
            post(discover_channel_models),
        )
        .route(
            "/console/v1/routing/channels/batch",
            post(update_channels_batch),
        )
        .route(
            "/console/v1/routing/channels/{id}",
            get(get_channel).put(update_channel),
        )
        .route(
            "/console/v1/routing/channels/{id}/recover",
            post(recover_channel),
        )
        .route(
            "/console/v1/routing/model-rules",
            get(list_rules).post(create_rule),
        )
        .route(
            "/console/v1/routing/model-rules/{id}",
            get(get_rule).put(update_rule),
        )
        .route(
            "/console/v1/providers/codex-oauth/channel-groups/{id}/credentials",
            get(list_codex_credentials).post(import_codex_credential),
        )
        .route(
            "/console/v1/providers/codex-oauth/channel-groups/{id}/oauth/flows",
            post(start_codex_oauth),
        )
        .route(
            "/console/v1/providers/codex-oauth/channel-groups/{id}/credentials/export",
            post(export_codex_credentials),
        )
        .route(
            "/console/v1/providers/codex-oauth/channel-groups/{id}/credentials/batch",
            post(update_codex_credentials_batch),
        )
        .route(
            "/console/v1/providers/codex-oauth/oauth/flows/{id}/complete",
            post(complete_codex_oauth),
        )
        .route(
            "/console/v1/providers/codex-oauth/credentials/{id}",
            get(get_codex_credential)
                .put(update_codex_credential)
                .delete(delete_codex_credential),
        )
        .route(
            "/console/v1/providers/codex-oauth/credentials/{id}/refresh",
            post(refresh_codex_credential),
        )
        .route(
            "/console/v1/providers/codex-oauth/credentials/{id}/quota/refresh",
            post(refresh_codex_quota),
        )
        .route(
            "/console/v1/providers/codex-oauth/credentials/{id}/quota/windows",
            get(get_codex_quota_window_history),
        )
        .route(
            "/console/v1/providers/codex-oauth/credentials/{id}/quota/reset",
            post(reset_codex_quota),
        )
        .route(
            "/console/v1/network/proxies",
            get(list_proxies).post(create_proxy),
        )
        .route("/console/v1/network/proxies/test", post(test_proxy))
        .route(
            "/console/v1/network/proxies/{id}",
            get(get_proxy).put(update_proxy).delete(delete_proxy),
        )
        .route(
            "/console/v1/transforms/templates",
            get(list_config_templates).post(create_config_template),
        )
        .route(
            "/console/v1/transforms/templates/{id}",
            get(get_config_template).put(update_config_template),
        )
        .route("/console/v1/request-logs", get(list_all_request_logs))
        .route("/console/v1/request-logs/{id}", get(get_request_log))
        .route("/console/v1/audit-logs", get(list_audit_logs))
        .route(
            "/console/v1/system/settings",
            get(get_system_settings).put(update_system_settings),
        )
        .route(
            "/console/v1/system/session-affinity/cache",
            get(get_session_affinity_cache).delete(clear_session_affinity_cache),
        )
        .route(
            "/console/v1/system/statistics/costs",
            get(get_system_cost_statistics),
        )
        .route("/console/v1/system/load", get(get_system_load))
        .route("/console/v1/system/reload", post(reload))
        .route_layer(middleware::from_fn(require_admin));

    let password_change_routes = Router::new()
        .route("/console/v1/auth/logout", post(logout))
        .route(
            "/console/v1/auth/complete-password-reset",
            post(complete_password_reset),
        )
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(state.auth_body_bytes));

    let full_session_routes = self_routes
        .merge(statistics_routes)
        .merge(control_routes)
        .route_layer(middleware::from_fn(require_full_session));

    let authenticated = password_change_routes
        .merge(full_session_routes)
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(state.console_body_bytes));

    Router::new()
        .merge(auth_routes)
        .merge(authenticated)
        .layer(middleware::from_fn(no_store))
        .layer(cors_layer(&state.allowed_origins))
        .with_state(state)
}

async fn authenticate(
    State(state): State<ConsoleState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_token(request.headers()) else {
        return unauthorized();
    };
    match state.auth.authenticate_access_token(token).await {
        Ok(principal) => {
            request.extensions_mut().insert(principal);
            next.run(request).await
        }
        Err(_) => unauthorized(),
    }
}

async fn require_admin(request: Request, next: Next) -> Response {
    if request
        .extensions()
        .get::<ConsolePrincipal>()
        .is_some_and(|principal| principal.role().is_admin())
    {
        next.run(request).await
    } else {
        forbidden()
    }
}

async fn require_full_session(request: Request, next: Next) -> Response {
    if request
        .extensions()
        .get::<ConsolePrincipal>()
        .is_some_and(|principal| !principal.session_purpose().requires_password_change())
    {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            Json(ErrorBody {
                error: "password_change_required",
            }),
        )
            .into_response()
    }
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn cors_layer(origins: &[String]) -> CorsLayer {
    let base = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::IF_MATCH,
        ])
        .expose_headers([header::ETAG]);
    if origins.is_empty() {
        base
    } else {
        let origins = origins
            .iter()
            .map(|origin| HeaderValue::from_str(origin).expect("validated Console origin"))
            .collect::<Vec<_>>();
        base.allow_origin(AllowOrigin::list(origins))
            .allow_credentials(true)
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
}

fn refresh_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|entry| entry.strip_prefix(&format!("{REFRESH_COOKIE_NAME}=")))
        .filter(|token| !token.is_empty())
}

fn apply_refresh_cookie(
    response: &mut Response,
    refresh_token: &str,
    refresh_expires_at: DateTime<Utc>,
) {
    let max_age = refresh_expires_at
        .signed_duration_since(Utc::now())
        .num_seconds()
        .max(0);
    let cookie = format!(
        "{REFRESH_COOKIE_NAME}={refresh_token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age}"
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("opaque refresh cookie is valid"),
    );
}

fn clear_refresh_cookie(response: &mut Response) {
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-ai_gateway_refresh=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
        ),
    );
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorBody {
            error: "unauthorized",
        }),
    )
        .into_response()
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorBody { error: "forbidden" }),
    )
        .into_response()
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[derive(Serialize)]
struct MutationResponse {
    id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<String>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct ReloadResponse {
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct ChannelBatchUpdateResponse {
    updated_ids: Vec<Uuid>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct UserBatchUpdateResponse {
    updated_ids: Vec<Uuid>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct CodexCredentialBatchResponse {
    updated_ids: Vec<Uuid>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct LoginResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    user: crate::application::ConsoleUser,
}

#[derive(Serialize)]
struct InvitationResponse {
    id: Uuid,
    user_id: Uuid,
    invitation_token: String,
    expires_at: DateTime<Utc>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct TemporaryPasswordResponse {
    user_id: Uuid,
    temporary_password: String,
    expires_at: DateTime<Utc>,
    correlation_id: Uuid,
}

#[derive(Serialize)]
struct RegistrationInvitationCodeCreateResponse {
    id: Uuid,
    invitation_code: String,
    correlation_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginInput {
    email: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterInput {
    invitation_code: String,
    email: String,
    display_name: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivateInvitationInput {
    invitation_token: String,
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordChangeInput {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletePasswordResetInput {
    new_password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemporaryPasswordInput {
    current_password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileUpdateInput {
    display_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeInput {
    reason: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InviteUserRequest {
    email: String,
    display_name: String,
    role: UserRole,
    #[serde(default)]
    initial_balance_amount: rust_decimal::Decimal,
    #[serde(default)]
    user_group_id: Option<Uuid>,
    #[serde(default)]
    default_api_key_policy_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationInvitationCodeCreateRequest {
    name: String,
    invitation_code: String,
    #[serde(default)]
    max_uses: Option<i64>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
    #[serde(default = "default_true")]
    enabled: bool,
    user_group_id: Uuid,
    #[serde(default)]
    initial_balance_amount: rust_decimal::Decimal,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationInvitationCodeUpdateRequest {
    name: String,
    #[serde(default)]
    max_uses: Option<i64>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
    enabled: bool,
    user_group_id: Uuid,
    #[serde(default)]
    initial_balance_amount: rust_decimal::Decimal,
}

const fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    api_key_id: Option<Uuid>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_format: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    started_after: Option<DateTime<Utc>>,
    #[serde(default)]
    started_before: Option<DateTime<Utc>>,
    #[serde(default)]
    billed: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelStatusQuery {
    #[serde(default)]
    window: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CostStatisticsQuery {
    #[serde(default)]
    started_after: Option<DateTime<Utc>>,
    #[serde(default)]
    started_before: Option<DateTime<Utc>>,
    #[serde(default)]
    granularity: Option<String>,
    #[serde(default)]
    user_id: Option<Uuid>,
    #[serde(default)]
    api_key_id: Option<Uuid>,
    #[serde(default)]
    channel_id: Option<Uuid>,
    #[serde(default)]
    codex_credential_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexQuotaWindowHistoryQuery {
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpendLeaderboardQuery {
    #[serde(default)]
    period: Option<String>,
    #[serde(default)]
    period_start: Option<NaiveDate>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionAffinityCacheQuery {
    #[serde(default)]
    rule_name: Option<String>,
}

impl LogQuery {
    fn into_filter(self) -> RequestLogFilter {
        RequestLogFilter {
            limit: self.limit.unwrap_or(50),
            user_id: self.user_id,
            api_key_id: self.api_key_id,
            model: self.model,
            api_format: self.api_format,
            outcome: self.outcome,
            started_after: self.started_after,
            started_before: self.started_before,
            billed: self.billed,
        }
    }
}

async fn login(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> Result<Response, ConsoleError> {
    let session = state
        .auth
        .login_with_user_agent(input.email, input.password, session_user_agent(&headers))
        .await?;
    Ok(session_response(session))
}

async fn register(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Json(input): Json<RegisterInput>,
) -> Result<Response, ConsoleError> {
    let session = state
        .auth
        .register_with_user_agent(
            SelfRegistrationInput {
                invitation_code: input.invitation_code,
                email: input.email,
                display_name: input.display_name,
                password: input.password,
            },
            session_user_agent(&headers),
        )
        .await?;
    Ok(session_response(session))
}

async fn refresh(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Response, ConsoleError> {
    let refresh_token =
        refresh_cookie(&headers).ok_or(ConsoleError::Auth(AuthError::InvalidToken))?;
    let session = state
        .auth
        .refresh_with_user_agent(refresh_token, session_user_agent(&headers))
        .await?;
    Ok(session_response(session))
}

async fn activate_invitation(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
    Json(input): Json<ActivateInvitationInput>,
) -> Result<Response, ConsoleError> {
    let session = state
        .auth
        .accept_invitation_with_user_agent(
            &input.invitation_token,
            input.password,
            session_user_agent(&headers),
        )
        .await?;
    Ok(session_response(session))
}

async fn logout(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Response, ConsoleError> {
    state.auth.logout(principal).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    clear_refresh_cookie(&mut response);
    Ok(response)
}

async fn complete_password_reset(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    headers: HeaderMap,
    Json(input): Json<CompletePasswordResetInput>,
) -> Result<Response, ConsoleError> {
    let session = state
        .auth
        .complete_temporary_password(principal, input.new_password, session_user_agent(&headers))
        .await?;
    Ok(session_response(session))
}

fn session_response(session: IssuedSession) -> Response {
    let IssuedSession {
        access_token,
        expires_in_seconds,
        refresh_token,
        refresh_expires_at,
        user,
    } = session;
    let mut response = Json(LoginResponse {
        access_token,
        token_type: "Bearer",
        expires_in: expires_in_seconds,
        user,
    })
    .into_response();
    apply_refresh_cookie(&mut response, &refresh_token, refresh_expires_at);
    response
}

async fn get_me(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<crate::persistence::ConsoleProfile>, ConsoleError> {
    state
        .auth
        .repository()
        .profile(principal.user_id())
        .await?
        .map(Json)
        .ok_or(ConsoleError::NotFound)
}

async fn update_me(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ProfileUpdateInput>,
) -> Result<Json<crate::persistence::ConsoleProfile>, ConsoleError> {
    if input.display_name.trim().is_empty() || input.display_name.len() > 200 {
        return Err(ConsoleError::Validation);
    }
    state
        .auth
        .repository()
        .update_display_name(principal.user_id(), &input.display_name)
        .await?
        .map(Json)
        .ok_or(ConsoleError::NotFound)
}

async fn get_user_settings(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<crate::persistence::UserSettingsView>, ConsoleError> {
    state
        .coordinator
        .user_settings(principal.user_id())
        .await?
        .map(Json)
        .ok_or(ConsoleError::NotFound)
}

async fn update_user_settings(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<UserSettingsInput>,
) -> Result<Json<crate::persistence::UserSettingsView>, ConsoleError> {
    Ok(Json(
        state
            .coordinator
            .update_user_settings(principal.user_id(), input)
            .await?,
    ))
}

async fn change_password(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<PasswordChangeInput>,
) -> Result<StatusCode, ConsoleError> {
    state
        .auth
        .change_password(principal, input.current_password, input.new_password)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_sessions(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<Vec<crate::persistence::ConsoleSession>>, ConsoleError> {
    Ok(Json(
        state
            .auth
            .repository()
            .sessions_for_user(principal.user_id(), principal.session_id())
            .await?,
    ))
}

async fn revoke_other_sessions(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<StatusCode, ConsoleError> {
    state
        .auth
        .repository()
        .revoke_other_sessions(principal.user_id(), principal.session_id())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_session(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    if state
        .auth
        .repository()
        .revoke_session_for_user(principal.user_id(), id)
        .await?
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if id == principal.session_id() {
            clear_refresh_cookie(&mut response);
        }
        Ok(response)
    } else {
        Err(ConsoleError::NotFound)
    }
}

fn session_user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)?
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

async fn list_own_api_keys(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<Vec<ConsoleApiKey>>, ConsoleError> {
    Ok(Json(
        state.coordinator.own_api_keys(principal.user_id()).await?,
    ))
}

async fn get_own_api_key_options(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<crate::persistence::SelfApiKeyOptions>, ConsoleError> {
    Ok(Json(
        state
            .coordinator
            .own_api_key_options(principal.user_id())
            .await?,
    ))
}

async fn get_api_hosts(
    State(state): State<ConsoleState>,
) -> Result<Json<crate::persistence::ApiHostsView>, ConsoleError> {
    Ok(Json(state.coordinator.api_hosts().await?))
}

async fn create_own_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<SelfApiKeyCreate>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    let result = state
        .coordinator
        .create_own_api_key(principal.user_id(), input)
        .await?;
    Ok((StatusCode::CREATED, Json(mutation_response(result))))
}

async fn get_own_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    let api_key = state
        .coordinator
        .own_api_key(principal.user_id(), id)
        .await?
        .ok_or(ConsoleError::NotFound)?;
    api_key_response(api_key)
}

async fn update_own_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<SelfApiKeyUpdate>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    let result = state
        .coordinator
        .update_own_api_key(principal.user_id(), id, input, if_match(&headers)?)
        .await?;
    Ok(Json(mutation_response(result)))
}

async fn revoke_own_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    Json(input): Json<RevokeInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    let result = state
        .coordinator
        .revoke_own_api_key(principal.user_id(), id, input.reason)
        .await?;
    Ok(Json(mutation_response(result)))
}

fn api_key_response(key: ConsoleApiKey) -> Result<Response, ConsoleError> {
    let updated_at = key.updated_at;
    let mut response = Json(key).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(updated_at)).expect("ETag is valid"),
    );
    Ok(response)
}

async fn list_own_request_logs(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<crate::persistence::ConsoleRequestLog>>, ConsoleError> {
    Ok(Json(
        state
            .request_logs
            .list_for_user(principal.user_id(), query.into_filter())
            .await?,
    ))
}

async fn get_own_request_log(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::persistence::ConsoleRequestLog>, ConsoleError> {
    state
        .request_logs
        .get_for_user(principal.user_id(), id)
        .await?
        .map(Json)
        .ok_or(ConsoleError::NotFound)
}

async fn get_own_usage(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<crate::persistence::PersonalUsageReport>, ConsoleError> {
    Ok(Json(
        state
            .request_logs
            .personal_usage(principal.user_id(), Utc::now().date_naive())
            .await?,
    ))
}

async fn list_own_codex_quotas(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<Vec<SelfCodexQuotaCredentialView>>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .self_quota_credentials(principal.user_id())
            .await?,
    ))
}

async fn get_own_codex_quota_window_history(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    Query(query): Query<CodexQuotaWindowHistoryQuery>,
) -> Result<Json<SelfCodexQuotaWindowHistory>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .self_quota_window_history(principal.user_id(), id, query.limit.unwrap_or(100))
            .await?,
    ))
}

async fn list_users(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.users)
            .expect("Console user DTO serializes"),
    ))
}

async fn invite_user(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<InviteUserRequest>,
) -> Result<(StatusCode, Json<InvitationResponse>), ConsoleError> {
    let invitation = state
        .auth
        .invite_user(
            principal,
            InviteUserInput {
                email: input.email,
                display_name: input.display_name,
                role: input.role,
                initial_balance_amount: input.initial_balance_amount,
                user_group_id: input.user_group_id,
                default_api_key_policy_id: input.default_api_key_policy_id,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(invitation_response(invitation))))
}

fn invitation_response(invitation: IssuedInvitation) -> InvitationResponse {
    InvitationResponse {
        id: invitation.created.invitation_id,
        user_id: invitation.created.user_id,
        invitation_token: invitation.token,
        expires_at: invitation.created.expires_at,
        correlation_id: invitation.created.correlation_id,
    }
}

async fn get_user(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::User).await
}

async fn reissue_user_invitation(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<InvitationResponse>), ConsoleError> {
    let invitation = state.auth.reissue_invitation(principal, id).await?;
    Ok((StatusCode::CREATED, Json(invitation_response(invitation))))
}

async fn issue_user_temporary_password(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    Json(input): Json<TemporaryPasswordInput>,
) -> Result<(StatusCode, Json<TemporaryPasswordResponse>), ConsoleError> {
    let issued = state
        .auth
        .issue_temporary_password(principal, id, input.current_password)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(temporary_password_response(issued)),
    ))
}

fn temporary_password_response(issued: IssuedTemporaryPassword) -> TemporaryPasswordResponse {
    TemporaryPasswordResponse {
        user_id: issued.user_id,
        temporary_password: issued.temporary_password,
        expires_at: issued.expires_at,
        correlation_id: issued.correlation_id,
    }
}

async fn replace_user(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UserInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    update_user_fields(state, principal, id, headers, input.into()).await
}

async fn update_user(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UserUpdateInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    update_user_fields(state, principal, id, headers, input).await
}

async fn update_user_fields(
    state: ConsoleState,
    principal: ConsolePrincipal,
    id: Uuid,
    headers: HeaderMap,
    input: UserUpdateInput,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateUser {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn delete_user(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::DeleteUser {
            id,
            deleted_by: principal.user_id(),
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn update_users_batch(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<UserBatchUpdateInput>,
) -> Result<Json<UserBatchUpdateResponse>, ConsoleError> {
    let result = state
        .coordinator
        .update_users_batch(principal.user_id(), input)
        .await?;
    Ok(Json(UserBatchUpdateResponse {
        updated_ids: result.updated_ids,
        correlation_id: result.correlation_id,
    }))
}

async fn list_user_groups(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.user_groups)
            .expect("Console user-group DTO serializes"),
    ))
}

async fn create_user_group(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<UserGroupInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(
        &state,
        principal,
        ControlPlaneMutation::CreateUserGroup(input),
    )
    .await
}

async fn get_user_group(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::UserGroup).await
}

async fn update_user_group(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UserGroupInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateUserGroup {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn delete_user_group(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::DeleteUserGroup {
            id,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_registration_invitation_codes(
    State(state): State<ConsoleState>,
) -> Result<Json<Vec<crate::persistence::RegistrationInvitationCode>>, ConsoleError> {
    Ok(Json(
        state
            .auth
            .repository()
            .registration_invitation_codes()
            .await?,
    ))
}

async fn create_registration_invitation_code(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<RegistrationInvitationCodeCreateRequest>,
) -> Result<(StatusCode, Json<RegistrationInvitationCodeCreateResponse>), ConsoleError> {
    let issued = state
        .auth
        .create_registration_invitation_code(
            principal,
            RegistrationInvitationCodeCreateInput {
                name: input.name,
                invitation_code: input.invitation_code,
                max_uses: input.max_uses,
                expires_at: input.expires_at,
                enabled: input.enabled,
                user_group_id: input.user_group_id,
                initial_balance_amount: input.initial_balance_amount,
            },
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(registration_invitation_code_create_response(issued)),
    ))
}

fn registration_invitation_code_create_response(
    issued: IssuedRegistrationInvitationCode,
) -> RegistrationInvitationCodeCreateResponse {
    RegistrationInvitationCodeCreateResponse {
        id: issued.id,
        invitation_code: issued.invitation_code,
        correlation_id: issued.correlation_id,
    }
}

async fn get_registration_invitation_code(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    let code = state
        .auth
        .repository()
        .registration_invitation_code(id)
        .await?
        .ok_or(ConsoleError::NotFound)?;
    let updated_at = code.updated_at;
    let mut response = Json(code).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(updated_at)).expect("ETag is valid"),
    );
    Ok(response)
}

async fn update_registration_invitation_code(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<RegistrationInvitationCodeUpdateRequest>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    let mutation = state
        .auth
        .update_registration_invitation_code(
            principal,
            id,
            RegistrationInvitationCodeUpdateInput {
                name: input.name,
                max_uses: input.max_uses,
                expires_at: input.expires_at,
                enabled: input.enabled,
                user_group_id: input.user_group_id,
                initial_balance_amount: input.initial_balance_amount,
            },
            if_match(&headers)?,
        )
        .await?;
    Ok(Json(MutationResponse {
        id: mutation.id,
        secret: None,
        correlation_id: mutation.correlation_id,
    }))
}

async fn list_api_key_policies(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.api_key_policies)
            .expect("Console policy DTO serializes"),
    ))
}

async fn create_api_key_policy(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ApiKeyPolicyInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(
        &state,
        principal,
        ControlPlaneMutation::CreateApiKeyPolicy(input),
    )
    .await
}

async fn get_api_key_policy(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::ApiKeyPolicy).await
}

async fn update_api_key_policy(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ApiKeyPolicyInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateApiKeyPolicy {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_models(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.models)
            .expect("Console model DTO serializes"),
    ))
}

async fn create_model(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ModelInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(&state, principal, ControlPlaneMutation::CreateModel(input)).await
}

async fn preview_models_sync(
    State(state): State<ConsoleState>,
    Json(input): Json<ModelSyncPreviewRequest>,
) -> Result<Json<ModelSyncPreview>, ConsoleError> {
    Ok(Json(state.model_sync.preview(input).await?))
}

async fn import_models(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ModelImportRequest>,
) -> Result<Json<ModelSyncResponse>, ConsoleError> {
    let result = state.model_sync.apply(principal.user_id(), input).await?;
    Ok(Json(ModelSyncResponse {
        model_count: result.model_count,
        imported_count: result.imported_count,
        updated_count: result.updated_count,
        correlation_id: result.correlation_id,
    }))
}

async fn get_model(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::Model).await
}

async fn update_model(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ModelInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateModel {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_api_keys(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.api_keys)
            .expect("Console API-key DTO serializes"),
    ))
}

async fn create_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ApiKeyCreate>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(&state, principal, ControlPlaneMutation::CreateApiKey(input)).await
}

async fn get_api_key(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::ApiKey).await
}

async fn update_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ApiKeyUpdate>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateApiKey {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn revoke_api_key(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    Json(input): Json<RevokeInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::RevokeApiKey {
            id,
            reason: input.reason,
        },
    )
    .await
}

async fn list_groups(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.channel_groups)
            .expect("Console channel-group DTO serializes"),
    ))
}

async fn create_group(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ChannelGroupInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(&state, principal, ControlPlaneMutation::CreateGroup(input)).await
}

async fn get_group(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::Group).await
}

async fn update_group(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ChannelGroupInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateGroup {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_channels(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.channels)
            .expect("Console channel DTO serializes"),
    ))
}

async fn create_channel(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ChannelCreateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(
        &state,
        principal,
        ControlPlaneMutation::CreateChannel(input),
    )
    .await
}

async fn discover_channel_models(
    State(state): State<ConsoleState>,
    Json(input): Json<ChannelModelDiscoveryInput>,
) -> Result<Json<ChannelModelDiscoveryResponse>, ConsoleError> {
    Ok(Json(state.channel_models.discover(input).await?))
}

async fn update_channels_batch(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ChannelBatchUpdateInput>,
) -> Result<Json<ChannelBatchUpdateResponse>, ConsoleError> {
    let result = state
        .coordinator
        .update_channels_batch(principal.user_id(), input)
        .await?;
    Ok(Json(ChannelBatchUpdateResponse {
        updated_ids: result.updated_ids,
        correlation_id: result.correlation_id,
    }))
}

async fn get_channel(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    let value = state
        .coordinator
        .channel_detail(id)
        .await?
        .map(to_json)
        .ok_or(ConsoleError::NotFound)?;
    resource_response(value)
}

async fn update_channel(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ChannelInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateChannel {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_codex_credentials(
    State(state): State<ConsoleState>,
    Path(channel_group_id): Path<Uuid>,
) -> Result<Json<Vec<CodexCredentialView>>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .list_credentials(channel_group_id)
            .await?,
    ))
}

async fn start_codex_oauth(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_group_id): Path<Uuid>,
    Json(input): Json<CodexOauthStartInput>,
) -> Result<(StatusCode, Json<CodexOauthStartResponse>), ConsoleError> {
    let response = state
        .codex_connector
        .start_oauth(principal.user_id(), channel_group_id, input)
        .await?;
    Ok((StatusCode::CREATED, Json(response)))
}

async fn complete_codex_oauth(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(flow_id): Path<Uuid>,
    Json(input): Json<CodexOauthCompleteInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    let result = state
        .codex_connector
        .complete_oauth(principal.user_id(), flow_id, input)
        .await?;
    let status = if result.action == "create" {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(mutation_response(result))))
}

async fn import_codex_credential(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_group_id): Path<Uuid>,
    Json(input): Json<CodexCredentialImportInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    let result = state
        .codex_connector
        .import_credential(principal.user_id(), channel_group_id, input)
        .await?;
    let status = if result.action == "create" {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(mutation_response(result))))
}

async fn export_codex_credentials(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_group_id): Path<Uuid>,
    Json(input): Json<CodexCredentialExportInput>,
) -> Result<Json<CodexCredentialExportBundle>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .export_credentials(principal.user_id(), channel_group_id, input)
            .await?,
    ))
}

async fn update_codex_credentials_batch(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_group_id): Path<Uuid>,
    Json(input): Json<CodexCredentialBatchInput>,
) -> Result<Json<CodexCredentialBatchResponse>, ConsoleError> {
    let result = state
        .codex_connector
        .update_credentials_batch(principal.user_id(), channel_group_id, input)
        .await?;
    Ok(Json(CodexCredentialBatchResponse {
        updated_ids: result.updated_ids,
        correlation_id: result.correlation_id,
    }))
}

async fn update_codex_credential(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<CodexCredentialUpdateInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    let result = state
        .codex_connector
        .update_credential(principal.user_id(), channel_id, input, if_match(&headers)?)
        .await?;
    Ok(Json(mutation_response(result)))
}

async fn delete_codex_credential(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<MutationResponse>, ConsoleError> {
    let result = state
        .codex_connector
        .delete_credential(principal.user_id(), channel_id, if_match(&headers)?)
        .await?;
    Ok(Json(mutation_response(result)))
}

async fn get_codex_credential(
    State(state): State<ConsoleState>,
    Path(channel_id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    let value = state
        .codex_connector
        .credential(channel_id)
        .await?
        .map(to_json)
        .ok_or(ConsoleError::NotFound)?;
    resource_response(value)
}

async fn refresh_codex_credential(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_id): Path<Uuid>,
) -> Result<StatusCode, ConsoleError> {
    state
        .codex_connector
        .refresh_credential(principal.user_id(), channel_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_codex_quota(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_id): Path<Uuid>,
) -> Result<StatusCode, ConsoleError> {
    state
        .codex_connector
        .refresh_quota(principal.user_id(), channel_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_codex_quota_window_history(
    State(state): State<ConsoleState>,
    Path(channel_id): Path<Uuid>,
    Query(query): Query<CodexQuotaWindowHistoryQuery>,
) -> Result<Json<CodexQuotaWindowHistory>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .quota_window_history(channel_id, query.limit.unwrap_or(100))
            .await?,
    ))
}

async fn reset_codex_quota(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(channel_id): Path<Uuid>,
) -> Result<Json<CodexQuotaResetResponse>, ConsoleError> {
    Ok(Json(
        state
            .codex_connector
            .reset_quota(principal.user_id(), channel_id)
            .await?,
    ))
}

async fn recover_channel(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    Json(input): Json<ChannelRecoverInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::RecoverChannel {
            id,
            expected_updated_at: input.updated_at,
        },
    )
    .await
}

async fn list_rules(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.model_rules)
            .expect("Console model-rule DTO serializes"),
    ))
}

async fn create_rule(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ModelRuleInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(&state, principal, ControlPlaneMutation::CreateRule(input)).await
}

async fn get_rule(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::Rule).await
}

async fn update_rule(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ModelRuleInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateRule {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_proxies(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.proxies)
            .expect("Console proxy DTO serializes"),
    ))
}

async fn create_proxy(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ProxyCreateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(&state, principal, ControlPlaneMutation::CreateProxy(input)).await
}

async fn test_proxy(
    State(state): State<ConsoleState>,
    Json(input): Json<ProxyTestInput>,
) -> Result<Json<ProxyTestResponse>, ConsoleError> {
    Ok(Json(state.proxy_tests.test(input).await?))
}

async fn get_proxy(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    get_resource(state, id, Resource::Proxy).await
}

async fn update_proxy(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ProxyInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateProxy {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn delete_proxy(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::DeleteProxy {
            id,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_config_templates(
    State(state): State<ConsoleState>,
) -> Result<Json<serde_json::Value>, ConsoleError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.config_templates)
            .expect("Console template DTO serializes"),
    ))
}

async fn create_config_template(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Json(input): Json<ConfigTemplateCreateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    mutate_created(
        &state,
        principal,
        ControlPlaneMutation::CreateConfigTemplate(input),
    )
    .await
}

async fn get_config_template(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ConsoleError> {
    let value = state
        .coordinator
        .config_template_detail(id)
        .await?
        .map(to_json)
        .ok_or(ConsoleError::NotFound)?;
    resource_response(value)
}

async fn update_config_template(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ConfigTemplateInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateConfigTemplate {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn list_all_request_logs(
    State(state): State<ConsoleState>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<crate::persistence::ConsoleRequestLog>>, ConsoleError> {
    Ok(Json(
        state.request_logs.list_all(query.into_filter()).await?,
    ))
}

async fn get_request_log(
    State(state): State<ConsoleState>,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::persistence::ConsoleRequestLog>, ConsoleError> {
    state
        .request_logs
        .get(id)
        .await?
        .map(Json)
        .ok_or(ConsoleError::NotFound)
}

async fn get_channel_status(
    State(state): State<ConsoleState>,
    Query(query): Query<ChannelStatusQuery>,
) -> Result<Json<crate::persistence::ChannelStatusReport>, ConsoleError> {
    let window = match query.window.as_deref().unwrap_or("24h") {
        "24h" => ChannelStatusWindow::Last24Hours,
        "3d" => ChannelStatusWindow::Last3Days,
        "7d" => ChannelStatusWindow::Last7Days,
        _ => return Err(ConsoleError::Validation),
    };
    Ok(Json(state.request_logs.channel_status(window).await?))
}

async fn get_own_cost_statistics(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Query(query): Query<CostStatisticsQuery>,
) -> Result<Json<crate::persistence::CostStatisticsReport>, ConsoleError> {
    if query.user_id.is_some() || query.channel_id.is_some() || query.codex_credential_id.is_some()
    {
        return Err(ConsoleError::Validation);
    }
    cost_statistics(&state, query, Some(principal.user_id()), None, None, false).await
}

async fn get_system_cost_statistics(
    State(state): State<ConsoleState>,
    Query(query): Query<CostStatisticsQuery>,
) -> Result<Json<crate::persistence::CostStatisticsReport>, ConsoleError> {
    let user_id = query.user_id;
    let channel_id = query.channel_id;
    let codex_credential_id = query.codex_credential_id;
    cost_statistics(
        &state,
        query,
        user_id,
        channel_id,
        codex_credential_id,
        true,
    )
    .await
}

async fn cost_statistics(
    state: &ConsoleState,
    query: CostStatisticsQuery,
    user_id: Option<Uuid>,
    channel_id: Option<Uuid>,
    codex_credential_id: Option<Uuid>,
    include_channel_details: bool,
) -> Result<Json<crate::persistence::CostStatisticsReport>, ConsoleError> {
    if channel_id.is_some() && codex_credential_id.is_some() {
        return Err(ConsoleError::Validation);
    }
    let ended_at = query.started_before.unwrap_or_else(Utc::now);
    let started_at = query
        .started_after
        .unwrap_or_else(|| ended_at - chrono::Duration::days(7));
    let granularity = match query.granularity.as_deref().unwrap_or("day") {
        "hour" => StatisticsGranularity::Hour,
        "day" => StatisticsGranularity::Day,
        _ => return Err(ConsoleError::Validation),
    };
    Ok(Json(
        state
            .request_logs
            .cost_statistics(CostStatisticsFilter {
                started_at,
                ended_at,
                granularity,
                user_id,
                api_key_id: query.api_key_id,
                channel_id,
                codex_credential_id,
                include_channel_details,
            })
            .await?,
    ))
}

async fn get_spend_leaderboard(
    State(state): State<ConsoleState>,
    Query(query): Query<SpendLeaderboardQuery>,
) -> Result<Json<crate::persistence::SpendLeaderboardReport>, ConsoleError> {
    let period = match query.period.as_deref().unwrap_or("day") {
        "day" => SpendLeaderboardPeriod::Day,
        "week" => SpendLeaderboardPeriod::Week,
        "month" => SpendLeaderboardPeriod::Month,
        _ => return Err(ConsoleError::Validation),
    };
    let limit = query.limit.unwrap_or(50);
    let period_start = query
        .period_start
        .unwrap_or_else(|| period.current_start_at(Utc::now()));
    Ok(Json(
        state
            .request_logs
            .spend_leaderboard(SpendLeaderboardFilter {
                period,
                period_start,
                limit,
            })
            .await?,
    ))
}

async fn list_audit_logs(
    State(state): State<ConsoleState>,
    Query(query): Query<LogQuery>,
) -> Result<Json<Vec<crate::persistence::ConsoleAuditLog>>, ConsoleError> {
    Ok(Json(
        state
            .coordinator
            .audit_logs(query.limit.unwrap_or(50))
            .await?,
    ))
}

async fn get_system_settings(State(state): State<ConsoleState>) -> Result<Response, ConsoleError> {
    let settings = state.coordinator.system_settings().await?;
    let updated_at = settings.updated_at;
    let mut response = Json(settings).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(updated_at)).expect("ETag is valid"),
    );
    Ok(response)
}

async fn get_session_affinity_cache(
    State(state): State<ConsoleState>,
) -> Json<crate::routing::SessionAffinityCacheSnapshot> {
    Json(state.coordinator.session_affinity_cache())
}

async fn clear_session_affinity_cache(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Query(query): Query<SessionAffinityCacheQuery>,
) -> Result<Json<crate::routing::SessionAffinityCacheClearResult>, ConsoleError> {
    let rule_name = query
        .rule_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if query.rule_name.is_some() && rule_name.is_none() {
        return Err(ConsoleError::Validation);
    }
    let result = state
        .coordinator
        .clear_session_affinity_cache(rule_name)
        .ok_or(ConsoleError::NotFound)?;
    tracing::info!(
        actor_user_id = %principal.user_id(),
        rule_name,
        cleared_entries = result.cleared_entries,
        "session affinity cache cleared"
    );
    Ok(Json(result))
}

async fn get_system_load(State(state): State<ConsoleState>) -> Json<SystemLoadReport> {
    Json(state.system_metrics.snapshot().await)
}

async fn update_system_settings(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    headers: HeaderMap,
    Json(input): Json<SystemSettingsInput>,
) -> Result<Json<MutationResponse>, ConsoleError> {
    mutate(
        &state,
        principal,
        ControlPlaneMutation::UpdateSystemSettings {
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}

async fn reload(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
) -> Result<Json<ReloadResponse>, ConsoleError> {
    Ok(Json(ReloadResponse {
        correlation_id: state.coordinator.manual_reload(principal.user_id()).await?,
    }))
}

enum Resource {
    User,
    UserGroup,
    ApiKeyPolicy,
    Model,
    ApiKey,
    Group,
    Rule,
    Proxy,
}

async fn get_resource(
    state: ConsoleState,
    id: Uuid,
    resource: Resource,
) -> Result<Response, ConsoleError> {
    let lists = state.coordinator.lists().await?;
    let value = match resource {
        Resource::User => lists
            .users
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::UserGroup => lists
            .user_groups
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::ApiKeyPolicy => lists
            .api_key_policies
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::Model => lists
            .models
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::ApiKey => lists
            .api_keys
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::Group => lists
            .channel_groups
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::Rule => lists
            .model_rules
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
        Resource::Proxy => lists
            .proxies
            .into_iter()
            .find(|item| item.id == id)
            .map(to_json),
    }
    .ok_or(ConsoleError::NotFound)?;
    resource_response(value)
}

fn resource_response(value: serde_json::Value) -> Result<Response, ConsoleError> {
    let updated_at: DateTime<Utc> =
        serde_json::from_value(value["updated_at"].clone()).map_err(|_| ConsoleError::Internal)?;
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(updated_at)).expect("ETag is valid"),
    );
    Ok(response)
}

fn to_json<T: Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value).expect("Console DTO serializes")
}

fn etag(updated_at: DateTime<Utc>) -> String {
    format!(
        "\"{}\"",
        updated_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    )
}

fn if_match(headers: &HeaderMap) -> Result<DateTime<Utc>, ConsoleError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or(ConsoleError::Validation)?;
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or(ConsoleError::Validation)?;
    if !value.ends_with('Z') {
        return Err(ConsoleError::Validation);
    }
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ConsoleError::Validation)
}

async fn mutate(
    state: &ConsoleState,
    principal: ConsolePrincipal,
    command: ControlPlaneMutation,
) -> Result<Json<MutationResponse>, ConsoleError> {
    if !principal.role().is_admin() {
        return Err(ConsoleError::Forbidden);
    }
    let result = state
        .coordinator
        .mutate(principal.user_id(), command)
        .await?;
    Ok(Json(mutation_response(result)))
}

async fn mutate_created(
    state: &ConsoleState,
    principal: ConsolePrincipal,
    command: ControlPlaneMutation,
) -> Result<(StatusCode, Json<MutationResponse>), ConsoleError> {
    let response = mutate(state, principal, command).await?;
    Ok((StatusCode::CREATED, response))
}

fn mutation_response(result: crate::persistence::MutationResult) -> MutationResponse {
    MutationResponse {
        id: result.id,
        secret: result.created_secret,
        correlation_id: result
            .correlation_id
            .expect("published mutation has correlation id"),
    }
}

enum ConsoleError {
    Auth(AuthError),
    ChannelModels(ChannelModelDiscoveryError),
    Codex(CodexConnectorError),
    ControlPlane(ControlPlaneError),
    ModelSync(ModelSyncError),
    ProxyTest(ProxyTestError),
    Repository(crate::persistence::RepositoryError),
    NotFound,
    Validation,
    Forbidden,
    Internal,
}

impl From<AuthError> for ConsoleError {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}
impl From<ChannelModelDiscoveryError> for ConsoleError {
    fn from(value: ChannelModelDiscoveryError) -> Self {
        Self::ChannelModels(value)
    }
}
impl From<CodexConnectorError> for ConsoleError {
    fn from(value: CodexConnectorError) -> Self {
        Self::Codex(value)
    }
}
impl From<ControlPlaneError> for ConsoleError {
    fn from(value: ControlPlaneError) -> Self {
        Self::ControlPlane(value)
    }
}
impl From<ModelSyncError> for ConsoleError {
    fn from(value: ModelSyncError) -> Self {
        Self::ModelSync(value)
    }
}
impl From<ProxyTestError> for ConsoleError {
    fn from(value: ProxyTestError) -> Self {
        Self::ProxyTest(value)
    }
}
impl From<crate::persistence::RepositoryError> for ConsoleError {
    fn from(value: crate::persistence::RepositoryError) -> Self {
        Self::Repository(value)
    }
}

impl IntoResponse for ConsoleError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::Auth(AuthError::InvalidCredentials)
            | Self::Auth(AuthError::InvalidToken)
            | Self::Auth(AuthError::InvalidInvitation) => {
                (StatusCode::UNAUTHORIZED, "unauthorized")
            }
            Self::Auth(AuthError::InvalidRegistrationCode) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_registration_code",
            ),
            Self::Auth(AuthError::RegistrationConflict) => {
                (StatusCode::CONFLICT, "registration_email_conflict")
            }
            Self::Auth(AuthError::PasswordChangeRequired) => {
                (StatusCode::FORBIDDEN, "password_change_required")
            }
            Self::Auth(AuthError::PasswordResetNotRequired) => {
                (StatusCode::CONFLICT, "password_reset_not_required")
            }
            Self::Auth(AuthError::ReauthenticationFailed) => {
                (StatusCode::FORBIDDEN, "reauthentication_failed")
            }
            Self::Auth(AuthError::NewPasswordMatchesTemporary) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "new_password_matches_temporary",
            ),
            Self::Auth(AuthError::CannotResetSelf) => (StatusCode::CONFLICT, "cannot_reset_self"),
            Self::Auth(AuthError::Forbidden) | Self::Forbidden => {
                (StatusCode::FORBIDDEN, "forbidden")
            }
            Self::Auth(AuthError::NotFound) => (StatusCode::NOT_FOUND, "not found"),
            Self::Auth(AuthError::InvalidInput) | Self::Validation => {
                (StatusCode::UNPROCESSABLE_ENTITY, "request rejected")
            }
            Self::Auth(AuthError::RateLimited) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many authentication attempts",
            ),
            Self::Auth(AuthError::Configuration) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Console operation failed",
            ),
            Self::Auth(AuthError::Repository(error)) => {
                (repository_status(&error), repository_error_message(&error))
            }
            Self::ChannelModels(ChannelModelDiscoveryError::InvalidConfiguration) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "channel_models_invalid_configuration",
            ),
            Self::ChannelModels(ChannelModelDiscoveryError::ResponseHeaderTimeout)
            | Self::ChannelModels(ChannelModelDiscoveryError::ResponseBodyTimeout) => {
                (StatusCode::BAD_GATEWAY, "upstream_models_timeout")
            }
            Self::ChannelModels(
                ChannelModelDiscoveryError::RequestFailed
                | ChannelModelDiscoveryError::UpstreamHttpStatus(_)
                | ChannelModelDiscoveryError::ResponseBodyFailed
                | ChannelModelDiscoveryError::ResponseTooLarge
                | ChannelModelDiscoveryError::InvalidResponse,
            ) => (StatusCode::BAD_GATEWAY, "upstream_models_unavailable"),
            Self::Codex(error) => codex_error_response(&error),
            Self::ControlPlane(error) => (
                control_plane_status(&error),
                control_plane_error_message(&error),
            ),
            Self::ModelSync(ModelSyncError::InvalidSelection)
            | Self::ModelSync(ModelSyncError::ConflictingSourceModelId) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Console operation rejected",
            ),
            Self::ModelSync(ModelSyncError::Catalog(_)) => {
                (StatusCode::BAD_GATEWAY, "Console operation rejected")
            }
            Self::ModelSync(ModelSyncError::ControlPlane(error)) => (
                control_plane_status(&error),
                control_plane_error_message(&error),
            ),
            Self::ProxyTest(ProxyTestError::InvalidConfiguration) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "proxy_test_invalid_configuration",
            ),
            Self::ProxyTest(ProxyTestError::CredentialsRequired) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "proxy_test_credentials_required",
            ),
            Self::ProxyTest(ProxyTestError::NotFound) => (StatusCode::NOT_FOUND, "not found"),
            Self::ProxyTest(ProxyTestError::RateLimited) => {
                (StatusCode::TOO_MANY_REQUESTS, "proxy_test_rate_limited")
            }
            Self::ProxyTest(
                ProxyTestError::ResponseHeaderTimeout | ProxyTestError::ResponseBodyTimeout,
            ) => (StatusCode::GATEWAY_TIMEOUT, "proxy_test_timeout"),
            Self::ProxyTest(
                ProxyTestError::RequestFailed
                | ProxyTestError::ProviderUnavailable
                | ProxyTestError::ResponseBodyFailed
                | ProxyTestError::ResponseTooLarge
                | ProxyTestError::InvalidResponse,
            ) => (StatusCode::BAD_GATEWAY, "proxy_test_unavailable"),
            Self::ProxyTest(ProxyTestError::Repository(error)) => {
                (repository_status(&error), repository_error_message(&error))
            }
            Self::Repository(error) => {
                (repository_status(&error), repository_error_message(&error))
            }
            Self::NotFound => (StatusCode::NOT_FOUND, "not found"),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Console operation failed",
            ),
        };
        (status, Json(ErrorBody { error })).into_response()
    }
}

fn control_plane_status(error: &ControlPlaneError) -> StatusCode {
    match error {
        ControlPlaneError::Compile(_)
        | ControlPlaneError::Repository(crate::persistence::RepositoryError::Validation) => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ControlPlaneError::Repository(crate::persistence::RepositoryError::NotFound) => {
            StatusCode::NOT_FOUND
        }
        ControlPlaneError::Repository(crate::persistence::RepositoryError::Conflict) => {
            StatusCode::CONFLICT
        }
        ControlPlaneError::InvalidActor => StatusCode::FORBIDDEN,
        ControlPlaneError::Repository(error) => repository_status(error),
    }
}

fn codex_error_response(error: &CodexConnectorError) -> (StatusCode, &'static str) {
    match error {
        CodexConnectorError::ControlPlane(error) => {
            return (
                control_plane_status(error),
                control_plane_error_message(error),
            );
        }
        CodexConnectorError::Repository(error) => {
            return (repository_status(error), repository_error_message(error));
        }
        _ => {}
    }
    let status = match error {
        CodexConnectorError::ControlPlane(_) | CodexConnectorError::Repository(_) => {
            unreachable!("nested Codex errors returned above")
        }
        CodexConnectorError::CredentialNotFound => StatusCode::NOT_FOUND,
        CodexConnectorError::OauthFlowExpired => StatusCode::GONE,
        CodexConnectorError::OauthStateMismatch
        | CodexConnectorError::InvalidCallback
        | CodexConnectorError::OauthDenied
        | CodexConnectorError::InvalidCredential
        | CodexConnectorError::MissingAccountId
        | CodexConnectorError::AccountChanged
        | CodexConnectorError::InvalidJwt
        | CodexConnectorError::InvalidTokenResponse
        | CodexConnectorError::InvalidModelsResponse
        | CodexConnectorError::NoModels
        | CodexConnectorError::InvalidQuotaResponse
        | CodexConnectorError::CredentialDisabled
        | CodexConnectorError::CredentialReauthenticationRequired
        | CodexConnectorError::RefreshTokenInvalid => StatusCode::UNPROCESSABLE_ENTITY,
        CodexConnectorError::UpstreamTimeout => StatusCode::GATEWAY_TIMEOUT,
        CodexConnectorError::UpstreamClient(_)
        | CodexConnectorError::InvalidEndpoint
        | CodexConnectorError::InvalidProxy
        | CodexConnectorError::TokenEndpointStatus(_)
        | CodexConnectorError::CodexBackendStatus(_)
        | CodexConnectorError::UpstreamUnavailable
        | CodexConnectorError::UpstreamResponseTooLarge => StatusCode::BAD_GATEWAY,
    };
    (status, error.code())
}

fn control_plane_error_message(error: &ControlPlaneError) -> &'static str {
    match error {
        ControlPlaneError::Compile(ConfigError::Compile(reason))
            if routing_dependency_invalid(reason) =>
        {
            "routing_dependency_invalid"
        }
        ControlPlaneError::Repository(error) => repository_error_message(error),
        _ => "Console operation rejected",
    }
}

/// Returns a stable, non-sensitive code for a rejected graph mutation. The
/// compiler's full diagnostic remains server-only because it may be derived
/// from opaque transform or credential-bearing configuration.
fn routing_dependency_invalid(reason: &str) -> bool {
    matches!(
        reason,
        "enabled model rule references a disabled upstream model"
            | "model rule references a missing channel group"
            | "model rule references a cross-format channel group"
            | "model rule references a missing channel"
            | "model rule references a cross-format channel"
            | "direct channel candidate does not support the model rule upstream model"
            | "direct channel candidate references a missing group"
            | "each enabled model rule must have at least one distinct model-capable candidate channel"
            | "all channel groups in every route priority tier must use the same selection strategy"
            | "channel references a missing group"
            | "channel and group use different API formats"
            | "channel references a missing or disabled proxy"
            | "channel references a missing or disabled template"
            | "channel references a cross-format template"
    )
}

fn repository_error_message(error: &crate::persistence::RepositoryError) -> &'static str {
    match error {
        crate::persistence::RepositoryError::DefaultApiKeyPolicyRequired => {
            "default_api_key_policy_required"
        }
        crate::persistence::RepositoryError::DefaultApiKeyPolicyDisabled => {
            "default_api_key_policy_disabled"
        }
        crate::persistence::RepositoryError::ApiKeyTargetNotAllowed => "api_key_target_not_allowed",
        crate::persistence::RepositoryError::ProtectedUserGroup => "protected_user_group",
        crate::persistence::RepositoryError::UserGroupInUse => "user_group_in_use",
        crate::persistence::RepositoryError::ProxyInUse => "proxy_in_use",
        crate::persistence::RepositoryError::CannotDeleteSelf => "cannot_delete_self",
        crate::persistence::RepositoryError::LastAdministrator => "last_administrator",
        crate::persistence::RepositoryError::CannotDisableSelf => "cannot_disable_self",
        crate::persistence::RepositoryError::CannotResetSelf => "cannot_reset_self",
        crate::persistence::RepositoryError::TemporaryPasswordUnavailable => {
            "temporary_password_unavailable"
        }
        crate::persistence::RepositoryError::RegistrationInvitationCodeConflict => {
            "registration_invitation_code_conflict"
        }
        _ => "Console operation rejected",
    }
}

fn repository_status(error: &crate::persistence::RepositoryError) -> StatusCode {
    match error {
        crate::persistence::RepositoryError::NotFound => StatusCode::NOT_FOUND,
        crate::persistence::RepositoryError::Conflict
        | crate::persistence::RepositoryError::ProtectedUserGroup
        | crate::persistence::RepositoryError::UserGroupInUse
        | crate::persistence::RepositoryError::ProxyInUse
        | crate::persistence::RepositoryError::CannotDeleteSelf
        | crate::persistence::RepositoryError::LastAdministrator
        | crate::persistence::RepositoryError::CannotDisableSelf
        | crate::persistence::RepositoryError::CannotResetSelf
        | crate::persistence::RepositoryError::RegistrationInvitationCodeConflict => {
            StatusCode::CONFLICT
        }
        crate::persistence::RepositoryError::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        crate::persistence::RepositoryError::TemporaryPasswordUnavailable => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        crate::persistence::RepositoryError::DefaultApiKeyPolicyRequired
        | crate::persistence::RepositoryError::DefaultApiKeyPolicyDisabled
        | crate::persistence::RepositoryError::ApiKeyTargetNotAllowed => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        crate::persistence::RepositoryError::Sql(error)
            if error
                .as_database_error()
                .and_then(|database| database.code())
                .is_some_and(|code| code == "40001" || code == "40P01") =>
        {
            StatusCode::CONFLICT
        }
        crate::persistence::RepositoryError::Sql(error)
            if error
                .as_database_error()
                .and_then(|database| database.code())
                .is_some_and(|code| {
                    matches!(
                        code.as_ref(),
                        "22001" | "22007" | "22P02" | "23502" | "23503" | "23505" | "23514"
                    )
                }) =>
        {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
