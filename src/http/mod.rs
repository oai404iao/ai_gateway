//! Axum routes, middleware, and HTTP error responses.

use axum::{Router, http::StatusCode, routing::get};

/// Builds the public HTTP router. Versioned proxy routes are added here.
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}
