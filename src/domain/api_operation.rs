use serde::{Deserialize, Serialize};

use super::ApiFormat;

/// The concrete public API operation for one logical request.
///
/// `ApiFormat` remains the routing and authorization dimension. Operations
/// distinguish paths inside one format, such as image generation versus image
/// editing.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiOperation {
    ChatCompletions,
    Responses,
    StandaloneWebSearch,
    ImagesGeneration,
    ImagesEdit,
}

impl ApiOperation {
    #[must_use]
    pub const fn api_format(self) -> ApiFormat {
        match self {
            Self::ChatCompletions => ApiFormat::OpenAiChatCompletions,
            Self::Responses | Self::StandaloneWebSearch => ApiFormat::OpenAiResponses,
            Self::ImagesGeneration | Self::ImagesEdit => ApiFormat::OpenAiImages,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Responses => "responses",
            Self::StandaloneWebSearch => "standalone_web_search",
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
        !matches!(self, Self::ImagesGeneration | Self::ImagesEdit)
    }

    #[must_use]
    pub const fn is_images(self) -> bool {
        matches!(self, Self::ImagesGeneration | Self::ImagesEdit)
    }
}
