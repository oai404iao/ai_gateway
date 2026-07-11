use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use reqwest::Url;
use serde::Deserialize;

use super::{ApiFormat, ApiKeyHash};

/// Permissions understood by the first data-plane slice.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
pub enum ApiKeyPermission {
    #[serde(rename = "proxy")]
    Proxy,
    #[serde(rename = "models.read")]
    ModelsRead,
}

/// Authenticated client identity retained in a compiled runtime snapshot.
#[derive(Clone, Debug)]
pub struct CompiledApiKey {
    id: Arc<str>,
    allowed_api_formats: HashSet<ApiFormat>,
    permissions: HashSet<ApiKeyPermission>,
}

impl CompiledApiKey {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn permits(&self, api_format: ApiFormat, permission: ApiKeyPermission) -> bool {
        self.allowed_api_formats.contains(&api_format) && self.permissions.contains(&permission)
    }
}

/// Upstream credentials held only by compiled channels.
#[derive(Clone)]
pub enum UpstreamAuth {
    None,
    Bearer(Arc<str>),
}

impl UpstreamAuth {
    #[must_use]
    pub fn bearer_token(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Bearer(token) => Some(token),
        }
    }
}

impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => formatter.write_str("UpstreamAuth::None"),
            Self::Bearer(_) => formatter.write_str("UpstreamAuth::Bearer(REDACTED)"),
        }
    }
}

/// A single upstream target selected by an MVP model rule.
#[derive(Clone, Debug)]
pub struct CompiledChannel {
    id: Arc<str>,
    api_format: ApiFormat,
    base_url: Url,
    upstream_auth: UpstreamAuth,
}

impl CompiledChannel {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }

    #[must_use]
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    #[must_use]
    pub fn upstream_auth(&self) -> &UpstreamAuth {
        &self.upstream_auth
    }

    pub(crate) fn new(
        id: Arc<str>,
        api_format: ApiFormat,
        base_url: Url,
        upstream_auth: UpstreamAuth,
    ) -> Self {
        Self {
            id,
            api_format,
            base_url,
            upstream_auth,
        }
    }
}

/// Compiled route selected by an exact client model and API format match.
#[derive(Clone, Debug)]
pub struct CompiledModelRule {
    client_model: Arc<str>,
    api_format: ApiFormat,
    upstream_model: Arc<str>,
    channel: Arc<CompiledChannel>,
}

impl CompiledModelRule {
    #[must_use]
    pub fn client_model(&self) -> &str {
        &self.client_model
    }

    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }

    #[must_use]
    pub fn upstream_model(&self) -> &str {
        &self.upstream_model
    }

    #[must_use]
    pub fn channel(&self) -> &Arc<CompiledChannel> {
        &self.channel
    }

    pub(crate) fn new(
        client_model: Arc<str>,
        api_format: ApiFormat,
        upstream_model: Arc<str>,
        channel: Arc<CompiledChannel>,
    ) -> Self {
        Self {
            client_model,
            api_format,
            upstream_model,
            channel,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelRouteKey {
    api_format: ApiFormat,
    client_model: Arc<str>,
}

impl ModelRouteKey {
    #[must_use]
    pub fn new(api_format: ApiFormat, client_model: impl Into<Arc<str>>) -> Self {
        Self {
            api_format,
            client_model: client_model.into(),
        }
    }
}

/// Immutable data-plane lookup tables swapped as one coherent snapshot.
#[derive(Debug)]
pub struct CompiledRuntimeConfig {
    api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
    model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
}

impl CompiledRuntimeConfig {
    #[must_use]
    pub fn new(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    ) -> Self {
        Self {
            api_keys,
            model_rules,
        }
    }

    /// Performs the only runtime client-key lookup. The snapshot indexes the
    /// SHA-256 digest, so plaintext client keys are never retained here.
    #[must_use]
    pub fn authenticate(&self, secret: &str) -> Option<Arc<CompiledApiKey>> {
        self.api_keys.get(&ApiKeyHash::from_secret(secret)).cloned()
    }

    #[must_use]
    pub fn model_rule(
        &self,
        api_format: ApiFormat,
        client_model: &str,
    ) -> Option<Arc<CompiledModelRule>> {
        self.model_rules
            .get(&ModelRouteKey::new(
                api_format,
                Arc::<str>::from(client_model),
            ))
            .cloned()
    }

    /// Lists models reachable under the MVP's format and permission rules.
    #[must_use]
    pub fn models_for(&self, api_key: &CompiledApiKey, api_format: ApiFormat) -> Vec<Arc<str>> {
        if !api_key.permits(api_format, ApiKeyPermission::ModelsRead) {
            return Vec::new();
        }

        let mut models = self
            .model_rules
            .values()
            .filter(|rule| rule.api_format() == api_format)
            .map(|rule| Arc::clone(&rule.client_model))
            .collect::<Vec<_>>();
        models.sort_unstable();
        models
    }
}

impl CompiledApiKey {
    pub(crate) fn new(
        id: Arc<str>,
        allowed_api_formats: HashSet<ApiFormat>,
        permissions: HashSet<ApiKeyPermission>,
    ) -> Self {
        Self {
            id,
            allowed_api_formats,
            permissions,
        }
    }
}
