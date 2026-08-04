//! Downstream HTTP response compression and representation-header cleanup.

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, header::CONTENT_ENCODING},
    middleware::Next,
    response::Response,
};
use tower_http::compression::{
    CompressionLayer, CompressionLevel,
    predicate::{DefaultPredicate, Predicate, SizeAbove},
};

const MINIMUM_COMPRESSION_BYTES: u64 = 1_024;

pub(super) fn compression_layer() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new()
        .quality(CompressionLevel::Fastest)
        .compress_when(DefaultPredicate::new().and(SizeAbove::new(MINIMUM_COMPRESSION_BYTES)))
}

/// Removes validators and digests after the inner compression layer changes
/// the downstream representation.
pub(super) async fn sanitize_compressed_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    if response_is_encoded(response.headers()) {
        for name in [
            HeaderName::from_static("etag"),
            HeaderName::from_static("content-md5"),
            HeaderName::from_static("digest"),
            HeaderName::from_static("content-digest"),
            HeaderName::from_static("repr-digest"),
        ] {
            response.headers_mut().remove(name);
        }
    }
    response
}

fn response_is_encoded(headers: &HeaderMap) -> bool {
    headers.get_all(CONTENT_ENCODING).iter().any(|value| {
        value
            .to_str()
            .is_ok_and(|value| !value.trim().eq_ignore_ascii_case("identity"))
    })
}
