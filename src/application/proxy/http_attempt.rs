//! Materializes one HTTP candidate from the same policy-filtered client input on every attempt.

use std::{io, time::Instant};

use axum::http::{
    HeaderMap, HeaderValue, Method, Uri,
    header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, RANGE},
};
use bytes::Bytes;

use super::{
    CompletionGuard, ProxyError, RequestOutcome,
    candidate::{self, JsonPreparationError},
    forward_request_headers, proxy_error_with_source,
};
use crate::{
    application::{
        ConnectorAttemptError,
        connector::PreparedUpstreamAttempt,
        request_body::{ImageEditBodyError, PreparedRequestBody, ReplayableRequestBody},
    },
    domain::{
        ApiOperation, CompiledChannel, RequestCompression, RequestProtocol, UpstreamTimeoutDefaults,
    },
    transforms::TransformApplyError,
    upstream::{
        ResolvedUpstreamPolicy, ResolvedUpstreamTimeouts, UPSTREAM_ACCEPT_ENCODING,
        UpstreamClientRegistry,
    },
};

pub(super) struct HttpRequestInput<'a> {
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub headers: &'a HeaderMap,
    pub body: &'a PreparedRequestBody,
    pub body_changed: bool,
    pub client_model: &'a str,
    pub operation: ApiOperation,
    pub protocol: RequestProtocol,
}

pub(super) struct PreparedHttpRequest {
    pub request: reqwest::RequestBuilder,
    pub timeouts: ResolvedUpstreamTimeouts,
}

impl HttpRequestInput<'_> {
    pub(super) async fn build_for(
        &self,
        channel: &CompiledChannel,
        upstream_model: &str,
        connector: &PreparedUpstreamAttempt,
        clients: &UpstreamClientRegistry,
        defaults: UpstreamTimeoutDefaults,
    ) -> Result<PreparedHttpRequest, PreparationError> {
        let policy = channel.upstream_policy();
        let transforms = policy.effective_transforms();
        if self.operation == ApiOperation::StandaloneWebSearch
            && !transforms.request_json().is_empty()
        {
            return Err(PreparationError::Request(
                ProxyError::invalid_request_with_code(
                    "Standalone web search does not support request JSON transforms.",
                    "body",
                    "standalone_web_search_json_transform_unsupported",
                ),
            ));
        }
        let body = match self.body {
            PreparedRequestBody::Json(bytes) => {
                let body = candidate::prepare_json(
                    bytes,
                    self.client_model,
                    upstream_model,
                    transforms.request_json(),
                    connector,
                    self.protocol,
                )
                .map_err(|error| match error {
                    JsonPreparationError::Model(_) => PreparationError::Request(
                        ProxyError::image_edit_body(ImageEditBodyError::InvalidJson),
                    ),
                    JsonPreparationError::Transform => {
                        PreparationError::Request(ProxyError::transform_failed())
                    }
                    JsonPreparationError::Connector(error) => PreparationError::AdaptBody(error),
                })?;
                ReplayableRequestBody::Memory(body)
            }
            PreparedRequestBody::ImageEdit(edit) => {
                let mut edit = edit.clone();
                if self.client_model != upstream_model {
                    edit = edit.rewrite_model(upstream_model).map_err(|error| {
                        PreparationError::Request(ProxyError::image_edit_body(error))
                    })?;
                }
                if !transforms.request_json().is_empty() {
                    return Err(PreparationError::Request(ProxyError::image_edit_body(
                        ImageEditBodyError::JsonTransformUnsupported,
                    )));
                }
                connector
                    .adapt_image_edit(edit)
                    .await
                    .map_err(PreparationError::AdaptBody)?
            }
        };
        let (body, encoded) =
            encode_upstream_request_body(body, channel.request_compression(), self.operation)
                .map_err(|source| {
                    tracing::error!(
                        event = "upstream_request_compression_failed",
                        channel_id = %channel.id(),
                        error = %source,
                        "failed to compress upstream request body"
                    );
                    PreparationError::Upstream(ProxyError::upstream_request_encoding_failed())
                })?;

        let body_changed = self.body_changed
            || self.client_model != upstream_model
            || !transforms.request_json().is_empty()
            || connector.changes_request_body()
            || encoded;
        let headers =
            candidate::transform_headers(self.headers, body_changed, transforms.request_headers())
                .map_err(PreparationError::Headers)?;
        let mut headers = forward_request_headers(&headers);
        let target = connector
            .upstream_url(channel, self.uri)
            .map_err(PreparationError::connector)?;
        connector
            .inject_headers(&mut headers, channel, self.protocol)
            .map_err(PreparationError::connector)?;
        if encoded {
            headers.insert(CONTENT_ENCODING, HeaderValue::from_static("zstd"));
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        } else {
            headers.remove(CONTENT_ENCODING);
        }
        headers.insert(
            ACCEPT_ENCODING,
            if headers.contains_key(RANGE) {
                HeaderValue::from_static("identity")
            } else {
                HeaderValue::from_static(UPSTREAM_ACCEPT_ENCODING)
            },
        );

        let resolved =
            ResolvedUpstreamPolicy::try_resolve_for_operation(self.operation, &defaults, policy)
                .map_err(|_| PreparationError::Upstream(ProxyError::upstream_unavailable()))?;
        let client = clients
            .client_for(policy, resolved)
            .map_err(|_| PreparationError::Upstream(ProxyError::upstream_unavailable()))?;
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&body.len().to_string())
                .expect("request body length is a valid header value"),
        );
        let body = body
            .reqwest_body()
            .await
            .map_err(PreparationError::OpenBody)?;
        Ok(PreparedHttpRequest {
            request: client
                .request(self.method.clone(), target)
                .headers(headers)
                .body(body),
            timeouts: resolved.timeouts(),
        })
    }
}

pub(super) enum PreparationError {
    Request(ProxyError),
    Headers(TransformApplyError),
    AdaptBody(ConnectorAttemptError),
    Upstream(ProxyError),
    OpenBody(ImageEditBodyError),
}

impl PreparationError {
    fn connector(error: ConnectorAttemptError) -> Self {
        Self::Upstream(ProxyError::connector_attempt(error))
    }

    pub(super) fn finish(self, completion: &mut CompletionGuard) -> ProxyError {
        let (outcome, error) = match self {
            Self::Request(error) => (RequestOutcome::ClientRequestError, error),
            Self::Headers(source) => {
                let error = ProxyError::transform_failed();
                completion.finish_with_error_details(
                    RequestOutcome::ClientRequestError,
                    proxy_error_with_source(&error, &source),
                );
                return error;
            }
            Self::AdaptBody(error) => {
                let error = ProxyError::connector_attempt(error);
                completion.set_client_visible_status(error.status.as_u16());
                let outcome = if error.status.is_client_error() {
                    RequestOutcome::ClientRequestError
                } else {
                    RequestOutcome::UpstreamUnavailable
                };
                (outcome, error)
            }
            Self::Upstream(error) => (RequestOutcome::UpstreamUnavailable, error),
            Self::OpenBody(error) => {
                let error = ProxyError::image_edit_body(error);
                completion.set_client_visible_status(error.status.as_u16());
                (RequestOutcome::UpstreamUnavailable, error)
            }
        };
        completion.finish_with_proxy_error(outcome, &error);
        error
    }
}

fn encode_upstream_request_body(
    body: ReplayableRequestBody,
    compression: RequestCompression,
    api_operation: ApiOperation,
) -> io::Result<(ReplayableRequestBody, bool)> {
    if compression == RequestCompression::Default {
        return Ok((body, false));
    }
    if api_operation != ApiOperation::Responses {
        return Ok((body, false));
    }
    let ReplayableRequestBody::Memory(body) = body else {
        return Err(io::Error::other(
            "Responses JSON request body unexpectedly used file storage",
        ));
    };
    let pre_compression_bytes = body.len();
    let compression_start = Instant::now();
    let body = zstd::stream::encode_all(std::io::Cursor::new(body), 3).map(Bytes::from)?;
    tracing::debug!(
        pre_compression_bytes,
        post_compression_bytes = body.len(),
        compression_duration_ms = compression_start.elapsed().as_millis(),
        "compressed upstream Responses request body with zstd"
    );
    Ok((ReplayableRequestBody::Memory(body), true))
}
