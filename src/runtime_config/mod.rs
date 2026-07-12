//! Bootstrap TOML validation and database control-plane snapshot compilation.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    net::SocketAddr,
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
use zeroize::{Zeroize, Zeroizing};

use crate::{
    domain::{
        AdminTokenVerifier, ApiFormat, ApiKeyHash, ApiKeyPermission, ChannelTimeoutPolicy,
        CompiledApiKey, CompiledChannel, CompiledChannelGroup, CompiledChannelUpstreamPolicy,
        CompiledConfigTemplate, CompiledModelRule, CompiledProxy, CompiledRouteTier,
        CompiledRuntimeConfig, ModelRouteKey, NoProxyHost, SelectionStrategy, UpstreamAuth,
    },
    persistence::{
        ApiKeyRecord, ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        ModelRuleRecord, ProxyRecord,
    },
    transforms::{TransformCompileError, TransformPlan, compile_document, declared_api_format},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    #[serde(default)]
    pub request_logging: RequestLoggingConfig,
    #[serde(default)]
    pub passive_health: PassiveHealthConfig,
    #[serde(default)]
    pub admin: AdminFileConfig,
    pub observability: ObservabilityConfig,
}
impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents =
            Zeroizing::new(
                fs::read_to_string(path).map_err(|source| ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                })?,
            );
        toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            line: source
                .span()
                .and_then(|span| contents[..span.start].lines().count().checked_add(1)),
            column: source.span().and_then(|span| {
                contents[..span.start]
                    .rsplit_once('\n')
                    .map_or(Some(span.start + 1), |(_, line)| line.len().checked_add(1))
            }),
        })
    }
    pub fn validate(self) -> Result<BootstrapConfig, ConfigError> {
        validate_server(&self.server)?;
        validate_database(&self.database)?;
        validate_upstream(&self.upstream)?;
        let admin = validate_admin(self.admin)?;
        if self.runtime_config.reload_interval_seconds == 0 {
            return Err(ConfigError::Compile(
                "runtime_config reload_interval_seconds must be greater than zero".into(),
            ));
        }
        if self.request_logging.queue_capacity == 0 {
            return Err(ConfigError::Compile(
                "request_logging queue_capacity must be greater than zero".into(),
            ));
        }
        if self.passive_health.connection_failure_threshold == 0
            || self.passive_health.cooldown_seconds == 0
        {
            return Err(ConfigError::Compile(
                "passive_health threshold and cooldown must be greater than zero".into(),
            ));
        }
        require("observability filter", &self.observability.filter)?;
        Ok(BootstrapConfig {
            server: self.server,
            database: self.database,
            upstream: self.upstream,
            runtime_config: self.runtime_config,
            request_logging: self.request_logging,
            passive_health: self.passive_health,
            admin,
            observability: self.observability,
        })
    }
}

pub struct BootstrapConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub request_logging: RequestLoggingConfig,
    pub passive_health: PassiveHealthConfig,
    pub admin: Option<AdminListenerConfig>,
    pub observability: ObservabilityConfig,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_request_body_bytes: usize,
    #[serde(default = "default_shutdown_grace_period_seconds")]
    pub shutdown_grace_period_seconds: u64,
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
pub struct RequestLoggingConfig {
    pub queue_capacity: usize,
}
/// Process-wide passive connectivity policy. Defaults are three connection
/// failures and a 30 second cooldown, documented in the shipped TOML files.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassiveHealthConfig {
    #[serde(default = "default_connection_failure_threshold")]
    pub connection_failure_threshold: u32,
    #[serde(default = "default_passive_health_cooldown_seconds")]
    pub cooldown_seconds: u64,
}
impl Default for PassiveHealthConfig {
    fn default() -> Self {
        Self {
            connection_failure_threshold: default_connection_failure_threshold(),
            cooldown_seconds: default_passive_health_cooldown_seconds(),
        }
    }
}
impl Default for RequestLoggingConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_request_log_queue_capacity(),
        }
    }
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    pub filter: String,
}

/// File-only representation. It is consumed by `validate` so the raw token
/// cannot outlive bootstrap construction.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminFileConfig {
    #[serde(default)]
    pub enabled: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub actor_user_id: Option<Uuid>,
    pub bearer_token: Option<String>,
}
impl Drop for AdminFileConfig {
    fn drop(&mut self) {
        if let Some(token) = &mut self.bearer_token {
            token.zeroize();
        }
    }
}

#[derive(Clone)]
pub struct AdminListenerConfig {
    pub address: SocketAddr,
    pub actor_user_id: Uuid,
    pub verifier: AdminTokenVerifier,
}

const fn default_shutdown_grace_period_seconds() -> u64 {
    30
}
const fn default_request_log_queue_capacity() -> usize {
    1_024
}
const fn default_connection_failure_threshold() -> u32 {
    3
}
const fn default_passive_health_cooldown_seconds() -> u64 {
    30
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
                    parse_strategy(&group.selection_strategy)?,
                )),
            );
        }
    }
    let proxies = compile_proxies(records.proxies)?;
    let templates = compile_templates(records.templates)?;
    let mut channels = HashMap::new();
    let mut all_channels = HashMap::new();
    let mut validated_channels = Vec::new();
    let mut channel_ids = HashSet::new();
    for channel in records.channels {
        if !channel_ids.insert(channel.id) {
            return Err(dup("channel id"));
        }
        validate_channel(&channel, &all_groups)?;
        validate_channel_resources(&channel, &proxies, &templates)?;
        all_channels.insert(channel.id, channel.clone());
        validated_channels.push(channel);
    }
    for channel in validated_channels {
        if channel.enabled && !channel.auto_disabled {
            let auth = compile_auth(&channel)?;
            let api_format = parse_format(&channel.api_format)?;
            let proxy = channel.proxy_id.map(|id| {
                proxies
                    .get(&id)
                    .cloned()
                    .expect("validated proxy reference")
            });
            let template = channel.config_template_id.map(|id| {
                templates
                    .get(&id)
                    .cloned()
                    .expect("validated template reference")
            });
            let channel_override = compile_channel_document(&channel, api_format)?;
            let defaults = template.as_ref().map_or_else(
                || TransformPlan::noop(api_format),
                |template| template.transform_plan(api_format).clone(),
            );
            let effective_transforms = TransformPlan::compose(&defaults, &channel_override)
                .map_err(transform_error("channel effective transform plan"))?;
            let upstream_policy = CompiledChannelUpstreamPolicy::new(
                proxy,
                template,
                channel_override,
                effective_transforms,
                compile_timeouts(&channel)?,
            );
            channels.insert(
                channel.id,
                Arc::new(CompiledChannel::new_with_policy(
                    channel.id,
                    channel.channel_group_id,
                    api_format,
                    parse_url(channel.id, &channel.base_url)?,
                    channel.weight,
                    auth,
                    channel
                        .available_models
                        .iter()
                        .map(|model| Arc::<str>::from(model.as_str()))
                        .collect(),
                    upstream_policy,
                )),
            );
        }
    }
    let api_keys = compile_keys(records.api_keys, &all_groups)?;
    let model_rules = compile_rules(
        records.model_rules,
        &all_groups,
        &all_channels,
        &groups,
        &channels,
    )?;
    Ok(CompiledRuntimeConfig::with_resources(
        api_keys,
        model_rules,
        channels,
        groups,
        proxies,
        templates,
    ))
}

fn compile_proxies(
    records: Vec<ProxyRecord>,
) -> Result<HashMap<Uuid, Arc<CompiledProxy>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("proxy id"));
        }
        require("proxy name", &record.name)?;
        let url = Url::parse(&record.proxy_url)
            .map_err(|_| ConfigError::Compile("proxy has an invalid URL".into()))?;
        if !matches!(url.scheme(), "http" | "https" | "socks4" | "socks5")
            || url.host().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(ConfigError::Compile(
                "proxy URL must use http, https, socks4, or socks5 without embedded credentials, query, or fragment".into(),
            ));
        }
        let no_proxy_hosts = record
            .no_proxy_hosts
            .iter()
            .map(|host| NoProxyHost::parse(host).map_err(|_| invalid_no_proxy_host()))
            .collect::<Result<Vec<_>, ConfigError>>()?;
        unique(&no_proxy_hosts, "proxy no_proxy_hosts")?;
        if let Some(username) = &record.username {
            require("proxy username", username)?;
        }
        if let Some(password) = &record.password {
            require("proxy password", password)?;
        }
        if record.enabled {
            result.insert(
                record.id,
                Arc::new(CompiledProxy::new(
                    record.id,
                    Arc::from(record.name),
                    url,
                    record.username.map(Arc::from),
                    record.password.map(Arc::from),
                    no_proxy_hosts.into(),
                )),
            );
        }
    }
    Ok(result)
}

fn compile_templates(
    records: Vec<ConfigTemplateRecord>,
) -> Result<HashMap<Uuid, Arc<CompiledConfigTemplate>>, ConfigError> {
    let mut result = HashMap::new();
    let mut ids = HashSet::new();
    for record in records {
        if !ids.insert(record.id) {
            return Err(dup("config template id"));
        }
        require("config template name", &record.name)?;
        let declared = declared_api_format(&record.document)
            .map_err(transform_error("config template document"))?;
        let chat = if declared.is_none() || declared == Some(ApiFormat::OpenAiChatCompletions) {
            compile_document(&record.document, ApiFormat::OpenAiChatCompletions)
                .map_err(transform_error("config template document"))?
        } else {
            TransformPlan::noop(ApiFormat::OpenAiChatCompletions)
        };
        let responses = if declared.is_none() || declared == Some(ApiFormat::OpenAiResponses) {
            compile_document(&record.document, ApiFormat::OpenAiResponses)
                .map_err(transform_error("config template document"))?
        } else {
            TransformPlan::noop(ApiFormat::OpenAiResponses)
        };
        if record.enabled {
            result.insert(
                record.id,
                Arc::new(CompiledConfigTemplate::new(
                    record.id,
                    Arc::from(record.name),
                    record.description.map(Arc::from),
                    declared,
                    chat,
                    responses,
                )),
            );
        }
    }
    Ok(result)
}

fn compile_channel_document(
    channel: &ChannelRecord,
    format: ApiFormat,
) -> Result<TransformPlan, ConfigError> {
    compile_document(&channel.override_document, format)
        .map_err(transform_error("channel override document"))
}

fn compile_timeouts(channel: &ChannelRecord) -> Result<ChannelTimeoutPolicy, ConfigError> {
    Ok(ChannelTimeoutPolicy::new(
        positive_timeout(channel.connect_timeout_ms, "connect_timeout_ms")?,
        positive_timeout(
            channel.response_header_timeout_ms,
            "response_header_timeout_ms",
        )?,
        positive_timeout(channel.stream_idle_timeout_ms, "stream_idle_timeout_ms")?,
    ))
}

fn positive_timeout(
    value: Option<i32>,
    name: &str,
) -> Result<Option<std::time::Duration>, ConfigError> {
    value
        .map(|value| {
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .map(std::time::Duration::from_millis)
                .ok_or_else(|| {
                    ConfigError::Compile(format!("channel {name} must be positive when configured"))
                })
        })
        .transpose()
}

fn transform_error(context: &'static str) -> impl FnOnce(TransformCompileError) -> ConfigError {
    move |error| ConfigError::Compile(format!("{context} is invalid: {error}"))
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
        if record.tokens_per_minute.is_some() {
            return Err(ConfigError::Compile(
                "active API key uses unsupported tokens_per_minute admission control".into(),
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
            positive_policy(record.requests_per_minute, "requests_per_minute")?,
            positive_policy(record.max_concurrent_requests, "max_concurrent_requests")?,
            record.quota_limit_amount,
            record.quota_used_amount,
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
    all_channels: &HashMap<Uuid, ChannelRecord>,
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
        validate_rule_references(&record, all_groups, all_channels)?;
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
            if !groups.contains_key(&channel.group_id()) {
                return Err(ConfigError::Compile(
                    "direct channel candidate belongs to a disabled channel group".into(),
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
        if candidates.is_empty() {
            return Err(ConfigError::Compile(
                "each enabled model rule must have at least one distinct eligible candidate channel"
                    .into(),
            ));
        }
        let mut tier_channels: HashMap<i32, Vec<Uuid>> = HashMap::new();
        for candidate in candidates {
            let channel = &channels[&candidate];
            let group = groups.get(&channel.group_id()).ok_or_else(|| {
                ConfigError::Compile("eligible channel has no enabled group".into())
            })?;
            tier_channels
                .entry(group.priority())
                .or_default()
                .push(candidate);
        }
        let mut priorities = tier_channels.keys().copied().collect::<Vec<_>>();
        priorities.sort_unstable();
        let mut tiers = Vec::with_capacity(priorities.len());
        for priority in priorities {
            let mut ids = tier_channels
                .remove(&priority)
                .expect("priority was collected");
            ids.sort_unstable();
            let first_group = groups
                .get(&channels[&ids[0]].group_id())
                .expect("eligible group");
            let strategy = first_group.selection_strategy();
            let mut aggregate_weight = 0_i64;
            for id in &ids {
                let channel = &channels[id];
                let group = groups.get(&channel.group_id()).expect("eligible group");
                if group.selection_strategy() != strategy {
                    return Err(ConfigError::Compile(
                        "all channel groups in every route priority tier must use the same selection strategy".into(),
                    ));
                }
                aggregate_weight = aggregate_weight
                    .checked_add(i64::from(channel.weight()))
                    .ok_or_else(|| {
                        ConfigError::Compile(
                            "route tier aggregate channel weight overflowed".into(),
                        )
                    })?;
            }
            if aggregate_weight <= 0 {
                return Err(ConfigError::Compile(
                    "route tier aggregate channel weight must be positive".into(),
                ));
            }
            tiers.push(CompiledRouteTier::new(priority, strategy, Arc::from(ids)));
        }
        let key = ModelRouteKey::new(format, Arc::<str>::from(record.client_model.as_str()));
        let rule = Arc::new(CompiledModelRule::new(
            record.id,
            record.model_id,
            Arc::from(record.client_model),
            format,
            Arc::from(record.upstream_model),
            Arc::from(tiers),
        ));
        if result.insert(key, rule).is_some() {
            return Err(ConfigError::Compile(
                "duplicate enabled model rule for the same client model and API format".into(),
            ));
        }
    }
    Ok(result)
}
fn validate_rule_references(
    record: &ModelRuleRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
    channels: &HashMap<Uuid, ChannelRecord>,
) -> Result<(), ConfigError> {
    let format = parse_format(&record.api_format)?;
    for group_id in &record.channel_group_ids {
        let group = groups.get(group_id).ok_or_else(|| {
            ConfigError::Compile("model rule references a missing channel group".into())
        })?;
        if parse_format(&group.api_format)? != format {
            return Err(ConfigError::Compile(
                "model rule references a cross-format channel group".into(),
            ));
        }
    }
    for channel_id in &record.channel_ids {
        let channel = channels.get(channel_id).ok_or_else(|| {
            ConfigError::Compile("model rule references a missing channel".into())
        })?;
        if parse_format(&channel.api_format)? != format {
            return Err(ConfigError::Compile(
                "model rule references a cross-format channel".into(),
            ));
        }
    }
    Ok(())
}

fn validate_group(record: &ChannelGroupRecord) -> Result<(), ConfigError> {
    require("channel group name", &record.name)?;
    parse_format(&record.api_format)?;
    if record.priority < 0 || SelectionStrategy::parse(&record.selection_strategy).is_none() {
        return Err(ConfigError::Compile(
            "invalid channel group selection metadata".into(),
        ));
    }
    Ok(())
}
fn parse_strategy(value: &str) -> Result<SelectionStrategy, ConfigError> {
    SelectionStrategy::parse(value)
        .ok_or_else(|| ConfigError::Compile("unsupported channel group selection strategy".into()))
}
fn validate_channel(
    record: &ChannelRecord,
    groups: &HashMap<Uuid, ChannelGroupRecord>,
) -> Result<(), ConfigError> {
    require("channel name", &record.name)?;
    let format = parse_format(&record.api_format)?;
    if !is_empty_document(&record.health_check) {
        return Err(ConfigError::Compile(
            "channel health check document must be an empty object".into(),
        ));
    }
    compile_document(&record.override_document, format)
        .map_err(transform_error("channel override document"))?;
    compile_timeouts(record)?;
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
        compile_auth(record)?;
    }
    Ok(())
}
fn validate_channel_resources(
    record: &ChannelRecord,
    proxies: &HashMap<Uuid, Arc<CompiledProxy>>,
    templates: &HashMap<Uuid, Arc<CompiledConfigTemplate>>,
) -> Result<(), ConfigError> {
    let format = parse_format(&record.api_format)?;
    if record.proxy_id.is_some_and(|id| !proxies.contains_key(&id)) {
        return Err(ConfigError::Compile(
            "channel references a missing or disabled proxy".into(),
        ));
    }
    if let Some(template_id) = record.config_template_id {
        let template = templates.get(&template_id).ok_or_else(|| {
            ConfigError::Compile("channel references a missing or disabled template".into())
        })?;
        if template
            .api_format()
            .is_some_and(|template_format| template_format != format)
        {
            return Err(ConfigError::Compile(
                "channel references a cross-format template".into(),
            ));
        }
    }
    Ok(())
}
fn is_empty_document(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(serde_json::Map::is_empty)
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
        for id in allowed {
            let group = groups
                .get(id)
                .ok_or_else(|| ConfigError::Compile("API key references a missing group".into()))?;
            if !formats.contains(&parse_format(&group.api_format)?) {
                return Err(ConfigError::Compile(
                    "API key group access references a disallowed format".into(),
                ));
            }
        }
    }
    positive_policy(record.requests_per_minute, "requests_per_minute")?;
    positive_policy(record.max_concurrent_requests, "max_concurrent_requests")?;
    if record
        .quota_limit_amount
        .is_some_and(|amount| amount.is_sign_negative())
        || record.quota_used_amount.is_sign_negative()
    {
        return Err(ConfigError::Compile(
            "API key quota amounts must be non-negative".into(),
        ));
    }
    Ok(())
}
fn positive_policy(value: Option<i32>, name: &str) -> Result<Option<u32>, ConfigError> {
    value
        .map(|value| {
            u32::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    ConfigError::Compile(format!("API key {name} must be positive when configured"))
                })
        })
        .transpose()
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
fn invalid_no_proxy_host() -> ConfigError {
    ConfigError::Compile("proxy no_proxy host pattern is invalid".into())
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
    if server.max_request_body_bytes == 0 || server.shutdown_grace_period_seconds == 0 {
        return Err(ConfigError::Compile(
            "server max_request_body_bytes and shutdown_grace_period_seconds must be greater than zero"
                .into(),
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
fn validate_admin(mut config: AdminFileConfig) -> Result<Option<AdminListenerConfig>, ConfigError> {
    if !config.enabled {
        return Ok(None);
    }
    let host = config
        .host
        .take()
        .ok_or_else(|| ConfigError::Compile("enabled admin host is required".into()))?;
    let address = host
        .parse::<std::net::IpAddr>()
        .map_err(|_| ConfigError::Compile("admin host must be a loopback IP address".into()))?;
    if !address.is_loopback() {
        return Err(ConfigError::Compile(
            "admin host must be a loopback IP address".into(),
        ));
    }
    let port = config
        .port
        .filter(|port| *port != 0)
        .ok_or_else(|| ConfigError::Compile("enabled admin port is required".into()))?;
    let actor_user_id = config
        .actor_user_id
        .ok_or_else(|| ConfigError::Compile("enabled admin actor_user_id is required".into()))?;
    let mut token = config
        .bearer_token
        .take()
        .ok_or_else(|| ConfigError::Compile("enabled admin bearer_token is required".into()))?;
    if token.trim().len() < 32 {
        token.zeroize();
        return Err(ConfigError::Compile(
            "admin bearer_token must contain at least 32 characters".into(),
        ));
    }
    let verifier = AdminTokenVerifier::from_token(&token);
    token.zeroize();
    Ok(Some(AdminListenerConfig {
        address: SocketAddr::new(address, port),
        actor_user_id,
        verifier,
    }))
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read configuration file {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse TOML configuration file {path} (line {line:?}, column {column:?})")]
    Parse {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
    },
    #[error("invalid runtime configuration: {0}")]
    Compile(String),
}

#[cfg(test)]
mod tests {
    use crate::persistence::{
        ChannelGroupRecord, ChannelRecord, ConfigTemplateRecord, ControlPlaneRecords,
        ModelRuleRecord, ProxyRecord,
    };

    use super::*;

    fn route_records(
        first_priority: i32,
        first_strategy: &str,
        second_priority: i32,
        second_strategy: &str,
        direct_duplicate: bool,
    ) -> ControlPlaneRecords {
        let first_group = Uuid::from_u128(1);
        let second_group = Uuid::from_u128(2);
        let first_channel = Uuid::from_u128(11);
        let second_channel = Uuid::from_u128(12);
        let group = |id, priority, strategy: &str| ChannelGroupRecord {
            id,
            name: id.to_string(),
            api_format: "open_ai_chat_completions".into(),
            priority,
            selection_strategy: strategy.into(),
            enabled: true,
        };
        let channel = |id, group_id| ChannelRecord {
            id,
            channel_group_id: group_id,
            api_format: "open_ai_chat_completions".into(),
            name: id.to_string(),
            base_url: format!("https://{id}.test"),
            enabled: true,
            auto_disabled: false,
            weight: 1,
            proxy_id: None,
            config_template_id: None,
            override_document: serde_json::json!({}),
            connect_timeout_ms: None,
            response_header_timeout_ms: None,
            stream_idle_timeout_ms: None,
            upstream_auth_kind: "none".into(),
            upstream_auth_header_name: None,
            upstream_api_key: None,
            available_models: vec!["upstream".into()],
            health_check: serde_json::json!({}),
        };
        ControlPlaneRecords {
            api_keys: vec![],
            groups: vec![
                group(first_group, first_priority, first_strategy),
                group(second_group, second_priority, second_strategy),
            ],
            channels: vec![
                channel(first_channel, first_group),
                channel(second_channel, second_group),
            ],
            model_rules: vec![ModelRuleRecord {
                id: Uuid::from_u128(20),
                client_model: "client".into(),
                api_format: "open_ai_chat_completions".into(),
                model_id: Uuid::from_u128(21),
                model_enabled: true,
                upstream_model: "upstream".into(),
                channel_group_ids: vec![first_group, second_group],
                channel_ids: direct_duplicate
                    .then_some(first_channel)
                    .into_iter()
                    .collect(),
                enabled: true,
            }],
            proxies: vec![],
            templates: vec![],
        }
    }
    #[test]
    fn bootstrap_rejects_dynamic_toml() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=1\n[observability]\nfilter='info'\n[[api_keys]]\nid='bad'";
        assert!(toml::from_str::<AppConfig>(value).is_err());
    }

    #[test]
    fn malformed_toml_error_never_retains_or_formats_raw_config() {
        let token = "fake-admin-token-must-never-appear-in-an-error";
        let path = std::env::temp_dir().join(format!("ai-gateway-invalid-{}.toml", Uuid::new_v4()));
        std::fs::write(
            &path,
            format!("[admin]\nbearer_token = '{token}'\nnot valid toml"),
        )
        .unwrap();
        let error = match AppConfig::load(&path) {
            Err(error) => error,
            Ok(_) => panic!("malformed TOML unexpectedly parsed"),
        };
        let display = error.to_string();
        let debug = format!("{error:?}");
        std::fs::remove_file(path).unwrap();
        assert!(!display.contains(token));
        assert!(!debug.contains(token));
    }

    #[test]
    fn bootstrap_rejects_zero_shutdown_grace_period() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\nshutdown_grace_period_seconds=0\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=1\n[observability]\nfilter='info'";
        assert!(
            toml::from_str::<AppConfig>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn bootstrap_defaults_stage_two_settings_when_absent() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'";
        let config = toml::from_str::<AppConfig>(value)
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(config.server.shutdown_grace_period_seconds, 30);
        assert_eq!(config.request_logging.queue_capacity, 1_024);
        assert_eq!(config.passive_health.connection_failure_threshold, 3);
        assert_eq!(config.passive_health.cooldown_seconds, 30);
    }

    #[test]
    fn bootstrap_preserves_ipv6_admin_socket_address() {
        let value = "[server]\nhost='127.0.0.1'\nport=1\nmax_request_body_bytes=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[observability]\nfilter='info'\n[admin]\nenabled=true\nhost='::1'\nport=9443\nactor_user_id='00000000-0000-0000-0000-000000000001'\nbearer_token='at-least-thirty-two-characters-long-token'";
        let config = toml::from_str::<AppConfig>(value)
            .unwrap()
            .validate()
            .unwrap();

        assert_eq!(
            config.admin.unwrap().address,
            std::net::SocketAddr::new(std::net::Ipv6Addr::LOCALHOST.into(), 9443)
        );
    }

    #[test]
    fn bootstrap_rejects_zero_request_log_queue_capacity() {
        let value = "[server]\nhost='x'\nport=1\nmax_request_body_bytes=1\nshutdown_grace_period_seconds=1\n[database]\nurl='postgres://x'\nmax_connections=1\nconnect_timeout_seconds=1\n[upstream]\nconnect_timeout_seconds=1\nresponse_header_timeout_seconds=2\nstream_idle_timeout_seconds=1\n[runtime_config]\nreload_interval_seconds=1\n[request_logging]\nqueue_capacity=0\n[observability]\nfilter='info'";
        assert!(
            toml::from_str::<AppConfig>(value)
                .unwrap()
                .validate()
                .is_err()
        );
    }

    #[test]
    fn compiler_builds_sorted_priority_tiers() {
        let snapshot = compile_control_plane(route_records(
            10,
            "weighted_random",
            2,
            "weighted_random",
            false,
        ))
        .unwrap();
        let rule = snapshot
            .model_rule(ApiFormat::OpenAiChatCompletions, "client")
            .unwrap();
        assert_eq!(rule.tiers().len(), 2);
        assert_eq!(rule.tiers()[0].priority(), 2);
        assert_eq!(rule.tiers()[1].priority(), 10);
    }

    #[test]
    fn compiler_rejects_strategy_mismatch_in_any_priority_tier() {
        assert!(
            compile_control_plane(route_records(
                0,
                "weighted_random",
                0,
                "weighted_round_robin",
                false,
            ))
            .is_err()
        );
    }

    #[test]
    fn compiler_rejects_direct_candidate_already_reached_through_group() {
        assert!(
            compile_control_plane(route_records(
                0,
                "weighted_random",
                1,
                "weighted_random",
                true,
            ))
            .is_err()
        );
    }

    #[test]
    fn compiler_rejects_nonempty_channel_documents_even_when_disabled() {
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.channels[0].enabled = false;
        records.channels[0].override_document = serde_json::json!({
            "headers": {"Authorization": "must-not-be-accepted"}
        });

        assert!(compile_control_plane(records).is_err());
    }

    fn proxy(id: Uuid, url: &str, enabled: bool) -> ProxyRecord {
        ProxyRecord {
            id,
            name: "egress".into(),
            proxy_url: url.into(),
            username: Some("proxy-user".into()),
            password: Some("proxy-password".into()),
            no_proxy_hosts: vec!["internal.test".into()],
            enabled,
        }
    }

    fn template(id: Uuid, format: &str, enabled: bool) -> ConfigTemplateRecord {
        ConfigTemplateRecord {
            id,
            name: "defaults".into(),
            description: None,
            document: serde_json::json!({
                "version": 1,
                "api_format": format,
                "request_headers": {"set": {"x-template": "template-default"}}
            }),
            enabled,
        }
    }

    #[test]
    fn compiler_validates_proxy_schemes_no_proxy_hosts_and_never_leaks_credentials() {
        for scheme in ["http", "https", "socks4", "socks5"] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            let id = Uuid::new_v4();
            records.proxies = vec![proxy(id, &format!("{scheme}://proxy.test:1080"), true)];
            records.channels[0].proxy_id = Some(id);
            assert!(
                compile_control_plane(records).is_ok(),
                "{scheme} should compile"
            );
        }

        let mut invalid_scheme = route_records(0, "weighted_random", 1, "weighted_random", false);
        let id = Uuid::new_v4();
        invalid_scheme.proxies = vec![proxy(id, "ftp://proxy.test", true)];
        invalid_scheme.channels[0].proxy_id = Some(id);
        let error = compile_control_plane(invalid_scheme)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("proxy-password"));
        assert!(!error.contains("proxy-user"));

        for hosts in [
            vec![" ".into()],
            vec!["same.test".into(), "same.test".into()],
        ] {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.proxies = vec![ProxyRecord {
                no_proxy_hosts: hosts,
                ..proxy(Uuid::new_v4(), "https://proxy.test", true)
            }];
            assert!(compile_control_plane(records).is_err());
        }
    }

    #[test]
    fn compiler_validates_channel_resources_timeouts_and_template_composition() {
        let proxy_id = Uuid::new_v4();
        let template_id = Uuid::new_v4();
        let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
        records.proxies = vec![proxy(proxy_id, "https://proxy.test", true)];
        records.templates = vec![template(template_id, "open_ai_chat_completions", true)];
        records.channels[0].proxy_id = Some(proxy_id);
        records.channels[0].config_template_id = Some(template_id);
        records.channels[0].override_document = serde_json::json!({
            "version": 1,
            "api_format": "open_ai_chat_completions",
            "request_headers": {"set": {"x-channel": "channel-override"}}
        });
        records.channels[0].connect_timeout_ms = Some(10);
        records.channels[0].response_header_timeout_ms = Some(20);
        records.channels[0].stream_idle_timeout_ms = Some(30);
        let channel_id = records.channels[0].id;

        let snapshot = compile_control_plane(records).unwrap();
        let channel = snapshot.channel(channel_id).unwrap();
        let policy = channel.upstream_policy();
        assert_eq!(policy.proxy().unwrap().id(), proxy_id);
        assert_eq!(policy.template().unwrap().id(), template_id);
        assert_eq!(
            policy.timeouts().connect(),
            Some(std::time::Duration::from_millis(10))
        );
        assert_eq!(
            policy.timeouts().response_header(),
            Some(std::time::Duration::from_millis(20))
        );
        assert_eq!(
            policy.timeouts().stream_idle(),
            Some(std::time::Duration::from_millis(30))
        );
        assert_eq!(
            policy
                .effective_transforms()
                .request_headers()
                .operations()
                .len(),
            2
        );

        let invalid_records = [
            (Some(Uuid::new_v4()), None, None, None),
            (Some(proxy_id), None, None, Some(false)),
            (None, Some(Uuid::new_v4()), None, None),
            (None, Some(template_id), None, Some(false)),
            (None, Some(template_id), Some(0), None),
        ];
        for (missing_proxy, template_reference, timeout, disabled) in invalid_records {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            let proxy_enabled = disabled.unwrap_or(true);
            records.proxies = vec![proxy(proxy_id, "https://proxy.test", proxy_enabled)];
            records.templates = vec![template(
                template_id,
                "open_ai_responses",
                disabled.unwrap_or(true),
            )];
            records.channels[0].proxy_id = missing_proxy;
            records.channels[0].config_template_id = template_reference;
            records.channels[0].connect_timeout_ms = timeout;
            let error = compile_control_plane(records).unwrap_err().to_string();
            assert!(!error.contains("proxy-password"));
            assert!(!error.contains("template-default"));
        }
    }

    #[test]
    fn compiler_validates_disabled_resources_without_leaking_record_values() {
        let url_user = "sentinel-proxy-url-user";
        let url_password = "sentinel-proxy-url-password";
        let document_value = "sentinel-disabled-template-value";

        let mut invalid_proxy = route_records(0, "weighted_random", 1, "weighted_random", false);
        invalid_proxy.proxies = vec![proxy(
            Uuid::new_v4(),
            &format!("https://{url_user}:{url_password}@proxy.test"),
            false,
        )];
        let proxy_error = compile_control_plane(invalid_proxy).unwrap_err();
        let proxy_rendered = format!("{proxy_error:?} {proxy_error}");
        assert!(!proxy_rendered.contains(url_user));
        assert!(!proxy_rendered.contains(url_password));

        let mut invalid_template = route_records(0, "weighted_random", 1, "weighted_random", false);
        invalid_template.templates = vec![ConfigTemplateRecord {
            document: serde_json::json!({
                "version": 1,
                "api_format": "open_ai_chat_completions",
                "unknown": document_value
            }),
            ..template(Uuid::new_v4(), "open_ai_chat_completions", false)
        }];
        let template_error = compile_control_plane(invalid_template).unwrap_err();
        let template_rendered = format!("{template_error:?} {template_error}");
        assert!(!template_rendered.contains(document_value));
    }

    #[test]
    fn compiler_validates_resources_referenced_by_disabled_and_auto_disabled_channels() {
        let cases = [
            (
                false,
                false,
                None,
                None,
                Some(Uuid::new_v4()),
                None,
                "missing proxy",
            ),
            (
                true,
                true,
                Some(false),
                None,
                Some(Uuid::new_v4()),
                None,
                "disabled proxy",
            ),
            (
                false,
                false,
                None,
                None,
                None,
                Some(Uuid::new_v4()),
                "missing template",
            ),
            (
                true,
                true,
                None,
                Some(false),
                None,
                Some(Uuid::new_v4()),
                "disabled template",
            ),
        ];
        for (
            enabled,
            auto_disabled,
            proxy_enabled,
            template_enabled,
            proxy_id,
            template_id,
            label,
        ) in cases
        {
            let mut records = route_records(0, "weighted_random", 1, "weighted_random", false);
            records.channels[0].enabled = enabled;
            records.channels[0].auto_disabled = auto_disabled;
            records.channels[0].proxy_id = proxy_id;
            records.channels[0].config_template_id = template_id;
            if let Some(proxy_enabled) = proxy_enabled {
                records.proxies = vec![proxy(
                    proxy_id.unwrap(),
                    "https://proxy.test",
                    proxy_enabled,
                )];
            }
            if let Some(template_enabled) = template_enabled {
                records.templates = vec![template(
                    template_id.unwrap(),
                    "open_ai_chat_completions",
                    template_enabled,
                )];
            }
            assert!(
                compile_control_plane(records).is_err(),
                "{label} was accepted"
            );
        }

        let mut cross_format = route_records(0, "weighted_random", 1, "weighted_random", false);
        cross_format.channels[0].enabled = false;
        let template_id = Uuid::new_v4();
        cross_format.channels[0].config_template_id = Some(template_id);
        cross_format.templates = vec![template(template_id, "open_ai_responses", true)];
        assert!(compile_control_plane(cross_format).is_err());
    }

    #[test]
    fn no_proxy_hosts_normalize_and_match_only_the_accepted_grammar() {
        let exact = NoProxyHost::parse("API.Example.Test").unwrap();
        assert_eq!(exact.dns_name(), Some("api.example.test"));
        assert!(exact.matches_host("api.example.test"));
        assert!(!exact.matches_host("sub.api.example.test"));

        let ipv4 = NoProxyHost::parse("192.0.2.1").unwrap();
        assert_eq!(ipv4.ip_addr(), Some("192.0.2.1".parse().unwrap()));
        assert!(ipv4.matches_host("192.0.2.1"));
        assert!(!ipv4.matches_host("192.0.2.2"));
        let ipv6 = NoProxyHost::parse("::1").unwrap();
        assert!(ipv6.matches_host("::1"));

        let suffix = NoProxyHost::parse("*.Example.Test").unwrap();
        assert!(suffix.is_dns_suffix());
        assert_eq!(suffix.dns_name(), Some("example.test"));
        assert!(suffix.matches_host("api.example.test"));
        assert!(suffix.matches_host("deep.api.example.test"));
        assert!(!suffix.matches_host("example.test"));
        assert!(!suffix.matches_host("other-example.test"));

        for malformed in [
            "sentinel malformed pattern",
            "api.example.test:443",
            "http://api.example.test",
            "*",
            "*.bad_underscore.test",
            "999.0.0.1",
            "api..example.test",
            "api.example.test.",
            "api*example.test",
        ] {
            let error = NoProxyHost::parse(malformed).unwrap_err();
            let rendered = format!("{error:?} {error}");
            assert!(!rendered.contains(malformed));
        }
    }
}
