use std::sync::OnceLock;

use axum::{
    body::Body,
    http::{Request, StatusCode, header::CONTENT_TYPE},
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use rmcp::{
    ErrorData,
    handler::server::tool::schema_for_input,
    model::{CallToolResult, ContentBlock, ImageContent, MetaObject, Tool, ToolAnnotations},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    application::ProxyService,
    domain::{
        ApiOperation, ImageMcpSettings, McpImageBackground, McpImageQuality, RequestLogSource,
    },
};

use super::McpRequestPrincipal;

pub(super) const IMAGEGEN_TOOL_NAME: &str = "image_gen.imagegen";
static IMAGEGEN_TOOL: OnceLock<Tool> = OnceLock::new();
const IMAGEGEN_DESCRIPTION: &str = "Generate one image from a text description. The configured \
MCP server fixes the model, background, quality, and size; callers cannot override them. The \
result is one original-detail PNG image. Generation can take several minutes and is \
non-idempotent: retrying after an uncertain network outcome can create and bill another image.";
const IMAGE_MIME_TYPE: &str = "image/png";
const MAX_PROMPT_CHARS: usize = 32_000;
const MAX_PROMPT_BYTES: usize = 64 * 1_024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImagegenArguments {
    /// Complete description of the image to generate.
    #[schemars(length(min = 1, max = 32000))]
    pub prompt: String,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum ImagegenStatus {
    Completed,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImagegenOutput {
    status: ImagegenStatus,
    mime_type: String,
}

#[derive(Deserialize)]
struct ImagesResponse {
    data: Vec<ImageData>,
}

#[derive(Deserialize)]
struct ImageData {
    #[serde(default)]
    b64_json: Option<String>,
}

#[must_use]
pub(super) fn imagegen_tool() -> Tool {
    IMAGEGEN_TOOL.get_or_init(build_imagegen_tool).clone()
}

fn build_imagegen_tool() -> Tool {
    Tool::new(
        IMAGEGEN_TOOL_NAME,
        IMAGEGEN_DESCRIPTION,
        schema_for_input::<ImagegenArguments>().expect("imagegen input schema is an object"),
    )
    .with_title("Image generation")
    .with_output_schema::<ImagegenOutput>()
    .with_annotations(
        ToolAnnotations::with_title("Image generation")
            .read_only(false)
            .destructive(false)
            .idempotent(false)
            .open_world(true),
    )
}

pub(super) async fn execute_imagegen(
    proxy: &ProxyService,
    principal: McpRequestPrincipal,
    arguments: ImagegenArguments,
    result_limit: usize,
) -> Result<CallToolResult, ErrorData> {
    if arguments.prompt.trim().is_empty()
        || arguments.prompt.chars().count() > MAX_PROMPT_CHARS
        || arguments.prompt.len() > MAX_PROMPT_BYTES
    {
        return Ok(tool_error(format!(
            "prompt must be non-empty, at most {MAX_PROMPT_CHARS} characters, and at most \
             {MAX_PROMPT_BYTES} bytes"
        )));
    }
    let settings = principal
        .server
        .image_settings()
        .ok_or_else(|| ErrorData::internal_error("MCP image kind mismatch", None))?;
    let body = generation_body(
        principal.server.model_rule().client_model(),
        &arguments.prompt,
        settings,
    );
    let body = serde_json::to_vec(&body)
        .map_err(|_| ErrorData::internal_error("failed to encode image request", None))?;
    let request = Request::post("/v1/images/generations")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|_| ErrorData::internal_error("failed to build image request", None))?;

    let response = match proxy
        .proxy_authenticated(
            ApiOperation::ImagesGeneration,
            request,
            principal.snapshot,
            principal.api_key,
            RequestLogSource::Mcp,
        )
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(tool_error(format!(
                "Image generation failed ({}): {}",
                error.status().as_u16(),
                error.message()
            )));
        }
    };
    let status = response.status();
    let bytes = match collect_bounded_body(response.into_body(), result_limit).await {
        Ok(bytes) => bytes,
        Err(BoundedBodyError::TooLarge) => {
            return Ok(tool_error(
                "Image response exceeded the configured MCP result limit.",
            ));
        }
        Err(BoundedBodyError::Unreadable) => {
            return Ok(tool_error("Image upstream response could not be read."));
        }
    };
    if !status.is_success() {
        return Ok(tool_error(image_error_message(status)));
    }
    let parsed = match serde_json::from_slice::<ImagesResponse>(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Ok(tool_error(
                "Image upstream returned an invalid JSON response.",
            ));
        }
    };
    drop(bytes);
    let Some(encoded) = parsed
        .data
        .into_iter()
        .next()
        .and_then(|image| image.b64_json)
    else {
        return Ok(tool_error("Image upstream returned no base64 image data."));
    };
    if let Err(result) = validate_png_base64(&encoded) {
        return Ok(tool_error(result));
    }

    let mut meta = MetaObject::new();
    meta.0
        .insert("codex/imageDetail".into(), Value::String("original".into()));
    let image = ImageContent::new(encoded, IMAGE_MIME_TYPE).with_meta(meta);
    let structured = serde_json::to_value(ImagegenOutput {
        status: ImagegenStatus::Completed,
        mime_type: IMAGE_MIME_TYPE.into(),
    })
    .map_err(|_| ErrorData::internal_error("failed to encode MCP image result", None))?;
    let mut result = CallToolResult::success(vec![ContentBlock::Image(image)]);
    result.structured_content = Some(structured);
    Ok(result)
}

enum BoundedBodyError {
    TooLarge,
    Unreadable,
}

async fn collect_bounded_body(body: Body, limit: usize) -> Result<Bytes, BoundedBodyError> {
    let mut body = body;
    let mut collected = BytesMut::with_capacity(limit.min(64 * 1_024));
    let mut too_large = false;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|_| BoundedBodyError::Unreadable)?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        if !too_large {
            if collected.len().saturating_add(data.len()) > limit {
                too_large = true;
            } else {
                collected.extend_from_slice(&data);
            }
        }
    }
    if too_large {
        Err(BoundedBodyError::TooLarge)
    } else {
        Ok(collected.freeze())
    }
}

fn generation_body(model: &str, prompt: &str, settings: &ImageMcpSettings) -> Value {
    json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "background": match settings.background {
            McpImageBackground::Auto => "auto",
            McpImageBackground::Opaque => "opaque",
            McpImageBackground::Transparent => "transparent",
        },
        "quality": match settings.quality {
            McpImageQuality::Auto => "auto",
            McpImageQuality::Low => "low",
            McpImageQuality::Medium => "medium",
            McpImageQuality::High => "high",
        },
        "size": settings.size.as_str(),
    })
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

fn image_error_message(status: StatusCode) -> String {
    format!(
        "Image generation failed ({}): The image upstream rejected the request.",
        status.as_u16()
    )
}

fn validate_png_base64(encoded: &str) -> Result<(), &'static str> {
    if encoded.is_empty() || encoded.trim().len() != encoded.len() {
        return Err("Image upstream returned invalid base64 image data.");
    }
    let decoded = BASE64_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| "Image upstream returned invalid base64 image data.")?;
    if !decoded.starts_with(PNG_SIGNATURE) {
        return Err("Image upstream returned data that is not a PNG image.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_body_uses_fixed_model_and_settings() {
        let body = generation_body(
            "image-model",
            "paint a moonlit lake",
            &ImageMcpSettings {
                background: McpImageBackground::Opaque,
                quality: McpImageQuality::High,
                size: "1536x1024".into(),
            },
        );

        assert_eq!(
            body,
            json!({
                "model": "image-model",
                "prompt": "paint a moonlit lake",
                "n": 1,
                "background": "opaque",
                "quality": "high",
                "size": "1536x1024",
            })
        );
    }

    #[test]
    fn png_validation_rejects_invalid_base64_and_non_png_data() {
        assert!(validate_png_base64("not-base64").is_err());
        assert!(validate_png_base64("aGVsbG8=").is_err());
        assert!(
            validate_png_base64(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
            )
            .is_ok()
        );
    }
}
