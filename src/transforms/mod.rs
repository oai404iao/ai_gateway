//! Compilation of the closed version-one transform DSL.
//!
//! This module deliberately contains plans only. Applying a plan belongs to the
//! data plane and is introduced separately so a malformed control-plane record
//! can never reach request execution.

use std::{collections::HashSet, fmt, sync::Arc};

use axum::body::Bytes;
use json_patch::{AddOperation, PatchOperation, RemoveOperation, ReplaceOperation};
use jsonptr::PointerBuf;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::domain::ApiFormat;

#[derive(Clone)]
pub struct TransformPlan {
    api_format: ApiFormat,
    request_headers: HeaderPlan,
    response_headers: HeaderPlan,
    request_json: JsonPatchPlan,
    sse_event_patches: SseEventPatchPlan,
}

impl fmt::Debug for TransformPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TransformPlan")
            .field("api_format", &self.api_format)
            .field(
                "request_header_operations",
                &self.request_headers.operations.len(),
            )
            .field(
                "response_header_operations",
                &self.response_headers.operations.len(),
            )
            .field(
                "request_json_operations",
                &self.request_json.operations.len(),
            )
            .field("sse_event_patches", &self.sse_event_patches)
            .finish()
    }
}

impl TransformPlan {
    #[must_use]
    pub fn noop(api_format: ApiFormat) -> Self {
        Self {
            api_format,
            request_headers: HeaderPlan::default(),
            response_headers: HeaderPlan::default(),
            request_json: JsonPatchPlan::default(),
            sse_event_patches: SseEventPatchPlan::empty(api_format),
        }
    }

    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }

    #[must_use]
    pub fn request_headers(&self) -> &HeaderPlan {
        &self.request_headers
    }

    #[must_use]
    pub fn response_headers(&self) -> &HeaderPlan {
        &self.response_headers
    }

    #[must_use]
    pub fn request_json(&self) -> &JsonPatchPlan {
        &self.request_json
    }

    #[must_use]
    pub fn sse_event_patches(&self) -> &SseEventPatchPlan {
        &self.sse_event_patches
    }

    /// Combines layers in their execution order: `defaults` then `override`.
    pub fn compose(defaults: &Self, override_plan: &Self) -> Result<Self, TransformCompileError> {
        if defaults.api_format != override_plan.api_format {
            return Err(TransformCompileError::FormatMismatch);
        }
        reject_removed_ancestors(&defaults.request_json, &override_plan.request_json)?;
        reject_sse_removed_ancestors(
            &defaults.sse_event_patches,
            &override_plan.sse_event_patches,
        )?;
        Ok(Self {
            api_format: defaults.api_format,
            request_headers: defaults
                .request_headers
                .append(&override_plan.request_headers),
            response_headers: defaults
                .response_headers
                .append(&override_plan.response_headers),
            request_json: defaults.request_json.append(&override_plan.request_json),
            sse_event_patches: defaults
                .sse_event_patches
                .append(&override_plan.sse_event_patches),
        })
    }
}

#[derive(Clone, Default)]
pub struct HeaderPlan {
    operations: Arc<[HeaderOperation]>,
}
impl fmt::Debug for HeaderPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HeaderPlan")
            .field("operation_count", &self.operations.len())
            .finish()
    }
}
impl HeaderPlan {
    #[must_use]
    pub fn operations(&self) -> &[HeaderOperation] {
        &self.operations
    }

    fn append(&self, other: &Self) -> Self {
        let mut operations = Vec::with_capacity(self.operations.len() + other.operations.len());
        operations.extend_from_slice(&self.operations);
        operations.extend_from_slice(&other.operations);
        Self {
            operations: operations.into(),
        }
    }

    /// Applies the already compiled operations in their declared order.
    pub fn apply(&self, headers: &mut HeaderMap) -> Result<(), TransformApplyError> {
        apply_header_plan(headers, self)
    }
}

#[derive(Clone)]
pub enum HeaderOperation {
    Set {
        name: HeaderName,
        value: HeaderValue,
    },
    Remove {
        name: HeaderName,
    },
    Rename {
        from: HeaderName,
        to: HeaderName,
    },
}
impl fmt::Debug for HeaderOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Set { .. } => formatter.write_str("HeaderOperation::Set(<redacted>)"),
            Self::Remove { .. } => formatter.write_str("HeaderOperation::Remove(<redacted>)"),
            Self::Rename { .. } => formatter.write_str("HeaderOperation::Rename(<redacted>)"),
        }
    }
}

#[derive(Clone, Default)]
pub struct JsonPatchPlan {
    operations: Arc<[JsonPatchOperation]>,
}
impl fmt::Debug for JsonPatchPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonPatchPlan")
            .field("operation_count", &self.operations.len())
            .finish()
    }
}
impl JsonPatchPlan {
    #[must_use]
    pub fn operations(&self) -> &[JsonPatchOperation] {
        &self.operations
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    fn append(&self, other: &Self) -> Self {
        let mut operations = Vec::with_capacity(self.operations.len() + other.operations.len());
        operations.extend_from_slice(&self.operations);
        operations.extend_from_slice(&other.operations);
        Self {
            operations: operations.into(),
        }
    }

    /// Applies the already compiled patch operations, retaining exact bytes for
    /// an empty plan.
    pub fn apply(&self, body: Bytes) -> Result<Bytes, TransformApplyError> {
        apply_json_patch_plan(body, self)
    }
}

/// Applies a compiled request-header plan. Header names are revalidated here so
/// execution remains safe even if an invalid plan is constructed internally.
pub fn apply_header_plan(
    headers: &mut HeaderMap,
    plan: &HeaderPlan,
) -> Result<(), TransformApplyError> {
    let connection_names = connection_header_names(headers);
    for operation in plan.operations() {
        match operation {
            HeaderOperation::Set { name, value } => {
                ensure_runtime_header_allowed(name, &connection_names)?;
                headers.insert(name.clone(), value.clone());
            }
            HeaderOperation::Remove { name } => {
                ensure_runtime_header_allowed(name, &connection_names)?;
                headers.remove(name);
            }
            HeaderOperation::Rename { from, to } => {
                ensure_runtime_header_allowed(from, &connection_names)?;
                ensure_runtime_header_allowed(to, &connection_names)?;
                let values = headers.get_all(from).iter().cloned().collect::<Vec<_>>();
                if values.is_empty() {
                    continue;
                }
                headers.remove(from);
                for value in values {
                    headers.append(to.clone(), value);
                }
            }
        }
    }
    Ok(())
}

/// Applies a compiled JSON Patch plan atomically. An empty plan returns the
/// original `Bytes` unchanged and does not parse or serialize it.
pub fn apply_json_patch_plan(
    body: Bytes,
    plan: &JsonPatchPlan,
) -> Result<Bytes, TransformApplyError> {
    if plan.is_empty() {
        return Ok(body);
    }

    let mut document =
        serde_json::from_slice(&body).map_err(|_| TransformApplyError::InvalidJsonBody)?;
    let operations = plan
        .operations()
        .iter()
        .map(compile_runtime_patch_operation)
        .collect::<Result<Vec<_>, _>>()?;
    json_patch::patch(&mut document, &operations).map_err(|_| TransformApplyError::PatchFailed)?;
    serde_json::to_vec(&document)
        .map(Bytes::from)
        .map_err(|_| TransformApplyError::SerializationFailed)
}

fn compile_runtime_patch_operation(
    operation: &JsonPatchOperation,
) -> Result<PatchOperation, TransformApplyError> {
    match operation {
        JsonPatchOperation::Add { path, value } => Ok(PatchOperation::Add(AddOperation {
            path: runtime_pointer(path)?,
            value: value.clone(),
        })),
        JsonPatchOperation::Replace { path, value } => {
            Ok(PatchOperation::Replace(ReplaceOperation {
                path: runtime_pointer(path)?,
                value: value.clone(),
            }))
        }
        JsonPatchOperation::Remove { path } => Ok(PatchOperation::Remove(RemoveOperation {
            path: runtime_pointer(path)?,
        })),
    }
}

fn runtime_pointer(pointer: &JsonPointer) -> Result<PointerBuf, TransformApplyError> {
    PointerBuf::parse(pointer.as_str()).map_err(|_| TransformApplyError::InvalidJsonPointer)
}

/// Parses the header names declared by one `Connection` header field value.
///
/// Values are split and ASCII-trimmed as bytes so an opaque non-UTF-8 token
/// does not discard valid neighboring tokens.
pub(crate) fn parse_connection_header_names(
    value: &HeaderValue,
) -> impl Iterator<Item = HeaderName> + '_ {
    value
        .as_bytes()
        .split(|byte| *byte == b',')
        .map(trim_ascii_bytes)
        .filter_map(|name| HeaderName::from_bytes(name).ok())
}

fn connection_header_names(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all("connection")
        .iter()
        .flat_map(parse_connection_header_names)
        .collect()
}

fn trim_ascii_bytes(bytes: &[u8]) -> &[u8] {
    let Some(start) = bytes.iter().position(|byte| !byte.is_ascii_whitespace()) else {
        return bytes;
    };
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .expect("a non-empty byte slice with a non-whitespace byte has an end");
    &bytes[start..=end]
}

fn ensure_runtime_header_allowed(
    name: &HeaderName,
    connection_names: &HashSet<HeaderName>,
) -> Result<(), TransformApplyError> {
    if is_protected_header(name.as_str(), HeaderScope::Request) || connection_names.contains(name) {
        Err(TransformApplyError::ProtectedHeader)
    } else {
        Ok(())
    }
}

/// Safe execution failures. Variants deliberately contain no configuration,
/// header, pointer, document, credential, or upstream data.
#[derive(Debug, Error)]
pub enum TransformApplyError {
    #[error("transform operates on a protected header")]
    ProtectedHeader,
    #[error("transform JSON body is invalid")]
    InvalidJsonBody,
    #[error("transform JSON pointer is invalid")]
    InvalidJsonPointer,
    #[error("transform JSON patch could not be applied")]
    PatchFailed,
    #[error("transform JSON body could not be serialized")]
    SerializationFailed,
}

#[derive(Clone)]
pub enum JsonPatchOperation {
    Add { path: JsonPointer, value: Value },
    Replace { path: JsonPointer, value: Value },
    Remove { path: JsonPointer },
}
impl fmt::Debug for JsonPatchOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Add { .. } => formatter.write_str("JsonPatchOperation::Add(<redacted>)"),
            Self::Replace { .. } => formatter.write_str("JsonPatchOperation::Replace(<redacted>)"),
            Self::Remove { .. } => formatter.write_str("JsonPatchOperation::Remove(<redacted>)"),
        }
    }
}

#[derive(Clone, Eq, Hash, PartialEq)]
pub struct JsonPointer {
    raw: Arc<str>,
    tokens: Arc<[Arc<str>]>,
}
impl fmt::Debug for JsonPointer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JsonPointer(<redacted>)")
    }
}
impl JsonPointer {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    #[must_use]
    pub fn tokens(&self) -> &[Arc<str>] {
        &self.tokens
    }
}

/// Format-specific, precompiled SSE event patch entries. Phase 3 selects these
/// typed selectors and never interprets a configured event name at runtime.
#[derive(Clone)]
pub enum SseEventPatchPlan {
    OpenAiChatCompletions {
        entries: Arc<[ChatCompletionsSsePatchEntry]>,
    },
    OpenAiResponses {
        entries: Arc<[ResponsesSsePatchEntry]>,
    },
}
impl fmt::Debug for SseEventPatchPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAiChatCompletions { entries } => formatter
                .debug_struct("SseEventPatchPlan::OpenAiChatCompletions")
                .field("entry_count", &entries.len())
                .finish(),
            Self::OpenAiResponses { entries } => formatter
                .debug_struct("SseEventPatchPlan::OpenAiResponses")
                .field("entry_count", &entries.len())
                .finish(),
        }
    }
}
impl SseEventPatchPlan {
    fn empty(api_format: ApiFormat) -> Self {
        match api_format {
            ApiFormat::OpenAiChatCompletions => Self::OpenAiChatCompletions {
                entries: Arc::default(),
            },
            ApiFormat::OpenAiResponses => Self::OpenAiResponses {
                entries: Arc::default(),
            },
        }
    }

    #[must_use]
    pub fn chat_completions_entries(&self) -> Option<&[ChatCompletionsSsePatchEntry]> {
        match self {
            Self::OpenAiChatCompletions { entries } => Some(entries),
            Self::OpenAiResponses { .. } => None,
        }
    }

    #[must_use]
    pub fn responses_entries(&self) -> Option<&[ResponsesSsePatchEntry]> {
        match self {
            Self::OpenAiChatCompletions { .. } => None,
            Self::OpenAiResponses { entries } => Some(entries),
        }
    }

    fn append(&self, other: &Self) -> Self {
        match (self, other) {
            (
                Self::OpenAiChatCompletions { entries: left },
                Self::OpenAiChatCompletions { entries: right },
            ) => Self::OpenAiChatCompletions {
                entries: append_entries(left, right),
            },
            (Self::OpenAiResponses { entries: left }, Self::OpenAiResponses { entries: right }) => {
                Self::OpenAiResponses {
                    entries: append_entries(left, right),
                }
            }
            _ => unreachable!("transform plans were format-checked before composition"),
        }
    }
}

fn append_entries<T: Clone>(left: &[T], right: &[T]) -> Arc<[T]> {
    let mut entries = Vec::with_capacity(left.len() + right.len());
    entries.extend_from_slice(left);
    entries.extend_from_slice(right);
    entries.into()
}

fn reject_sse_removed_ancestors(
    defaults: &SseEventPatchPlan,
    override_plan: &SseEventPatchPlan,
) -> Result<(), TransformCompileError> {
    match (defaults, override_plan) {
        (
            SseEventPatchPlan::OpenAiChatCompletions { entries: defaults },
            SseEventPatchPlan::OpenAiChatCompletions {
                entries: override_plan,
            },
        ) => {
            for default in defaults.iter() {
                if let Some(override_entry) = override_plan
                    .iter()
                    .find(|entry| entry.event == default.event)
                {
                    reject_removed_ancestors(&default.json, &override_entry.json)?;
                }
            }
        }
        (
            SseEventPatchPlan::OpenAiResponses { entries: defaults },
            SseEventPatchPlan::OpenAiResponses {
                entries: override_plan,
            },
        ) => {
            for default in defaults.iter() {
                if let Some(override_entry) = override_plan
                    .iter()
                    .find(|entry| entry.event == default.event)
                {
                    reject_removed_ancestors(&default.json, &override_entry.json)?;
                }
            }
        }
        _ => unreachable!("transform plans were format-checked before composition"),
    }
    Ok(())
}

fn reject_removed_ancestors(
    defaults: &JsonPatchPlan,
    override_plan: &JsonPatchPlan,
) -> Result<(), TransformCompileError> {
    for operation in defaults.operations() {
        let JsonPatchOperation::Remove { path: removed } = operation else {
            continue;
        };
        if override_plan
            .operations()
            .iter()
            .any(|operation| removed_path_invalidates_operation(removed, operation))
        {
            return Err(TransformCompileError::IncompatibleJsonPatchLayers);
        }
    }
    Ok(())
}

fn operation_path(operation: &JsonPatchOperation) -> &JsonPointer {
    match operation {
        JsonPatchOperation::Add { path, .. }
        | JsonPatchOperation::Replace { path, .. }
        | JsonPatchOperation::Remove { path } => path,
    }
}

fn removed_path_invalidates_operation(
    removed: &JsonPointer,
    operation: &JsonPatchOperation,
) -> bool {
    let path = operation_path(operation);
    let removed_is_prefix = removed.tokens.len() <= path.tokens.len()
        && removed
            .tokens
            .iter()
            .zip(path.tokens.iter())
            .all(|(left, right)| left == right);
    if !removed_is_prefix {
        return false;
    }
    removed.tokens.len() < path.tokens.len()
        || matches!(
            operation,
            JsonPatchOperation::Replace { .. } | JsonPatchOperation::Remove { .. }
        )
}

/// The sole Chat selector. It matches ordinary unnamed `data:` frames, whose
/// payloads are Chat Completion chunks, when SSE execution is introduced.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum ChatCompletionsSseEvent {
    ChatCompletionChunk,
}
impl fmt::Debug for ChatCompletionsSseEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ChatCompletionsSseEvent(<redacted>)")
    }
}

#[derive(Clone)]
pub struct ChatCompletionsSsePatchEntry {
    event: ChatCompletionsSseEvent,
    json: JsonPatchPlan,
}
impl fmt::Debug for ChatCompletionsSsePatchEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatCompletionsSsePatchEntry")
            .field("json_operations", &self.json.operations.len())
            .finish()
    }
}
impl ChatCompletionsSsePatchEntry {
    #[must_use]
    pub fn event(&self) -> ChatCompletionsSseEvent {
        self.event
    }

    #[must_use]
    pub fn json(&self) -> &JsonPatchPlan {
        &self.json
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub enum ResponsesSseEvent {
    ResponseOutputTextDelta,
    ResponseRefusalDelta,
    ResponseFunctionCallArgumentsDelta,
    ResponseOutputTextDone,
    ResponseCompleted,
}
impl fmt::Debug for ResponsesSseEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ResponsesSseEvent(<redacted>)")
    }
}

#[derive(Clone)]
pub struct ResponsesSsePatchEntry {
    event: ResponsesSseEvent,
    json: JsonPatchPlan,
}
impl fmt::Debug for ResponsesSsePatchEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResponsesSsePatchEntry")
            .field("json_operations", &self.json.operations.len())
            .finish()
    }
}
impl ResponsesSsePatchEntry {
    #[must_use]
    pub fn event(&self) -> ResponsesSseEvent {
        self.event
    }

    #[must_use]
    pub fn json(&self) -> &JsonPatchPlan {
        &self.json
    }
}

/// Compiles the sole accepted document shape. `{}` is an explicit no-op; every
/// other object must carry a version and matching format.
pub fn compile_document(
    document: &Value,
    expected_format: ApiFormat,
) -> Result<TransformPlan, TransformCompileError> {
    let object = document
        .as_object()
        .ok_or(TransformCompileError::DocumentMustBeObject)?;
    if object.is_empty() {
        return Ok(TransformPlan::noop(expected_format));
    }
    reject_unknown(
        object,
        &[
            "version",
            "api_format",
            "request_headers",
            "response_headers",
            "request_json",
            "sse",
        ],
    )?;
    if object.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(TransformCompileError::UnsupportedVersion);
    }
    let format = parse_format(object.get("api_format").and_then(Value::as_str))?;
    if format != expected_format {
        return Err(TransformCompileError::FormatMismatch);
    }
    Ok(TransformPlan {
        api_format: format,
        request_headers: compile_headers(object.get("request_headers"), HeaderScope::Request)?,
        response_headers: compile_headers(object.get("response_headers"), HeaderScope::Response)?,
        request_json: compile_patch(object.get("request_json"), PatchScope::Request)?,
        sse_event_patches: compile_sse_event_patches(object.get("sse"), format)?,
    })
}

/// Returns the declared format without exposing the document in an error. An
/// empty document is format-neutral and therefore returns `None`.
pub fn declared_api_format(document: &Value) -> Result<Option<ApiFormat>, TransformCompileError> {
    let object = document
        .as_object()
        .ok_or(TransformCompileError::DocumentMustBeObject)?;
    if object.is_empty() {
        Ok(None)
    } else {
        parse_format(object.get("api_format").and_then(Value::as_str)).map(Some)
    }
}

fn compile_headers(
    value: Option<&Value>,
    scope: HeaderScope,
) -> Result<HeaderPlan, TransformCompileError> {
    let Some(value) = value else {
        return Ok(HeaderPlan::default());
    };
    let object = value
        .as_object()
        .ok_or(TransformCompileError::HeaderPlanMustBeObject)?;
    reject_unknown(object, &["set", "remove", "rename"])?;
    let mut seen = HashSet::new();
    let mut operations = Vec::new();
    if let Some(set) = object.get("set") {
        let set = set
            .as_object()
            .ok_or(TransformCompileError::HeaderSetMustBeObject)?;
        for (raw_name, raw_value) in set {
            let name = checked_header(raw_name, scope)?;
            if !seen.insert(name.clone()) {
                return Err(TransformCompileError::ConflictingHeaderOperation);
            }
            let value = raw_value
                .as_str()
                .ok_or(TransformCompileError::HeaderValueMustBeString)?;
            let value = HeaderValue::from_str(value)
                .map_err(|_| TransformCompileError::InvalidHeaderValue)?;
            operations.push(HeaderOperation::Set { name, value });
        }
    }
    if let Some(remove) = object.get("remove") {
        let remove = remove
            .as_array()
            .ok_or(TransformCompileError::HeaderRemoveMustBeArray)?;
        for raw_name in remove {
            let name = checked_header(
                raw_name
                    .as_str()
                    .ok_or(TransformCompileError::HeaderNameMustBeString)?,
                scope,
            )?;
            if !seen.insert(name.clone()) {
                return Err(TransformCompileError::ConflictingHeaderOperation);
            }
            operations.push(HeaderOperation::Remove { name });
        }
    }
    if let Some(rename) = object.get("rename") {
        let rename = rename
            .as_object()
            .ok_or(TransformCompileError::HeaderRenameMustBeObject)?;
        for (from, raw_to) in rename {
            let from = checked_header(from, scope)?;
            let to = checked_header(
                raw_to
                    .as_str()
                    .ok_or(TransformCompileError::HeaderNameMustBeString)?,
                scope,
            )?;
            if !seen.insert(from.clone()) || !seen.insert(to.clone()) {
                return Err(TransformCompileError::ConflictingHeaderOperation);
            }
            operations.push(HeaderOperation::Rename { from, to });
        }
    }
    Ok(HeaderPlan {
        operations: operations.into(),
    })
}

fn compile_patch(
    value: Option<&Value>,
    scope: PatchScope,
) -> Result<JsonPatchPlan, TransformCompileError> {
    let Some(value) = value else {
        return Ok(JsonPatchPlan::default());
    };
    let values = value
        .as_array()
        .ok_or(TransformCompileError::PatchMustBeArray)?;
    let mut paths = Vec::<JsonPointer>::new();
    let mut operations = Vec::with_capacity(values.len());
    for value in values {
        let object = value
            .as_object()
            .ok_or(TransformCompileError::PatchOperationMustBeObject)?;
        reject_unknown(object, &["op", "path", "value"])?;
        let op = object
            .get("op")
            .and_then(Value::as_str)
            .ok_or(TransformCompileError::PatchOperationMissingField)?;
        let path = parse_pointer(
            object
                .get("path")
                .and_then(Value::as_str)
                .ok_or(TransformCompileError::PatchOperationMissingField)?,
        )?;
        validate_pointer(&path, scope)?;
        if paths.iter().any(|other| pointers_conflict(other, &path)) {
            return Err(TransformCompileError::ConflictingJsonOperation);
        }
        paths.push(path.clone());
        let operation = match op {
            "add" => JsonPatchOperation::Add {
                path,
                value: object
                    .get("value")
                    .cloned()
                    .ok_or(TransformCompileError::PatchValueRequired)?,
            },
            "replace" => JsonPatchOperation::Replace {
                path,
                value: object
                    .get("value")
                    .cloned()
                    .ok_or(TransformCompileError::PatchValueRequired)?,
            },
            "remove" if !object.contains_key("value") => JsonPatchOperation::Remove { path },
            "remove" => return Err(TransformCompileError::PatchValueForbidden),
            _ => return Err(TransformCompileError::UnsupportedPatchOperation),
        };
        operations.push(operation);
    }
    Ok(JsonPatchPlan {
        operations: operations.into(),
    })
}

fn compile_sse_event_patches(
    value: Option<&Value>,
    api_format: ApiFormat,
) -> Result<SseEventPatchPlan, TransformCompileError> {
    let Some(value) = value else {
        return Ok(SseEventPatchPlan::empty(api_format));
    };
    let entries = value
        .as_array()
        .ok_or(TransformCompileError::SseEventPatchesMustBeArray)?;
    match api_format {
        ApiFormat::OpenAiChatCompletions => {
            let mut seen = HashSet::new();
            let mut compiled = Vec::with_capacity(entries.len());
            for entry in entries {
                let object = sse_entry_object(entry)?;
                let event = parse_chat_completions_sse_event(object)?;
                if !seen.insert(event) {
                    return Err(TransformCompileError::DuplicateSseEvent);
                }
                compiled.push(ChatCompletionsSsePatchEntry {
                    event,
                    json: compile_patch(object.get("json"), PatchScope::Sse(api_format))?,
                });
            }
            Ok(SseEventPatchPlan::OpenAiChatCompletions {
                entries: compiled.into(),
            })
        }
        ApiFormat::OpenAiResponses => {
            let mut seen = HashSet::new();
            let mut compiled = Vec::with_capacity(entries.len());
            for entry in entries {
                let object = sse_entry_object(entry)?;
                let event = parse_responses_sse_event(object)?;
                if !seen.insert(event) {
                    return Err(TransformCompileError::DuplicateSseEvent);
                }
                compiled.push(ResponsesSsePatchEntry {
                    event,
                    json: compile_patch(object.get("json"), PatchScope::Sse(api_format))?,
                });
            }
            Ok(SseEventPatchPlan::OpenAiResponses {
                entries: compiled.into(),
            })
        }
    }
}

fn sse_entry_object(value: &Value) -> Result<&Map<String, Value>, TransformCompileError> {
    let object = value
        .as_object()
        .ok_or(TransformCompileError::SsePatchEntryMustBeObject)?;
    reject_unknown(object, &["event", "json"])?;
    if !object.contains_key("json") {
        return Err(TransformCompileError::SsePatchEntryMissingField);
    }
    Ok(object)
}

fn parse_chat_completions_sse_event(
    object: &Map<String, Value>,
) -> Result<ChatCompletionsSseEvent, TransformCompileError> {
    match sse_event_name(object)? {
        "chat.completion.chunk" => Ok(ChatCompletionsSseEvent::ChatCompletionChunk),
        _ => Err(TransformCompileError::UnsupportedSseEvent),
    }
}

fn parse_responses_sse_event(
    object: &Map<String, Value>,
) -> Result<ResponsesSseEvent, TransformCompileError> {
    match sse_event_name(object)? {
        "response.output_text.delta" => Ok(ResponsesSseEvent::ResponseOutputTextDelta),
        "response.refusal.delta" => Ok(ResponsesSseEvent::ResponseRefusalDelta),
        "response.function_call_arguments.delta" => {
            Ok(ResponsesSseEvent::ResponseFunctionCallArgumentsDelta)
        }
        "response.output_text.done" => Ok(ResponsesSseEvent::ResponseOutputTextDone),
        "response.completed" => Ok(ResponsesSseEvent::ResponseCompleted),
        _ => Err(TransformCompileError::UnsupportedSseEvent),
    }
}

fn sse_event_name(object: &Map<String, Value>) -> Result<&str, TransformCompileError> {
    object
        .get("event")
        .and_then(Value::as_str)
        .ok_or(TransformCompileError::SsePatchEntryMissingField)
}

fn parse_pointer(raw: &str) -> Result<JsonPointer, TransformCompileError> {
    if !raw.is_empty() && !raw.starts_with('/') {
        return Err(TransformCompileError::InvalidJsonPointer);
    }
    let mut tokens = Vec::new();
    for token in raw.split('/').skip(1) {
        let mut decoded = String::with_capacity(token.len());
        let mut chars = token.chars();
        while let Some(character) = chars.next() {
            if character != '~' {
                decoded.push(character);
                continue;
            }
            match chars.next() {
                Some('0') => decoded.push('~'),
                Some('1') => decoded.push('/'),
                _ => return Err(TransformCompileError::InvalidJsonPointer),
            }
        }
        tokens.push(Arc::<str>::from(decoded));
    }
    Ok(JsonPointer {
        raw: Arc::from(raw),
        tokens: tokens.into(),
    })
}

fn validate_pointer(pointer: &JsonPointer, scope: PatchScope) -> Result<(), TransformCompileError> {
    if pointer.tokens().is_empty() {
        return Err(TransformCompileError::RootJsonPointer);
    }
    match scope {
        PatchScope::Request
            if pointer
                .tokens()
                .iter()
                .any(|token| matches!(token.as_ref(), "model" | "stream")) =>
        {
            Err(TransformCompileError::ProtectedJsonPath)
        }
        PatchScope::Request => Ok(()),
        PatchScope::Sse(api_format) if is_immutable_sse_path(pointer, api_format) => {
            Err(TransformCompileError::ProtectedSseJsonPath)
        }
        PatchScope::Sse(_) => Ok(()),
    }
}

fn is_immutable_sse_path(pointer: &JsonPointer, api_format: ApiFormat) -> bool {
    let tokens = pointer.tokens();
    let Some(first) = tokens.first().map(AsRef::as_ref) else {
        return true;
    };
    match api_format {
        ApiFormat::OpenAiChatCompletions => {
            matches!(first, "id" | "object" | "created" | "model")
                || is_array_item_or_its_index(tokens, "choices")
                || tokens
                    .iter()
                    .any(|token| matches!(token.as_ref(), "id" | "index" | "item_id"))
        }
        ApiFormat::OpenAiResponses => {
            matches!(first, "type" | "sequence_number")
                || matches!(tokens, [response] if response.as_ref() == "response")
                || matches!(tokens, [response, field, ..]
                    if response.as_ref() == "response"
                        && matches!(field.as_ref(), "id" | "status"))
                || is_array_item_or_its_index(tokens, "output")
                || tokens.iter().any(|token| {
                    matches!(
                        token.as_ref(),
                        "id" | "item_id"
                            | "call_id"
                            | "response_id"
                            | "index"
                            | "output_index"
                            | "content_index"
                            | "type"
                            | "sequence_number"
                    )
                })
        }
    }
}

fn is_array_item_or_its_index(tokens: &[Arc<str>], array: &str) -> bool {
    tokens
        .iter()
        .position(|token| token.as_ref() == array)
        .is_some_and(|position| tokens.len() <= position + 2)
}

fn pointers_conflict(left: &JsonPointer, right: &JsonPointer) -> bool {
    left.tokens.len() <= right.tokens.len()
        && left
            .tokens
            .iter()
            .zip(right.tokens.iter())
            .all(|(a, b)| a == b)
        || right.tokens.len() <= left.tokens.len()
            && right
                .tokens
                .iter()
                .zip(left.tokens.iter())
                .all(|(a, b)| a == b)
}

fn checked_header(raw: &str, scope: HeaderScope) -> Result<HeaderName, TransformCompileError> {
    let name = HeaderName::from_bytes(raw.as_bytes())
        .map_err(|_| TransformCompileError::InvalidHeaderName)?;
    if is_protected_header(name.as_str(), scope) {
        return Err(TransformCompileError::ProtectedHeader);
    }
    Ok(name)
}

fn is_protected_header(name: &str, scope: HeaderScope) -> bool {
    const HOP_BY_HOP: &[&str] = &[
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ];
    if HOP_BY_HOP.contains(&name) {
        return true;
    }
    match scope {
        HeaderScope::Request => matches!(
            name,
            "host" | "content-length" | "authorization" | "proxy-authorization" | "cookie"
        ),
        HeaderScope::Response => matches!(name, "content-length" | "set-cookie"),
    }
}

fn parse_format(value: Option<&str>) -> Result<ApiFormat, TransformCompileError> {
    match value {
        Some("open_ai_chat_completions") => Ok(ApiFormat::OpenAiChatCompletions),
        Some("open_ai_responses") => Ok(ApiFormat::OpenAiResponses),
        _ => Err(TransformCompileError::InvalidApiFormat),
    }
}

fn reject_unknown(
    object: &Map<String, Value>,
    allowed: &[&str],
) -> Result<(), TransformCompileError> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        Err(TransformCompileError::UnknownField)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum HeaderScope {
    Request,
    Response,
}

#[derive(Clone, Copy)]
enum PatchScope {
    Request,
    Sse(ApiFormat),
}

#[derive(Debug, Error)]
pub enum TransformCompileError {
    #[error("transform document must be a JSON object")]
    DocumentMustBeObject,
    #[error("transform document contains an unknown field")]
    UnknownField,
    #[error("transform document must use version 1")]
    UnsupportedVersion,
    #[error("transform document has an invalid API format")]
    InvalidApiFormat,
    #[error("transform document API format does not match its channel")]
    FormatMismatch,
    #[error("header plan must be an object")]
    HeaderPlanMustBeObject,
    #[error("header set must be an object")]
    HeaderSetMustBeObject,
    #[error("header remove must be an array")]
    HeaderRemoveMustBeArray,
    #[error("header rename must be an object")]
    HeaderRenameMustBeObject,
    #[error("header name must be a string")]
    HeaderNameMustBeString,
    #[error("header value must be a string")]
    HeaderValueMustBeString,
    #[error("invalid header name")]
    InvalidHeaderName,
    #[error("invalid header value")]
    InvalidHeaderValue,
    #[error("transform operates on a protected header")]
    ProtectedHeader,
    #[error("duplicate or conflicting header operation")]
    ConflictingHeaderOperation,
    #[error("JSON patch must be an array")]
    PatchMustBeArray,
    #[error("JSON patch operation must be an object")]
    PatchOperationMustBeObject,
    #[error("JSON patch operation is missing a required field")]
    PatchOperationMissingField,
    #[error("unsupported JSON patch operation")]
    UnsupportedPatchOperation,
    #[error("JSON patch add and replace require a value")]
    PatchValueRequired,
    #[error("JSON patch remove cannot have a value")]
    PatchValueForbidden,
    #[error("invalid JSON Pointer")]
    InvalidJsonPointer,
    #[error("JSON patch cannot target the document root")]
    RootJsonPointer,
    #[error("transform operates on protected model or stream JSON data")]
    ProtectedJsonPath,
    #[error("duplicate or conflicting JSON patch operation")]
    ConflictingJsonOperation,
    #[error("template and channel JSON patch layers are incompatible")]
    IncompatibleJsonPatchLayers,
    #[error("SSE event patches must be an array")]
    SseEventPatchesMustBeArray,
    #[error("SSE patch entry must be an object")]
    SsePatchEntryMustBeObject,
    #[error("SSE patch entry is missing a required field")]
    SsePatchEntryMissingField,
    #[error("SSE event is not allowed for this API format")]
    UnsupportedSseEvent,
    #[error("SSE patch document contains a duplicate event selector")]
    DuplicateSseEvent,
    #[error("SSE transform operates on an immutable event envelope field")]
    ProtectedSseJsonPath,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const CHAT: ApiFormat = ApiFormat::OpenAiChatCompletions;
    const RESPONSES: ApiFormat = ApiFormat::OpenAiResponses;

    fn document(format: &str) -> Value {
        json!({"version": 1, "api_format": format})
    }

    #[test]
    fn compiler_rejects_unknown_protected_conflicting_and_cross_format_documents() {
        let cases = [
            json!({"version": 1, "api_format": "open_ai_chat_completions", "script": "secret-document-value"}),
            json!({"version": 1, "api_format": "open_ai_chat_completions", "request_headers": {"set": {"authorization": "secret-document-value"}}}),
            json!({"version": 1, "api_format": "open_ai_chat_completions", "request_headers": {"set": {"x-trace": "first"}, "remove": ["x-trace"]}}),
            document("open_ai_responses"),
        ];

        for document in cases {
            let error = compile_document(&document, CHAT).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("secret-document-value"));
            assert!(!rendered.contains("authorization"));
        }
    }

    #[test]
    fn compiler_limits_sse_selectors_and_protects_immutable_envelope_fields() {
        let selector = json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "sse": [{"event": "response.completed", "json": []}]
        });
        assert!(matches!(
            compile_document(&selector, CHAT),
            Err(TransformCompileError::UnsupportedSseEvent)
        ));

        let immutable = json!({
            "version": 1,
            "api_format": "open_ai_responses",
            "sse": [{
                "event": "response.completed",
                "json": [{"op": "replace", "path": "/response/id", "value": "secret-document-value"}]
            }]
        });
        let error = compile_document(&immutable, RESPONSES).unwrap_err();
        assert!(matches!(error, TransformCompileError::ProtectedSseJsonPath));
        assert!(!error.to_string().contains("secret-document-value"));
    }

    #[test]
    fn compose_keeps_template_defaults_before_channel_overrides() {
        let defaults = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-layer": "template"}},
                "request_json": [{"op": "add", "path": "/metadata/template", "value": true}]
            }),
            CHAT,
        )
        .unwrap();
        let override_plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-layer": "channel"}},
                "request_json": [{"op": "add", "path": "/metadata/channel", "value": true}]
            }),
            CHAT,
        )
        .unwrap();

        let plan = TransformPlan::compose(&defaults, &override_plan).unwrap();
        let headers = plan.request_headers().operations();
        assert_eq!(headers.len(), 2);
        assert!(matches!(
            &headers[0],
            HeaderOperation::Set { name, value }
                if name == "x-layer" && value == "template"
        ));
        assert!(matches!(
            &headers[1],
            HeaderOperation::Set { name, value }
                if name == "x-layer" && value == "channel"
        ));
        let paths = plan
            .request_json()
            .operations()
            .iter()
            .map(|operation| match operation {
                JsonPatchOperation::Add { path, .. }
                | JsonPatchOperation::Replace { path, .. }
                | JsonPatchOperation::Remove { path } => path.as_str(),
            })
            .collect::<Vec<_>>();
        assert_eq!(paths, ["/metadata/template", "/metadata/channel"]);
    }

    #[test]
    fn debug_redacts_transform_operations_values_and_pointers() {
        let header_name = "x-sentinel-header-name";
        let header_value = "sentinel-header-value";
        let pointer = "/sentinel-json-pointer";
        let json_value = "sentinel-json-value";
        let sse_pointer = "/sentinel-sse-pointer";
        let sse_value = "sentinel-sse-value";
        let plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_responses",
                "request_headers": {"set": {header_name: header_value}},
                "request_json": [{"op": "add", "path": pointer, "value": json_value}],
                "sse": [{
                    "event": "response.output_text.delta",
                    "json": [{"op": "add", "path": sse_pointer, "value": sse_value}]
                }]
            }),
            RESPONSES,
        )
        .unwrap();

        let mut rendered = format!("{plan:?}");
        rendered.push_str(&format!("{:?}", plan.request_headers().operations()));
        rendered.push_str(&format!("{:?}", plan.request_json().operations()));
        rendered.push_str(&format!("{:?}", plan.sse_event_patches()));
        for operation in plan.request_json().operations() {
            let path = match operation {
                JsonPatchOperation::Add { path, .. }
                | JsonPatchOperation::Replace { path, .. }
                | JsonPatchOperation::Remove { path } => path,
            };
            rendered.push_str(&format!("{path:?}"));
        }
        for entry in plan.sse_event_patches().responses_entries().unwrap() {
            rendered.push_str(&format!("{entry:?} {:?}", entry.json().operations()));
            for operation in entry.json().operations() {
                let path = match operation {
                    JsonPatchOperation::Add { path, .. }
                    | JsonPatchOperation::Replace { path, .. }
                    | JsonPatchOperation::Remove { path } => path,
                };
                rendered.push_str(&format!("{path:?}"));
            }
        }

        for sentinel in [
            header_name,
            header_value,
            pointer,
            json_value,
            sse_pointer,
            sse_value,
        ] {
            assert!(!rendered.contains(sentinel), "debug leaked {sentinel}");
        }
    }

    #[test]
    fn compiler_rejects_root_request_and_sse_patches() {
        let request_root = json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_json": [{"op": "replace", "path": "", "value": "sentinel-root-value"}]
        });
        assert!(matches!(
            compile_document(&request_root, CHAT),
            Err(TransformCompileError::RootJsonPointer)
        ));

        let sse_root = json!({
            "version": 1,
            "api_format": "open_ai_responses",
            "sse": [{
                "event": "response.output_text.delta",
                "json": [{"op": "replace", "path": "", "value": "sentinel-sse-root-value"}]
            }]
        });
        assert!(matches!(
            compile_document(&sse_root, RESPONSES),
            Err(TransformCompileError::RootJsonPointer)
        ));
    }

    #[test]
    fn composition_rejects_removed_ancestors_but_allows_overrides_and_recovery() {
        let removed_ancestor = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "remove", "path": "/metadata"}]
            }),
            CHAT,
        )
        .unwrap();
        let descendant_patch = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "add", "path": "/metadata/channel", "value": true}]
            }),
            CHAT,
        )
        .unwrap();
        assert!(matches!(
            TransformPlan::compose(&removed_ancestor, &descendant_patch),
            Err(TransformCompileError::IncompatibleJsonPatchLayers)
        ));

        let same_target_default = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "add", "path": "/metadata/flag", "value": "template"}]
            }),
            CHAT,
        )
        .unwrap();
        let same_target_override = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "add", "path": "/metadata/flag", "value": "channel"}]
            }),
            CHAT,
        )
        .unwrap();
        assert!(TransformPlan::compose(&same_target_default, &same_target_override).is_ok());

        let recovery = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "add", "path": "/metadata", "value": {}}]
            }),
            CHAT,
        )
        .unwrap();
        assert!(TransformPlan::compose(&removed_ancestor, &recovery).is_ok());
    }

    #[test]
    fn executor_applies_header_set_remove_and_rename_in_declared_plan_order() {
        let plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {
                    "set": {"x-set": "new"},
                    "remove": ["x-remove"],
                    "rename": {"x-rename": "x-renamed"}
                }
            }),
            CHAT,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-set", HeaderValue::from_static("old"));
        headers.insert("x-remove", HeaderValue::from_static("discard"));
        headers.append("x-rename", HeaderValue::from_static("first"));
        headers.append("x-rename", HeaderValue::from_static("second"));

        apply_header_plan(&mut headers, plan.request_headers()).unwrap();

        assert_eq!(headers.get("x-set").unwrap(), "new");
        assert!(headers.get("x-remove").is_none());
        assert!(headers.get("x-rename").is_none());
        assert_eq!(headers.get_all("x-renamed").iter().count(), 2);
    }

    #[test]
    fn executor_rejects_headers_declared_hop_by_hop_by_the_client_connection_header() {
        let plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_headers": {"set": {"x-client-hop": "changed"}}
            }),
            CHAT,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("x-client-hop"));
        headers.insert("x-client-hop", HeaderValue::from_static("original"));

        assert!(matches!(
            apply_header_plan(&mut headers, plan.request_headers()),
            Err(TransformApplyError::ProtectedHeader)
        ));
        assert_eq!(headers.get("x-client-hop").unwrap(), "original");
    }

    #[test]
    fn connection_parser_keeps_valid_tokens_beside_opaque_bytes() {
        let value = HeaderValue::from_bytes(b" \tx-before\t,\xff, x-after \t").unwrap();

        let names = parse_connection_header_names(&value).collect::<Vec<_>>();

        assert_eq!(
            names,
            [
                HeaderName::from_static("x-before"),
                HeaderName::from_static("x-after"),
            ]
        );
    }

    #[test]
    fn executor_applies_rfc_json_patch_add_replace_and_remove() {
        let plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [
                    {"op": "add", "path": "/payload/added", "value": true},
                    {"op": "replace", "path": "/payload/replace", "value": "after"},
                    {"op": "remove", "path": "/payload/remove"}
                ]
            }),
            CHAT,
        )
        .unwrap();

        let body = apply_json_patch_plan(
            Bytes::from_static(br#"{"payload":{"replace":"before","remove":"gone"}}"#),
            plan.request_json(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["payload"]["added"], true);
        assert_eq!(value["payload"]["replace"], "after");
        assert!(value["payload"].get("remove").is_none());
    }

    #[test]
    fn executor_reports_patch_failure_without_mutating_the_original_body_bytes() {
        let plan = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [
                    {"op": "replace", "path": "/payload/value", "value": "changed"},
                    {"op": "remove", "path": "/payload/missing"}
                ]
            }),
            CHAT,
        )
        .unwrap();
        let original = Bytes::from_static(br#"{"payload":{"value":"original"}}"#);

        assert!(matches!(
            apply_json_patch_plan(original.clone(), plan.request_json()),
            Err(TransformApplyError::PatchFailed)
        ));
        assert_eq!(original, br#"{"payload":{"value":"original"}}"#.as_slice());
    }

    #[test]
    fn executor_rejects_invalid_json_only_when_a_patch_is_enabled() {
        let patched = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "add", "path": "/added", "value": true}]
            }),
            CHAT,
        )
        .unwrap();

        assert!(matches!(
            apply_json_patch_plan(Bytes::from_static(b"not-json"), patched.request_json()),
            Err(TransformApplyError::InvalidJsonBody)
        ));
    }

    #[test]
    fn executor_returns_empty_body_plans_byte_for_byte_without_parsing() {
        let plan = TransformPlan::noop(CHAT);
        let original = Bytes::from_static(b"not-json \xff with unusual whitespace\n");

        let forwarded = apply_json_patch_plan(original.clone(), plan.request_json()).unwrap();

        assert_eq!(forwarded, original);
    }
}
