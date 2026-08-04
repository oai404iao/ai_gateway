//! Compilation of the closed, versioned transform DSL.
//!
//! This module deliberately contains plans only. Applying a plan belongs to the
//! data plane and is introduced separately so a malformed control-plane record
//! can never reach request execution.

use std::{collections::HashSet, fmt, sync::Arc};

use axum::body::Bytes;
use bytes::BytesMut;
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

    /// Applies this plan to upstream response headers using response-specific
    /// protected-header rules.
    pub fn apply_to_response(&self, headers: &mut HeaderMap) -> Result<(), TransformApplyError> {
        apply_response_header_plan(headers, self)
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
    apply_header_plan_in_scope(headers, plan, HeaderScope::Request)
}

/// Applies a compiled response-header plan before upstream hop-by-hop cleanup.
pub fn apply_response_header_plan(
    headers: &mut HeaderMap,
    plan: &HeaderPlan,
) -> Result<(), TransformApplyError> {
    apply_header_plan_in_scope(headers, plan, HeaderScope::Response)
}

fn apply_header_plan_in_scope(
    headers: &mut HeaderMap,
    plan: &HeaderPlan,
    scope: HeaderScope,
) -> Result<(), TransformApplyError> {
    let connection_names = connection_header_names(headers);
    for operation in plan.operations() {
        match operation {
            HeaderOperation::Set { name, value } => {
                ensure_runtime_header_allowed(name, &connection_names, scope)?;
                headers.insert(name.clone(), value.clone());
            }
            HeaderOperation::Remove { name } => {
                ensure_runtime_header_allowed(name, &connection_names, scope)?;
                headers.remove(name);
            }
            HeaderOperation::Rename { from, to } => {
                ensure_runtime_header_allowed(from, &connection_names, scope)?;
                ensure_runtime_header_allowed(to, &connection_names, scope)?;
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
    apply_json_patch_value(&mut document, plan)?;
    serde_json::to_vec(&document)
        .map(Bytes::from)
        .map_err(|_| TransformApplyError::SerializationFailed)
}

/// Applies a compiled plan to a parsed document. SSE processing uses this to
/// parse an event once while applying matching entries in their stored order.
pub fn apply_json_patch_value(
    document: &mut Value,
    plan: &JsonPatchPlan,
) -> Result<(), TransformApplyError> {
    let mut candidate = document.clone();
    for operation in plan.operations() {
        apply_json_patch_operation(&mut candidate, operation)?;
    }
    *document = candidate;
    Ok(())
}

fn apply_json_patch_operation(
    document: &mut Value,
    operation: &JsonPatchOperation,
) -> Result<(), TransformApplyError> {
    match operation {
        JsonPatchOperation::Add { path, value } => apply_static_patch(
            document,
            PatchOperation::Add(AddOperation {
                path: runtime_pointer(path)?,
                value: value.clone(),
            }),
        ),
        JsonPatchOperation::Replace { path, value } => apply_static_patch(
            document,
            PatchOperation::Replace(ReplaceOperation {
                path: runtime_pointer(path)?,
                value: value.clone(),
            }),
        ),
        JsonPatchOperation::Remove { path } => apply_static_patch(
            document,
            PatchOperation::Remove(RemoveOperation {
                path: runtime_pointer(path)?,
            }),
        ),
        JsonPatchOperation::Advanced(operation) => apply_advanced_patch(document, operation),
    }
}

fn apply_static_patch(
    document: &mut Value,
    operation: PatchOperation,
) -> Result<(), TransformApplyError> {
    json_patch::patch(document, &[operation]).map_err(|_| TransformApplyError::PatchFailed)
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
    scope: HeaderScope,
) -> Result<(), TransformApplyError> {
    if is_protected_header(name.as_str(), scope) || connection_names.contains(name) {
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
    #[error("SSE event frame exceeds the configured limit")]
    SseFrameTooLarge,
}

#[derive(Clone)]
pub enum JsonPatchOperation {
    Add { path: JsonPointer, value: Value },
    Replace { path: JsonPointer, value: Value },
    Remove { path: JsonPointer },
    Advanced(AdvancedJsonOperation),
}
impl fmt::Debug for JsonPatchOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Add { .. } => formatter.write_str("JsonPatchOperation::Add(<redacted>)"),
            Self::Replace { .. } => formatter.write_str("JsonPatchOperation::Replace(<redacted>)"),
            Self::Remove { .. } => formatter.write_str("JsonPatchOperation::Remove(<redacted>)"),
            Self::Advanced(..) => formatter.write_str("JsonPatchOperation::Advanced(<redacted>)"),
        }
    }
}

/// Version-two JSON rewrite operations. They remain deliberately bounded:
/// values can only reference the operation target, conditions can only inspect
/// the operation target, and there is no arbitrary expression evaluation.
#[derive(Clone)]
pub enum AdvancedJsonOperation {
    Add {
        path: JsonPointer,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
    Replace {
        path: JsonPointer,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
    Remove {
        path: JsonPointer,
        when: Option<JsonCondition>,
    },
    ArrayAppend {
        path: JsonPointer,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
    ArrayPrepend {
        path: JsonPointer,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
    ArrayInsert {
        path: JsonPointer,
        index: usize,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
    ArrayRemove {
        path: JsonPointer,
        index: usize,
        when: Option<JsonCondition>,
    },
    Merge {
        path: JsonPointer,
        value: JsonValueExpression,
        when: Option<JsonCondition>,
    },
}
impl fmt::Debug for AdvancedJsonOperation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AdvancedJsonOperation(<redacted>)")
    }
}
impl AdvancedJsonOperation {
    fn path(&self) -> &JsonPointer {
        match self {
            Self::Add { path, .. }
            | Self::Replace { path, .. }
            | Self::Remove { path, .. }
            | Self::ArrayAppend { path, .. }
            | Self::ArrayPrepend { path, .. }
            | Self::ArrayInsert { path, .. }
            | Self::ArrayRemove { path, .. }
            | Self::Merge { path, .. } => path,
        }
    }

    fn when(&self) -> Option<&JsonCondition> {
        match self {
            Self::Add { when, .. }
            | Self::Replace { when, .. }
            | Self::Remove { when, .. }
            | Self::ArrayAppend { when, .. }
            | Self::ArrayPrepend { when, .. }
            | Self::ArrayInsert { when, .. }
            | Self::ArrayRemove { when, .. }
            | Self::Merge { when, .. } => when.as_ref(),
        }
    }

    fn value(&self) -> Option<&JsonValueExpression> {
        match self {
            Self::Add { value, .. }
            | Self::Replace { value, .. }
            | Self::ArrayAppend { value, .. }
            | Self::ArrayPrepend { value, .. }
            | Self::ArrayInsert { value, .. }
            | Self::Merge { value, .. } => Some(value),
            Self::Remove { .. } | Self::ArrayRemove { .. } => None,
        }
    }

    fn is_array_operation(&self) -> bool {
        matches!(
            self,
            Self::ArrayAppend { .. }
                | Self::ArrayPrepend { .. }
                | Self::ArrayInsert { .. }
                | Self::ArrayRemove { .. }
        )
    }

    fn is_remove(&self) -> bool {
        matches!(self, Self::Remove { .. })
    }

    fn can_create_target(&self) -> bool {
        matches!(self, Self::Add { .. })
    }

    fn requires_existing_target(&self) -> bool {
        !self.can_create_target()
    }
}

/// A bounded value expression used by version-two operations.
#[derive(Clone)]
pub enum JsonValueExpression {
    Literal(Value),
    Current,
    Template(Arc<str>),
    Array(Arc<[JsonValueExpression]>),
    Object(Arc<[(Arc<str>, JsonValueExpression)]>),
}
impl fmt::Debug for JsonValueExpression {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JsonValueExpression(<redacted>)")
    }
}
impl JsonValueExpression {
    fn resolve(&self, current: Option<&Value>) -> Result<Value, TransformApplyError> {
        match self {
            Self::Literal(value) => Ok(value.clone()),
            Self::Current => current.cloned().ok_or(TransformApplyError::PatchFailed),
            Self::Template(template) => {
                let replacement = if template.contains("{{value}}") {
                    render_template_value(current.ok_or(TransformApplyError::PatchFailed)?)?
                } else {
                    String::new()
                };
                Ok(Value::String(template.replace("{{value}}", &replacement)))
            }
            Self::Array(items) => items
                .iter()
                .map(|item| item.resolve(current))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Self::Object(entries) => {
                let mut object = Map::with_capacity(entries.len());
                for (key, value) in entries.iter() {
                    object.insert(key.to_string(), value.resolve(current)?);
                }
                Ok(Value::Object(object))
            }
        }
    }
}

fn render_template_value(value: &Value) -> Result<String, TransformApplyError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        value => serde_json::to_string(value).map_err(|_| TransformApplyError::SerializationFailed),
    }
}

/// One condition over the existing value at an operation's target path.
#[derive(Clone)]
pub enum JsonCondition {
    Exists(bool),
    Type(JsonValueType),
    Equals(Value),
}
impl fmt::Debug for JsonCondition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JsonCondition(<redacted>)")
    }
}
impl JsonCondition {
    fn matches(&self, current: Option<&Value>) -> bool {
        match self {
            Self::Exists(expected) => current.is_some() == *expected,
            Self::Type(expected) => current.is_some_and(|value| expected.matches(value)),
            Self::Equals(expected) => current == Some(expected),
        }
    }
}

#[derive(Clone, Copy)]
pub enum JsonValueType {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
}
impl fmt::Debug for JsonValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JsonValueType(<redacted>)")
    }
}
impl JsonValueType {
    fn matches(self, value: &Value) -> bool {
        match self {
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::String => value.is_string(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Null => value.is_null(),
        }
    }
}

fn apply_advanced_patch(
    document: &mut Value,
    operation: &AdvancedJsonOperation,
) -> Result<(), TransformApplyError> {
    let path = operation.path();
    let current = document.pointer(path.as_str());
    if operation
        .when()
        .is_some_and(|condition| !condition.matches(current))
    {
        return Ok(());
    }
    let value = operation
        .value()
        .map(|expression| expression.resolve(current))
        .transpose()?;

    match operation {
        AdvancedJsonOperation::Add { .. } => apply_static_patch(
            document,
            PatchOperation::Add(AddOperation {
                path: runtime_pointer(path)?,
                value: value.expect("advanced add has a value expression"),
            }),
        ),
        AdvancedJsonOperation::Replace { .. } => apply_static_patch(
            document,
            PatchOperation::Replace(ReplaceOperation {
                path: runtime_pointer(path)?,
                value: value.expect("advanced replace has a value expression"),
            }),
        ),
        AdvancedJsonOperation::Remove { .. } => apply_static_patch(
            document,
            PatchOperation::Remove(RemoveOperation {
                path: runtime_pointer(path)?,
            }),
        ),
        AdvancedJsonOperation::ArrayAppend { .. } => {
            let array = target_array_mut(document, path)?;
            array.push(value.expect("advanced array append has a value expression"));
            Ok(())
        }
        AdvancedJsonOperation::ArrayPrepend { .. } => {
            let array = target_array_mut(document, path)?;
            array.insert(
                0,
                value.expect("advanced array prepend has a value expression"),
            );
            Ok(())
        }
        AdvancedJsonOperation::ArrayInsert { index, .. } => {
            let array = target_array_mut(document, path)?;
            if *index > array.len() {
                return Err(TransformApplyError::PatchFailed);
            }
            array.insert(
                *index,
                value.expect("advanced array insert has a value expression"),
            );
            Ok(())
        }
        AdvancedJsonOperation::ArrayRemove { index, .. } => {
            let array = target_array_mut(document, path)?;
            if *index >= array.len() {
                return Err(TransformApplyError::PatchFailed);
            }
            array.remove(*index);
            Ok(())
        }
        AdvancedJsonOperation::Merge { .. } => {
            let mut incoming = value
                .expect("advanced merge has a value expression")
                .as_object()
                .cloned()
                .ok_or(TransformApplyError::PatchFailed)?;
            let target = document
                .pointer_mut(path.as_str())
                .and_then(Value::as_object_mut)
                .ok_or(TransformApplyError::PatchFailed)?;
            target.append(&mut incoming);
            Ok(())
        }
    }
}

fn target_array_mut<'a>(
    document: &'a mut Value,
    path: &JsonPointer,
) -> Result<&'a mut Vec<Value>, TransformApplyError> {
    document
        .pointer_mut(path.as_str())
        .and_then(Value::as_array_mut)
        .ok_or(TransformApplyError::PatchFailed)
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
    OpenAiImages,
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
            Self::OpenAiImages => formatter.write_str("SseEventPatchPlan::OpenAiImages"),
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
            ApiFormat::OpenAiImages => Self::OpenAiImages,
        }
    }

    #[must_use]
    pub fn chat_completions_entries(&self) -> Option<&[ChatCompletionsSsePatchEntry]> {
        match self {
            Self::OpenAiChatCompletions { entries } => Some(entries),
            Self::OpenAiResponses { .. } | Self::OpenAiImages => None,
        }
    }

    #[must_use]
    pub fn responses_entries(&self) -> Option<&[ResponsesSsePatchEntry]> {
        match self {
            Self::OpenAiChatCompletions { .. } | Self::OpenAiImages => None,
            Self::OpenAiResponses { entries } => Some(entries),
        }
    }

    /// Empty entries are semantic no-ops and must not activate SSE handling.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.has_operations()
    }

    #[must_use]
    pub fn has_operations(&self) -> bool {
        match self {
            Self::OpenAiChatCompletions { entries } => {
                entries.iter().any(|entry| !entry.json.is_empty())
            }
            Self::OpenAiResponses { entries } => entries.iter().any(|entry| !entry.json.is_empty()),
            Self::OpenAiImages => false,
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
            (Self::OpenAiImages, Self::OpenAiImages) => Self::OpenAiImages,
            _ => unreachable!("transform plans were format-checked before composition"),
        }
    }
}

/// Pull-driven SSE frame transformer. It holds at most one original upstream
/// chunk plus the current unfinished frame, and yields at most one frame per
/// call to [`Self::next_frame`].
pub struct SseTransformer {
    plan: SseEventPatchPlan,
    frame: BytesMut,
    source: Option<Bytes>,
    source_offset: usize,
    line_has_content: bool,
    pending_cr: bool,
}

impl SseTransformer {
    pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

    #[must_use]
    pub fn new(plan: SseEventPatchPlan) -> Self {
        Self {
            plan,
            frame: BytesMut::new(),
            source: None,
            source_offset: 0,
            line_has_content: false,
            pending_cr: false,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }

    #[must_use]
    pub fn has_operations(&self) -> bool {
        self.plan.has_operations()
    }

    /// Supplies one upstream chunk. Call [`Self::next_frame`] until it returns
    /// `Ok(None)` before supplying the next chunk.
    pub fn push(&mut self, bytes: Bytes) {
        assert!(
            self.source.is_none(),
            "source chunk must be drained before push"
        );
        self.source = Some(bytes);
        self.source_offset = 0;
    }

    /// Scans the supplied source incrementally and yields at most one complete
    /// frame. Frame bytes are retained only after the 8 MiB ceiling check.
    pub fn next_frame(&mut self) -> Result<Option<Bytes>, TransformApplyError> {
        loop {
            let Some(source_len) = self.source.as_ref().map(Bytes::len) else {
                return Ok(None);
            };
            if self.source_offset == source_len {
                self.source = None;
                self.source_offset = 0;
                return Ok(None);
            }

            // A CR is ambiguous until its successor is available. Resolve it
            // before consuming that successor so a completed frame never skips
            // a byte from the following frame.
            if self.pending_cr {
                let byte = self.source.as_ref().expect("source checked above")[self.source_offset];
                self.pending_cr = false;
                if byte == b'\n' {
                    self.append(byte)?;
                    self.source_offset += 1;
                }
                if self.finish_line() {
                    return transform_sse_frame(self.take_frame(), &self.plan).map(Some);
                }
                if byte == b'\n' {
                    continue;
                }
            }

            let byte = self.source.as_ref().expect("source checked above")[self.source_offset];
            self.append(byte)?;
            self.source_offset += 1;
            match byte {
                b'\n' => {
                    if self.finish_line() {
                        return transform_sse_frame(self.take_frame(), &self.plan).map(Some);
                    }
                }
                b'\r' => self.pending_cr = true,
                _ => self.line_has_content = true,
            }
        }
    }

    fn append(&mut self, byte: u8) -> Result<(), TransformApplyError> {
        if self.frame.len() == Self::MAX_FRAME_BYTES {
            return Err(TransformApplyError::SseFrameTooLarge);
        }
        self.frame.extend_from_slice(&[byte]);
        Ok(())
    }

    fn finish_line(&mut self) -> bool {
        let complete_frame = !self.line_has_content;
        self.line_has_content = false;
        complete_frame
    }

    fn take_frame(&mut self) -> Bytes {
        self.frame.split().freeze()
    }

    /// A clean EOF preserves an unfinished frame exactly as received. A final
    /// CR remains residual because it may have been the first half of CRLF.
    pub fn finish(&mut self) -> Option<Bytes> {
        (!self.frame.is_empty()).then(|| self.frame.split().freeze())
    }
}

/// Applies Responses SSE event rules to one Responses WebSocket JSON message.
///
/// WebSocket transport carries the same typed event objects as SSE but without
/// the `event:`/`data:` envelope, so the compiled selector and JSON patch plan
/// can be reused without reconstructing an SSE frame.
pub fn apply_websocket_event_plan(
    payload: Bytes,
    plan: &SseEventPatchPlan,
) -> Result<Bytes, TransformApplyError> {
    if !plan.has_operations() {
        return Ok(payload);
    }
    if payload.len() > SseTransformer::MAX_FRAME_BYTES {
        return Err(TransformApplyError::SseFrameTooLarge);
    }
    let SseEventPatchPlan::OpenAiResponses { entries } = plan else {
        return Ok(payload);
    };
    let Ok(mut value) = serde_json::from_slice::<Value>(&payload) else {
        return Ok(payload);
    };
    if !value.is_object() {
        return Ok(payload);
    }
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
        return Ok(payload);
    };
    let Some(selector) = response_selector(event_type) else {
        return Ok(payload);
    };
    if !apply_matching_response_entries(&mut value, entries, selector)? {
        return Ok(payload);
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| TransformApplyError::SerializationFailed)
}

struct SseLine {
    start: usize,
    content_end: usize,
    end: usize,
}

fn transform_sse_frame(
    frame: Bytes,
    plan: &SseEventPatchPlan,
) -> Result<Bytes, TransformApplyError> {
    let mut event = None;
    let mut data = Vec::new();
    let mut has_data = false;
    let mut cursor = 0;
    while let Some(line) = next_sse_line(&frame, &mut cursor) {
        if line.start == line.content_end {
            break;
        }
        let content = &frame[line.start..line.content_end];
        let Some((field, value)) = sse_field(content) else {
            continue;
        };
        match field {
            b"event" => event = Some(value),
            b"data" => {
                if has_data {
                    data.push(b'\n');
                }
                data.extend_from_slice(value);
                has_data = true;
            }
            _ => {}
        }
    }
    if !has_data {
        return Ok(frame);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&data) else {
        return Ok(frame);
    };
    if !value.is_object() {
        return Ok(frame);
    }

    let changed = match plan {
        SseEventPatchPlan::OpenAiChatCompletions { entries } => {
            let chat_chunk =
                value.get("object").and_then(Value::as_str) == Some("chat.completion.chunk");
            if !chat_chunk || event.is_some_and(|name| !name.is_empty()) {
                false
            } else {
                apply_matching_chat_entries(&mut value, entries)?
            }
        }
        SseEventPatchPlan::OpenAiResponses { entries } => {
            let Some(event_type) = value.get("type").and_then(Value::as_str) else {
                return Ok(frame);
            };
            let Some(selector) = response_selector(event_type) else {
                return Ok(frame);
            };
            if event.is_some_and(|name| name != event_type.as_bytes()) {
                false
            } else {
                apply_matching_response_entries(&mut value, entries, selector)?
            }
        }
        SseEventPatchPlan::OpenAiImages => false,
    };
    if !changed {
        return Ok(frame);
    }

    let json = serde_json::to_vec(&value).map_err(|_| TransformApplyError::SerializationFailed)?;
    let rewritten = rewrite_sse_data_lines(&frame, &json);
    Ok(Bytes::from(rewritten))
}

fn next_sse_line(frame: &[u8], cursor: &mut usize) -> Option<SseLine> {
    let start = *cursor;
    let mut index = start;
    while index < frame.len() {
        let end = match frame[index] {
            b'\n' => index + 1,
            b'\r' if index + 1 < frame.len() && frame[index + 1] == b'\n' => index + 2,
            b'\r' => index + 1,
            _ => {
                index += 1;
                continue;
            }
        };
        *cursor = end;
        return Some(SseLine {
            start,
            content_end: index,
            end,
        });
    }
    None
}

fn rewrite_sse_data_lines(frame: &[u8], json: &[u8]) -> Vec<u8> {
    let mut rewritten = Vec::with_capacity(frame.len() + json.len());
    let mut frame_cursor = 0;
    let mut scan_cursor = 0;
    let mut replaced = false;
    while let Some(line) = next_sse_line(frame, &mut scan_cursor) {
        if line.start == line.content_end {
            break;
        }
        if sse_field(&frame[line.start..line.content_end])
            .is_some_and(|(field, _)| field == b"data")
        {
            rewritten.extend_from_slice(&frame[frame_cursor..line.start]);
            if !replaced {
                rewritten.extend_from_slice(b"data: ");
                rewritten.extend_from_slice(json);
                rewritten.extend_from_slice(&frame[line.content_end..line.end]);
                replaced = true;
            }
            frame_cursor = line.end;
        }
    }
    rewritten.extend_from_slice(&frame[frame_cursor..]);
    rewritten
}

fn sse_field(line: &[u8]) -> Option<(&[u8], &[u8])> {
    if line.first() == Some(&b':') {
        return None;
    }
    let (field, value) = match line.iter().position(|byte| *byte == b':') {
        Some(position) => (&line[..position], &line[position + 1..]),
        None => (line, &[][..]),
    };
    Some((field, value.strip_prefix(b" ").unwrap_or(value)))
}

fn apply_matching_chat_entries(
    value: &mut Value,
    entries: &[ChatCompletionsSsePatchEntry],
) -> Result<bool, TransformApplyError> {
    let mut changed = false;
    for entry in entries {
        if entry.event == ChatCompletionsSseEvent::ChatCompletionChunk && !entry.json.is_empty() {
            apply_json_patch_value(value, &entry.json)?;
            changed = true;
        }
    }
    Ok(changed)
}

fn apply_matching_response_entries(
    value: &mut Value,
    entries: &[ResponsesSsePatchEntry],
    selector: ResponsesSseEvent,
) -> Result<bool, TransformApplyError> {
    let mut changed = false;
    for entry in entries {
        if entry.event == selector && !entry.json.is_empty() {
            apply_json_patch_value(value, &entry.json)?;
            changed = true;
        }
    }
    Ok(changed)
}

fn response_selector(event: &str) -> Option<ResponsesSseEvent> {
    match event {
        "response.output_text.delta" => Some(ResponsesSseEvent::ResponseOutputTextDelta),
        "response.refusal.delta" => Some(ResponsesSseEvent::ResponseRefusalDelta),
        "response.function_call_arguments.delta" => {
            Some(ResponsesSseEvent::ResponseFunctionCallArgumentsDelta)
        }
        "response.output_text.done" => Some(ResponsesSseEvent::ResponseOutputTextDone),
        "response.completed" => Some(ResponsesSseEvent::ResponseCompleted),
        _ => None,
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
        (SseEventPatchPlan::OpenAiImages, SseEventPatchPlan::OpenAiImages) => {}
        _ => unreachable!("transform plans were format-checked before composition"),
    }
    Ok(())
}

fn reject_removed_ancestors(
    defaults: &JsonPatchPlan,
    override_plan: &JsonPatchPlan,
) -> Result<(), TransformCompileError> {
    for operation in defaults.operations() {
        let Some(removed) = operation_removed_path(operation) else {
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
        JsonPatchOperation::Advanced(operation) => operation.path(),
    }
}

fn operation_removed_path(operation: &JsonPatchOperation) -> Option<&JsonPointer> {
    match operation {
        JsonPatchOperation::Remove { path } => Some(path),
        JsonPatchOperation::Advanced(operation) if operation.is_remove() => Some(operation.path()),
        _ => None,
    }
}

fn operation_can_create_target(operation: &JsonPatchOperation) -> bool {
    matches!(operation, JsonPatchOperation::Add { .. })
        || matches!(operation, JsonPatchOperation::Advanced(operation) if operation.can_create_target())
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
    removed.tokens.len() < path.tokens.len() || !operation_can_create_target(operation)
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

/// Compiles the accepted document shape. `{}` is an explicit no-op; every
/// other object must carry a supported version and matching format.
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
    let version = parse_document_version(object.get("version"))?;
    let format = parse_format(object.get("api_format").and_then(Value::as_str))?;
    if format != expected_format {
        return Err(TransformCompileError::FormatMismatch);
    }
    Ok(TransformPlan {
        api_format: format,
        request_headers: compile_headers(object.get("request_headers"), HeaderScope::Request)?,
        response_headers: compile_headers(object.get("response_headers"), HeaderScope::Response)?,
        request_json: compile_patch(object.get("request_json"), PatchScope::Request, version)?,
        sse_event_patches: compile_sse_event_patches(object.get("sse"), format, version)?,
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
    version: TransformDocumentVersion,
) -> Result<JsonPatchPlan, TransformCompileError> {
    let Some(value) = value else {
        return Ok(JsonPatchPlan::default());
    };
    let values = value
        .as_array()
        .ok_or(TransformCompileError::PatchMustBeArray)?;
    let mut operations = Vec::with_capacity(values.len());
    for value in values {
        let object = value
            .as_object()
            .ok_or(TransformCompileError::PatchOperationMustBeObject)?;
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
        let operation = match version {
            TransformDocumentVersion::V1 => compile_v1_patch_operation(object, op, path)?,
            TransformDocumentVersion::V2 => compile_v2_patch_operation(object, op, path)?,
        };
        if operations
            .iter()
            .any(|other| json_operations_conflict(other, &operation))
        {
            return Err(TransformCompileError::ConflictingJsonOperation);
        }
        operations.push(operation);
    }
    Ok(JsonPatchPlan {
        operations: operations.into(),
    })
}

fn compile_v1_patch_operation(
    object: &Map<String, Value>,
    op: &str,
    path: JsonPointer,
) -> Result<JsonPatchOperation, TransformCompileError> {
    reject_unknown(object, &["op", "path", "value"])?;
    match op {
        "add" => Ok(JsonPatchOperation::Add {
            path,
            value: object
                .get("value")
                .cloned()
                .ok_or(TransformCompileError::PatchValueRequired)?,
        }),
        "replace" => Ok(JsonPatchOperation::Replace {
            path,
            value: object
                .get("value")
                .cloned()
                .ok_or(TransformCompileError::PatchValueRequired)?,
        }),
        "remove" if !object.contains_key("value") => Ok(JsonPatchOperation::Remove { path }),
        "remove" => Err(TransformCompileError::PatchValueForbidden),
        _ => Err(TransformCompileError::UnsupportedPatchOperation),
    }
}

fn compile_v2_patch_operation(
    object: &Map<String, Value>,
    op: &str,
    path: JsonPointer,
) -> Result<JsonPatchOperation, TransformCompileError> {
    reject_unknown(object, &["op", "path", "value", "index", "when"])?;
    let when = compile_patch_condition(object.get("when"))?;
    let value = |object: &Map<String, Value>| {
        object
            .get("value")
            .ok_or(TransformCompileError::PatchValueRequired)
            .and_then(|value| compile_value_expression(value, 0))
    };
    let forbid_value = |object: &Map<String, Value>| {
        if object.contains_key("value") {
            Err(TransformCompileError::PatchValueForbidden)
        } else {
            Ok(())
        }
    };
    let forbid_index = |object: &Map<String, Value>| {
        if object.contains_key("index") {
            Err(TransformCompileError::PatchIndexForbidden)
        } else {
            Ok(())
        }
    };
    let index = |object: &Map<String, Value>| {
        object
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or(TransformCompileError::PatchIndexRequired)
    };

    let operation = match op {
        "add" => {
            forbid_index(object)?;
            AdvancedJsonOperation::Add {
                path,
                value: value(object)?,
                when,
            }
        }
        "replace" => {
            forbid_index(object)?;
            AdvancedJsonOperation::Replace {
                path,
                value: value(object)?,
                when,
            }
        }
        "remove" => {
            forbid_value(object)?;
            forbid_index(object)?;
            AdvancedJsonOperation::Remove { path, when }
        }
        "array_append" => {
            forbid_index(object)?;
            AdvancedJsonOperation::ArrayAppend {
                path,
                value: value(object)?,
                when,
            }
        }
        "array_prepend" => {
            forbid_index(object)?;
            AdvancedJsonOperation::ArrayPrepend {
                path,
                value: value(object)?,
                when,
            }
        }
        "array_insert" => AdvancedJsonOperation::ArrayInsert {
            path,
            index: index(object)?,
            value: value(object)?,
            when,
        },
        "array_remove" => {
            forbid_value(object)?;
            AdvancedJsonOperation::ArrayRemove {
                path,
                index: index(object)?,
                when,
            }
        }
        "merge" => {
            forbid_index(object)?;
            AdvancedJsonOperation::Merge {
                path,
                value: value(object)?,
                when,
            }
        }
        _ => return Err(TransformCompileError::UnsupportedPatchOperation),
    };
    if operation.requires_existing_target()
        && matches!(operation.when(), Some(JsonCondition::Exists(false)))
    {
        return Err(TransformCompileError::IncompatiblePatchCondition);
    }
    Ok(JsonPatchOperation::Advanced(operation))
}

fn compile_patch_condition(
    value: Option<&Value>,
) -> Result<Option<JsonCondition>, TransformCompileError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or(TransformCompileError::PatchConditionMustBeObject)?;
    reject_unknown(object, &["exists", "type", "equals"])?;
    if object.len() != 1 {
        return Err(TransformCompileError::PatchConditionMustHaveOnePredicate);
    }
    if let Some(exists) = object.get("exists") {
        return exists
            .as_bool()
            .map(JsonCondition::Exists)
            .ok_or(TransformCompileError::InvalidPatchCondition)
            .map(Some);
    }
    if let Some(kind) = object.get("type").and_then(Value::as_str) {
        return parse_json_value_type(kind)
            .map(JsonCondition::Type)
            .map(Some);
    }
    if let Some(expected) = object.get("equals") {
        return Ok(Some(JsonCondition::Equals(expected.clone())));
    }
    Err(TransformCompileError::PatchConditionMustHaveOnePredicate)
}

fn parse_json_value_type(value: &str) -> Result<JsonValueType, TransformCompileError> {
    match value {
        "object" => Ok(JsonValueType::Object),
        "array" => Ok(JsonValueType::Array),
        "string" => Ok(JsonValueType::String),
        "number" => Ok(JsonValueType::Number),
        "boolean" => Ok(JsonValueType::Boolean),
        "null" => Ok(JsonValueType::Null),
        _ => Err(TransformCompileError::InvalidPatchCondition),
    }
}

const MAX_VALUE_EXPRESSION_DEPTH: usize = 32;
const MAX_TEMPLATE_BYTES: usize = 4 * 1024;

fn compile_value_expression(
    value: &Value,
    depth: usize,
) -> Result<JsonValueExpression, TransformCompileError> {
    if depth > MAX_VALUE_EXPRESSION_DEPTH {
        return Err(TransformCompileError::ValueExpressionTooDeep);
    }
    match value {
        Value::Array(values) => values
            .iter()
            .map(|value| compile_value_expression(value, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| JsonValueExpression::Array(values.into())),
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            match object.get("$ref").and_then(Value::as_str) {
                Some("current") => Ok(JsonValueExpression::Current),
                _ => Err(TransformCompileError::InvalidValueExpression),
            }
        }
        Value::Object(object) if object.len() == 1 && object.contains_key("$template") => {
            let template = object
                .get("$template")
                .and_then(Value::as_str)
                .ok_or(TransformCompileError::InvalidValueExpression)?;
            if template.len() > MAX_TEMPLATE_BYTES {
                return Err(TransformCompileError::TemplateTooLong);
            }
            Ok(JsonValueExpression::Template(Arc::from(template)))
        }
        Value::Object(object) if object.len() == 1 && object.contains_key("$literal") => {
            Ok(JsonValueExpression::Literal(
                object
                    .get("$literal")
                    .cloned()
                    .expect("$literal marker was checked"),
            ))
        }
        Value::Object(object) => {
            let mut entries = Vec::with_capacity(object.len());
            for (key, value) in object {
                entries.push((
                    Arc::from(key.as_str()),
                    compile_value_expression(value, depth + 1)?,
                ));
            }
            Ok(JsonValueExpression::Object(entries.into()))
        }
        value => Ok(JsonValueExpression::Literal(value.clone())),
    }
}

fn json_operations_conflict(left: &JsonPatchOperation, right: &JsonPatchOperation) -> bool {
    let left_path = operation_path(left);
    let right_path = operation_path(right);
    if !pointers_conflict(left_path, right_path) {
        return false;
    }
    !(left_path == right_path && operation_is_array_only(left) && operation_is_array_only(right))
}

fn operation_is_array_only(operation: &JsonPatchOperation) -> bool {
    matches!(operation, JsonPatchOperation::Advanced(operation) if operation.is_array_operation())
}

fn compile_sse_event_patches(
    value: Option<&Value>,
    api_format: ApiFormat,
    version: TransformDocumentVersion,
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
                    json: compile_patch(object.get("json"), PatchScope::Sse(api_format), version)?,
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
                    json: compile_patch(object.get("json"), PatchScope::Sse(api_format), version)?,
                });
            }
            Ok(SseEventPatchPlan::OpenAiResponses {
                entries: compiled.into(),
            })
        }
        ApiFormat::OpenAiImages if entries.is_empty() => Ok(SseEventPatchPlan::OpenAiImages),
        ApiFormat::OpenAiImages => Err(TransformCompileError::UnsupportedSseEvent),
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
        ApiFormat::OpenAiImages => true,
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
        "proxy-authorization",
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
            "host"
                | "content-length"
                | "authorization"
                | "proxy-authorization"
                | "cookie"
                | "accept-encoding"
        ),
        HeaderScope::Response => {
            matches!(
                name,
                "content-length" | "content-type" | "set-cookie" | "content-encoding"
            )
        }
    }
}

fn parse_format(value: Option<&str>) -> Result<ApiFormat, TransformCompileError> {
    match value {
        Some("open_ai_chat_completions") => Ok(ApiFormat::OpenAiChatCompletions),
        Some("open_ai_responses") => Ok(ApiFormat::OpenAiResponses),
        Some("open_ai_images") => Ok(ApiFormat::OpenAiImages),
        _ => Err(TransformCompileError::InvalidApiFormat),
    }
}

#[derive(Clone, Copy)]
enum TransformDocumentVersion {
    V1,
    V2,
}

fn parse_document_version(
    value: Option<&Value>,
) -> Result<TransformDocumentVersion, TransformCompileError> {
    match value.and_then(Value::as_u64) {
        Some(1) => Ok(TransformDocumentVersion::V1),
        Some(2) => Ok(TransformDocumentVersion::V2),
        _ => Err(TransformCompileError::UnsupportedVersion),
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
    #[error("transform document must use version 1 or 2")]
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
    #[error("JSON patch array operation has an invalid or missing index")]
    PatchIndexRequired,
    #[error("JSON patch operation must not have an index")]
    PatchIndexForbidden,
    #[error("JSON patch condition must be an object")]
    PatchConditionMustBeObject,
    #[error("JSON patch condition must contain exactly one supported predicate")]
    PatchConditionMustHaveOnePredicate,
    #[error("JSON patch condition is invalid")]
    InvalidPatchCondition,
    #[error("JSON patch condition cannot run this operation against a missing target")]
    IncompatiblePatchCondition,
    #[error("JSON patch value expression is invalid")]
    InvalidValueExpression,
    #[error("JSON patch value expression is too deeply nested")]
    ValueExpressionTooDeep,
    #[error("JSON patch template is too long")]
    TemplateTooLong,
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
    const IMAGES: ApiFormat = ApiFormat::OpenAiImages;

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

        let images_selector = json!({
            "version": 1,
            "api_format": "open_ai_images",
            "sse": [{"event": "image_generation.partial_image", "json": []}]
        });
        assert!(matches!(
            compile_document(&images_selector, IMAGES),
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
    fn image_noop_transform_layers_compose_without_sse_state() {
        let plan = compile_document(
            &json!({"version": 1, "api_format": "open_ai_images"}),
            IMAGES,
        )
        .unwrap();

        let composed = TransformPlan::compose(&plan, &plan).unwrap();

        assert!(composed.sse_event_patches().is_empty());
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
            .map(|operation| operation_path(operation).as_str())
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
            rendered.push_str(&format!("{:?}", operation_path(operation)));
        }
        for entry in plan.sse_event_patches().responses_entries().unwrap() {
            rendered.push_str(&format!("{entry:?} {:?}", entry.json().operations()));
            for operation in entry.json().operations() {
                rendered.push_str(&format!("{:?}", operation_path(operation)));
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
                    {"op": "add", "path": "/payload/items/-", "value": "tail"},
                    {"op": "replace", "path": "/payload/replace", "value": "after"},
                    {"op": "remove", "path": "/payload/remove"}
                ]
            }),
            CHAT,
        )
        .unwrap();

        let body = apply_json_patch_plan(
            Bytes::from_static(
                br#"{"payload":{"items":["head"],"replace":"before","remove":"gone"}}"#,
            ),
            plan.request_json(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(value["payload"]["added"], true);
        assert_eq!(value["payload"]["items"], json!(["head", "tail"]));
        assert_eq!(value["payload"]["replace"], "after");
        assert!(value["payload"].get("remove").is_none());
    }

    #[test]
    fn executor_applies_v2_array_merge_conditions_and_current_value_expressions() {
        let plan = compile_document(
            &json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [
                    {
                        "op": "array_prepend",
                        "path": "/items",
                        "value": "first",
                        "when": {"type": "array"}
                    },
                    {
                        "op": "array_append",
                        "path": "/items",
                        "value": "last",
                        "when": {"exists": true}
                    },
                    {
                        "op": "array_insert",
                        "path": "/items",
                        "index": 1,
                        "value": "inserted"
                    },
                    {
                        "op": "array_remove",
                        "path": "/items",
                        "index": 3
                    },
                    {
                        "op": "merge",
                        "path": "/metadata",
                        "value": {
                            "original": {"$ref": "current"},
                            "gateway": "console"
                        },
                        "when": {"type": "object"}
                    },
                    {
                        "op": "replace",
                        "path": "/label",
                        "value": {"$template": "gateway-{{value}}"},
                        "when": {"type": "string"}
                    },
                    {
                        "op": "add",
                        "path": "/optional",
                        "value": true,
                        "when": {"exists": false}
                    }
                ]
            }),
            CHAT,
        )
        .unwrap();

        let body = apply_json_patch_plan(
            Bytes::from_static(
                br#"{"items":["middle","removed"],"metadata":{"source":"client"},"label":"tag"}"#,
            ),
            plan.request_json(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            value["items"],
            json!(["first", "inserted", "middle", "last"])
        );
        assert_eq!(
            value["metadata"],
            json!({
                "source": "client",
                "original": {"source": "client"},
                "gateway": "console"
            })
        );
        assert_eq!(value["label"], "gateway-tag");
        assert_eq!(value["optional"], true);
    }

    #[test]
    fn v2_conditions_skip_operations_and_failures_are_atomic() {
        let skip = compile_document(
            &json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [{
                    "op": "replace",
                    "path": "/label",
                    "value": "changed",
                    "when": {"equals": "different"}
                }]
            }),
            CHAT,
        )
        .unwrap();
        let mut value = json!({"label": "original"});
        apply_json_patch_value(&mut value, skip.request_json()).unwrap();
        assert_eq!(value, json!({"label": "original"}));

        let failing = compile_document(
            &json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [
                    {"op": "array_append", "path": "/items", "value": "changed"},
                    {"op": "array_remove", "path": "/items", "index": 5}
                ]
            }),
            CHAT,
        )
        .unwrap();
        let mut value = json!({"items": ["original"]});
        assert!(matches!(
            apply_json_patch_value(&mut value, failing.request_json()),
            Err(TransformApplyError::PatchFailed)
        ));
        assert_eq!(value, json!({"items": ["original"]}));
    }

    #[test]
    fn compiler_keeps_v1_strict_and_validates_v2_operation_shape_without_leaking_values() {
        let cases = [
            json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "array_append", "path": "/items", "value": "secret"}]
            }),
            json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [{"op": "array_insert", "path": "/items", "value": "secret"}]
            }),
            json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [{
                    "op": "replace",
                    "path": "/label",
                    "value": {"$ref": "other-secret"},
                    "when": {"exists": true, "type": "string"}
                }]
            }),
            json!({
                "version": 2,
                "api_format": "open_ai_chat_completions",
                "request_json": [{
                    "op": "array_append",
                    "path": "/items",
                    "value": "secret",
                    "when": {"exists": false}
                }]
            }),
        ];

        for document in cases {
            let error = compile_document(&document, CHAT).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains("secret"));
        }
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

    fn sse_plan(document: Value, api_format: ApiFormat) -> SseEventPatchPlan {
        compile_document(&document, api_format)
            .unwrap()
            .sse_event_patches()
            .clone()
    }

    fn transform_chunks(plan: SseEventPatchPlan, chunks: &[&[u8]]) -> Vec<u8> {
        let mut transformer = SseTransformer::new(plan);
        let mut output = Vec::new();
        for chunk in chunks {
            transformer.push(Bytes::copy_from_slice(chunk));
            while let Some(frame) = transformer.next_frame().unwrap() {
                output.extend_from_slice(&frame);
            }
        }
        if let Some(residual) = transformer.finish() {
            output.extend_from_slice(&residual);
        }
        output
    }

    #[test]
    fn sse_transformer_handles_lf_crlf_and_a_split_crlf_delimiter() {
        let plan = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{
                    "event": "chat.completion.chunk",
                    "json": [{"op": "add", "path": "/patched", "value": true}]
                }]
            }),
            CHAT,
        );
        let output = transform_chunks(
            plan,
            &[
                b"data: {\"object\":\"chat.completion.chunk\"}\r",
                b"\n\r",
                b"\ndata: {\"object\":\"chat.completion.chunk\"}\n",
                b"\n",
            ],
        );

        assert_eq!(
            output,
            b"data: {\"object\":\"chat.completion.chunk\",\"patched\":true}\r\n\r\ndata: {\"object\":\"chat.completion.chunk\",\"patched\":true}\n\n"
        );
    }

    #[test]
    fn sse_transformer_matches_only_format_specific_events_and_event_names() {
        let chat = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{"event": "chat.completion.chunk", "json": [{"op": "add", "path": "/patched", "value": "chat"}]}]
            }),
            CHAT,
        );
        assert_eq!(
            transform_chunks(
                chat,
                &[
                    b"event: other\ndata: {\"object\":\"chat.completion.chunk\"}\n\n",
                    b"data: {\"type\":\"response.output_text.delta\"}\n\n",
                ],
            ),
            b"event: other\ndata: {\"object\":\"chat.completion.chunk\"}\n\ndata: {\"type\":\"response.output_text.delta\"}\n\n"
        );

        let responses = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_responses",
                "sse": [{"event": "response.output_text.delta", "json": [{"op": "add", "path": "/patched", "value": "responses"}]}]
            }),
            RESPONSES,
        );
        assert_eq!(
            transform_chunks(
                responses,
                &[
                    b"event: response.completed\ndata: {\"type\":\"response.output_text.delta\"}\n\n",
                    b"event: response.output_text.delta\ndata: {\"type\":\"response.completed\"}\n\n",
                    b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\"}\n\n",
                ],
            ),
            b"event: response.completed\ndata: {\"type\":\"response.output_text.delta\"}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.completed\"}\n\nevent: response.output_text.delta\ndata: {\"patched\":\"responses\",\"type\":\"response.output_text.delta\"}\n\n"
        );
    }

    #[test]
    fn sse_transformer_applies_v2_current_value_templates() {
        let plan = sse_plan(
            json!({
                "version": 2,
                "api_format": "open_ai_responses",
                "sse": [{
                    "event": "response.output_text.delta",
                    "json": [{
                        "op": "replace",
                        "path": "/delta",
                        "value": {"$template": "gateway-{{value}}"},
                        "when": {"type": "string"}
                    }]
                }]
            }),
            RESPONSES,
        );
        let output = transform_chunks(
            plan,
            &[b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n"],
        );

        assert_eq!(
            output,
            b"data: {\"delta\":\"gateway-hello\",\"type\":\"response.output_text.delta\"}\n\n"
        );
    }

    #[test]
    fn composed_sse_entries_run_template_before_channel_and_reconstruct_multiline_data() {
        let defaults = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{"event": "chat.completion.chunk", "json": [{"op": "add", "path": "/metadata/layer", "value": "template"}]}]
            }),
            CHAT,
        )
        .unwrap();
        let channel = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{"event": "chat.completion.chunk", "json": [{"op": "replace", "path": "/metadata/layer", "value": "channel"}]}]
            }),
            CHAT,
        )
        .unwrap();
        let output = transform_chunks(
            TransformPlan::compose(&defaults, &channel)
                .unwrap()
                .sse_event_patches()
                .clone(),
            &[
                b": keep\ndata: {\"object\":\"chat.completion.chunk\",\"metadata\":\n",
                b"data: {}}\nunknown: retained\n\n",
            ],
        );

        assert_eq!(
            output,
            b": keep\ndata: {\"metadata\":{\"layer\":\"channel\"},\"object\":\"chat.completion.chunk\"}\nunknown: retained\n\n"
        );
    }

    #[test]
    fn sse_transformer_preserves_non_matching_frames_and_clean_eof_residuals_exactly() {
        let plan = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{"event": "chat.completion.chunk", "json": [{"op": "add", "path": "/patched", "value": true}]}]
            }),
            CHAT,
        );
        let passthrough =
            b": comment\nunknown: field\ndata: [DONE]\n\ndata: not-json\n\ndata: [1,2]\n\n";
        let residual = b"data: {\"object\":\"chat.completion.chunk\"}";
        let output = transform_chunks(plan, &[passthrough, residual]);

        assert_eq!(
            output,
            [passthrough.as_slice(), residual.as_slice()].concat()
        );
    }

    #[test]
    fn websocket_event_transform_reuses_responses_sse_selector_without_envelope() {
        let plan = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_responses",
                "sse": [{
                    "event": "response.output_text.delta",
                    "json": [{"op": "add", "path": "/patched", "value": true}]
                }]
            }),
            RESPONSES,
        );
        assert_eq!(
            apply_websocket_event_plan(
                Bytes::from_static(br#"{"type":"response.output_text.delta","delta":"hello"}"#,),
                &plan,
            )
            .unwrap(),
            Bytes::from_static(
                br#"{"delta":"hello","patched":true,"type":"response.output_text.delta"}"#,
            )
        );
        assert_eq!(
            apply_websocket_event_plan(
                Bytes::from_static(br#"{"type":"response.completed"}"#),
                &plan,
            )
            .unwrap(),
            Bytes::from_static(br#"{"type":"response.completed"}"#)
        );
    }

    #[test]
    fn sse_transformer_rejects_frames_larger_than_its_ceiling() {
        let mut transformer =
            SseTransformer::new(TransformPlan::noop(CHAT).sse_event_patches().clone());

        transformer.push(Bytes::from(vec![b'x'; SseTransformer::MAX_FRAME_BYTES + 1]));
        assert!(matches!(
            transformer.next_frame(),
            Err(TransformApplyError::SseFrameTooLarge)
        ));
    }

    #[test]
    fn sse_transformer_yields_prior_same_chunk_frames_before_a_later_patch_failure() {
        let plan = sse_plan(
            json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "sse": [{
                    "event": "chat.completion.chunk",
                    "json": [{"op": "replace", "path": "/missing", "value": true}]
                }]
            }),
            CHAT,
        );
        let mut transformer = SseTransformer::new(plan);
        transformer.push(Bytes::from_static(
            b"data: [DONE]\n\ndata: {\"object\":\"chat.completion.chunk\"}\n\n",
        ));

        assert_eq!(
            transformer.next_frame().unwrap(),
            Some(Bytes::from_static(b"data: [DONE]\n\n"))
        );
        assert!(matches!(
            transformer.next_frame(),
            Err(TransformApplyError::PatchFailed)
        ));
    }

    #[test]
    fn response_header_plans_apply_in_layer_order_and_protect_static_and_dynamic_hops() {
        let request_document = json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"accept-encoding": "identity"}}
        });
        assert!(matches!(
            compile_document(&request_document, CHAT),
            Err(TransformCompileError::ProtectedHeader)
        ));

        let template = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "response_headers": {"set": {"x-layer": "template"}}
            }),
            CHAT,
        )
        .unwrap();
        let channel = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "response_headers": {"set": {"x-layer": "channel"}}
            }),
            CHAT,
        )
        .unwrap();
        let plan = TransformPlan::compose(&template, &channel).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-layer", HeaderValue::from_static("upstream"));
        plan.response_headers()
            .apply_to_response(&mut headers)
            .unwrap();
        assert_eq!(headers.get("x-layer").unwrap(), "channel");

        for protected in [
            "content-length",
            "content-type",
            "set-cookie",
            "content-encoding",
            "connection",
        ] {
            let document = json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "response_headers": {"set": {protected: "changed"}}
            });
            assert!(matches!(
                compile_document(&document, CHAT),
                Err(TransformCompileError::ProtectedHeader)
            ));
        }

        let dynamic = compile_document(
            &json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "response_headers": {"set": {"x-upstream-hop": "changed"}}
            }),
            CHAT,
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("x-upstream-hop"));
        assert!(matches!(
            dynamic.response_headers().apply_to_response(&mut headers),
            Err(TransformApplyError::ProtectedHeader)
        ));
    }
}
