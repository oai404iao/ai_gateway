//! Axum routes, middleware, and HTTP error responses.

pub mod console;
/// Embedded Console web UI; only compiled with `embedded-console-ui`.
#[cfg(feature = "embedded-console-ui")]
pub mod console_ui;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::{
        application::ProxyService,
        runtime_config::{RuntimeConfig, UpstreamConfig, compile_control_plane},
    };

    use super::router;

    #[tokio::test]
    async fn public_router_does_not_expose_console_paths() {
        let runtime = Arc::new(RuntimeConfig::new(
            compile_control_plane(Default::default()).unwrap(),
        ));
        let proxy = ProxyService::new(
            runtime,
            1_024,
            &UpstreamConfig {
                connect_timeout_seconds: 1,
                response_header_timeout_seconds: 2,
                stream_idle_timeout_seconds: 1,
            },
        )
        .unwrap();
        let response = router(proxy)
            .oneshot(Request::get("/console/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
