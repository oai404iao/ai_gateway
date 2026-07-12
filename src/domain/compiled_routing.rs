use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use chrono::{DateTime, Utc};
use reqwest::{Url, header::HeaderName};
use serde::Deserialize;
use uuid::Uuid;

use super::{ApiFormat, ApiKeyHash};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
pub enum ApiKeyPermission {
    #[serde(rename = "proxy")]
    Proxy,
    #[serde(rename = "models.read")]
    ModelsRead,
}

#[derive(Clone, Debug)]
pub struct CompiledApiKey {
    id: Uuid,
    user_id: Uuid,
    allowed_api_formats: HashSet<ApiFormat>,
    permissions: HashSet<ApiKeyPermission>,
    allowed_group_ids: Option<HashSet<Uuid>>,
    expires_at: Option<DateTime<Utc>>,
}
impl CompiledApiKey {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }
    #[must_use]
    pub fn permits(&self, format: ApiFormat, permission: ApiKeyPermission) -> bool {
        self.allowed_api_formats.contains(&format)
            && self.permissions.contains(&permission)
            && !self.is_expired()
    }
    #[must_use]
    pub fn permits_group(&self, group_id: Uuid) -> bool {
        self.allowed_group_ids
            .as_ref()
            .is_none_or(|groups| groups.contains(&group_id))
    }
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|expires| expires <= Utc::now())
    }
    pub(crate) fn new(
        id: Uuid,
        user_id: Uuid,
        formats: HashSet<ApiFormat>,
        permissions: HashSet<ApiKeyPermission>,
        groups: Option<HashSet<Uuid>>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id,
            user_id,
            allowed_api_formats: formats,
            permissions,
            allowed_group_ids: groups,
            expires_at,
        }
    }
}

#[derive(Clone)]
pub enum UpstreamAuth {
    None,
    Bearer(Arc<str>),
    Header { name: HeaderName, value: Arc<str> },
}
impl fmt::Debug for UpstreamAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("UpstreamAuth::None"),
            Self::Bearer(_) => f.write_str("UpstreamAuth::Bearer(REDACTED)"),
            Self::Header { name, .. } => f
                .debug_struct("UpstreamAuth::Header")
                .field("name", name)
                .field("value", &"REDACTED")
                .finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledChannel {
    id: Uuid,
    group_id: Uuid,
    api_format: ApiFormat,
    base_url: Url,
    weight: i32,
    upstream_auth: UpstreamAuth,
    available_models: HashSet<Arc<str>>,
}
impl CompiledChannel {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn group_id(&self) -> Uuid {
        self.group_id
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
    pub fn weight(&self) -> i32 {
        self.weight
    }
    #[must_use]
    pub fn upstream_auth(&self) -> &UpstreamAuth {
        &self.upstream_auth
    }
    #[must_use]
    pub fn supports_model(&self, model: &str) -> bool {
        self.available_models.contains(model)
    }
    pub(crate) fn new(
        id: Uuid,
        group_id: Uuid,
        api_format: ApiFormat,
        base_url: Url,
        weight: i32,
        upstream_auth: UpstreamAuth,
        available_models: HashSet<Arc<str>>,
    ) -> Self {
        Self {
            id,
            group_id,
            api_format,
            base_url,
            weight,
            upstream_auth,
            available_models,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledChannelGroup {
    id: Uuid,
    api_format: ApiFormat,
    priority: i32,
    selection_strategy: Arc<str>,
}
impl CompiledChannelGroup {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn api_format(&self) -> ApiFormat {
        self.api_format
    }
    #[must_use]
    pub fn priority(&self) -> i32 {
        self.priority
    }
    #[must_use]
    pub fn selection_strategy(&self) -> &str {
        &self.selection_strategy
    }
    pub(crate) fn new(
        id: Uuid,
        api_format: ApiFormat,
        priority: i32,
        selection_strategy: Arc<str>,
    ) -> Self {
        Self {
            id,
            api_format,
            priority,
            selection_strategy,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CompiledModelRule {
    id: Uuid,
    model_id: Uuid,
    client_model: Arc<str>,
    api_format: ApiFormat,
    upstream_model: Arc<str>,
    candidate_channel_ids: Arc<[Uuid]>,
}
impl CompiledModelRule {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    #[must_use]
    pub fn model_id(&self) -> Uuid {
        self.model_id
    }
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
    pub fn candidate_channel_ids(&self) -> &[Uuid] {
        &self.candidate_channel_ids
    }
    pub(crate) fn new(
        id: Uuid,
        model_id: Uuid,
        client_model: Arc<str>,
        api_format: ApiFormat,
        upstream_model: Arc<str>,
        candidates: Arc<[Uuid]>,
    ) -> Self {
        Self {
            id,
            model_id,
            client_model,
            api_format,
            upstream_model,
            candidate_channel_ids: candidates,
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

#[derive(Debug)]
pub struct CompiledRuntimeConfig {
    api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
    model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
    channels: HashMap<Uuid, Arc<CompiledChannel>>,
    groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
}
impl CompiledRuntimeConfig {
    #[must_use]
    pub fn new(
        api_keys: HashMap<ApiKeyHash, Arc<CompiledApiKey>>,
        model_rules: HashMap<ModelRouteKey, Arc<CompiledModelRule>>,
        channels: HashMap<Uuid, Arc<CompiledChannel>>,
        groups: HashMap<Uuid, Arc<CompiledChannelGroup>>,
    ) -> Self {
        Self {
            api_keys,
            model_rules,
            channels,
            groups,
        }
    }
    #[must_use]
    pub fn empty() -> Self {
        Self::new(
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        )
    }
    #[must_use]
    pub fn authenticate(&self, secret: &str) -> Option<Arc<CompiledApiKey>> {
        self.api_keys
            .get(&ApiKeyHash::from_secret(secret))
            .filter(|key| !key.is_expired())
            .cloned()
    }
    #[must_use]
    pub fn model_rule(&self, format: ApiFormat, model: &str) -> Option<Arc<CompiledModelRule>> {
        self.model_rules
            .get(&ModelRouteKey::new(format, Arc::<str>::from(model)))
            .cloned()
    }
    #[must_use]
    pub fn channel(&self, id: Uuid) -> Option<Arc<CompiledChannel>> {
        self.channels.get(&id).cloned()
    }
    #[must_use]
    pub fn group(&self, id: Uuid) -> Option<Arc<CompiledChannelGroup>> {
        self.groups.get(&id).cloned()
    }
    #[must_use]
    pub fn models_for(&self, key: &CompiledApiKey, format: ApiFormat) -> Vec<Arc<str>> {
        if !key.permits(format, ApiKeyPermission::Proxy)
            || !key.permits(format, ApiKeyPermission::ModelsRead)
        {
            return vec![];
        }
        let mut models = self
            .model_rules
            .values()
            .filter(|rule| {
                rule.api_format == format
                    && rule.candidate_channel_ids.iter().any(|id| {
                        self.channels
                            .get(id)
                            .is_some_and(|channel| key.permits_group(channel.group_id))
                    })
            })
            .map(|rule| Arc::clone(&rule.client_model))
            .collect::<Vec<_>>();
        models.sort_unstable();
        models.dedup();
        models
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::Arc,
    };

    use chrono::{Duration, Utc};
    use reqwest::header::HeaderName;
    use uuid::Uuid;

    use super::{ApiFormat, ApiKeyHash, CompiledApiKey, CompiledRuntimeConfig, UpstreamAuth};

    #[test]
    fn expired_keys_are_rejected_at_authentication_time() {
        let secret = "expired-client-secret";
        let key = Arc::new(CompiledApiKey::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            HashSet::from([ApiFormat::OpenAiChatCompletions]),
            HashSet::new(),
            None,
            Some(Utc::now() - Duration::seconds(1)),
        ));
        let snapshot = CompiledRuntimeConfig::new(
            HashMap::from([(ApiKeyHash::from_secret(secret), key)]),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        assert!(snapshot.authenticate(secret).is_none());
    }

    #[test]
    fn custom_upstream_header_debug_redacts_credential() {
        let auth = UpstreamAuth::Header {
            name: HeaderName::from_static("x-api-key"),
            value: Arc::from("upstream-secret"),
        };
        assert!(!format!("{auth:?}").contains("upstream-secret"));
    }
}
