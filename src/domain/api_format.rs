use serde::{Deserialize, Serialize};

/// The only client API formats supported by the gateway.
///
/// Rules, channel groups, and channels must always use the same format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    OpenAiChatCompletions,
    OpenAiResponses,
}
