//! Bootstrap TOML validation and database control-plane snapshot compilation.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::ArcSwap;
use reqwest::{
    Url,
    header::{HeaderName, HeaderValue},
};
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::{
    domain::{
        ApiFormat, ApiKeyHash, ApiKeyPermission, CompiledApiKey, CompiledChannel,
        CompiledChannelGroup, CompiledModelRule, CompiledRuntimeConfig, ModelRouteKey,
        UpstreamAuth,
    },
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ControlPlaneRecords, ModelRuleRecord,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub observability: ObservabilityConfig,
}
impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }
    pub fn validate(self) -> Result<BootstrapConfig, ConfigError> {
        validate_server(&self.server)?;
        validate_database(&self.database)?;
        validate_upstream(&self.upstream)?;
        if self.runtime_config.reload_interval_seconds == 0 {
            return Err(ConfigError::Compile(
                "runtime_config reload_interval_seconds must be greater than zero".into(),
            ));
        }
        require("observability filter", &self.observability.filter)?;
        Ok(BootstrapConfig {
            server: self.server,
            database: self.database,
            upstream: self.upstream,
            runtime_config: self.runtime_config,
            observability: self.observability,
        })
    }
}

pub struct BootstrapConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub observability: ObservabilityConfig,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_request_body_bytes: usize,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub connect_timeout_seconds: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    pub connect_timeout_seconds: u64,
    pub response_header_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfigSettings {
    pub reload_interval_seconds: u64,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    pub filter: String,
}

pub struct RuntimeConfig {
    current: ArcSwap<CompiledRuntimeConfig>,
}
impl RuntimeConfig {
    #[must_use]
    pub fn new(initial: CompiledRuntimeConfig) -> Self {
        Self {
            current: ArcSwap::from_pointee(initial),
        }
    }
    #[must_use]
    pub fn snapshot(&self) -> Arc<CompiledRuntimeConfig> {
        self.current.load_full()
    }
    pub fn replace_snapshot(&self, next: Arc<CompiledRuntimeConfig>) {
        self.current.store(next);
    }
}

/// Compiles a complete, already transactionally-read database control plane.
/// It deliberately contains no database access, making each runtime snapshot coherent.
pub fn compile_control_plane(
    records: ControlPlaneRecords,
) -> Result<CompiledRuntimeConfig, ConfigError> {
    let mut all_groups = HashMap::new();
    let mut groups = HashMap::new();
    for group in records.groups {
        validate_group(&group)?;
        insert_unique(&mut all_groups, group.id, group, "channel group")?;
    }
    for group in all_groups.values() {
        if group.enabled {
            groups.insert(
                group.id,
                Arc::new(CompiledChannelGroup::new(
                    group.id,
                    parse_format(&group.api_format)?,
                    group.priority,
                    Arc::from(group.selection_strategy.as_str()),
                )),
            );
        }
    }
    let mut channels = HashMap::new();
    let mut channel_ids = HashSet::new();
    for channel in records.channels {
        if !channel_ids.insert(channel.id) {
            return Err(dup("channel id"));
        }
        validate_channel(&channel, &all_groups)?;
        if channel.enabled && !channel.auto_disabled {
            let auth = compile_auth(&channel)?;
            channels.insert(
                channel.id,
                Arc::new(CompiledChannel::new(
                    channel.id,
                    channel.channel_group_id,
                    parse_format(&channel.api_format)?,
                    parse_url(channel.id, &channel.base_url)?,
                    channel.weight,
                    auth,
                    channel
                        .available_models
                        .iter()
                        .map(|model| Arc::<str>::from(model.as_str()))
                        .collect(),
                )),
            );
        }
    }
    let api_keys = compile_keys(records.api_keys, &all_groups)?;
    let model_rules = compile_rules(records.model_rules, &all_groups, &groups, &channels)?;
    Ok(CompiledRuntimeConfig::new(
        api_keys,
        model_rules,
        channels,
        groups,
    ))
}

fn compile_keys(
    records: Vec<ApiKeyRecord>,
    all_groups: &HashMap<Uuid, ChannelGroupRecord>,
) -> Result<HashMap<ApiKeyHash, Arc<CompiledApiKey>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("API key id"));
        }
        validate_key(&record, all_groups)?;
        let usable = record.status == "active" && record.user_status == "active";
        if !usable {
            continue;
        }
        if has_admission_control(&record) {
            return Err(ConfigError::Compile(
                "active API key uses admission control not supported in MVP-2 stage 1".into(),
            ));
        }
        let formats = record
            .allowed_api_formats
            .iter()
            .map(|value| parse_format(value))
            .collect::<Result<HashSet<_>, _>>()?;
        let permissions = record
            .permissions
            .iter()
            .map(|value| parse_permission(value))
            .collect::<Result<HashSet<_>, _>>()?;
        let groups = record
            .allowed_group_ids
            .map(|groups| groups.into_iter().collect());
        let secret = Zeroizing::new(record.secret_value);
        let key = Arc::new(CompiledApiKey::new(
            record.id,
            record.user_id,
            formats,
            permissions,
            groups,
            record.expires_at,
        ));
        if result
            .insert(ApiKeyHash::from_secret(secret.as_str()), key)
            .is_some()
        {
            return Err(ConfigError::Compile(
                "duplicate active API key secret".into(),
            ));
        }
    }
    Ok(result)
}
fn compile_rules(
    records: Vec<ModelRuleRecord>,
    all_groups: &HashMap<Uuid, ChannelGroupRecord>,
    groups: &HashMap<Uuid, Arc<CompiledChannelGroup>>,
    channels: &HashMap<Uuid, Arc<CompiledChannel>>,
) -> Result<HashMap<ModelRouteKey, Arc<CompiledModelRule>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("model rule id"));
        }
        validate_rule(&record)?;
        if !record.enabled {
            continue;
        }
        if !record.model_enabled {
            return Err(ConfigError::Compile(
                "enabled model rule references a disabled model".into(),
            ));
        }
        let format = parse_format(&record.api_format)?;
        let mut candidates = HashSet::new();
        for group_id in &record.channel_group_ids {
            let group = all_groups.get(group_id).ok_or_else(|| {
                ConfigError::Compile("enabled model rule references a missing channel group".into())
            })?;
            if !group.enabled || parse_format(&group.api_format)? != format {
                return Err(ConfigError::Compile(
                    "enabled model rule references an unavailable or cross-format channel group"
                        .into(),
                ));
            }
            for channel in channels
                .values()
                .filter(|channel| channel.group_id() == *group_id)
            {
                if !channel.supports_model(&record.upstream_model) {
                    return Err(ConfigError::Compile(
                        "eligible channel does not support the model rule upstream model".into(),
                    ));
                }
                candidates.insert(channel.id());
            }
        }
        for channel_id in &record.channel_ids {
            let channel = channels.get(channel_id).ok_or_else(|| {
                ConfigError::Compile(
                    "enabled model rule references a missing, disabled, or auto-disabled channel"
                        .into(),
                )
            })?;
            if channel.api_format() != format {
                return Err(ConfigError::Compile(
                    "enabled model rule references a cross-format channel".into(),
                ));
            }
            if !channel.supports_model(&record.upstream_model) {
                return Err(ConfigError::Compile(
                    "eligible channel does not support the model rule upstream model".into(),
                ));
            }
            if !candidates.insert(*channel_id) {
                return Err(ConfigError::Compile(
                    "model rule selects the same channel directly and through a channel group"
                        .into(),
                ));
            }
        }
        if candidates.len() != 1 {
            return Err(ConfigError::Compile(
                "each enabled model rule must have exactly one distinct eligible candidate channel"
                    .into(),
            ));
        }
        let candidate = *candidates.iter().next().expect("validated non-empty");
        let group_id = channels[&candidate].group_id();
        if !groups.contains_key(&group_id) {
            return Err(ConfigError::Compile(
                "eligible channel has no enabled group".into(),
            ));
        }
        let key = ModelRouteKey::new(format, Arc::<str>::from(record.client_model.as_str()));
        let rule = Arc::new(CompiledModelRule::new(
            record.id,
            record.model_id,
            Arc::from(record.client_model),
            format,
            Arc::from(record.upstream_model),
            Arc::from([candidate]),
        ));
        if result.insert(key, rule).is_some() {
            return Err(ConfigError::Compile(
                "duplicate enabled model rule for the same client model and API format".into(),
            ));
        }
    }
    Ok(result)
}

fn validate_group(record: &ChannelGroupRecord) -> Result<(), ConfigError> {
    require("channel group name", &record.name)?;
    parse_format(&record.api_format)?;
    if record.priority < 0
        || !matches!(
            record.selection_strategy.as_str(),
            "weighted_random" | "weighted_round_robin"
        )
    {
        return Err(ConfigError::Compile(
            "invalid channel group selection metadata".into(),
        ));
    }
    Ok(())
}
fn validate_channel(
    record: &ChannelRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
) -> Result<(), ConfigError> {
    require("channel name", &record.name)?;
    let format = parse_format(&record.api_format)?;
    if record.weight <= 0 {
        return Err(ConfigError::Compile(
            "channel weight must be positive".into(),
        ));
    }
    unique(&record.available_models, "channel available_models")?;
    for model in &record.available_models {
        require("channel available model", model)?;
    }
    if !matches!(
        record.upstream_auth_kind.as_str(),
        "none" | "bearer" | "header"
    ) {
        return Err(ConfigError::Compile(
            "unsupported upstream auth kind".into(),
        ));
    }
    if record.enabled {
        let group = groups
            .get(&record.channel_group_id)
            .ok_or_else(|| ConfigError::Compile("channel references a missing group".into()))?;
        if parse_format(&group.api_format)? != format {
            return Err(ConfigError::Compile(
                "channel and group use different API formats".into(),
            ));
        }
        parse_url(record.id, &record.base_url)?;
        if record.proxy_id.is_some()
            || record.config_template_id.is_some()
            || !record
                .override_document
                .as_object()
                .is_some_and(|value| value.is_empty())
            || record.connect_timeout_ms.is_some()
            || record.response_header_timeout_ms.is_some()
            || record.stream_idle_timeout_ms.is_some()
            || !record
                .health_check
                .as_object()
                .is_some_and(|value| value.is_empty())
        {
            return Err(ConfigError::Compile(
                "enabled channel uses a control-plane feature not supported in MVP-2 stage 1"
                    .into(),
            ));
        }
        compile_auth(record)?;
    }
    Ok(())
}
fn validate_key(
    record: &ApiKeyRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
) -> Result<(), ConfigError> {
    require("API key secret", &record.secret_value)?;
    if !matches!(
        record.status.as_str(),
        "active" | "disabled" | "revoked" | "expired"
    ) || !matches!(
        record.user_status.as_str(),
        "active" | "suspended" | "disabled"
    ) {
        return Err(ConfigError::Compile(
            "invalid API key or user status".into(),
        ));
    }
    if record.allowed_api_formats.is_empty() || record.permissions.is_empty() {
        return Err(ConfigError::Compile(
            "API key must allow formats and grant permissions".into(),
        ));
    }
    unique(&record.allowed_api_formats, "API key allowed_api_formats")?;
    unique(&record.permissions, "API key permissions")?;
    let formats = record
        .allowed_api_formats
        .iter()
        .map(|value| parse_format(value))
        .collect::<Result<HashSet<_>, _>>()?;
    for permission in &record.permissions {
        parse_permission(permission)?;
    }
    if let Some(allowed) = &record.allowed_group_ids {
        if allowed.is_empty() {
            return Err(ConfigError::Compile(
                "API key allowed_group_ids must be null or non-empty".into(),
            ));
        }
        unique(allowed, "API key allowed_group_ids")?;
        if record.status == "active" && record.user_status == "active" {
            for id in allowed {
                let group = groups.get(id).ok_or_else(|| {
                    ConfigError::Compile("API key references a missing group".into())
                })?;
                if !formats.contains(&parse_format(&group.api_format)?) {
                    return Err(ConfigError::Compile(
                        "API key group access references a disallowed format".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}
fn has_admission_control(record: &ApiKeyRecord) -> bool {
    record.requests_per_minute.is_some()
        || record.tokens_per_minute.is_some()
        || record.max_concurrent_requests.is_some()
        || record.quota_limit_amount.is_some()
        || !record.quota_used_amount.is_zero()
}
fn validate_rule(record: &ModelRuleRecord) -> Result<(), ConfigError> {
    require("model rule client_model", &record.client_model)?;
    require("model rule upstream_model", &record.upstream_model)?;
    parse_format(&record.api_format)?;
    unique(&record.channel_group_ids, "model rule channel_group_ids")?;
    unique(&record.channel_ids, "model rule channel_ids")?;
    if record.channel_group_ids.is_empty() && record.channel_ids.is_empty() {
        return Err(ConfigError::Compile(
            "model rule must select at least one target".into(),
        ));
    }
    Ok(())
}
fn compile_auth(channel: &ChannelRecord) -> Result<UpstreamAuth, ConfigError> {
    match channel.upstream_auth_kind.as_str() {
        "none"
            if channel.upstream_auth_header_name.is_none()
                && channel.upstream_api_key.is_none() =>
        {
            Ok(UpstreamAuth::None)
        }
        "bearer" if channel.upstream_auth_header_name.is_none() => Ok(UpstreamAuth::Bearer(
            secret_header(channel.upstream_api_key.as_deref())?,
        )),
        "header" => {
            let name = channel
                .upstream_auth_header_name
                .as_deref()
                .ok_or_else(|| {
                    ConfigError::Compile("header upstream auth requires a header name".into())
                })?;
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ConfigError::Compile("invalid upstream auth header name".into()))?;
            if matches!(
                name.as_str(),
                "authorization"
                    | "host"
                    | "content-length"
                    | "connection"
                    | "transfer-encoding"
                    | "proxy-authorization"
                    | "proxy-authenticate"
                    | "keep-alive"
                    | "te"
                    | "trailer"
                    | "upgrade"
                    | "proxy-connection"
            ) {
                return Err(ConfigError::Compile(
                    "unsafe upstream auth header name".into(),
                ));
            }
            Ok(UpstreamAuth::Header {
                name,
                value: secret_header(channel.upstream_api_key.as_deref())?,
            })
        }
        _ => Err(ConfigError::Compile(
            "invalid upstream auth configuration".into(),
        )),
    }
}
fn secret_header(value: Option<&str>) -> Result<Arc<str>, ConfigError> {
    let value =
        value.ok_or_else(|| ConfigError::Compile("upstream auth requires credentials".into()))?;
    require("upstream auth credential", value)?;
    HeaderValue::from_str(value).map_err(|_| {
        ConfigError::Compile("upstream auth credential is not a valid HTTP header value".into())
    })?;
    Ok(Arc::from(value))
}
fn parse_format(value: &str) -> Result<ApiFormat, ConfigError> {
    match value {
        "open_ai_chat_completions" => Ok(ApiFormat::OpenAiChatCompletions),
        "open_ai_responses" => Ok(ApiFormat::OpenAiResponses),
        _ => Err(ConfigError::Compile("unsupported API format".into())),
    }
}
fn parse_permission(value: &str) -> Result<ApiKeyPermission, ConfigError> {
    match value {
        "proxy" => Ok(ApiKeyPermission::Proxy),
        "models.read" => Ok(ApiKeyPermission::ModelsRead),
        _ => Err(ConfigError::Compile(
            "unsupported API key permission".into(),
        )),
    }
}
fn parse_url(id: Uuid, value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value)
        .map_err(|_| ConfigError::Compile(format!("channel {id} has an invalid base URL")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConfigError::Compile(
            "channel base URL must be an http(s) URL without credentials, query, or fragment"
                .into(),
        ));
    }
    Ok(url)
}
fn require(field: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        Err(ConfigError::Compile(format!("{field} must not be blank")))
    } else {
        Ok(())
    }
}
fn unique<T: Eq + std::hash::Hash>(items: &[T], field: &str) -> Result<(), ConfigError> {
    if items.iter().collect::<HashSet<_>>().len() != items.len() {
        Err(ConfigError::Compile(format!("duplicate {field} item")))
    } else {
        Ok(())
    }
}
fn insert_unique<T>(
    map: &mut HashMap<Uuid, T>,
    id: Uuid,
    value: T,
    field: &str,
) -> Result<(), ConfigError> {
    if map.insert(id, value).is_some() {
        Err(dup(field))
    } else {
        Ok(())
    }
}
fn dup(field: &str) -> ConfigError {
    ConfigError::Compile(format!("duplicate {field}"))
}
fn validate_server(server: &ServerConfig) -> Result<(), ConfigError> {
    require("server host", &server.host)?;
    if server.max_request_body_bytes == 0 {
        return Err(ConfigError::Compile(
            "server max_request_body_bytes must be greater than zero".into(),
        ));
    }
    Ok(())
}
fn validate_database(database: &DatabaseConfig) -> Result<(), ConfigError> {
    if database.max_connections == 0 || database.connect_timeout_seconds == 0 {
        return Err(ConfigError::Compile(
            "database limits must be greater than zero".into(),
        ));
    }
    let url = Url::parse(&database.url)
        .map_err(|_| ConfigError::Compile("database URL is invalid".into()))?;
    if url.scheme() != "postgres" && url.scheme() != "postgresql" {
        return Err(ConfigError::Compile(
            "database URL must use postgres".into(),
        ));
    }
    Ok(())
}
fn validate_upstream(upstream: &UpstreamConfig) -> Result<(), ConfigError> {
    if upstream.connect_timeout_seconds == 0
        || upstream.response_header_timeout_seconds <= upstream.connect_timeout_seconds
        || upstream.stream_idle_timeout_seconds == 0
    {
        return Err(ConfigError::Compile(
            "invalid upstream timeout settings".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse TOML configuration file {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid runtime configuration: {0}")]
    Compile(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_rejects_dynamic_toml() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'\n[[api_keys]]\nid='bad'";
        assert!(toml::from_str::<AppConfig>(value).is_err());
    }
}
