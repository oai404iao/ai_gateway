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

/// Request-body compression selected by a channel group.
///
/// `Default` deliberately means no request compression. Keeping it as an
/// explicit wire value leaves room for additional algorithms without changing
/// the control-plane shape.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestCompression {
    #[default]
    Default,
    Zstd,
}

impl RequestCompression {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Zstd => "zstd",
        }
    }

    #[must_use]
    pub const fn is_encoded(self) -> bool {
        matches!(self, Self::Zstd)
    }
}
