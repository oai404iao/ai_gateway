//! Embedded Console web UI: static asset serving, SPA fallback, and cache +
//! security headers. Only compiled when the `embedded-console-ui` cargo
//! feature is enabled.
//!
//! Routing rules (see docs/console-ui-design.md §3):
//! - `/console/v1/*` is owned by the API router. This fallback never returns
//!   `index.html` for an API path; an unmatched API path yields a JSON 404.
//! - Fingerprinted `/assets/*` files are served with a long immutable
//!   cache; `index.html` and other root files use `no-cache`.
//! - The SPA fallback only answers `GET`/`HEAD`.

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "web/console/dist"]
struct ConsoleAssets;

const INDEX_HTML: &str = "index.html";

/// Builds the stateless UI router. Merge this after the Console API router so
/// the API's explicit `/console/v1/*` routes take precedence over this
/// fallback.
pub fn router() -> Router<()> {
    Router::new()
        .fallback(spa_fallback)
        .layer(axum::middleware::from_fn(security_headers))
}

async fn spa_fallback(request: Request) -> Response {
    let path = request.uri().path();
    if path.starts_with("/console/v1") {
        return api_not_found();
    }
    let method = request.method();
    if method != Method::GET && method != Method::HEAD {
        return method_not_allowed();
    }
    let head = method == Method::HEAD;
    let asset_path = path.trim_start_matches('/');
    if let Some(file) = ConsoleAssets::get(asset_path) {
        return serve_asset(asset_path, file, head);
    }
    serve_index(head)
}

fn serve_asset(path: &str, file: rust_embed::EmbeddedFile, head: bool) -> Response {
    let content_type = mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_owned();
    let cache_control = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    asset_response(&content_type, cache_control, file.data, head)
}

fn serve_index(head: bool) -> Response {
    let Some(file) = ConsoleAssets::get(INDEX_HTML) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Console UI index.html not found; run `pnpm --dir web/console build`",
        )
            .into_response();
    };
    asset_response("text/html; charset=utf-8", "no-cache", file.data, head)
}

fn asset_response(
    content_type: &str,
    cache_control: &'static str,
    data: Cow<'static, [u8]>,
    head: bool,
) -> Response {
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .header(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        )
        .body(if head {
            Body::empty()
        } else {
            Body::from(data.into_owned())
        })
        .expect("static asset response is valid");
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    response
}

fn api_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"error":"not found"}"#))
        .expect("api not-found response is valid")
}

fn method_not_allowed() -> Response {
    Response::builder()
        .status(StatusCode::METHOD_NOT_ALLOWED)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::empty())
        .expect("method-not-allowed response is valid")
}

/// Adds baseline security headers to every UI response. API responses are not
/// routed through this middleware because the API router is merged separately.
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    // Allow same-origin scripts/styles/fonts/connect; block framing and
    // external origins. Vite production assets are same-origin and hashed.
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data:; font-src 'self' data:; connect-src 'self'; \
             frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
        ),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("geolocation=(), microphone=(), camera=()"),
    );
    response
}

#[cfg(all(test, feature = "embedded-console-ui"))]
mod tests {
    use super::ConsoleAssets;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use tower::ServiceExt;

    fn router() -> axum::Router<()> {
        super::router()
    }

    async fn body_text(response: axum::response::Response) -> Vec<u8> {
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response body collects")
            .to_vec()
    }

    #[tokio::test]
    async fn root_serves_index_html_with_no_cache_and_security_headers() {
        let response = router()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache"
        );
        assert!(response.headers().get("content-security-policy").is_some());
        assert_eq!(
            response.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
        let body = body_text(response).await;
        let html = std::str::from_utf8(&body).unwrap();
        assert!(html.contains("root"), "index.html must mount the SPA root");
    }

    #[tokio::test]
    async fn unknown_api_path_returns_json_not_found_not_index() {
        let response = router()
            .oneshot(
                Request::get("/console/v1/does-not-exist")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let body = body_text(response).await;
        assert!(std::str::from_utf8(&body).unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn fingerprinted_assets_are_cached_immutably() {
        let asset = ConsoleAssets::iter()
            .find(|path| path.starts_with("assets/"))
            .expect("dist contains at least one fingerprinted asset");
        let response = router()
            .oneshot(
                Request::get(format!("/{asset}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
    }

    #[tokio::test]
    async fn unknown_html_route_falls_back_to_index() {
        let response = router()
            .oneshot(
                Request::get("/account/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn non_get_head_methods_are_rejected() {
        let response = router()
            .oneshot(Request::post("/whatever").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn head_returns_no_body() {
        let response = router()
            .oneshot(Request::head("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(body_text(response).await.is_empty());
    }
}
