//! JWT-authenticated Console API for self-service and role-gated control-plane work.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    limit::RequestBodyLimitLayer,
};
use uuid::Uuid;

use crate::{
    application::{
        AuthError, ChannelModelDiscoveryError, ChannelModelDiscoveryInput,
        ChannelModelDiscoveryResponse, ChannelModelDiscoveryService, ConsoleAuthService,
        ControlPlaneCoordinator, ControlPlaneError, IssuedInvitation, IssuedSession,
        ModelImportRequest, ModelSyncError, ModelSyncPreview, ModelSyncPreviewRequest,
        ModelSyncResponse, ModelSyncService, SystemLoadReport, SystemMetricsService,
    },
    domain::{ConsolePrincipal, UserRole},
    persistence::{
        ApiKeyCreate, ApiKeyPolicyInput, ApiKeyUpdate, ChannelBatchUpdateInput, ChannelCreateInput,
        ChannelGroupInput, ChannelInput, ChannelStatusWindow, ConfigTemplateCreateInput,
        ConfigTemplateInput, ConsoleApiKey, ControlPlaneMutation, CostStatisticsFilter,
        InviteUserInput, ModelInput, ModelRuleInput, ProxyCreateInput, ProxyInput,
        RequestLogFilter, RequestLogRepository, SelfApiKeyCreate, SelfApiKeyUpdate,
        StatisticsGranularity, SystemSettingsInput, UserInput,
    },
    runtime_config::ConfigError,
};

const REFRESH_COOKIE_NAME: &str = "__Host-ai_gateway_refresh";

#[derive(Clone)]
pub struct ConsoleState {
    pub coordinator: ControlPlaneCoordinator,
    pub channel_models: ChannelModelDiscoveryService,
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
        .route("/console/v1/auth/refresh", post(refresh))
        .route(
            "/console/v1/auth/activate-invitation",
            post(activate_invitation),
        )
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(state.auth_body_bytes));

    let self_routes = Router::new()
        .route("/console/v1/auth/logout", post(logout))
        .route("/console/v1/me", get(get_me).patch(update_me))
        .route("/console/v1/me/password", post(change_password))
        .route("/console/v1/me/sessions", get(list_sessions))
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
        .route("/console/v1/me/request-logs/{id}", get(get_own_request_log));

    let statistics_routes = Router::new()
        .route(
            "/console/v1/statistics/channel-status",
            get(get_channel_status),
        )
        .route("/console/v1/statistics/costs", get(get_cost_statistics));

    let control_routes = Router::new()
        .route("/console/v1/users", get(list_users).post(invite_user))
        .route("/console/v1/users/{id}", get(get_user).put(update_user))
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
            "/console/v1/routing/model-rules",
            get(list_rules).post(create_rule),
        )
        .route(
            "/console/v1/routing/model-rules/{id}",
            get(get_rule).put(update_rule),
        )
        .route(
            "/console/v1/network/proxies",
            get(list_proxies).post(create_proxy),
        )
        .route(
            "/console/v1/network/proxies/{id}",
            get(get_proxy).put(update_proxy),
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
        .route("/console/v1/system/load", get(get_system_load))
        .route("/console/v1/system/reload", post(reload))
        .route_layer(middleware::from_fn(require_admin));

    let authenticated = self_routes
        .merge(statistics_routes)
        .merge(control_routes)
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginInput {
    email: String,
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
    default_api_key_policy_id: Option<Uuid>,
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
    Json(input): Json<LoginInput>,
) -> Result<Response, ConsoleError> {
    let session = state.auth.login(input.email, input.password).await?;
    Ok(session_response(session))
}

async fn refresh(
    State(state): State<ConsoleState>,
    headers: HeaderMap,
) -> Result<Response, ConsoleError> {
    let refresh_token =
        refresh_cookie(&headers).ok_or(ConsoleError::Auth(AuthError::InvalidToken))?;
    let session = state.auth.refresh(refresh_token).await?;
    Ok(session_response(session))
}

async fn activate_invitation(
    State(state): State<ConsoleState>,
    Json(input): Json<ActivateInvitationInput>,
) -> Result<Response, ConsoleError> {
    let session = state
        .auth
        .accept_invitation(&input.invitation_token, input.password)
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
            .sessions_for_user(principal.user_id())
            .await?,
    ))
}

async fn revoke_session(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ConsoleError> {
    if state
        .auth
        .repository()
        .revoke_session_for_user(principal.user_id(), id)
        .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ConsoleError::NotFound)
    }
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

async fn update_user(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UserInput>,
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

async fn get_cost_statistics(
    State(state): State<ConsoleState>,
    Extension(principal): Extension<ConsolePrincipal>,
    Query(query): Query<CostStatisticsQuery>,
) -> Result<Json<crate::persistence::CostStatisticsReport>, ConsoleError> {
    let ended_at = query.started_before.unwrap_or_else(Utc::now);
    let started_at = query
        .started_after
        .unwrap_or_else(|| ended_at - chrono::Duration::days(7));
    let granularity = match query.granularity.as_deref().unwrap_or("day") {
        "hour" => StatisticsGranularity::Hour,
        "day" => StatisticsGranularity::Day,
        _ => return Err(ConsoleError::Validation),
    };
    let user_id = if principal.role().is_admin() {
        query.user_id
    } else {
        if query
            .user_id
            .is_some_and(|user_id| user_id != principal.user_id())
        {
            return Err(ConsoleError::Forbidden);
        }
        Some(principal.user_id())
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
    ControlPlane(ControlPlaneError),
    ModelSync(ModelSyncError),
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
            Self::Auth(AuthError::Forbidden) | Self::Forbidden => {
                (StatusCode::FORBIDDEN, "forbidden")
            }
            Self::Auth(AuthError::InvalidInput) | Self::Validation => {
                (StatusCode::UNPROCESSABLE_ENTITY, "request rejected")
            }
            Self::Auth(AuthError::RateLimited) => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many authentication attempts",
            ),
            Self::Auth(AuthError::Configuration) | Self::Auth(AuthError::Repository(_)) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Console operation failed",
            ),
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
        _ => "Console operation rejected",
    }
}

fn repository_status(error: &crate::persistence::RepositoryError) -> StatusCode {
    match error {
        crate::persistence::RepositoryError::NotFound => StatusCode::NOT_FOUND,
        crate::persistence::RepositoryError::Conflict => StatusCode::CONFLICT,
        crate::persistence::RepositoryError::Validation => StatusCode::UNPROCESSABLE_ENTITY,
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
