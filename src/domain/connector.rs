use serde::{Deserialize, Serialize};

/// The in-process connector that adapts one client API format to an upstream.
///
/// `ApiFormat` remains the client-visible protocol. Connector kinds describe
/// upstream behavior and therefore must not be used as API-key permissions or
/// model-rule formats.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ConnectorKind {
    #[default]
    OpenAiCompatible,
    CodexOauth,
    Plugin(ConnectorId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConnectorId {
    bytes: [u8; 64],
    len: u8,
}

impl ConnectorId {
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..usize::from(self.len)])
            .expect("connector IDs are validated ASCII")
    }
}

impl ConnectorKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "general" => Some(Self::OpenAiCompatible),
            "codex" => Some(Self::CodexOauth),
            _ => {
                if value.is_empty()
                    || value.len() > 64
                    || !value.as_bytes()[0].is_ascii_lowercase()
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'_' | b'-')
                    })
                {
                    return None;
                }
                let mut bytes = [0; 64];
                bytes[..value.len()].copy_from_slice(value.as_bytes());
                Some(Self::Plugin(ConnectorId {
                    bytes,
                    len: value.len() as u8,
                }))
            }
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::OpenAiCompatible => "general",
            Self::CodexOauth => "codex",
            Self::Plugin(id) => id.as_str(),
        }
    }
}

impl Serialize for ConnectorKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ConnectorKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| serde::de::Error::custom("invalid connector ID"))
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectorKind;

    #[test]
    fn connector_ids_are_bounded_and_round_trip() {
        for id in ["general", "codex", "acme-native_v1", &"a".repeat(64)] {
            let parsed = ConnectorKind::parse(id).unwrap();
            assert_eq!(parsed.as_str(), id);
            assert_eq!(
                serde_json::from_value::<ConnectorKind>(serde_json::json!(id)).unwrap(),
                parsed
            );
            assert_eq!(serde_json::to_value(parsed).unwrap(), id);
        }
        for id in ["", "Acme", "0acme", "../acme", "a.b", "á", &"a".repeat(65)] {
            assert!(ConnectorKind::parse(id).is_none());
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
