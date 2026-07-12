//! Local-only management HTTP DTOs and authentication boundary.

use axum::{
    Json, Router,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    application::{ControlPlaneCoordinator, ControlPlaneError},
    domain::AdminTokenVerifier,
    persistence::{
        AdminMutation, ApiKeyCreate, ApiKeyUpdate, ChannelCreateInput, ChannelGroupInput,
        ChannelInput, ConfigTemplateInput, ModelRuleInput, ProxyCreateInput, ProxyInput,
    },
};

#[derive(Clone)]
pub struct AdminState {
    pub coordinator: ControlPlaneCoordinator,
    pub actor_user_id: Uuid,
    pub verifier: AdminTokenVerifier,
}

/// Builds only the separate management listener. The public `http::router`
/// intentionally never nests this router.
pub fn router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/admin/v1/api-keys",
            get(list_api_keys).post(create_api_key),
        )
        .route(
            "/admin/v1/api-keys/{id}",
            get(get_api_key).put(update_api_key),
        )
        .route("/admin/v1/api-keys/{id}/revoke", post(revoke_api_key))
        .route(
            "/admin/v1/channel-groups",
            get(list_groups).post(create_group),
        )
        .route(
            "/admin/v1/channel-groups/{id}",
            get(get_group).put(update_group),
        )
        .route(
            "/admin/v1/channels",
            get(list_channels).post(create_channel),
        )
        .route(
            "/admin/v1/channels/{id}",
            get(get_channel).put(update_channel),
        )
        .route("/admin/v1/model-rules", get(list_rules).post(create_rule))
        .route("/admin/v1/model-rules/{id}", get(get_rule).put(update_rule))
        .route("/admin/v1/proxies", get(list_proxies).post(create_proxy))
        .route("/admin/v1/proxies/{id}", get(get_proxy).put(update_proxy))
        .route(
            "/admin/v1/config-templates",
            get(list_config_templates).post(create_config_template),
        )
        .route(
            "/admin/v1/config-templates/{id}",
            get(get_config_template).put(update_config_template),
        )
        .route("/admin/v1/reload", post(reload))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .layer(middleware::from_fn(no_store))
        .with_state(state)
}

async fn authenticate(State(state): State<AdminState>, request: Request, next: Next) -> Response {
    let supplied = bearer_token(request.headers());
    if !supplied.is_some_and(|token| state.verifier.matches(token)) {
        return unauthorized();
    }
    match state
        .coordinator
        .verify_active_actor(state.actor_user_id)
        .await
    {
        Ok(()) => next.run(request).await,
        Err(_) => unavailable(),
    }
}
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
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
fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorBody {
            error: "management unavailable",
        }),
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeInput {
    reason: String,
}

async fn list_api_keys(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.api_keys)
            .expect("admin DTO serializes"),
    ))
}
async fn list_groups(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.channel_groups)
            .expect("admin DTO serializes"),
    ))
}
async fn list_channels(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.channels)
            .expect("admin DTO serializes"),
    ))
}
async fn list_rules(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.model_rules)
            .expect("admin DTO serializes"),
    ))
}
async fn list_proxies(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.proxies)
            .expect("admin DTO serializes"),
    ))
}
async fn list_config_templates(
    State(state): State<AdminState>,
) -> Result<Json<serde_json::Value>, AdminError> {
    Ok(Json(
        serde_json::to_value(state.coordinator.lists().await?.config_templates)
            .expect("admin DTO serializes"),
    ))
}
async fn create_api_key(
    State(state): State<AdminState>,
    Json(input): Json<ApiKeyCreate>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateApiKey(input)).await
}
async fn get_api_key(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::ApiKey).await
}
async fn update_api_key(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ApiKeyUpdate>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateApiKey {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn revoke_api_key(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    Json(input): Json<RevokeInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::RevokeApiKey {
            id,
            reason: input.reason,
        },
    )
    .await
}
async fn create_group(
    State(state): State<AdminState>,
    Json(input): Json<ChannelGroupInput>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateGroup(input)).await
}
async fn get_group(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::Group).await
}
async fn update_group(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ChannelGroupInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateGroup {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn create_channel(
    State(state): State<AdminState>,
    Json(input): Json<ChannelCreateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateChannel(input)).await
}
async fn get_channel(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::Channel).await
}
async fn update_channel(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ChannelInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateChannel {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn get_rule(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::Rule).await
}
async fn create_rule(
    State(state): State<AdminState>,
    Json(input): Json<ModelRuleInput>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateRule(input)).await
}
async fn update_rule(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ModelRuleInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateRule {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn create_proxy(
    State(state): State<AdminState>,
    Json(input): Json<ProxyCreateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateProxy(input)).await
}
async fn get_proxy(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::Proxy).await
}
async fn update_proxy(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ProxyInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateProxy {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn create_config_template(
    State(state): State<AdminState>,
    Json(input): Json<ConfigTemplateInput>,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    mutate_created(&state, AdminMutation::CreateConfigTemplate(input)).await
}
async fn get_config_template(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AdminError> {
    get_resource(state, id, Resource::ConfigTemplate).await
}
async fn update_config_template(
    State(state): State<AdminState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<ConfigTemplateInput>,
) -> Result<Json<MutationResponse>, AdminError> {
    mutate(
        &state,
        AdminMutation::UpdateConfigTemplate {
            id,
            input,
            expected_updated_at: if_match(&headers)?,
        },
    )
    .await
}
async fn reload(State(state): State<AdminState>) -> Result<Json<ReloadResponse>, AdminError> {
    let correlation_id = state.coordinator.manual_reload(state.actor_user_id).await?;
    Ok(Json(ReloadResponse { correlation_id }))
}
enum Resource {
    ApiKey,
    Group,
    Channel,
    Rule,
    Proxy,
    ConfigTemplate,
}
async fn get_resource(
    state: AdminState,
    id: Uuid,
    resource: Resource,
) -> Result<Response, AdminError> {
    let lists = state.coordinator.lists().await?;
    let value = match resource {
        Resource::ApiKey => lists
            .api_keys
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
        Resource::Group => lists
            .channel_groups
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
        Resource::Channel => lists
            .channels
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
        Resource::Rule => lists
            .model_rules
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
        Resource::Proxy => lists
            .proxies
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
        Resource::ConfigTemplate => lists
            .config_templates
            .into_iter()
            .find(|item| item.id == id)
            .map(|item| serde_json::to_value(item).expect("admin DTO serializes")),
    }
    .ok_or(AdminError(ControlPlaneError::Repository(
        crate::persistence::RepositoryError::NotFound,
    )))?;
    let updated_at: DateTime<Utc> =
        serde_json::from_value(value["updated_at"].clone()).expect("admin DTO timestamp");
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::ETAG,
        HeaderValue::from_str(&etag(updated_at)).expect("ETag is valid"),
    );
    Ok(response)
}
fn etag(updated_at: DateTime<Utc>) -> String {
    format!(
        "\"{}\"",
        updated_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    )
}
fn if_match(headers: &HeaderMap) -> Result<DateTime<Utc>, AdminError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AdminError(ControlPlaneError::Repository(
                crate::persistence::RepositoryError::Validation,
            ))
        })?;
    let value = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or_else(|| {
            AdminError(ControlPlaneError::Repository(
                crate::persistence::RepositoryError::Validation,
            ))
        })?;
    if !value.ends_with('Z') {
        return Err(AdminError(ControlPlaneError::Repository(
            crate::persistence::RepositoryError::Validation,
        )));
    }
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| {
            AdminError(ControlPlaneError::Repository(
                crate::persistence::RepositoryError::Validation,
            ))
        })
}
async fn mutate(
    state: &AdminState,
    command: AdminMutation,
) -> Result<Json<MutationResponse>, AdminError> {
    let result = state
        .coordinator
        .mutate(state.actor_user_id, command)
        .await?;
    Ok(Json(MutationResponse {
        id: result.id,
        secret: result.created_secret,
        correlation_id: result.correlation_id.expect("mutation has correlation id"),
    }))
}
async fn mutate_created(
    state: &AdminState,
    command: AdminMutation,
) -> Result<(StatusCode, Json<MutationResponse>), AdminError> {
    let response = mutate(state, command).await?;
    Ok((StatusCode::CREATED, response))
}

struct AdminError(ControlPlaneError);
impl From<ControlPlaneError> for AdminError {
    fn from(value: ControlPlaneError) -> Self {
        Self(value)
    }
}
impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        let status = match self.0 {
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
            ControlPlaneError::InvalidActor => StatusCode::SERVICE_UNAVAILABLE,
            ControlPlaneError::Repository(crate::persistence::RepositoryError::Sql(ref error))
                if error
                    .as_database_error()
                    .and_then(|database| database.code())
                    .is_some_and(|code| code == "40001" || code == "40P01") =>
            {
                StatusCode::CONFLICT
            }
            ControlPlaneError::Repository(crate::persistence::RepositoryError::Sql(ref error))
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
            ControlPlaneError::Repository(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let error = if status == StatusCode::INTERNAL_SERVER_ERROR {
            "management operation failed"
        } else {
            "management operation rejected"
        };
        (status, Json(serde_json::json!({"error":error}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use crate::{
        application::ControlPlaneCoordinator,
        persistence::ControlPlaneRepository,
        routing::{PassiveHealthPolicy, RoutingRuntime},
        runtime_config::{RuntimeConfig, compile_control_plane},
    };

    use super::{AdminState, router};

    fn app() -> axum::Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .expect("valid lazy PostgreSQL URL");
        let runtime = Arc::new(RuntimeConfig::new(
            compile_control_plane(Default::default()).expect("empty snapshot is valid"),
        ));
        let coordinator = ControlPlaneCoordinator::new(
            ControlPlaneRepository::new(pool),
            runtime,
            RoutingRuntime::new(PassiveHealthPolicy::default()),
        );
        router(AdminState {
            coordinator,
            actor_user_id: uuid::Uuid::nil(),
            verifier: crate::domain::AdminTokenVerifier::from_token(
                "a-managed-token-that-is-at-least-thirty-two-chars",
            ),
        })
    }

    async fn response(headers: &[(&str, &str)]) -> (axum::http::StatusCode, Vec<u8>) {
        let mut request = Request::builder().uri("/admin/v1/reload").method("POST");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = app()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, body)
    }

    #[tokio::test]
    async fn missing_malformed_and_wrong_bearer_auth_are_identical() {
        let missing = response(&[]).await;
        let malformed = response(&[("authorization", "Basic nope")]).await;
        let wrong = response(&[("authorization", "Bearer wrong")]).await;

        assert_eq!(missing.0, axum::http::StatusCode::UNAUTHORIZED);
        assert_eq!(missing, malformed);
        assert_eq!(missing, wrong);
    }
}
