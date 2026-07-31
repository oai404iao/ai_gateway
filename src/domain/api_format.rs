use serde::{Deserialize, Serialize};

/// The client API formats supported by the gateway.
///
/// Rules, channel groups, and channels must always use the same format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    OpenAiChatCompletions,
    OpenAiResponses,
    OpenAiImages,
}

impl ApiFormat {
    pub const ALL: [Self; 3] = [
        Self::OpenAiChatCompletions,
        Self::OpenAiResponses,
        Self::OpenAiImages,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiChatCompletions => "open_ai_chat_completions",
            Self::OpenAiResponses => "open_ai_responses",
            Self::OpenAiImages => "open_ai_images",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "open_ai_chat_completions" => Some(Self::OpenAiChatCompletions),
            "open_ai_responses" => Some(Self::OpenAiResponses),
            "open_ai_images" => Some(Self::OpenAiImages),
            _ => None,
        }
    }
}
