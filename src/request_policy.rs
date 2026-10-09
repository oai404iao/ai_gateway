//! Public request allowlists and provider-independent outbound Header safety.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::LazyLock,
};

use axum::http::{HeaderMap, HeaderName, header::CONNECTION};
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, value::RawValue};

use crate::domain::ApiOperation;

const CONTRACT_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/docs/reference/request-allowlists.json"
));

static CONTRACT: LazyLock<RequestPolicyContract> = LazyLock::new(|| {
    let contract = serde_json::from_str::<RequestPolicyContract>(CONTRACT_JSON)
        .expect("request allowlist contract must be valid JSON");
    validate_contract(&contract).expect("request allowlist contract must be internally consistent");
    contract
});

const REQUIRED_INTERFACES: [RequestInterface; 6] = [
    RequestInterface::ChatCompletions,
    RequestInterface::ResponsesHttp,
    RequestInterface::ResponsesWebSocket,
    RequestInterface::StandaloneWebSearch,
    RequestInterface::ImagesGeneration,
    RequestInterface::ImagesEdit,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestInterface {
    ChatCompletions,
    ResponsesHttp,
    ResponsesWebSocket,
    StandaloneWebSearch,
    ImagesGeneration,
    ImagesEdit,
}

impl RequestInterface {
    #[must_use]
    pub(crate) const fn for_http(api_operation: ApiOperation) -> Self {
        match api_operation {
            ApiOperation::ChatCompletions => Self::ChatCompletions,
            ApiOperation::Responses => Self::ResponsesHttp,
            ApiOperation::ResponsesWebSocket => Self::ResponsesWebSocket,
            ApiOperation::StandaloneWebSearch => Self::StandaloneWebSearch,
            ApiOperation::ImagesGeneration => Self::ImagesGeneration,
            ApiOperation::ImagesEdit => Self::ImagesEdit,
        }
    }

    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::ResponsesHttp => "responses_http",
            Self::ResponsesWebSocket => "responses_websocket",
            Self::StandaloneWebSearch => "standalone_web_search",
            Self::ImagesGeneration => "images_generation",
            Self::ImagesEdit => "images_edit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestPolicyLayer {
    Client,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestPolicyLocation {
    Header,
    Body,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestPolicyFailure {
    InvalidBody,
    UnsupportedField,
    UnsupportedValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RequestPolicyError {
    connector_code: Option<&'static str>,
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    location: RequestPolicyLocation,
    failure: RequestPolicyFailure,
    field: Option<String>,
}

impl RequestPolicyError {
    fn invalid_body(layer: RequestPolicyLayer, interface: RequestInterface) -> Self {
        Self {
            connector_code: None,
            layer,
            interface,
            location: RequestPolicyLocation::Body,
            failure: RequestPolicyFailure::InvalidBody,
            field: None,
        }
    }

    fn field(
        layer: RequestPolicyLayer,
        interface: RequestInterface,
        location: RequestPolicyLocation,
        failure: RequestPolicyFailure,
        field: &str,
    ) -> Self {
        Self {
            connector_code: None,
            layer,
            interface,
            location,
            failure,
            field: Some(bounded_field_name(field)),
        }
    }

    pub(crate) fn connector_body_rejection(
        interface: RequestInterface,
        code: &'static str,
    ) -> Self {
        Self {
            connector_code: Some(code),
            layer: RequestPolicyLayer::Client,
            interface,
            location: RequestPolicyLocation::Body,
            failure: RequestPolicyFailure::UnsupportedField,
            field: None,
        }
    }

    #[must_use]
    pub(crate) fn message(&self) -> String {
        if self.connector_code.is_some() {
            return "The selected connector rejected an unsupported body field or value.".into();
        }
        if self.failure == RequestPolicyFailure::InvalidBody {
            return "Request body must be a JSON object.".to_owned();
        }
        let layer = match self.layer {
            RequestPolicyLayer::Client => "client request",
        };
        let location = match self.location {
            RequestPolicyLocation::Header => "header",
            RequestPolicyLocation::Body => "body field",
        };
        let field = self.field.as_deref().unwrap_or("unknown");
        let reason = match self.failure {
            RequestPolicyFailure::UnsupportedField => "is not supported",
            RequestPolicyFailure::UnsupportedValue => "has an unsupported value",
            RequestPolicyFailure::InvalidBody => unreachable!(),
        };
        format!(
            "The {layer} {location} `{field}` {reason} for `{}`.",
            self.interface.as_str()
        )
    }

    #[must_use]
    pub(crate) const fn param(&self) -> &'static str {
        match self.location {
            RequestPolicyLocation::Header => "headers",
            RequestPolicyLocation::Body => "body",
        }
    }

    #[must_use]
    pub(crate) const fn code(&self) -> &'static str {
        if let Some(code) = self.connector_code {
            return code;
        }
        match (self.layer, self.location, self.failure) {
            (_, _, RequestPolicyFailure::InvalidBody) => "invalid_request",
            (
                RequestPolicyLayer::Client,
                RequestPolicyLocation::Header,
                RequestPolicyFailure::UnsupportedField,
            ) => "request_header_unsupported",
            (
                RequestPolicyLayer::Client,
                RequestPolicyLocation::Header,
                RequestPolicyFailure::UnsupportedValue,
            ) => "request_header_value_unsupported",
            (
                RequestPolicyLayer::Client,
                RequestPolicyLocation::Body,
                RequestPolicyFailure::UnsupportedField,
            ) => "request_body_field_unsupported",
            (
                RequestPolicyLayer::Client,
                RequestPolicyLocation::Body,
                RequestPolicyFailure::UnsupportedValue,
            ) => "request_body_field_value_unsupported",
        }
    }
}

fn bounded_field_name(field: &str) -> String {
    const MAX_FIELD_CHARS: usize = 128;
    let mut bounded = field.chars().take(MAX_FIELD_CHARS).collect::<String>();
    if field.chars().count() > MAX_FIELD_CHARS {
        bounded.push('…');
    }
    bounded
}

#[derive(Debug)]
pub(crate) struct AppliedJsonBody {
    pub(crate) body: Bytes,
    pub(crate) changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldDisposition {
    Allow,
    Ignore,
}

pub(crate) fn apply_json_body_policy(
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    body: Bytes,
) -> Result<AppliedJsonBody, RequestPolicyError> {
    let policy = body_policy(layer, interface);
    let raw_fields = serde_json::from_slice::<BTreeMap<String, &RawValue>>(&body)
        .map_err(|_| RequestPolicyError::invalid_body(layer, interface))?;
    let mut ignored_fields = Vec::new();
    let mut changed = false;
    for (field, raw_value) in &raw_fields {
        let inspected_value = policy
            .ignore
            .get(field)
            .and_then(|rule| rule.accepted_values.as_ref())
            .map(|_| {
                serde_json::from_str::<Value>(raw_value.get())
                    .map_err(|_| RequestPolicyError::invalid_body(layer, interface))
            })
            .transpose()?;
        let disposition = field_disposition_for_policy(
            layer,
            interface,
            policy,
            field,
            inspected_value.as_ref(),
        )?;
        if disposition == FieldDisposition::Ignore {
            ignored_fields.push(field.clone());
            changed = true;
        }
    }
    if !changed {
        return Ok(AppliedJsonBody {
            body,
            changed: false,
        });
    }
    let mut value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| RequestPolicyError::invalid_body(layer, interface))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RequestPolicyError::invalid_body(layer, interface))?;
    for field in ignored_fields {
        object.remove(&field);
    }
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| RequestPolicyError::invalid_body(layer, interface))?;
    Ok(AppliedJsonBody {
        body,
        changed: true,
    })
}

pub(crate) fn apply_client_fast_mode_filter(
    interface: RequestInterface,
    body: Bytes,
    enabled: bool,
) -> Result<AppliedJsonBody, RequestPolicyError> {
    if !enabled {
        return Ok(AppliedJsonBody {
            body,
            changed: false,
        });
    }
    let mut value = serde_json::from_slice::<Value>(&body)
        .map_err(|_| RequestPolicyError::invalid_body(RequestPolicyLayer::Client, interface))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| RequestPolicyError::invalid_body(RequestPolicyLayer::Client, interface))?;
    if object.remove("service_tier").is_none() {
        return Ok(AppliedJsonBody {
            body,
            changed: false,
        });
    }
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| RequestPolicyError::invalid_body(RequestPolicyLayer::Client, interface))?;
    Ok(AppliedJsonBody {
        body,
        changed: true,
    })
}

pub(crate) fn body_field_disposition(
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    field: &str,
    value: Option<&Value>,
) -> Result<FieldDisposition, RequestPolicyError> {
    field_disposition_for_policy(
        layer,
        interface,
        body_policy(layer, interface),
        field,
        value,
    )
}

fn field_disposition_for_policy(
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    policy: &BodyPolicy,
    field: &str,
    value: Option<&Value>,
) -> Result<FieldDisposition, RequestPolicyError> {
    if contains_sorted(&policy.allow, field) {
        return Ok(FieldDisposition::Allow);
    }
    if let Some(rule) = policy.ignore.get(field) {
        if rule.accepted_values.as_ref().is_some_and(|accepted| {
            value.is_none_or(|value| !accepted.iter().any(|candidate| candidate == value))
        }) {
            return Err(RequestPolicyError::field(
                layer,
                interface,
                RequestPolicyLocation::Body,
                RequestPolicyFailure::UnsupportedValue,
                field,
            ));
        }
        return Ok(FieldDisposition::Ignore);
    }
    if contains_sorted(&policy.reject, field) {
        return Err(RequestPolicyError::field(
            layer,
            interface,
            RequestPolicyLocation::Body,
            RequestPolicyFailure::UnsupportedField,
            field,
        ));
    }
    match policy.unknown {
        UnknownAction::Allow => Ok(FieldDisposition::Allow),
        UnknownAction::Ignore => Ok(FieldDisposition::Ignore),
        UnknownAction::Reject => Err(RequestPolicyError::field(
            layer,
            interface,
            RequestPolicyLocation::Body,
            RequestPolicyFailure::UnsupportedField,
            field,
        )),
    }
}

pub(crate) fn filter_client_headers(
    interface: RequestInterface,
    headers: &HeaderMap,
) -> Result<HeaderMap, RequestPolicyError> {
    let connection_names = connection_header_names(headers);
    filter_headers(
        RequestPolicyLayer::Client,
        interface,
        &contract().client_headers,
        headers,
        Some(&connection_names),
    )
}

#[must_use]
pub(crate) fn client_header_allowed(name: &HeaderName) -> bool {
    header_action(&contract().client_headers, name) == UnknownAction::Allow
}

#[must_use]
pub(crate) fn client_header_explicitly_ignored(name: &HeaderName) -> bool {
    contains_sorted(&contract().client_headers.ignore, name.as_str())
}

pub(crate) fn header_is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

pub(crate) fn request_header_is_protected(name: &str) -> bool {
    header_is_hop_by_hop(name)
        || matches!(
            name,
            "host"
                | "content-length"
                | "content-encoding"
                | "authorization"
                | "cookie"
                | "accept-encoding"
        )
}

pub(crate) fn connector_header_is_forbidden(name: &HeaderName) -> bool {
    (name != axum::http::header::AUTHORIZATION && request_header_is_protected(name.as_str()))
        || name.as_str().starts_with("sec-websocket-")
        || client_header_explicitly_ignored(name)
}

/// Run after connector authentication, before host compression or handshake headers.
pub(crate) fn sanitize_outbound_request_headers(headers: &mut HeaderMap) {
    let connection_names = connection_header_names(headers);
    let removed = headers
        .keys()
        .filter(|name| {
            // Ingress, transforms and plugins cannot supply Cookie; explicit host credentials can.
            (**name != axum::http::header::COOKIE && connector_header_is_forbidden(name))
                || (**name != axum::http::header::AUTHORIZATION && connection_names.contains(*name))
        })
        .cloned()
        .collect::<Vec<_>>();
    for name in removed {
        headers.remove(name);
    }
}

fn filter_headers(
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    policy: &HeaderPolicy,
    headers: &HeaderMap,
    internal_connection_names: Option<&HashSet<HeaderName>>,
) -> Result<HeaderMap, RequestPolicyError> {
    let mut filtered = HeaderMap::new();
    for (name, value) in headers {
        let action = header_action(policy, name);
        let internal_connection_header = internal_connection_names
            .is_some_and(|names| names.contains(name) || *name == CONNECTION);
        match action {
            UnknownAction::Allow => {
                filtered.append(name.clone(), value.clone());
            }
            UnknownAction::Ignore if internal_connection_header => {
                filtered.append(name.clone(), value.clone());
            }
            UnknownAction::Ignore => {}
            UnknownAction::Reject => {
                return Err(RequestPolicyError::field(
                    layer,
                    interface,
                    RequestPolicyLocation::Header,
                    RequestPolicyFailure::UnsupportedField,
                    name.as_str(),
                ));
            }
        }
    }
    Ok(filtered)
}

fn header_action(policy: &HeaderPolicy, name: &HeaderName) -> UnknownAction {
    let name = name.as_str();
    if contains_sorted(&policy.allow, name) {
        UnknownAction::Allow
    } else if contains_sorted(&policy.ignore, name) {
        UnknownAction::Ignore
    } else if policy
        .allow_prefixes
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        UnknownAction::Allow
    } else {
        policy.unknown
    }
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|name| HeaderName::from_bytes(name.trim().as_bytes()).ok())
        .collect()
}

fn body_policy(_: RequestPolicyLayer, interface: RequestInterface) -> &'static BodyPolicy {
    &interface_policy(interface).client_body
}

fn interface_policy(interface: RequestInterface) -> &'static InterfacePolicy {
    contract()
        .interfaces
        .get(interface.as_str())
        .expect("required request interface policy must exist")
}

fn contract() -> &'static RequestPolicyContract {
    &CONTRACT
}

fn contains_sorted(values: &[String], candidate: &str) -> bool {
    values
        .binary_search_by(|value| value.as_str().cmp(candidate))
        .is_ok()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestPolicyContract {
    version: u32,
    verified_at: String,
    sources: RequestPolicySources,
    client_headers: HeaderPolicy,
    interfaces: BTreeMap<String, InterfacePolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestPolicySources {
    openai_node_commit: String,
    codex_commit: String,
    codex_standalone_commit: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfacePolicy {
    client_body: BodyPolicy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeaderPolicy {
    unknown: UnknownAction,
    allow: Vec<String>,
    allow_prefixes: Vec<String>,
    ignore: Vec<String>,
    generated: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyPolicy {
    unknown: UnknownAction,
    allow: Vec<String>,
    ignore: BTreeMap<String, IgnoredFieldRule>,
    reject: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IgnoredFieldRule {
    #[serde(default)]
    accepted_values: Option<Vec<Value>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum UnknownAction {
    Allow,
    Ignore,
    Reject,
}

fn validate_contract(contract: &RequestPolicyContract) -> Result<(), String> {
    if contract.version != 4 {
        return Err("request allowlist contract version must be 4".into());
    }
    let verified = contract.verified_at.as_bytes();
    if verified.len() != 10
        || verified[4] != b'-'
        || verified[7] != b'-'
        || verified
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err("request allowlist verification date must use YYYY-MM-DD".into());
    }
    for commit in [
        &contract.sources.openai_node_commit,
        &contract.sources.codex_commit,
        &contract.sources.codex_standalone_commit,
    ] {
        if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("request allowlist source commits must be full Git SHAs".into());
        }
    }
    validate_header_policy(&contract.client_headers)?;
    if contract.client_headers.unknown != UnknownAction::Ignore {
        return Err("unknown client headers must be ignored".into());
    }
    if contract.interfaces.len() != REQUIRED_INTERFACES.len() {
        return Err("request allowlist contract must define exactly six interfaces".into());
    }
    for interface in REQUIRED_INTERFACES {
        let policy = contract
            .interfaces
            .get(interface.as_str())
            .ok_or_else(|| format!("missing request interface `{}`", interface.as_str()))?;
        validate_body_policy(&policy.client_body)?;
        if policy.client_body.unknown != UnknownAction::Reject {
            return Err(format!(
                "unknown client body fields must be rejected for `{}`",
                interface.as_str()
            ));
        }
        if !contains_sorted(&policy.client_body.allow, "model") {
            return Err(format!(
                "client body policy must allow `model` for `{}`",
                interface.as_str()
            ));
        }
        if interface == RequestInterface::ResponsesWebSocket
            && !contains_sorted(&policy.client_body.allow, "type")
        {
            return Err("Responses WebSocket client policy must allow `type`".into());
        }
        if interface == RequestInterface::ImagesEdit
            && (!contains_sorted(&policy.client_body.allow, "image")
                || !contains_sorted(&policy.client_body.allow, "image[]")
                || !contains_sorted(&policy.client_body.allow, "mask"))
        {
            return Err("Images edit client policy must allow image aliases and `mask`".into());
        }
    }
    Ok(())
}

fn validate_header_policy(policy: &HeaderPolicy) -> Result<(), String> {
    validate_sorted_unique("allowed headers", &policy.allow)?;
    validate_sorted_unique("allowed header prefixes", &policy.allow_prefixes)?;
    validate_sorted_unique("ignored headers", &policy.ignore)?;
    validate_sorted_unique("generated headers", &policy.generated)?;
    if let Some(name) = policy
        .allow
        .iter()
        .find(|name| contains_sorted(&policy.ignore, name))
    {
        return Err(format!(
            "request policy header `{name}` has multiple policy actions"
        ));
    }
    for name in policy
        .allow
        .iter()
        .chain(&policy.ignore)
        .chain(&policy.generated)
    {
        let parsed = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid request policy header `{name}`"))?;
        if parsed.as_str() != name {
            return Err(format!("request policy header `{name}` must be lowercase"));
        }
    }
    for prefix in &policy.allow_prefixes {
        if prefix.is_empty()
            || prefix.bytes().any(|byte| {
                !matches!(
                    byte,
                    b'a'..=b'z' | b'0'..=b'9' | b'!' | b'#'..=b'\'' | b'*' | b'+' | b'-' | b'.'
                        | b'^' | b'_' | b'`' | b'|' | b'~'
                )
            })
        {
            return Err(format!("invalid request policy header prefix `{prefix}`"));
        }
    }
    Ok(())
}

fn validate_body_policy(policy: &BodyPolicy) -> Result<(), String> {
    validate_sorted_unique("allowed body fields", &policy.allow)?;
    validate_sorted_unique("rejected body fields", &policy.reject)?;
    let mut seen = BTreeSet::new();
    for field in &policy.allow {
        seen.insert(field);
    }
    for field in policy.ignore.keys() {
        if !seen.insert(field) {
            return Err(format!(
                "request body field `{field}` has multiple policy actions"
            ));
        }
    }
    for field in &policy.reject {
        if !seen.insert(field) {
            return Err(format!(
                "request body field `{field}` has multiple policy actions"
            ));
        }
    }
    if seen.iter().any(|field| field.is_empty()) {
        return Err("request body field names must not be empty".into());
    }
    Ok(())
}

fn validate_sorted_unique(label: &str, values: &[String]) -> Result<(), String> {
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(format!("{label} must be sorted and unique"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn embedded_contract_is_complete_and_current() {
        assert_eq!(contract().version, 4);
        assert_eq!(contract().verified_at.len(), 10);
        assert_eq!(contract().sources.openai_node_commit.len(), 40);
        assert_eq!(contract().sources.codex_commit.len(), 40);
        for interface in REQUIRED_INTERFACES {
            assert!(contract().interfaces.contains_key(interface.as_str()));
        }
    }

    #[test]
    fn client_json_policy_preserves_chat_thinking_extensions_and_rejects_unknown_fields() {
        let original = Bytes::from_static(
            br#"{ "model" : "deepseek-chat", "messages" : [], "thinking" : {"type":"enabled"}, "enable_thinking" : true, "stream" : false }"#,
        );
        let allowed = apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::ChatCompletions,
            original.clone(),
        )
        .unwrap();
        assert!(!allowed.changed);
        assert_eq!(allowed.body, original);

        let error = apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::ChatCompletions,
            Bytes::from_static(br#"{"model":"gpt-5","messages":[],"future_field":true}"#),
        )
        .unwrap_err();
        assert_eq!(error.code(), "request_body_field_unsupported");
        assert!(error.message().contains("future_field"));
    }

    #[test]
    fn responses_http_client_policy_preserves_codex_client_metadata() {
        let original = Bytes::from_static(
            br#"{"model":"gpt-5-codex","input":[],"client_metadata":{"session_id":"session","thread_id":"thread"},"stream":true,"store":false}"#,
        );
        let allowed = apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::ResponsesHttp,
            original.clone(),
        )
        .unwrap();
        assert!(!allowed.changed);
        assert_eq!(allowed.body, original);
    }

    #[test]
    fn standalone_web_search_policy_preserves_current_codex_fields_and_rejects_unknowns() {
        let original = Bytes::from_static(
            br#"{"id":"session-123","model":"gpt-5-codex","reasoning":{"effort":"medium"},"input":"find it","commands":{"search_query":[{"q":"example"}]},"settings":{"external_web_access":true},"max_output_tokens":300}"#,
        );
        let client = apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::StandaloneWebSearch,
            original.clone(),
        )
        .unwrap();
        assert!(!client.changed);
        assert_eq!(client.body, original);

        let error = apply_json_body_policy(
            RequestPolicyLayer::Client,
            RequestInterface::StandaloneWebSearch,
            Bytes::from_static(
                br#"{"id":"session-123","model":"gpt-5-codex","future_field":true}"#,
            ),
        )
        .unwrap_err();
        assert_eq!(error.code(), "request_body_field_unsupported");
        assert!(error.message().contains("future_field"));
    }

    #[test]
    fn image_turn_header_survives_public_images_policy() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-image-turn-id",
            HeaderValue::from_static("caller-image-turn"),
        );
        for interface in REQUIRED_INTERFACES {
            let client = filter_client_headers(interface, &headers).unwrap();
            assert_eq!(client["x-codex-image-turn-id"], "caller-image-turn");
            if interface == RequestInterface::ChatCompletions {
                continue;
            }
        }
    }

    #[test]
    fn every_client_interface_rejects_unknown_top_level_body_fields() {
        for interface in REQUIRED_INTERFACES {
            let error = if interface == RequestInterface::ImagesEdit {
                body_field_disposition(
                    RequestPolicyLayer::Client,
                    interface,
                    "future_field",
                    Some(&Value::String("value".into())),
                )
                .unwrap_err()
            } else {
                apply_json_body_policy(
                    RequestPolicyLayer::Client,
                    interface,
                    Bytes::from_static(br#"{"future_field":true}"#),
                )
                .unwrap_err()
            };
            assert_eq!(
                error.code(),
                "request_body_field_unsupported",
                "{}",
                interface.as_str()
            );
        }
    }

    #[test]
    fn client_multipart_policy_ignores_only_the_compatible_moderation_default() {
        let value = Value::String("auto".into());
        assert_eq!(
            body_field_disposition(
                RequestPolicyLayer::Client,
                RequestInterface::ImagesEdit,
                "moderation",
                Some(&value),
            )
            .unwrap(),
            FieldDisposition::Ignore
        );
        let value = Value::String("low".into());
        let error = body_field_disposition(
            RequestPolicyLayer::Client,
            RequestInterface::ImagesEdit,
            "moderation",
            Some(&value),
        )
        .unwrap_err();
        assert_eq!(error.code(), "request_body_field_value_unsupported");
    }

    #[test]
    fn header_policy_keeps_only_declared_client_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("x-hop"));
        headers.insert("x-hop", HeaderValue::from_static("internal"));
        headers.insert("forwarded", HeaderValue::from_static("for=192.0.2.1"));
        headers.insert("x-unknown", HeaderValue::from_static("drop"));
        headers.insert("x-stainless-lang", HeaderValue::from_static("rust"));
        headers.insert("traceparent", HeaderValue::from_static("trace"));
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert(
            "x-codex-turn-metadata",
            HeaderValue::from_static(r#"{"search_context_size":"medium"}"#),
        );
        headers.insert(
            "x-codex-beta-features",
            HeaderValue::from_static("remote_compaction_v2"),
        );
        headers.insert(
            "x-codex-routing-hint",
            HeaderValue::from_static("model=gpt-5.6-sol;tier=priority"),
        );
        headers.insert(
            "x-codex-turn-state",
            HeaderValue::from_static("opaque-turn-state"),
        );
        headers.insert(
            "x-openai-internal-codex-responses-lite",
            HeaderValue::from_static("true"),
        );
        headers.insert(
            "x-responsesapi-include-timing-metrics",
            HeaderValue::from_static("true"),
        );

        let client = filter_client_headers(RequestInterface::ResponsesHttp, &headers).unwrap();
        assert!(client.contains_key(CONNECTION));
        assert!(client.contains_key("x-hop"));
        assert!(client.contains_key("x-stainless-lang"));
        assert!(client.contains_key("traceparent"));
        assert!(client.contains_key("originator"));
        assert!(client.contains_key("x-codex-turn-metadata"));
        assert!(client.contains_key("x-codex-beta-features"));
        assert!(client.contains_key("x-codex-routing-hint"));
        assert!(client.contains_key("x-codex-turn-state"));
        assert!(client.contains_key("x-openai-internal-codex-responses-lite"));
        assert!(client.contains_key("x-responsesapi-include-timing-metrics"));
        assert!(!client.contains_key("forwarded"));
        assert!(!client.contains_key("x-unknown"));
    }

    #[test]
    fn common_forwarding_metadata_is_explicitly_ignored_by_client_policy() {
        let mut headers = HeaderMap::new();
        for name in [
            "cf-connecting-ip",
            "cf-connecting-ipv6",
            "cf-ipcountry",
            "cf-pseudo-ipv4",
            "cf-ray",
            "cf-visitor",
            "forwarded",
            "true-client-ip",
            "via",
            "x-client-ip",
            "x-forwarded-for",
            "x-forwarded-host",
            "x-forwarded-port",
            "x-forwarded-proto",
            "x-original-forwarded-for",
            "x-real-ip",
        ] {
            assert!(
                client_header_explicitly_ignored(&HeaderName::from_static(name)),
                "{name} is not explicitly ignored"
            );
            headers.insert(name, HeaderValue::from_static("discard"));
        }
        headers.insert(
            "x-forwarded-custom",
            HeaderValue::from_static("preserve-transform-header"),
        );
        sanitize_outbound_request_headers(&mut headers);
        assert!(headers.get("forwarded").is_none());
        assert!(headers.get("x-forwarded-for").is_none());
        assert!(headers.get("cf-connecting-ip").is_none());
        assert_eq!(
            headers.get("x-forwarded-custom").unwrap(),
            "preserve-transform-header"
        );
        assert!(!client_header_explicitly_ignored(&HeaderName::from_static(
            "x-forwarded-custom"
        )));
    }

    #[test]
    fn final_connector_cleanup_preserves_auth_but_not_transport_or_connection_headers() {
        let mut headers = HeaderMap::new();
        for name in [
            "host",
            "content-length",
            "content-encoding",
            "accept-encoding",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "proxy-connection",
            "te",
            "trailer",
            "transfer-encoding",
            "upgrade",
            "sec-websocket-key",
            "sec-websocket-extensions",
            "forwarded",
            "x-forwarded-for",
        ] {
            let name = HeaderName::from_static(name);
            assert!(connector_header_is_forbidden(&name));
            headers.insert(name, HeaderValue::from_static("discard"));
        }
        headers.append(
            CONNECTION,
            HeaderValue::from_static("x-private-hop, Authorization"),
        );
        assert!(connector_header_is_forbidden(&HeaderName::from_static(
            "cookie"
        )));
        headers.insert("cookie", HeaderValue::from_static("host-credential"));
        headers.append(CONNECTION, HeaderValue::from_static("X-Other-Hop"));
        for name in ["x-private-hop", "x-other-hop"] {
            headers.insert(name, HeaderValue::from_static("discard"));
        }
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer upstream-secret"),
        );
        headers.insert("x-api-key", HeaderValue::from_static("upstream-secret"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        sanitize_outbound_request_headers(&mut headers);
        assert_eq!(headers.len(), 4);
        assert_eq!(headers["cookie"], "host-credential");
        assert_eq!(headers["authorization"], "Bearer upstream-secret");
        assert_eq!(headers["x-api-key"], "upstream-secret");
        assert_eq!(headers["content-type"], "application/json");
        assert!(!connector_header_is_forbidden(&HeaderName::from_static(
            "authorization"
        )));
        assert!(request_header_is_protected("authorization"));
        headers.insert(CONNECTION, HeaderValue::from_static("cookie"));
        sanitize_outbound_request_headers(&mut headers);
        assert!(!headers.contains_key("cookie"));
    }

    #[test]
    fn session_affinity_headers_must_be_part_of_the_client_contract() {
        for name in [
            "session-id",
            "session_id",
            "thread-id",
            "thread_id",
            "x-session-id",
        ] {
            assert!(
                client_header_allowed(&HeaderName::from_static(name)),
                "{name} is not allowed"
            );
        }
        assert!(!client_header_allowed(&HeaderName::from_static(
            "x-private-session"
        )));
    }
}
