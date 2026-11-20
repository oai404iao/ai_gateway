//! Candidate-specific transforms shared by HTTP requests and WebSocket creates.

use axum::http::{HeaderMap, HeaderName, header::CONTENT_ENCODING};
use bytes::Bytes;
use serde_json::Value;

use crate::{
    application::{ConnectorAttemptError, connector::PreparedUpstreamAttempt},
    domain::RequestProtocol,
    request_policy::{RequestInterface, apply_client_fast_mode_filter},
    transforms::{
        HeaderPlan, JsonPatchPlan, TransformApplyError, apply_header_plan, apply_json_patch_plan,
    },
};

use super::ProxyError;

pub(super) enum JsonPreparationError {
    Model(ProxyError),
    Transform,
    Connector(ConnectorAttemptError),
}

pub(super) fn prepare_json(
    body: &Bytes,
    client_model: &str,
    upstream_model: &str,
    plan: &JsonPatchPlan,
    connector: &PreparedUpstreamAttempt,
    protocol: RequestProtocol,
    fast_filter: Option<RequestInterface>,
) -> Result<Bytes, JsonPreparationError> {
    let body = rewrite_model_alias(body.clone(), client_model, upstream_model)
        .map_err(JsonPreparationError::Model)?;
    let body = apply_json_patch_plan(body, plan).map_err(|_| JsonPreparationError::Transform)?;
    // A channel transform may have added a tier after ingress filtering.
    let body = if let Some(interface) = fast_filter
        && !plan.is_empty()
    {
        apply_client_fast_mode_filter(interface, body, true)
            .map_err(|_| JsonPreparationError::Transform)?
            .body
    } else {
        body
    };
    connector
        .adapt_json_body(body, protocol)
        .map_err(JsonPreparationError::Connector)
}

pub(super) fn transform_headers(
    client_headers: &HeaderMap,
    body_changed: bool,
    plan: &HeaderPlan,
) -> Result<HeaderMap, TransformApplyError> {
    let mut headers = client_headers.clone();
    if body_changed {
        remove_rewritten_request_entity_headers(&mut headers);
    }
    // The plan must still see Connection-nominated names, but not stale entity metadata.
    apply_header_plan(&mut headers, plan)?;
    Ok(headers)
}

fn rewrite_model_alias(
    original_body: Bytes,
    client_model: &str,
    upstream_model: &str,
) -> Result<Bytes, ProxyError> {
    if client_model == upstream_model {
        return Ok(original_body);
    }

    let mut value = serde_json::from_slice::<Value>(&original_body).map_err(|_| {
        ProxyError::invalid_request("Request body must contain a JSON object.", "model")
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        ProxyError::invalid_request("Request body must contain a JSON object.", "model")
    })?;
    object.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| ProxyError::invalid_request("Request body cannot be rewritten.", "model"))
}

fn remove_rewritten_request_entity_headers(headers: &mut HeaderMap) {
    headers.remove(CONTENT_ENCODING);
    for name in [
        HeaderName::from_static("content-md5"),
        HeaderName::from_static("digest"),
        HeaderName::from_static("content-digest"),
        HeaderName::from_static("repr-digest"),
        HeaderName::from_static("etag"),
        HeaderName::from_static("last-modified"),
    ] {
        headers.remove(name);
    }
}
