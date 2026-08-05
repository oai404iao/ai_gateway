//! Axum routes, middleware, and HTTP error responses.

pub mod console;
/// Embedded Console web UI; only compiled with `embedded-console-ui`.
#[cfg(feature = "embedded-console-ui")]
pub mod console_ui;
mod content_coding;

use axum::{
    Json, Router,
    extract::{OriginalUri, Request, State, ws::WebSocketUpgrade},
    http::{HeaderMap, StatusCode},
    middleware,
    response::Response,
    routing::{get, post},
};

use crate::{
    application::{ModelsResponse, ProxyError, ProxyService},
    domain::ApiOperation,
    upstream::MAX_UPSTREAM_MESSAGE_BYTES,
};

/// Builds the public HTTP router with a reusable data-plane service.
pub fn router(proxy: ProxyService) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/responses", post(responses).get(responses_websocket))
        .route("/v1/alpha/search", post(standalone_web_search))
        .route("/v1/images/generations", post(images_generations))
        .route("/v1/images/edits", post(images_edits))
        .with_state(proxy)
        .layer(content_coding::compression_layer())
        .layer(middleware::from_fn(
            content_coding::sanitize_compressed_response,
        ))
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
    proxy.proxy(ApiOperation::ChatCompletions, request).await
}

async fn responses(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy.proxy(ApiOperation::Responses, request).await
}

async fn standalone_web_search(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy
        .proxy(ApiOperation::StandaloneWebSearch, request)
        .await
}

async fn images_generations(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy.proxy(ApiOperation::ImagesGeneration, request).await
}

async fn images_edits(
    State(proxy): State<ProxyService>,
    request: Request,
) -> Result<Response, ProxyError> {
    proxy.proxy(ApiOperation::ImagesEdit, request).await
}

async fn responses_websocket(
    State(proxy): State<ProxyService>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Result<Response, ProxyError> {
    let session = proxy.prepare_responses_websocket(&headers, &uri)?;
    let request_limit = proxy.websocket_request_limit();
    Ok(websocket
        .read_buffer_size(64 * 1024)
        .write_buffer_size(32 * 1024)
        .max_write_buffer_size(MAX_UPSTREAM_MESSAGE_BYTES.saturating_add(32 * 1024))
        .max_message_size(request_limit)
        .max_frame_size(request_limit)
        .on_upgrade(move |socket| session.run(socket)))
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
        runtime_config::{RuntimeConfig, compile_control_plane},
    };

    use super::router;

    #[tokio::test]
    async fn public_router_does_not_expose_console_paths() {
        let runtime = Arc::new(RuntimeConfig::new(
            compile_control_plane(Default::default()).unwrap(),
        ));
        let proxy = ProxyService::new(runtime, 1_024).unwrap();
        let response = router(proxy)
            .oneshot(Request::get("/console/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
