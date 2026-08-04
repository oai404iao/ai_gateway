//! Bounded replayable request bodies and multipart Images edit adaptation.

use std::{
    collections::BTreeSet,
    fmt, io,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::{
    body::{Body, Bytes},
    http::{
        HeaderMap,
        header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE},
    },
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::BytesMut;
use futures_util::{Stream, StreamExt, stream};
use serde_json::{Map, Number, Value};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};

use crate::{
    request_policy::{
        FieldDisposition, RequestInterface, RequestPolicyError, RequestPolicyLayer,
        apply_json_body_policy, body_field_disposition,
    },
    runtime_config::RequestLimitsConfig,
};

const BODY_READ_CHUNK_BYTES: usize = 64 * 1_024;
const MAX_MULTIPART_FIELDS: usize = 64;
const MAX_MULTIPART_BOUNDARY_BYTES: usize = 70;
const MAX_MULTIPART_PREAMBLE_BYTES: usize = 8 * 1_024;
const MAX_MULTIPART_PART_HEADER_BYTES: usize = 16 * 1_024;
const MAX_MULTIPART_BOUNDARY_PADDING_BYTES: usize = 1_024;
const MAX_MULTIPART_TEXT_FIELD_BYTES: usize = 64 * 1_024;
const MAX_MULTIPART_TEXT_BYTES: usize = 1_024 * 1_024;
const MAX_MULTIPART_FIELD_NAME_BYTES: usize = 128;
const MAX_MULTIPART_FILE_NAME_BYTES: usize = 255;
const MAX_IMAGE_EDIT_IMAGES: usize = 16;
const MAX_IMAGE_EDIT_MASKS: usize = 1;
const MAX_CODEX_EDIT_IMAGES: usize = 5;
const MAX_CODEX_IMAGE_MIME_BYTES: usize = 128;
const REBUILT_BODY_OVERHEAD_BYTES: usize = 64 * 1_024;
// One request can temporarily retain the captured multipart plus a rebuilt
// multipart or base64-expanded Codex JSON body.
const SPOOL_CAPACITY_WARNING_BODY_MULTIPLIER: u64 = 3;

type ReplayStream = Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>;

/// Request-body limits used by the public proxy.
///
/// Converting from a plain byte limit preserves the historical constructor
/// surface for tests and embedders. Production startup converts the complete
/// TOML-backed `RequestLimitsConfig` instead.
#[derive(Clone)]
pub struct ProxyRequestBodyLimits {
    pub(crate) proxy_body_bytes: usize,
    image_edit: ImageEditBodyPolicy,
}

impl From<usize> for ProxyRequestBodyLimits {
    fn from(proxy_body_bytes: usize) -> Self {
        let image_edit_body_bytes = proxy_body_bytes.max(1);
        Self {
            proxy_body_bytes,
            image_edit: ImageEditBodyPolicy::new(
                image_edit_body_bytes,
                image_edit_body_bytes,
                image_edit_body_bytes.min(64 * 1_024),
                std::env::temp_dir().join(format!(
                    "ai-gateway-image-edit-spool-{}",
                    std::process::id()
                )),
            ),
        }
    }
}

impl From<RequestLimitsConfig> for ProxyRequestBodyLimits {
    fn from(value: RequestLimitsConfig) -> Self {
        Self {
            proxy_body_bytes: value.proxy_body_bytes,
            image_edit: ImageEditBodyPolicy::new(
                value.image_edit_body_bytes,
                value.image_edit_file_bytes,
                value.image_edit_memory_bytes,
                value.image_edit_spool_directory,
            ),
        }
    }
}

impl From<&RequestLimitsConfig> for ProxyRequestBodyLimits {
    fn from(value: &RequestLimitsConfig) -> Self {
        value.clone().into()
    }
}

impl ProxyRequestBodyLimits {
    pub(crate) fn image_edit(&self) -> &ImageEditBodyPolicy {
        &self.image_edit
    }
}

#[derive(Clone)]
pub(crate) struct ImageEditBodyPolicy {
    max_body_bytes: usize,
    max_file_bytes: usize,
    memory_bytes: usize,
    store: ImageBodyStore,
}

impl ImageEditBodyPolicy {
    fn new(
        max_body_bytes: usize,
        max_file_bytes: usize,
        memory_bytes: usize,
        spool_directory: PathBuf,
    ) -> Self {
        Self {
            max_body_bytes,
            max_file_bytes,
            memory_bytes,
            store: ImageBodyStore::new(spool_directory, max_body_bytes),
        }
    }

    pub(crate) async fn capture(
        &self,
        headers: &HeaderMap,
        body: Body,
    ) -> Result<ImageEditRequestBody, ImageEditBodyError> {
        reject_encoded_body(headers)?;
        let mut content_types = headers.get_all(CONTENT_TYPE).iter();
        let content_type = content_types
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|_| content_types.next().is_none())
            .ok_or(ImageEditBodyError::UnsupportedContentType)?;
        let boundary = multer::parse_boundary(content_type)
            .map_err(|_| ImageEditBodyError::UnsupportedContentType)?;
        if boundary.is_empty() || boundary.len() > MAX_MULTIPART_BOUNDARY_BYTES {
            return Err(ImageEditBodyError::UnsupportedContentType);
        }
        if headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > self.max_body_bytes as u64)
        {
            return Err(ImageEditBodyError::BodyTooLarge);
        }

        let mut writer =
            ReplayableBodyWriter::new(self.store.clone(), self.memory_bytes, self.max_body_bytes);
        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ImageEditBodyError::Unreadable)?;
            writer.write(&chunk).await?;
        }
        let body = writer.finish().await?;
        let inspection = inspect_multipart(&body, &boundary, self.max_file_bytes).await?;
        let model: Arc<str> = inspection.model.into();
        Ok(ImageEditRequestBody {
            body,
            boundary: boundary.into(),
            text_fields: inspection.text_fields.into(),
            ignored_part_fields: Arc::from([]),
            wire_model: Arc::clone(&model),
            model,
            image_count: inspection.image_count,
            image_bytes: inspection.image_bytes,
            mask_count: inspection.mask_count,
            policy: self.clone(),
        })
    }

    pub(crate) async fn spool_snapshot(&self) -> ImageBodySpoolSnapshot {
        self.store.snapshot().await
    }
}

fn reject_encoded_body(headers: &HeaderMap) -> Result<(), ImageEditBodyError> {
    let supported = headers.get_all(CONTENT_ENCODING).iter().all(|value| {
        value.to_str().is_ok_and(|value| {
            value
                .split(',')
                .all(|encoding| encoding.trim().eq_ignore_ascii_case("identity"))
        })
    });
    supported
        .then_some(())
        .ok_or(ImageEditBodyError::UnsupportedContentEncoding)
}

#[derive(Clone)]
pub(crate) enum PreparedRequestBody {
    Json(Bytes),
    ImageEdit(ImageEditRequestBody),
}

impl PreparedRequestBody {
    pub(crate) fn json_bytes(&self) -> Option<&Bytes> {
        match self {
            Self::Json(body) => Some(body),
            Self::ImageEdit(_) => None,
        }
    }

    pub(crate) fn image_edit(&self) -> Option<&ImageEditRequestBody> {
        match self {
            Self::Json(_) => None,
            Self::ImageEdit(body) => Some(body),
        }
    }

    pub(crate) fn request_value(&self) -> Value {
        match self {
            Self::Json(body) => serde_json::from_slice(body)
                .expect("JSON proxy bodies are validated before billing"),
            Self::ImageEdit(body) => body.request_value(),
        }
    }

    pub(crate) fn apply_policy(
        self,
        layer: RequestPolicyLayer,
        interface: RequestInterface,
    ) -> Result<(Self, bool), RequestPolicyError> {
        match self {
            Self::Json(body) => {
                let applied = apply_json_body_policy(layer, interface, body)?;
                Ok((Self::Json(applied.body), applied.changed))
            }
            Self::ImageEdit(body) => {
                debug_assert_eq!(interface, RequestInterface::ImagesEdit);
                let (body, changed) = body.apply_field_policy(layer, interface)?;
                Ok((Self::ImageEdit(body), changed))
            }
        }
    }

    pub(crate) async fn rewrite_model(
        self,
        client_model: &str,
        upstream_model: &str,
    ) -> Result<Self, ImageEditBodyError> {
        if client_model == upstream_model {
            return Ok(self);
        }
        match self {
            Self::Json(body) => {
                let mut value = serde_json::from_slice::<Value>(&body)
                    .map_err(|_| ImageEditBodyError::InvalidJson)?;
                let object = value
                    .as_object_mut()
                    .ok_or(ImageEditBodyError::InvalidJson)?;
                object.insert("model".to_owned(), Value::String(upstream_model.to_owned()));
                serde_json::to_vec(&value)
                    .map(Bytes::from)
                    .map(Self::Json)
                    .map_err(|_| ImageEditBodyError::InvalidJson)
            }
            Self::ImageEdit(body) => body.rewrite_model(upstream_model).map(Self::ImageEdit),
        }
    }

    pub(crate) async fn into_openai_replayable(
        self,
    ) -> Result<ReplayableRequestBody, ImageEditBodyError> {
        match self {
            Self::Json(body) => Ok(ReplayableRequestBody::Memory(body)),
            Self::ImageEdit(body) => body.into_openai_replayable().await,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ImageEditRequestBody {
    body: ReplayableRequestBody,
    boundary: Arc<str>,
    text_fields: Arc<[MultipartTextField]>,
    ignored_part_fields: Arc<[String]>,
    wire_model: Arc<str>,
    model: Arc<str>,
    image_count: usize,
    image_bytes: usize,
    mask_count: usize,
    policy: ImageEditBodyPolicy,
}

impl ImageEditRequestBody {
    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn stream_requested(&self) -> bool {
        self.text_fields.iter().any(|field| {
            field.name == "stream"
                && std::str::from_utf8(&field.value).is_ok_and(|value| {
                    value.trim().eq_ignore_ascii_case("true") || value.trim() == "1"
                })
        })
    }

    fn request_value(&self) -> Value {
        let mut object = Map::new();
        for field in self.text_fields.iter() {
            let Ok(value) = std::str::from_utf8(&field.value) else {
                continue;
            };
            let value = match field.name.as_str() {
                "n" => value
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(Number::from)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(value.to_owned())),
                "stream" if value.trim().eq_ignore_ascii_case("true") => Value::Bool(true),
                "stream" if value.trim().eq_ignore_ascii_case("false") => Value::Bool(false),
                _ => Value::String(value.to_owned()),
            };
            object.insert(field.name.clone(), value);
        }
        Value::Object(object)
    }

    fn apply_field_policy(
        mut self,
        layer: RequestPolicyLayer,
        interface: RequestInterface,
    ) -> Result<(Self, bool), RequestPolicyError> {
        let mut retained = Vec::with_capacity(self.text_fields.len());
        let mut ignored = self
            .ignored_part_fields
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut changed = false;
        for (field, present) in [
            ("image", self.image_count > 0),
            ("mask", self.mask_count > 0),
        ] {
            if !present {
                continue;
            }
            if body_field_disposition(layer, interface, field, None)? == FieldDisposition::Ignore {
                ignored.insert(field.to_owned());
                if field == "image" {
                    ignored.insert("image[]".to_owned());
                    self.image_count = 0;
                    self.image_bytes = 0;
                } else {
                    self.mask_count = 0;
                }
                changed = true;
            }
        }
        for field in self.text_fields.iter() {
            let value = std::str::from_utf8(&field.value)
                .ok()
                .map(|value| Value::String(value.to_owned()));
            match body_field_disposition(layer, interface, &field.name, value.as_ref())? {
                FieldDisposition::Allow => retained.push(field.clone()),
                FieldDisposition::Ignore => {
                    ignored.insert(field.name.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.text_fields = retained.into();
            self.ignored_part_fields = ignored.into_iter().collect::<Vec<_>>().into();
        }
        Ok((self, changed))
    }

    fn rewrite_model(mut self, upstream_model: &str) -> Result<Self, ImageEditBodyError> {
        if upstream_model.contains(['\r', '\n']) {
            return Err(ImageEditBodyError::InvalidModel);
        }
        self.model = upstream_model.to_owned().into();
        let mut fields = self.text_fields.to_vec();
        for field in &mut fields {
            if field.name == "model" {
                field.value = Bytes::copy_from_slice(upstream_model.as_bytes());
            }
        }
        self.text_fields = fields.into();
        Ok(self)
    }

    async fn into_openai_replayable(self) -> Result<ReplayableRequestBody, ImageEditBodyError> {
        if self.model == self.wire_model && self.ignored_part_fields.is_empty() {
            return Ok(self.body);
        }
        self.rebuild_parts().await
    }

    async fn rebuild_parts(self) -> Result<ReplayableRequestBody, ImageEditBodyError> {
        let upstream_model = Arc::clone(&self.model);
        let max_bytes = self
            .body
            .len()
            .saturating_add(upstream_model.len())
            .saturating_add(REBUILT_BODY_OVERHEAD_BYTES);
        let mut writer = ReplayableBodyWriter::new(
            self.policy.store.clone(),
            self.policy.memory_bytes,
            max_bytes,
        );
        let mut multipart = trusted_multipart(&self.body, &self.boundary).await?;
        let mut replaced = false;
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|_| ImageEditBodyError::MalformedMultipart)?
        {
            let field_name = field.name().map(str::to_owned);
            if field_name.as_deref().is_some_and(|name| {
                self.ignored_part_fields
                    .binary_search_by(|ignored| ignored.as_str().cmp(name))
                    .is_ok()
            }) {
                while field
                    .chunk()
                    .await
                    .map_err(|_| ImageEditBodyError::MalformedMultipart)?
                    .is_some()
                {}
                continue;
            }
            writer.write(b"--").await?;
            writer.write(self.boundary.as_bytes()).await?;
            writer.write(b"\r\n").await?;
            let replace = field_name.as_deref() == Some("model");
            for (name, value) in field.headers() {
                if replace && is_stale_rebuilt_part_header(name.as_str()) {
                    continue;
                }
                writer.write(name.as_str().as_bytes()).await?;
                writer.write(b": ").await?;
                writer.write(value.as_bytes()).await?;
                writer.write(b"\r\n").await?;
            }
            writer.write(b"\r\n").await?;
            if replace {
                while field
                    .chunk()
                    .await
                    .map_err(|_| ImageEditBodyError::MalformedMultipart)?
                    .is_some()
                {}
                writer.write(upstream_model.as_bytes()).await?;
                replaced = true;
            } else {
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|_| ImageEditBodyError::MalformedMultipart)?
                {
                    writer.write(&chunk).await?;
                }
            }
            writer.write(b"\r\n").await?;
        }
        writer.write(b"--").await?;
        writer.write(self.boundary.as_bytes()).await?;
        writer.write(b"--\r\n").await?;
        if !replaced {
            return Err(ImageEditBodyError::MalformedMultipart);
        }
        writer.finish().await
    }

    pub(crate) async fn to_codex_json(&self) -> Result<ReplayableRequestBody, ImageEditBodyError> {
        if self.image_count > MAX_CODEX_EDIT_IMAGES {
            return Err(ImageEditBodyError::CodexTooManyImages);
        }
        if self.mask_count > 0 {
            let error = body_field_disposition(
                RequestPolicyLayer::CodexOauth,
                RequestInterface::ImagesEdit,
                "mask",
                None,
            )
            .expect_err("validated Codex Images edit policy must reject mask");
            return Err(ImageEditBodyError::RequestPolicy(error));
        }
        let fields = CodexEditFields::parse(&self.text_fields)?;
        let text_bytes = self.text_fields.iter().fold(0_usize, |total, field| {
            total.saturating_add(field.value.len())
        });
        let max_bytes = base64_encoded_len(self.image_bytes)
            .saturating_add(text_bytes.saturating_mul(6))
            .saturating_add(REBUILT_BODY_OVERHEAD_BYTES);
        let mut writer = ReplayableBodyWriter::new(
            self.policy.store.clone(),
            self.policy.memory_bytes,
            max_bytes,
        );
        writer.write(br#"{"images":["#).await?;
        let mut multipart = trusted_multipart(&self.body, &self.boundary).await?;
        let mut image_index = 0_usize;
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|_| ImageEditBodyError::MalformedMultipart)?
        {
            let name = field.name().map(str::to_owned);
            if name.as_deref().is_some_and(|name| {
                self.ignored_part_fields
                    .binary_search_by(|ignored| ignored.as_str().cmp(name))
                    .is_ok()
            }) {
                while field
                    .chunk()
                    .await
                    .map_err(|_| ImageEditBodyError::MalformedMultipart)?
                    .is_some()
                {}
                continue;
            }
            if name.as_deref().is_some_and(is_image_field) {
                if image_index > 0 {
                    writer.write(b",").await?;
                }
                let mime = image_mime(&field)?;
                writer.write(br#"{"image_url":"data:"#).await?;
                writer.write(mime.as_bytes()).await?;
                writer.write(b";base64,").await?;
                write_base64_field(&mut field, &mut writer, self.policy.max_file_bytes).await?;
                writer.write(br#""}"#).await?;
                image_index = image_index.saturating_add(1);
            } else {
                while field
                    .chunk()
                    .await
                    .map_err(|_| ImageEditBodyError::MalformedMultipart)?
                    .is_some()
                {}
            }
        }
        if image_index != self.image_count {
            return Err(ImageEditBodyError::MalformedMultipart);
        }
        writer.write(b"]").await?;
        fields.write_json_fields(&mut writer).await?;
        writer.write(b"}").await?;
        writer.finish().await
    }
}

fn is_stale_rebuilt_part_header(name: &str) -> bool {
    matches!(
        name,
        "content-length"
            | "content-md5"
            | "digest"
            | "content-digest"
            | "repr-digest"
            | "etag"
            | "last-modified"
    )
}

#[derive(Clone)]
struct MultipartTextField {
    name: String,
    value: Bytes,
}

struct MultipartInspection {
    text_fields: Vec<MultipartTextField>,
    model: String,
    image_count: usize,
    image_bytes: usize,
    mask_count: usize,
}

async fn inspect_multipart(
    body: &ReplayableRequestBody,
    boundary: &str,
    max_file_bytes: usize,
) -> Result<MultipartInspection, ImageEditBodyError> {
    let mut multipart = validated_multipart(body, boundary).await?;
    let mut field_count = 0_usize;
    let mut image_count = 0_usize;
    let mut image_bytes = 0_usize;
    let mut mask_count = 0_usize;
    let mut model = None;
    let mut text_fields = Vec::new();
    let mut text_bytes = 0_usize;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| ImageEditBodyError::MalformedMultipart)?
    {
        field_count = field_count.saturating_add(1);
        if field_count > MAX_MULTIPART_FIELDS {
            return Err(ImageEditBodyError::TooManyFields);
        }
        let name = field
            .name()
            .map(str::to_owned)
            .ok_or(ImageEditBodyError::MalformedMultipart)?;
        if name.len() > MAX_MULTIPART_FIELD_NAME_BYTES {
            return Err(ImageEditBodyError::FieldNameTooLong);
        }
        if field
            .file_name()
            .is_some_and(|name| name.len() > MAX_MULTIPART_FILE_NAME_BYTES)
        {
            return Err(ImageEditBodyError::FileNameTooLong);
        }
        let is_image = is_image_field(&name);
        let is_mask = name == "mask";
        let is_file = is_image || is_mask || field.file_name().is_some();
        if is_file {
            let policy_name = if name == "image[]" {
                "image"
            } else {
                name.as_str()
            };
            body_field_disposition(
                RequestPolicyLayer::Client,
                RequestInterface::ImagesEdit,
                policy_name,
                None,
            )
            .map_err(ImageEditBodyError::RequestPolicy)?;
            if !is_image && !is_mask {
                return Err(ImageEditBodyError::UnexpectedFileField);
            }
            let mut length = 0_usize;
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| ImageEditBodyError::MalformedMultipart)?
            {
                length = length.saturating_add(chunk.len());
                if length > max_file_bytes {
                    return Err(ImageEditBodyError::FileTooLarge);
                }
            }
            if is_image {
                image_count = image_count.saturating_add(1);
                image_bytes = image_bytes.saturating_add(length);
                if image_count > MAX_IMAGE_EDIT_IMAGES {
                    return Err(ImageEditBodyError::TooManyImages);
                }
            } else {
                mask_count = mask_count.saturating_add(1);
                if mask_count > MAX_IMAGE_EDIT_MASKS {
                    return Err(ImageEditBodyError::TooManyMasks);
                }
            }
            continue;
        }

        let mut value = BytesMut::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|_| ImageEditBodyError::MalformedMultipart)?
        {
            if value.len().saturating_add(chunk.len()) > MAX_MULTIPART_TEXT_FIELD_BYTES {
                return Err(ImageEditBodyError::TextFieldTooLarge);
            }
            value.extend_from_slice(&chunk);
        }
        let value = value.freeze();
        text_bytes = text_bytes.saturating_add(value.len());
        if text_bytes > MAX_MULTIPART_TEXT_BYTES {
            return Err(ImageEditBodyError::TextFieldTooLarge);
        }
        if name == "model" {
            if model.is_some() {
                return Err(ImageEditBodyError::DuplicateModel);
            }
            let parsed = std::str::from_utf8(&value)
                .map_err(|_| ImageEditBodyError::InvalidModel)?
                .to_owned();
            if parsed.trim().is_empty() {
                return Err(ImageEditBodyError::EmptyModel);
            }
            if parsed.chars().count() > 300 {
                return Err(ImageEditBodyError::ModelTooLong);
            }
            model = Some(parsed);
        }
        text_fields.push(MultipartTextField { name, value });
    }
    if image_count == 0 {
        return Err(ImageEditBodyError::MissingImage);
    }
    Ok(MultipartInspection {
        text_fields,
        model: model.ok_or(ImageEditBodyError::MissingModel)?,
        image_count,
        image_bytes,
        mask_count,
    })
}

fn base64_encoded_len(length: usize) -> usize {
    length.saturating_add(2).div_euclid(3).saturating_mul(4)
}

fn is_image_field(name: &str) -> bool {
    matches!(name, "image" | "image[]")
}

async fn validated_multipart(
    body: &ReplayableRequestBody,
    boundary: &str,
) -> Result<multer::Multipart<'static>, ImageEditBodyError> {
    let stream = guard_multipart_stream(body.stream().await?, boundary);
    Ok(multer::Multipart::new(stream, boundary.to_owned()))
}

async fn trusted_multipart(
    body: &ReplayableRequestBody,
    boundary: &str,
) -> Result<multer::Multipart<'static>, ImageEditBodyError> {
    Ok(multer::Multipart::new(
        body.stream().await?,
        boundary.to_owned(),
    ))
}

fn guard_multipart_stream(stream: ReplayStream, boundary: &str) -> ReplayStream {
    let validator = MultipartStructureValidator::new(boundary);
    Box::pin(stream::try_unfold(
        (stream, validator),
        |(mut stream, mut validator)| async move {
            match stream.next().await {
                Some(Ok(chunk)) => {
                    validator.push(&chunk)?;
                    Ok(Some((chunk, (stream, validator))))
                }
                Some(Err(error)) => Err(error),
                None => {
                    validator.finish()?;
                    Ok(None)
                }
            }
        },
    ))
}

/// Bounds framing regions that `multer` otherwise buffers while searching for
/// the first boundary and each part's header terminator. Field payload bytes
/// remain streaming and are still validated by the parser itself.
struct MultipartStructureValidator {
    state: MultipartStructureState,
    first_boundary: BytePatternMatcher,
    field_boundary: BytePatternMatcher,
    header_terminator: BytePatternMatcher,
}

enum MultipartStructureState {
    FindingFirstBoundary { scanned: usize },
    ReadingBoundarySuffix(BoundarySuffix),
    ReadingHeaders { bytes: usize },
    ReadingData,
    Done,
}

enum BoundarySuffix {
    Start,
    Dash,
    Padding { bytes: usize },
    CarriageReturn,
}

impl MultipartStructureValidator {
    fn new(boundary: &str) -> Self {
        Self {
            state: MultipartStructureState::FindingFirstBoundary { scanned: 0 },
            first_boundary: BytePatternMatcher::new(format!("--{boundary}").into_bytes()),
            field_boundary: BytePatternMatcher::new(format!("\r\n--{boundary}").into_bytes()),
            header_terminator: BytePatternMatcher::new(b"\r\n\r\n".to_vec()),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), io::Error> {
        for byte in bytes {
            self.push_byte(*byte)?;
        }
        Ok(())
    }

    fn push_byte(&mut self, byte: u8) -> Result<(), io::Error> {
        match &mut self.state {
            MultipartStructureState::FindingFirstBoundary { scanned } => {
                *scanned = scanned.saturating_add(1);
                if self.first_boundary.push(byte) {
                    self.state =
                        MultipartStructureState::ReadingBoundarySuffix(BoundarySuffix::Start);
                } else if scanned.saturating_sub(self.first_boundary.matched())
                    > MAX_MULTIPART_PREAMBLE_BYTES
                {
                    return Err(invalid_multipart_structure());
                }
            }
            MultipartStructureState::ReadingBoundarySuffix(suffix) => match suffix {
                BoundarySuffix::Start => match byte {
                    b'-' => *suffix = BoundarySuffix::Dash,
                    b' ' | b'\t' => *suffix = BoundarySuffix::Padding { bytes: 1 },
                    b'\r' => *suffix = BoundarySuffix::CarriageReturn,
                    _ => return Err(invalid_multipart_structure()),
                },
                BoundarySuffix::Dash if byte == b'-' => {
                    self.state = MultipartStructureState::Done;
                }
                BoundarySuffix::Dash => return Err(invalid_multipart_structure()),
                BoundarySuffix::Padding { bytes } if matches!(byte, b' ' | b'\t') => {
                    *bytes = bytes.saturating_add(1);
                    if *bytes > MAX_MULTIPART_BOUNDARY_PADDING_BYTES {
                        return Err(invalid_multipart_structure());
                    }
                }
                BoundarySuffix::Padding { .. } if byte == b'\r' => {
                    *suffix = BoundarySuffix::CarriageReturn;
                }
                BoundarySuffix::Padding { .. } => return Err(invalid_multipart_structure()),
                BoundarySuffix::CarriageReturn if byte == b'\n' => {
                    self.header_terminator.reset();
                    self.state = MultipartStructureState::ReadingHeaders { bytes: 0 };
                }
                BoundarySuffix::CarriageReturn => return Err(invalid_multipart_structure()),
            },
            MultipartStructureState::ReadingHeaders { bytes } => {
                *bytes = bytes.saturating_add(1);
                if *bytes > MAX_MULTIPART_PART_HEADER_BYTES {
                    return Err(invalid_multipart_structure());
                }
                if self.header_terminator.push(byte) {
                    self.field_boundary.reset();
                    self.state = MultipartStructureState::ReadingData;
                }
            }
            MultipartStructureState::ReadingData => {
                if self.field_boundary.push(byte) {
                    self.state =
                        MultipartStructureState::ReadingBoundarySuffix(BoundarySuffix::Start);
                }
            }
            MultipartStructureState::Done => {}
        }
        Ok(())
    }

    fn finish(&self) -> Result<(), io::Error> {
        matches!(self.state, MultipartStructureState::Done)
            .then_some(())
            .ok_or_else(invalid_multipart_structure)
    }
}

struct BytePatternMatcher {
    pattern: Vec<u8>,
    prefix: Vec<usize>,
    matched: usize,
}

impl BytePatternMatcher {
    fn new(pattern: Vec<u8>) -> Self {
        debug_assert!(!pattern.is_empty());
        let mut prefix = vec![0; pattern.len()];
        let mut matched = 0;
        for index in 1..pattern.len() {
            while matched > 0 && pattern[index] != pattern[matched] {
                matched = prefix[matched - 1];
            }
            if pattern[index] == pattern[matched] {
                matched += 1;
            }
            prefix[index] = matched;
        }
        Self {
            pattern,
            prefix,
            matched: 0,
        }
    }

    fn push(&mut self, byte: u8) -> bool {
        while self.matched > 0 && byte != self.pattern[self.matched] {
            self.matched = self.prefix[self.matched - 1];
        }
        if byte == self.pattern[self.matched] {
            self.matched += 1;
        }
        if self.matched == self.pattern.len() {
            self.matched = 0;
            true
        } else {
            false
        }
    }

    fn matched(&self) -> usize {
        self.matched
    }

    fn reset(&mut self) {
        self.matched = 0;
    }
}

fn invalid_multipart_structure() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid multipart structure")
}

struct CodexEditFields {
    prompt: String,
    background: Option<String>,
    model: String,
    n: Option<u64>,
    quality: Option<String>,
    size: Option<String>,
}

impl CodexEditFields {
    fn parse(fields: &[MultipartTextField]) -> Result<Self, ImageEditBodyError> {
        let prompt = required_text_field(fields, "prompt")?;
        let model = required_text_field(fields, "model")?;
        let background = validated_optional_text_field(
            fields,
            "background",
            &["transparent", "opaque", "auto"],
        )?;
        let quality =
            validated_optional_text_field(fields, "quality", &["low", "medium", "high", "auto"])?;
        let size = optional_text_field(fields, "size")?;
        let n = optional_text_field(fields, "n")?
            .map(|value| {
                value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| ImageEditBodyError::CodexInvalidField)
            })
            .transpose()?;
        if let Some(stream) = optional_text_field(fields, "stream")?
            && !(stream.trim().eq_ignore_ascii_case("false") || stream.trim() == "0")
        {
            return Err(ImageEditBodyError::StreamingUnsupported);
        }
        Ok(Self {
            prompt,
            background,
            model,
            n,
            quality,
            size,
        })
    }

    async fn write_json_fields(
        &self,
        writer: &mut ReplayableBodyWriter,
    ) -> Result<(), ImageEditBodyError> {
        write_json_field(writer, "prompt", &Value::String(self.prompt.clone())).await?;
        if let Some(background) = &self.background {
            write_json_field(writer, "background", &Value::String(background.clone())).await?;
        }
        write_json_field(writer, "model", &Value::String(self.model.clone())).await?;
        if let Some(n) = self.n {
            write_json_field(writer, "n", &Value::Number(Number::from(n))).await?;
        }
        if let Some(quality) = &self.quality {
            write_json_field(writer, "quality", &Value::String(quality.clone())).await?;
        }
        if let Some(size) = &self.size {
            write_json_field(writer, "size", &Value::String(size.clone())).await?;
        }
        Ok(())
    }
}

fn required_text_field(
    fields: &[MultipartTextField],
    name: &'static str,
) -> Result<String, ImageEditBodyError> {
    optional_text_field(fields, name)?.ok_or(ImageEditBodyError::CodexMissingField)
}

fn validated_optional_text_field(
    fields: &[MultipartTextField],
    name: &'static str,
    allowed: &[&str],
) -> Result<Option<String>, ImageEditBodyError> {
    let value = optional_text_field(fields, name)?;
    if value
        .as_deref()
        .is_some_and(|value| !allowed.contains(&value))
    {
        return Err(ImageEditBodyError::CodexInvalidField);
    }
    Ok(value)
}

fn optional_text_field(
    fields: &[MultipartTextField],
    name: &'static str,
) -> Result<Option<String>, ImageEditBodyError> {
    let mut matches = fields.iter().filter(|field| field.name == name);
    let Some(field) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err(ImageEditBodyError::CodexDuplicateField);
    }
    std::str::from_utf8(&field.value)
        .map(str::to_owned)
        .map(Some)
        .map_err(|_| ImageEditBodyError::CodexInvalidField)
}

async fn write_json_field(
    writer: &mut ReplayableBodyWriter,
    name: &'static str,
    value: &Value,
) -> Result<(), ImageEditBodyError> {
    writer.write(b",\"").await?;
    writer.write(name.as_bytes()).await?;
    writer.write(b"\":").await?;
    let encoded = serde_json::to_vec(value).map_err(|_| ImageEditBodyError::InvalidJson)?;
    writer.write(&encoded).await
}

fn image_mime(field: &multer::Field<'_>) -> Result<String, ImageEditBodyError> {
    if let Some(content_type) = field.content_type() {
        let mime = content_type.essence_str();
        if mime.starts_with("image/") && mime.len() <= MAX_CODEX_IMAGE_MIME_BYTES {
            return Ok(mime.to_owned());
        }
        return Err(ImageEditBodyError::CodexImageContentType);
    }
    let file_name = field
        .file_name()
        .map(str::to_ascii_lowercase)
        .ok_or(ImageEditBodyError::CodexImageContentType)?;
    let mime = if file_name.ends_with(".png") {
        "image/png"
    } else if file_name.ends_with(".jpg") || file_name.ends_with(".jpeg") {
        "image/jpeg"
    } else if file_name.ends_with(".webp") {
        "image/webp"
    } else if file_name.ends_with(".gif") {
        "image/gif"
    } else {
        return Err(ImageEditBodyError::CodexImageContentType);
    };
    Ok(mime.to_owned())
}

async fn write_base64_field(
    field: &mut multer::Field<'_>,
    writer: &mut ReplayableBodyWriter,
    max_file_bytes: usize,
) -> Result<(), ImageEditBodyError> {
    let mut remainder = Vec::with_capacity(2);
    let mut length = 0_usize;
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| ImageEditBodyError::MalformedMultipart)?
    {
        length = length.saturating_add(chunk.len());
        if length > max_file_bytes {
            return Err(ImageEditBodyError::FileTooLarge);
        }
        let mut input = Vec::with_capacity(remainder.len().saturating_add(chunk.len()));
        input.extend_from_slice(&remainder);
        input.extend_from_slice(&chunk);
        let encode_bytes = input.len() / 3 * 3;
        if encode_bytes > 0 {
            let encoded = BASE64_STANDARD.encode(&input[..encode_bytes]);
            writer.write(encoded.as_bytes()).await?;
        }
        remainder.clear();
        remainder.extend_from_slice(&input[encode_bytes..]);
    }
    if !remainder.is_empty() {
        let encoded = BASE64_STANDARD.encode(&remainder);
        writer.write(encoded.as_bytes()).await?;
    }
    Ok(())
}

#[derive(Clone)]
pub(crate) enum ReplayableRequestBody {
    Memory(Bytes),
    TempFile {
        body: Arc<TempFileBody>,
        length: usize,
    },
}

impl fmt::Debug for ReplayableRequestBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplayableRequestBody")
            .field(
                "storage",
                &match self {
                    Self::Memory(_) => "memory",
                    Self::TempFile { .. } => "temp_file",
                },
            )
            .field("length", &self.len())
            .finish()
    }
}

impl ReplayableRequestBody {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Memory(body) => body.len(),
            Self::TempFile { length, .. } => *length,
        }
    }

    pub(crate) async fn reqwest_body(&self) -> Result<reqwest::Body, ImageEditBodyError> {
        match self {
            Self::Memory(body) => Ok(reqwest::Body::from(body.clone())),
            Self::TempFile { .. } => Ok(reqwest::Body::wrap_stream(self.stream().await?)),
        }
    }

    async fn stream(&self) -> Result<ReplayStream, ImageEditBodyError> {
        match self {
            Self::Memory(body) => {
                let body = body.clone();
                // `multer` drains streams that remain immediately ready. Yield
                // between chunks, including before EOF, so a configured large
                // memory threshold cannot become one parser-sized allocation.
                Ok(Box::pin(stream::try_unfold(
                    (body, 0_usize),
                    |(body, offset)| async move {
                        tokio::task::yield_now().await;
                        if offset >= body.len() {
                            return Ok(None);
                        }
                        let end = offset.saturating_add(BODY_READ_CHUNK_BYTES).min(body.len());
                        Ok::<_, io::Error>(Some((body.slice(offset..end), (body, end))))
                    },
                )))
            }
            Self::TempFile { body, .. } => {
                let accounting = Arc::clone(&body.accounting);
                let mut file = Arc::clone(&body.file).lock_owned().await;
                if file.seek(io::SeekFrom::Start(0)).await.is_err() {
                    accounting
                        .metrics
                        .storage_failures_total
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(ImageEditBodyError::StorageUnavailable);
                }
                Ok(Box::pin(stream::try_unfold(
                    (file, accounting),
                    |(mut file, accounting)| async move {
                        // Keep parser and outbound reads cooperatively chunked
                        // even when the local filesystem completes immediately.
                        tokio::task::yield_now().await;
                        let mut buffer = vec![0_u8; BODY_READ_CHUNK_BYTES];
                        let read = match file.read(&mut buffer).await {
                            Ok(read) => read,
                            Err(error) => {
                                accounting
                                    .metrics
                                    .storage_failures_total
                                    .fetch_add(1, Ordering::Relaxed);
                                return Err(error);
                            }
                        };
                        if read == 0 {
                            Ok(None)
                        } else {
                            buffer.truncate(read);
                            Ok(Some((Bytes::from(buffer), (file, accounting))))
                        }
                    },
                )))
            }
        }
    }
}

pub(crate) struct TempFileBody {
    file: Arc<Mutex<File>>,
    accounting: Arc<TempFileAccounting>,
}

struct TempFileAccounting {
    metrics: Arc<ImageBodySpoolMetrics>,
    length: AtomicU64,
}

impl TempFileAccounting {
    fn new(metrics: Arc<ImageBodySpoolMetrics>) -> Arc<Self> {
        metrics.active_files.fetch_add(1, Ordering::Relaxed);
        Arc::new(Self {
            metrics,
            length: AtomicU64::new(0),
        })
    }

    fn add_bytes(&self, length: usize) {
        let length = u64::try_from(length).unwrap_or(u64::MAX);
        self.length.fetch_add(length, Ordering::Relaxed);
        self.metrics
            .active_bytes
            .fetch_add(length, Ordering::Relaxed);
    }
}

impl Drop for TempFileAccounting {
    fn drop(&mut self) {
        self.metrics.active_files.fetch_sub(1, Ordering::Relaxed);
        self.metrics
            .active_bytes
            .fetch_sub(self.length.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

enum WriterStorage {
    Memory(BytesMut),
    TempFile {
        file: File,
        accounting: Arc<TempFileAccounting>,
    },
}

struct ReplayableBodyWriter {
    store: ImageBodyStore,
    storage: WriterStorage,
    length: usize,
    memory_bytes: usize,
    max_bytes: usize,
}

impl ReplayableBodyWriter {
    fn new(store: ImageBodyStore, memory_bytes: usize, max_bytes: usize) -> Self {
        Self {
            store,
            storage: WriterStorage::Memory(BytesMut::with_capacity(memory_bytes.min(64 * 1_024))),
            length: 0,
            memory_bytes,
            max_bytes,
        }
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), ImageEditBodyError> {
        let next = self
            .length
            .checked_add(bytes.len())
            .ok_or(ImageEditBodyError::BodyTooLarge)?;
        if next > self.max_bytes {
            return Err(ImageEditBodyError::BodyTooLarge);
        }
        if matches!(self.storage, WriterStorage::Memory(_)) && next > self.memory_bytes {
            self.spill().await?;
        }
        match &mut self.storage {
            WriterStorage::Memory(body) => body.extend_from_slice(bytes),
            WriterStorage::TempFile { file, accounting } => {
                for chunk in bytes.chunks(BODY_READ_CHUNK_BYTES) {
                    if file.write_all(chunk).await.is_err() {
                        return Err(self.store.storage_failed());
                    }
                    accounting.add_bytes(chunk.len());
                }
            }
        }
        self.length = next;
        Ok(())
    }

    async fn spill(&mut self) -> Result<(), ImageEditBodyError> {
        let memory =
            match std::mem::replace(&mut self.storage, WriterStorage::Memory(BytesMut::new())) {
                WriterStorage::Memory(memory) => memory,
                WriterStorage::TempFile { file, accounting } => {
                    self.storage = WriterStorage::TempFile { file, accounting };
                    return Ok(());
                }
            };
        let (mut file, accounting) = self.store.create_file().await?;
        for chunk in memory.chunks(BODY_READ_CHUNK_BYTES) {
            if file.write_all(chunk).await.is_err() {
                return Err(self.store.storage_failed());
            }
            accounting.add_bytes(chunk.len());
        }
        self.storage = WriterStorage::TempFile { file, accounting };
        Ok(())
    }

    async fn finish(self) -> Result<ReplayableRequestBody, ImageEditBodyError> {
        match self.storage {
            WriterStorage::Memory(body) => Ok(ReplayableRequestBody::Memory(body.freeze())),
            WriterStorage::TempFile {
                mut file,
                accounting,
            } => {
                file.flush()
                    .await
                    .map_err(|_| self.store.storage_failed())?;
                file.seek(io::SeekFrom::Start(0))
                    .await
                    .map_err(|_| self.store.storage_failed())?;
                let length = u64::try_from(self.length).unwrap_or(u64::MAX);
                self.store
                    .metrics
                    .spooled_total
                    .fetch_add(1, Ordering::Relaxed);
                self.store
                    .metrics
                    .spooled_bytes_total
                    .fetch_add(length, Ordering::Relaxed);
                let available_bytes = self.store.available_space().await;
                let low_capacity = available_bytes
                    .is_some_and(|available| available < self.store.warning_floor_bytes);
                if low_capacity {
                    tracing::warn!(
                        target: "ai_gateway::image_body_spool",
                        event = "image_body_spooled",
                        body_bytes = length,
                        available_bytes = ?available_bytes,
                        "Images edit spool filesystem is low on available capacity"
                    );
                } else {
                    tracing::debug!(
                        target: "ai_gateway::image_body_spool",
                        event = "image_body_spooled",
                        body_bytes = length,
                        available_bytes = ?available_bytes,
                        "Images edit request body spilled to disk"
                    );
                }
                Ok(ReplayableRequestBody::TempFile {
                    body: Arc::new(TempFileBody {
                        file: Arc::new(Mutex::new(file)),
                        accounting,
                    }),
                    length: self.length,
                })
            }
        }
    }
}

#[derive(Clone)]
struct ImageBodyStore {
    directory: Arc<PathBuf>,
    metrics: Arc<ImageBodySpoolMetrics>,
    warning_floor_bytes: u64,
}

impl ImageBodyStore {
    fn new(directory: PathBuf, max_body_bytes: usize) -> Self {
        Self {
            directory: Arc::new(directory),
            metrics: Arc::new(ImageBodySpoolMetrics::default()),
            warning_floor_bytes: u64::try_from(max_body_bytes)
                .unwrap_or(u64::MAX)
                .saturating_mul(SPOOL_CAPACITY_WARNING_BODY_MULTIPLIER),
        }
    }

    async fn ensure_directory(&self) -> Result<(), io::Error> {
        tokio::fs::create_dir_all(self.directory.as_path()).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(
                self.directory.as_path(),
                std::fs::Permissions::from_mode(0o700),
            )
            .await?;
        }
        Ok(())
    }

    async fn create_file(&self) -> Result<(File, Arc<TempFileAccounting>), ImageEditBodyError> {
        self.ensure_directory()
            .await
            .map_err(|_| self.storage_failed())?;
        let directory = Arc::clone(&self.directory);
        let file = tokio::task::spawn_blocking(move || {
            let file = tempfile::tempfile_in(directory.as_path())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            Ok::<_, io::Error>(file)
        })
        .await
        .map_err(|_| self.storage_failed())?
        .map_err(|_| self.storage_failed())?;
        Ok((
            File::from_std(file),
            TempFileAccounting::new(Arc::clone(&self.metrics)),
        ))
    }

    fn storage_failed(&self) -> ImageEditBodyError {
        self.metrics
            .storage_failures_total
            .fetch_add(1, Ordering::Relaxed);
        ImageEditBodyError::StorageUnavailable
    }

    async fn available_space(&self) -> Option<u64> {
        let directory = Arc::clone(&self.directory);
        tokio::task::spawn_blocking(move || fs2::available_space(directory.as_path()))
            .await
            .ok()
            .and_then(Result::ok)
    }

    async fn snapshot(&self) -> ImageBodySpoolSnapshot {
        let available_bytes = if self.ensure_directory().await.is_ok() {
            self.available_space().await
        } else {
            None
        };
        ImageBodySpoolSnapshot {
            active_files: self.metrics.active_files.load(Ordering::Relaxed),
            active_bytes: self.metrics.active_bytes.load(Ordering::Relaxed),
            available_bytes,
            spooled_total: self.metrics.spooled_total.load(Ordering::Relaxed),
            spooled_bytes_total: self.metrics.spooled_bytes_total.load(Ordering::Relaxed),
            storage_failures_total: self.metrics.storage_failures_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct ImageBodySpoolMetrics {
    active_files: AtomicU64,
    active_bytes: AtomicU64,
    spooled_total: AtomicU64,
    spooled_bytes_total: AtomicU64,
    storage_failures_total: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ImageBodySpoolSnapshot {
    pub(crate) active_files: u64,
    pub(crate) active_bytes: u64,
    pub(crate) available_bytes: Option<u64>,
    pub(crate) spooled_total: u64,
    pub(crate) spooled_bytes_total: u64,
    pub(crate) storage_failures_total: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ImageEditBodyError {
    RequestPolicy(RequestPolicyError),
    BodyTooLarge,
    FileTooLarge,
    Unreadable,
    UnsupportedContentType,
    UnsupportedContentEncoding,
    MalformedMultipart,
    TooManyFields,
    FieldNameTooLong,
    FileNameTooLong,
    UnexpectedFileField,
    TooManyImages,
    TooManyMasks,
    TextFieldTooLarge,
    MissingImage,
    MissingModel,
    DuplicateModel,
    InvalidModel,
    EmptyModel,
    ModelTooLong,
    StreamingUnsupported,
    InvalidJson,
    JsonTransformUnsupported,
    CodexTooManyImages,
    CodexMissingField,
    CodexDuplicateField,
    CodexInvalidField,
    CodexImageContentType,
    StorageUnavailable,
}

#[cfg(test)]
mod tests {
    use futures_util::TryStreamExt;

    use super::*;

    fn multipart_body(
        boundary: &str,
        model: &str,
        extra_fields: &[(&str, &str)],
        images: &[(&str, &[u8])],
        mask: Option<&[u8]>,
    ) -> Bytes {
        multipart_body_with_model_headers(boundary, model, &[], extra_fields, images, mask)
    }

    fn multipart_body_with_model_headers(
        boundary: &str,
        model: &str,
        model_headers: &[(&str, &str)],
        extra_fields: &[(&str, &str)],
        images: &[(&str, &[u8])],
        mask: Option<&[u8]>,
    ) -> Bytes {
        let mut body = Vec::new();
        let text_field = |body: &mut Vec<u8>, name: &str, value: &str| {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            );
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        };
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(b"Content-Disposition: form-data; name=\"model\"\r\n");
        for (name, value) in model_headers {
            body.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(model.as_bytes());
        body.extend_from_slice(b"\r\n");
        for (name, value) in extra_fields {
            text_field(&mut body, name, value);
        }
        for (index, (mime, bytes)) in images.iter().enumerate() {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"image[]\"; filename=\"image-{index}.png\"\r\nContent-Type: {mime}\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        if let Some(mask) = mask {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                b"Content-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\n",
            );
            body.extend_from_slice(mask);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        Bytes::from(body)
    }

    fn headers(boundary: &str, length: usize) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}")
                .parse()
                .unwrap(),
        );
        headers.insert(CONTENT_LENGTH, length.to_string().parse().unwrap());
        headers
    }

    fn policy(memory_bytes: usize, body_bytes: usize) -> (tempfile::TempDir, ImageEditBodyPolicy) {
        let directory = tempfile::tempdir().unwrap();
        let policy = ImageEditBodyPolicy::new(
            body_bytes,
            body_bytes,
            memory_bytes,
            directory.path().join("spool"),
        );
        (directory, policy)
    }

    async fn replay(body: &ReplayableRequestBody) -> Bytes {
        body.stream()
            .await
            .unwrap()
            .try_fold(BytesMut::new(), |mut bytes, chunk| async move {
                bytes.extend_from_slice(&chunk);
                Ok(bytes)
            })
            .await
            .unwrap()
            .freeze()
    }

    #[tokio::test]
    async fn small_multipart_edit_stays_in_memory_and_preserves_bytes() {
        let boundary = "small-boundary";
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "add a hat")],
            &[("image/png", b"image-bytes")],
            None,
        );
        let (_directory, policy) = policy(1_024 * 1_024, 2 * 1_024 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes.clone()))
            .await
            .unwrap();

        assert_eq!(edit.model(), "gpt-image-2");
        assert_eq!(edit.image_count, 1);
        assert_eq!(edit.mask_count, 0);
        assert!(matches!(edit.body, ReplayableRequestBody::Memory(_)));
        assert_eq!(replay(&edit.body).await, bytes);
    }

    #[tokio::test]
    async fn multipart_structure_guard_bounds_preamble_and_part_headers() {
        let boundary = "structure-limit-boundary";
        let valid = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "test")],
            &[("image/png", b"image")],
            None,
        );
        let (_directory, policy) = policy(1_024, 128 * 1_024);

        let mut chunked_validator = MultipartStructureValidator::new(boundary);
        for chunk in valid.chunks(3) {
            chunked_validator.push(chunk).unwrap();
        }
        chunked_validator.finish().unwrap();

        let long_boundary = "b".repeat(MAX_MULTIPART_BOUNDARY_BYTES + 1);
        let error = policy
            .capture(
                &headers(&long_boundary, valid.len()),
                Body::from(valid.clone()),
            )
            .await
            .err()
            .expect("overlong multipart boundary must fail");
        assert_eq!(error, ImageEditBodyError::UnsupportedContentType);

        let mut long_preamble = vec![b'x'; MAX_MULTIPART_PREAMBLE_BYTES + 1];
        long_preamble.extend_from_slice(&valid);
        let error = policy
            .capture(
                &headers(boundary, long_preamble.len()),
                Body::from(long_preamble),
            )
            .await
            .err()
            .expect("overlong multipart preamble must fail");
        assert_eq!(error, ImageEditBodyError::MalformedMultipart);

        let long_header = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\nX-Fill: {}\r\n\r\ngpt-image-2\r\n--{boundary}--\r\n",
            "x".repeat(MAX_MULTIPART_PART_HEADER_BYTES)
        );
        let error = policy
            .capture(
                &headers(boundary, long_header.len()),
                Body::from(long_header),
            )
            .await
            .err()
            .expect("overlong multipart part headers must fail");
        assert_eq!(error, ImageEditBodyError::MalformedMultipart);
    }

    #[tokio::test]
    async fn multipart_capture_enforces_file_and_entity_header_limits() {
        let boundary = "entity-limit-boundary";
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "test")],
            &[("image/png", b"12345")],
            None,
        );
        let directory = tempfile::tempdir().unwrap();
        let policy =
            ImageEditBodyPolicy::new(128 * 1_024, 4, 1_024, directory.path().join("spool"));
        let error = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes.clone()))
            .await
            .err()
            .expect("an oversized image part must fail");
        assert_eq!(error, ImageEditBodyError::FileTooLarge);

        let mut encoded_headers = headers(boundary, bytes.len());
        encoded_headers.insert(CONTENT_ENCODING, "gzip".parse().unwrap());
        let error = policy
            .capture(&encoded_headers, Body::from(bytes))
            .await
            .err()
            .expect("encoded multipart bodies must fail");
        assert_eq!(error, ImageEditBodyError::UnsupportedContentEncoding);
    }

    #[tokio::test]
    async fn multipart_unknown_file_field_uses_request_policy_contract() {
        let boundary = "unknown-file-boundary";
        let bytes = Bytes::from(format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"model\"\r\n\r\n\
             gpt-image-2\r\n\
             --{boundary}\r\n\
             Content-Disposition: form-data; name=\"image\"; filename=\"image.png\"\r\n\
             Content-Type: image/png\r\n\r\n\
             image\r\n\
             --{boundary}\r\n\
             Content-Disposition: form-data; name=\"future_file\"; filename=\"future.bin\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\
             future\r\n\
             --{boundary}--\r\n"
        ));
        let (_directory, policy) = policy(1_024, 128 * 1_024);
        let error = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .err()
            .expect("unknown multipart file fields must fail");
        let ImageEditBodyError::RequestPolicy(error) = error else {
            panic!("unknown file field must use the shared request policy");
        };
        assert_eq!(error.code(), "request_body_field_unsupported");
        assert!(error.message().contains("future_file"));
    }

    #[tokio::test]
    async fn large_multipart_edit_spills_replays_and_releases_accounting() {
        let boundary = "spill-boundary";
        let image = vec![7_u8; 128 * 1_024];
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "keep the subject")],
            &[("image/png", &image)],
            None,
        );
        let (_directory, policy) = policy(1_024, 512 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes.clone()))
            .await
            .unwrap();

        assert!(matches!(edit.body, ReplayableRequestBody::TempFile { .. }));
        assert_eq!(replay(&edit.body).await, bytes);
        let active = policy.spool_snapshot().await;
        assert_eq!(active.active_files, 1);
        assert_eq!(active.active_bytes, bytes.len() as u64);
        assert_eq!(active.storage_failures_total, 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let spool_mode = std::fs::metadata(policy.store.directory.as_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(spool_mode, 0o700);
            let ReplayableRequestBody::TempFile { body, .. } = &edit.body else {
                unreachable!();
            };
            let file_mode = body
                .file
                .lock()
                .await
                .metadata()
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(file_mode, 0o600);
        }

        drop(edit);
        let released = policy.spool_snapshot().await;
        assert_eq!(released.active_files, 0);
        assert_eq!(released.active_bytes, 0);
        assert_eq!(released.spooled_total, 1);

        tokio::fs::remove_dir_all(policy.store.directory.as_path())
            .await
            .unwrap();
        let recreated = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        assert!(matches!(
            recreated.body,
            ReplayableRequestBody::TempFile { .. }
        ));
    }

    #[tokio::test]
    async fn in_progress_spill_is_accounted_and_released_on_drop() {
        let (_directory, policy) = policy(8, 1_024);
        let mut writer =
            ReplayableBodyWriter::new(policy.store.clone(), policy.memory_bytes, 1_024);

        writer.write(&[1_u8; 32]).await.unwrap();
        let active = policy.spool_snapshot().await;
        assert_eq!(active.active_files, 1);
        assert_eq!(active.active_bytes, 32);
        assert_eq!(active.spooled_total, 0);

        drop(writer);
        let released = policy.spool_snapshot().await;
        assert_eq!(released.active_files, 0);
        assert_eq!(released.active_bytes, 0);
        assert_eq!(released.spooled_total, 0);
    }

    #[tokio::test]
    async fn replay_stream_retains_accounting_after_the_body_handle_is_dropped() {
        let (_directory, policy) = policy(8, 1_024);
        let expected = Bytes::from_static(b"request body held by the stream");
        let mut writer =
            ReplayableBodyWriter::new(policy.store.clone(), policy.memory_bytes, 1_024);
        writer.write(&expected).await.unwrap();
        let body = writer.finish().await.unwrap();
        let stream = body.stream().await.unwrap();

        drop(body);
        assert_eq!(policy.spool_snapshot().await.active_files, 1);

        let replayed = stream
            .try_fold(BytesMut::new(), |mut bytes, chunk| async move {
                bytes.extend_from_slice(&chunk);
                Ok(bytes)
            })
            .await
            .unwrap()
            .freeze();
        assert_eq!(replayed, expected);
        let released = policy.spool_snapshot().await;
        assert_eq!(released.active_files, 0);
        assert_eq!(released.active_bytes, 0);
    }

    #[tokio::test]
    async fn capacity_probe_failure_does_not_count_as_a_request_storage_failure() {
        let directory = tempfile::tempdir().unwrap();
        let invalid_directory = directory.path().join("not-a-directory");
        tokio::fs::write(&invalid_directory, b"file").await.unwrap();
        let policy = ImageEditBodyPolicy::new(4_096, 4_096, 1, invalid_directory);

        let probed = policy.spool_snapshot().await;
        assert_eq!(probed.available_bytes, None);
        assert_eq!(probed.storage_failures_total, 0);

        let boundary = "storage-failure-boundary";
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "test")],
            &[("image/png", b"image")],
            None,
        );
        let error = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .err()
            .expect("spilling into a file path must fail");
        assert_eq!(error, ImageEditBodyError::StorageUnavailable);
        assert_eq!(policy.spool_snapshot().await.storage_failures_total, 1);
    }

    #[tokio::test]
    async fn model_alias_rebuilds_multipart_without_loading_the_image() {
        let boundary = "alias-boundary";
        let image = vec![9_u8; 96 * 1_024];
        let bytes = multipart_body_with_model_headers(
            boundary,
            "client-image",
            &[("Content-Length", "12"), ("Digest", "sha-256=stale")],
            &[("prompt", "add a blue sky")],
            &[("image/png", &image)],
            None,
        );
        let (_directory, policy) = policy(1_024, 512 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap()
            .rewrite_model("upstream-image")
            .unwrap();

        assert_eq!(edit.model(), "upstream-image");
        let rebuilt = edit.clone().into_openai_replayable().await.unwrap();
        let rebuilt_text = String::from_utf8_lossy(&replay(&rebuilt).await).to_ascii_lowercase();
        assert!(!rebuilt_text.contains("content-length:"));
        assert!(!rebuilt_text.contains("digest:"));
        let inspection = inspect_multipart(&rebuilt, boundary, 512 * 1_024)
            .await
            .unwrap();
        assert_eq!(inspection.model, "upstream-image");
        assert_eq!(inspection.image_count, 1);
        let adapted: Value =
            serde_json::from_slice(&replay(&edit.to_codex_json().await.unwrap()).await).unwrap();
        assert_eq!(adapted["model"], "upstream-image");
    }

    #[tokio::test]
    async fn codex_adapter_streams_images_into_data_urls() {
        let boundary = "codex-boundary";
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[
                ("prompt", "add a red hat"),
                ("background", "auto"),
                ("n", "1"),
                ("quality", "high"),
                ("size", "1024x1024"),
            ],
            &[("image/png", b"first"), ("image/jpeg", b"second-image")],
            None,
        );
        let (_directory, policy) = policy(16, 512 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        let adapted = edit.to_codex_json().await.unwrap();
        let value: Value = serde_json::from_slice(&replay(&adapted).await).unwrap();

        assert_eq!(value["model"], "gpt-image-2");
        assert_eq!(value["prompt"], "add a red hat");
        assert_eq!(value["background"], "auto");
        assert_eq!(value["n"], 1);
        assert_eq!(value["quality"], "high");
        assert_eq!(value["size"], "1024x1024");
        assert_eq!(
            value["images"][0]["image_url"],
            format!("data:image/png;base64,{}", BASE64_STANDARD.encode(b"first"))
        );
        assert_eq!(
            value["images"][1]["image_url"],
            format!(
                "data:image/jpeg;base64,{}",
                BASE64_STANDARD.encode(b"second-image")
            )
        );
    }

    #[tokio::test]
    async fn codex_adapter_accepts_compatible_openai_edit_defaults() {
        let boundary = "codex-openai-defaults-boundary";
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[
                ("prompt", "改为蓝色衣服"),
                ("size", "auto"),
                ("output_format", "png"),
                ("moderation", "auto"),
                ("quality", "auto"),
            ],
            &[("image/png", b"image")],
            None,
        );
        let (_directory, policy) = policy(16, 128 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        let (body, client_changed) = PreparedRequestBody::ImageEdit(edit)
            .apply_policy(RequestPolicyLayer::Client, RequestInterface::ImagesEdit)
            .unwrap();
        assert!(client_changed);
        let standard = body.clone().into_openai_replayable().await.unwrap();
        let standard_bytes = replay(&standard).await;
        let standard_text = String::from_utf8_lossy(&standard_bytes);
        assert!(!standard_text.contains("name=\"moderation\""));
        assert!(standard_text.contains("name=\"output_format\""));

        let (body, codex_changed) = body
            .apply_policy(RequestPolicyLayer::CodexOauth, RequestInterface::ImagesEdit)
            .unwrap();
        assert!(codex_changed);
        let adapted = body.image_edit().unwrap().to_codex_json().await.unwrap();
        let value: Value = serde_json::from_slice(&replay(&adapted).await).unwrap();

        assert_eq!(value["model"], "gpt-image-2");
        assert_eq!(value["prompt"], "改为蓝色衣服");
        assert_eq!(value["size"], "auto");
        assert_eq!(value["quality"], "auto");
        assert!(value.get("output_format").is_none());
        assert!(value.get("moderation").is_none());
    }

    #[tokio::test]
    async fn codex_adapter_accounts_for_worst_case_json_text_escaping() {
        let boundary = "codex-escaped-text-boundary";
        let prompt = "\u{0001}".repeat(2_048);
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", prompt.as_str())],
            &[("image/png", b"image")],
            None,
        );
        let (_directory, policy) = policy(16, 128 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        let adapted = edit.to_codex_json().await.unwrap();
        let value: Value = serde_json::from_slice(&replay(&adapted).await).unwrap();

        assert_eq!(value["prompt"], prompt);
    }

    #[tokio::test]
    async fn codex_adapter_enforces_provider_specific_limits() {
        let boundary = "codex-limit-boundary";
        let images = (0..6)
            .map(|_| ("image/png", b"x".as_slice()))
            .collect::<Vec<_>>();
        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "compose")],
            &images,
            None,
        );
        let (_directory, policy) = policy(1_024, 512 * 1_024);
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        assert_eq!(
            edit.to_codex_json().await.unwrap_err(),
            ImageEditBodyError::CodexTooManyImages
        );

        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "compose")],
            &[("image/png", b"x")],
            Some(b"mask"),
        );
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        let error = edit.to_codex_json().await.unwrap_err();
        let ImageEditBodyError::RequestPolicy(error) = error else {
            panic!("Codex mask rejection must use the shared request policy");
        };
        assert_eq!(error.code(), "codex_request_body_field_unsupported");

        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "compose")],
            &[("text/plain", b"x")],
            None,
        );
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        assert_eq!(
            edit.to_codex_json().await.unwrap_err(),
            ImageEditBodyError::CodexImageContentType
        );

        let bytes = multipart_body(
            boundary,
            "gpt-image-2",
            &[("prompt", "compose"), ("quality", "ultra")],
            &[("image/png", b"x")],
            None,
        );
        let edit = policy
            .capture(&headers(boundary, bytes.len()), Body::from(bytes))
            .await
            .unwrap();
        assert_eq!(
            edit.to_codex_json().await.unwrap_err(),
            ImageEditBodyError::CodexInvalidField
        );
    }
}
