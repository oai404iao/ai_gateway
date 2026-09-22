use serde::{Deserialize, Serialize};

use super::ApiFormat;

/// The concrete public API operation for one logical request.
///
/// Operations isolate routing within an API format, including HTTP versus
/// WebSocket Responses. Format permission alone does not authorize a capability.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiOperation {
    #[serde(rename = "chat_completion")]
    ChatCompletions,
    Responses,
    #[serde(rename = "responses-ws")]
    ResponsesWebSocket,
    #[serde(rename = "web_search")]
    StandaloneWebSearch,
    ImagesGeneration,
    ImagesEdit,
}

impl ApiOperation {
    /// Historical facts and journal versions retain their original encoding.
    /// This normalization is not a Console request deserialization alias.
    pub(crate) fn normalize_stored_name<'a>(name: &'a str, protocol: &str) -> &'a str {
        match name {
            "chat_completions" => "chat_completion",
            "standalone_web_search" => "web_search",
            "responses" if protocol == "websocket" => "responses-ws",
            name => name,
        }
    }

    #[must_use]
    pub const fn api_format(self) -> ApiFormat {
        match self {
            Self::ChatCompletions => ApiFormat::OpenAiChatCompletions,
            Self::Responses | Self::ResponsesWebSocket | Self::StandaloneWebSearch => {
                ApiFormat::OpenAiResponses
            }
            Self::ImagesGeneration | Self::ImagesEdit => ApiFormat::OpenAiImages,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completion",
            Self::Responses => "responses",
            Self::ResponsesWebSocket => "responses-ws",
            Self::StandaloneWebSearch => "web_search",
            Self::ImagesGeneration => "images_generation",
            Self::ImagesEdit => "images_edit",
        }
    }

    /// Baseline `model_rules` rows persist only an API format. Until the joint
    /// capability cutover stores an explicit operation, a legacy row maps to
    /// its format's default operation; sibling operations (standalone search,
    /// image edit) stay unconfigured and fail closed instead of borrowing it.
    #[must_use]
    pub fn for_legacy_format(api_format: &str) -> Self {
        ApiFormat::parse(api_format).map_or(Self::ChatCompletions, Self::legacy_default)
    }

    #[must_use]
    pub const fn legacy_default(api_format: ApiFormat) -> Self {
        match api_format {
            ApiFormat::OpenAiChatCompletions => Self::ChatCompletions,
            ApiFormat::OpenAiResponses => Self::Responses,
            ApiFormat::OpenAiImages => Self::ImagesGeneration,
        }
    }

    #[must_use]
    pub const fn permits_automatic_retry(self) -> bool {
        !matches!(
            self,
            Self::ResponsesWebSocket | Self::ImagesGeneration | Self::ImagesEdit
        )
    }

    #[must_use]
    pub const fn is_images(self) -> bool {
        matches!(self, Self::ImagesGeneration | Self::ImagesEdit)
    }
}
