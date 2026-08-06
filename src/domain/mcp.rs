//! Statically registered MCP server kinds and their compiled runtime settings.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
#[cfg(feature = "mcp-server")]
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::CompiledModelRule;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpServerKind {
    WebSearch,
    Image,
}

impl McpServerKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WebSearch => "web_search",
            Self::Image => "image",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "web_search" => Some(Self::WebSearch),
            "image" => Some(Self::Image),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSearchExternalWebAccess {
    Cached,
    Indexed,
    #[default]
    Live,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSearchContextSize {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct McpSearchTokenLimits {
    #[serde(default = "default_short_output_tokens")]
    pub short: u64,
    #[serde(default = "default_medium_output_tokens")]
    pub medium: u64,
    #[serde(default = "default_long_output_tokens")]
    pub long: u64,
}

impl Default for McpSearchTokenLimits {
    fn default() -> Self {
        Self {
            short: default_short_output_tokens(),
            medium: default_medium_output_tokens(),
            long: default_long_output_tokens(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchMcpSettings {
    #[serde(default)]
    pub external_web_access: McpSearchExternalWebAccess,
    #[serde(default)]
    pub search_context_size: McpSearchContextSize,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    #[serde(default)]
    pub max_output_tokens: McpSearchTokenLimits,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpImageBackground {
    #[default]
    Auto,
    Opaque,
    Transparent,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpImageQuality {
    #[default]
    Auto,
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMcpSettings {
    #[serde(default)]
    pub background: McpImageBackground,
    #[serde(default)]
    pub quality: McpImageQuality,
    #[serde(default = "default_image_size")]
    pub size: String,
}

impl Default for ImageMcpSettings {
    fn default() -> Self {
        Self {
            background: McpImageBackground::default(),
            quality: McpImageQuality::default(),
            size: default_image_size(),
        }
    }
}

fn default_image_size() -> String {
    "auto".into()
}

const fn default_short_output_tokens() -> u64 {
    1_000
}

const fn default_medium_output_tokens() -> u64 {
    3_000
}

const fn default_long_output_tokens() -> u64 {
    6_000
}

#[derive(Clone, Debug)]
enum CompiledMcpServerSettings {
    WebSearch {
        settings: WebSearchMcpSettings,
        #[cfg(feature = "mcp-server")]
        continuation_scope: [u8; 32],
    },
    Image(ImageMcpSettings),
}

#[derive(Clone, Debug)]
pub struct CompiledMcpServer {
    id: Uuid,
    slug: Arc<str>,
    name: Arc<str>,
    description: Option<Arc<str>>,
    model_rule: Arc<CompiledModelRule>,
    settings: CompiledMcpServerSettings,
}

impl CompiledMcpServer {
    #[must_use]
    pub fn new_web_search(
        id: Uuid,
        slug: Arc<str>,
        name: Arc<str>,
        description: Option<Arc<str>>,
        model_rule: Arc<CompiledModelRule>,
        settings: WebSearchMcpSettings,
    ) -> Self {
        #[cfg(feature = "mcp-server")]
        let continuation_scope = web_search_continuation_scope(&model_rule, &settings);
        Self {
            id,
            slug,
            name,
            description,
            model_rule,
            settings: CompiledMcpServerSettings::WebSearch {
                settings,
                #[cfg(feature = "mcp-server")]
                continuation_scope,
            },
        }
    }

    #[must_use]
    pub fn new_image(
        id: Uuid,
        slug: Arc<str>,
        name: Arc<str>,
        description: Option<Arc<str>>,
        model_rule: Arc<CompiledModelRule>,
        settings: ImageMcpSettings,
    ) -> Self {
        Self {
            id,
            slug,
            name,
            description,
            model_rule,
            settings: CompiledMcpServerSettings::Image(settings),
        }
    }

    #[must_use]
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    #[must_use]
    pub fn kind(&self) -> McpServerKind {
        match &self.settings {
            CompiledMcpServerSettings::WebSearch { .. } => McpServerKind::WebSearch,
            CompiledMcpServerSettings::Image(_) => McpServerKind::Image,
        }
    }

    #[must_use]
    pub fn model_rule(&self) -> &Arc<CompiledModelRule> {
        &self.model_rule
    }

    #[must_use]
    pub fn web_search_settings(&self) -> Option<&WebSearchMcpSettings> {
        match &self.settings {
            CompiledMcpServerSettings::WebSearch { settings, .. } => Some(settings),
            CompiledMcpServerSettings::Image(_) => None,
        }
    }

    #[must_use]
    pub fn image_settings(&self) -> Option<&ImageMcpSettings> {
        match &self.settings {
            CompiledMcpServerSettings::Image(settings) => Some(settings),
            CompiledMcpServerSettings::WebSearch { .. } => None,
        }
    }

    #[cfg(feature = "mcp-server")]
    #[must_use]
    pub(crate) fn continuation_scope(&self) -> Option<&[u8; 32]> {
        match &self.settings {
            CompiledMcpServerSettings::WebSearch {
                continuation_scope, ..
            } => Some(continuation_scope),
            CompiledMcpServerSettings::Image(_) => None,
        }
    }
}

#[cfg(feature = "mcp-server")]
fn web_search_continuation_scope(
    model_rule: &CompiledModelRule,
    settings: &WebSearchMcpSettings,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ai-gateway/mcp/web-search-continuation-scope/v1\0");
    hasher.update(model_rule.id().as_bytes());
    hasher.update(model_rule.upstream_model_id().as_bytes());
    hash_value(&mut hasher, model_rule.client_model().as_bytes());
    hash_value(&mut hasher, model_rule.upstream_model().as_bytes());
    let settings = serde_json::to_vec(settings).expect("typed MCP settings serialize");
    hash_value(&mut hasher, &settings);
    hasher.finalize().into()
}

#[cfg(feature = "mcp-server")]
fn hash_value(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
