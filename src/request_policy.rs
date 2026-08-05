//! Machine-readable client and connector request allowlist enforcement.

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

const REQUIRED_INTERFACES: [RequestInterface; 5] = [
    RequestInterface::ChatCompletions,
    RequestInterface::ResponsesHttp,
    RequestInterface::ResponsesWebSocket,
    RequestInterface::ImagesGeneration,
    RequestInterface::ImagesEdit,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestInterface {
    ChatCompletions,
    ResponsesHttp,
    ResponsesWebSocket,
    ImagesGeneration,
    ImagesEdit,
}

impl RequestInterface {
    #[must_use]
    pub(crate) const fn for_http(api_operation: ApiOperation) -> Self {
        match api_operation {
            ApiOperation::ChatCompletions => Self::ChatCompletions,
            ApiOperation::Responses => Self::ResponsesHttp,
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
            Self::ImagesGeneration => "images_generation",
            Self::ImagesEdit => "images_edit",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RequestPolicyLayer {
    Client,
    CodexOauth,
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
    layer: RequestPolicyLayer,
    interface: RequestInterface,
    location: RequestPolicyLocation,
    failure: RequestPolicyFailure,
    field: Option<String>,
}

impl RequestPolicyError {
    fn invalid_body(layer: RequestPolicyLayer, interface: RequestInterface) -> Self {
        Self {
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
            layer,
            interface,
            location,
            failure,
            field: Some(bounded_field_name(field)),
        }
    }

    #[must_use]
    pub(crate) fn message(&self) -> String {
        if self.failure == RequestPolicyFailure::InvalidBody {
            return "Request body must be a JSON object.".to_owned();
        }
        let layer = match self.layer {
            RequestPolicyLayer::Client => "client request",
            RequestPolicyLayer::CodexOauth => "Codex OAuth upstream request",
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
            (
                RequestPolicyLayer::CodexOauth,
                RequestPolicyLocation::Header,
                RequestPolicyFailure::UnsupportedField,
            ) => "codex_request_header_unsupported",
            (
                RequestPolicyLayer::CodexOauth,
                RequestPolicyLocation::Header,
                RequestPolicyFailure::UnsupportedValue,
            ) => "codex_request_header_value_unsupported",
            (
                RequestPolicyLayer::CodexOauth,
                RequestPolicyLocation::Body,
                RequestPolicyFailure::UnsupportedField,
            ) => "codex_request_body_field_unsupported",
            (
                RequestPolicyLayer::CodexOauth,
                RequestPolicyLocation::Body,
                RequestPolicyFailure::UnsupportedValue,
            ) => "codex_request_body_field_value_unsupported",
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
    if layer == RequestPolicyLayer::CodexOauth {
        for (field, override_value) in &connector_policy(interface).body_overrides {
            let current_value = raw_fields
                .get(field)
                .and_then(|raw| serde_json::from_str::<Value>(raw.get()).ok());
            if current_value.as_ref() != Some(override_value) {
                changed = true;
            }
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
    if layer == RequestPolicyLayer::CodexOauth {
        for (field, override_value) in &connector_policy(interface).body_overrides {
            object.insert(field.clone(), override_value.clone());
        }
    }
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| RequestPolicyError::invalid_body(layer, interface))?;
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

pub(crate) fn filter_codex_headers(
    interface: RequestInterface,
    headers: &HeaderMap,
) -> Result<HeaderMap, RequestPolicyError> {
    filter_headers(
        RequestPolicyLayer::CodexOauth,
        interface,
        &connector_policy(interface).headers,
        headers,
        None,
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

pub(crate) fn strip_explicitly_ignored_client_headers(headers: &mut HeaderMap) {
    for name in &contract().client_headers.ignore {
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

fn body_policy(layer: RequestPolicyLayer, interface: RequestInterface) -> &'static BodyPolicy {
    let policy = interface_policy(interface);
    match layer {
        RequestPolicyLayer::Client => &policy.client_body,
        RequestPolicyLayer::CodexOauth => {
            &policy
                .codex_oauth
                .as_ref()
                .expect("Codex request policy must exist for a supported interface")
                .body
        }
    }
}

fn connector_policy(interface: RequestInterface) -> &'static ConnectorPolicy {
    interface_policy(interface)
        .codex_oauth
        .as_ref()
        .expect("Codex request policy must exist for a supported interface")
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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterfacePolicy {
    client_body: BodyPolicy,
    #[serde(default)]
    codex_oauth: Option<ConnectorPolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorPolicy {
    headers: HeaderPolicy,
    body: BodyPolicy,
    body_overrides: BTreeMap<String, Value>,
    generated_body_fields: Vec<String>,
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
    if contract.version != 1 {
        return Err("request allowlist contract version must be 1".into());
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
        return Err("request allowlist contract must define exactly five interfaces".into());
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
        if interface == RequestInterface::ChatCompletions {
            if policy.codex_oauth.is_some() {
                return Err("Chat Completions must not define a Codex policy".into());
            }
            continue;
        }
        let codex = policy
            .codex_oauth
            .as_ref()
            .ok_or_else(|| format!("missing Codex policy for `{}`", interface.as_str()))?;
        validate_header_policy(&codex.headers)?;
        validate_body_policy(&codex.body)?;
        validate_sorted_unique("generated body fields", &codex.generated_body_fields)?;
        if codex.headers.unknown != UnknownAction::Ignore {
            return Err(format!(
                "unknown Codex headers must be ignored for `{}`",
                interface.as_str()
            ));
        }
        if let Some(name) = codex
            .headers
            .generated
            .iter()
            .find(|name| contains_sorted(&contract.client_headers.ignore, name))
        {
            return Err(format!(
                "Codex generated header `{name}` conflicts with the client ignore policy for `{}`",
                interface.as_str()
            ));
        }
        if codex.body.unknown != UnknownAction::Reject {
            return Err(format!(
                "unknown Codex body fields must be rejected for `{}`",
                interface.as_str()
            ));
        }
        if !contains_sorted(&codex.body.allow, "model") {
            return Err(format!(
                "Codex body policy must allow `model` for `{}`",
                interface.as_str()
            ));
        }
        if interface == RequestInterface::ImagesEdit
            && (!contains_sorted(&codex.body.allow, "image")
                || !contains_sorted(&codex.body.allow, "image[]")
                || !contains_sorted(&codex.body.reject, "mask"))
        {
            return Err(
                "Codex Images edit policy must allow image aliases and reject `mask`".into(),
            );
        }
        let client_fields = classified_body_fields(&policy.client_body);
        let codex_fields = classified_body_fields(&codex.body);
        if let Some(field) = client_fields.difference(&codex_fields).next() {
            return Err(format!(
                "client field `{field}` lacks an explicit Codex action for `{}`",
                interface.as_str()
            ));
        }
        for field in codex.body_overrides.keys() {
            if !contains_sorted(&codex.body.allow, field) {
                return Err(format!(
                    "Codex body override `{field}` must also be allowed for `{}`",
                    interface.as_str()
                ));
            }
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

fn classified_body_fields(policy: &BodyPolicy) -> BTreeSet<&str> {
    policy
        .allow
        .iter()
        .map(String::as_str)
        .chain(policy.ignore.keys().map(String::as_str))
        .chain(policy.reject.iter().map(String::as_str))
        .collect()
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
    use serde_json::json;

    use super::*;

    #[test]
    fn embedded_contract_is_complete_and_current() {
        assert_eq!(contract().version, 1);
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
    fn codex_json_policy_uses_explicit_ignore_reject_and_override_rules() {
        let applied = apply_json_body_policy(
            RequestPolicyLayer::CodexOauth,
            RequestInterface::ImagesGeneration,
            Bytes::from_static(
                br#"{"model":"gpt-image-2","prompt":"x","output_format":"png","moderation":"auto","user":"u"}"#,
            ),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&applied.body).unwrap();
        assert_eq!(value, json!({"model":"gpt-image-2","prompt":"x"}));

        let error = apply_json_body_policy(
            RequestPolicyLayer::CodexOauth,
            RequestInterface::ImagesGeneration,
            Bytes::from_static(br#"{"model":"gpt-image-2","prompt":"x","output_format":"jpeg"}"#),
        )
        .unwrap_err();
        assert_eq!(error.code(), "codex_request_body_field_value_unsupported");

        let applied = apply_json_body_policy(
            RequestPolicyLayer::CodexOauth,
            RequestInterface::ResponsesHttp,
            Bytes::from_static(
                br#"{"model":"gpt-5-codex","input":[],"max_output_tokens":1,"stream":true,"store":true}"#,
            ),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&applied.body).unwrap();
        assert!(value.get("max_output_tokens").is_none());
        assert_eq!(value["stream"], true);
        assert_eq!(value["store"], false);

        let error = apply_json_body_policy(
            RequestPolicyLayer::CodexOauth,
            RequestInterface::ResponsesHttp,
            Bytes::from_static(
                br#"{"model":"gpt-5-codex","input":[],"stream":true,"previous_response_id":"resp_1"}"#,
            ),
        )
        .unwrap_err();
        assert_eq!(error.code(), "codex_request_body_field_value_unsupported");

        let error = apply_json_body_policy(
            RequestPolicyLayer::CodexOauth,
            RequestInterface::ResponsesHttp,
            Bytes::from_static(
                br#"{"model":"gpt-5-codex","input":[],"stream":true,"future_field":true}"#,
            ),
        )
        .unwrap_err();
        assert_eq!(error.code(), "codex_request_body_field_unsupported");
    }

    #[test]
    fn header_policies_keep_only_declared_client_and_codex_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(CONNECTION, HeaderValue::from_static("x-hop"));
        headers.insert("x-hop", HeaderValue::from_static("internal"));
        headers.insert("forwarded", HeaderValue::from_static("for=192.0.2.1"));
        headers.insert("x-unknown", HeaderValue::from_static("drop"));
        headers.insert("x-stainless-lang", HeaderValue::from_static("rust"));
        headers.insert("traceparent", HeaderValue::from_static("trace"));

        let client = filter_client_headers(RequestInterface::ResponsesHttp, &headers).unwrap();
        assert!(client.contains_key(CONNECTION));
        assert!(client.contains_key("x-hop"));
        assert!(client.contains_key("x-stainless-lang"));
        assert!(client.contains_key("traceparent"));
        assert!(!client.contains_key("forwarded"));
        assert!(!client.contains_key("x-unknown"));

        let codex = filter_codex_headers(RequestInterface::ResponsesHttp, &client).unwrap();
        assert!(codex.contains_key("traceparent"));
        assert!(!codex.contains_key("x-hop"));
        assert!(!codex.contains_key("x-stainless-lang"));
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
        strip_explicitly_ignored_client_headers(&mut headers);
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
