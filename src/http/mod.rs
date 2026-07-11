//! Axum routes, middleware, and HTTP error responses.

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};

use crate::{
    application::{ModelsResponse, ProxyError, ProxyService},
    domain::ApiFormat,
};

/// Builds the public HTTP router with a reusable data-plane service.
pub fn router(proxy: ProxyService) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses))
        .with_state(proxy)
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn models(
    State(proxy): State<ProxyService>,
    headers: HeaderMap,
) -> Result<Json<ModelsResponse>, ProxyError> {
    Ok(Json(proxy.list_models(&headers)?))
}

async fn chat_completions(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy.proxy(ApiFormat::OpenAiChatCompletions, request).await
}

async fn responses(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy.proxy(ApiFormat::OpenAiResponses, request).await
}
