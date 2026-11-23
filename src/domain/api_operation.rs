use serde::{Deserialize, Serialize};

use super::ApiFormat;

/// The concrete public API operation for one logical request.
///
/// Operations isolate routing within an API format, including HTTP versus
/// WebSocket Responses. The selected candidate must belong to an authorized channel.
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
