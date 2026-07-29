use serde::{Deserialize, Serialize};

/// The in-process connector that adapts one client API format to an upstream.
///
/// `ApiFormat` remains the client-visible protocol. Connector kinds describe
/// upstream behavior and therefore must not be used as API-key permissions or
/// model-rule formats.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    #[default]
    OpenAiCompatible,
    CodexOauth,
}

impl ConnectorKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "openai_compatible" => Some(Self::OpenAiCompatible),
            "codex_oauth" => Some(Self::CodexOauth),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::CodexOauth => "codex_oauth",
        }
    }
}
