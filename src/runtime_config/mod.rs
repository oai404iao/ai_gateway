//! TOML loading, validation, and immutable data-plane configuration snapshots.

use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::ArcSwap;
use reqwest::{Url, header::HeaderValue};
use serde::Deserialize;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::domain::{
    ApiFormat, ApiKeyHash, ApiKeyPermission, CompiledApiKey, CompiledChannel, CompiledModelRule,
    CompiledRuntimeConfig, ModelRouteKey, UpstreamAuth,
};

#[derive(Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub observability: ObservabilityConfig,
    #[serde(default)]
    api_keys: Vec<ApiKeyConfig>,
    #[serde(default)]
    channels: Vec<ChannelConfig>,
    #[serde(default)]
    model_rules: Vec<ModelRuleConfig>,
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

    /// Consumes uncompiled TOML data and produces the process bootstrap
    /// settings plus a secret-minimised immutable routing snapshot.
    pub fn compile(self) -> Result<GatewayConfig, ConfigError> {
        let Self {
            server,
            database,
            upstream,
            runtime_config,
            observability,
            api_keys,
            channels,
            model_rules,
        } = self;

        let runtime = compile_runtime_config(api_keys, channels, model_rules)?;
        Ok(GatewayConfig {
            server,
            database,
            upstream,
            runtime_config,
            observability,
            runtime: RuntimeConfig::new(runtime),
        })
    }
}

/// Process-level settings and the compiled data-plane snapshot.
///
/// Client API key plaintext is intentionally absent. Database credentials and
/// upstream credentials have different lifetimes and remain confined to their
/// respective bootstrap/channel settings.
pub struct GatewayConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub upstream: UpstreamConfig,
    pub runtime_config: RuntimeConfigSettings,
    pub observability: ObservabilityConfig,
    pub runtime: RuntimeConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_request_body_bytes: usize,
}

#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub connect_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpstreamConfig {
    pub connect_timeout_seconds: u64,
    pub response_header_timeout_seconds: u64,
    pub stream_idle_timeout_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuntimeConfigSettings {
    pub reload_interval_seconds: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ObservabilityConfig {
    pub filter: String,
}

#[derive(Deserialize)]
struct ApiKeyConfig {
    id: String,
    key: String,
    allowed_api_formats: Vec<ApiFormat>,
    permissions: Vec<ApiKeyPermission>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

#[derive(Deserialize)]
struct ChannelConfig {
    id: String,
    api_format: ApiFormat,
    base_url: String,
    #[serde(default)]
    upstream_bearer_token: Option<String>,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

#[derive(Deserialize)]
struct ModelRuleConfig {
    client_model: String,
    api_format: ApiFormat,
    upstream_model: String,
    channel_id: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

const fn enabled_by_default() -> bool {
    true
}

/// Atomically swaps whole, immutable routing snapshots.
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

    pub fn replace(&self, next: CompiledRuntimeConfig) {
        self.replace_snapshot(Arc::new(next));
    }

    pub fn replace_snapshot(&self, next: Arc<CompiledRuntimeConfig>) {
        self.current.store(next);
    }
}

fn compile_runtime_config(
    api_keys: Vec<ApiKeyConfig>,
    channels: Vec<ChannelConfig>,
    model_rules: Vec<ModelRuleConfig>,
) -> Result<CompiledRuntimeConfig, ConfigError> {
    let api_keys = compile_api_keys(api_keys)?;
    let channels = compile_channels(channels)?;
    let model_rules = compile_model_rules(model_rules, &channels)?;

    Ok(CompiledRuntimeConfig::new(api_keys, model_rules))
}

fn compile_api_keys(
    api_keys: Vec<ApiKeyConfig>,
) -> Result<HashMap<ApiKeyHash, Arc<CompiledApiKey>>, ConfigError> {
    let mut compiled = HashMap::with_capacity(api_keys.len());
    let mut ids = HashSet::with_capacity(api_keys.len());

    for api_key in api_keys {
        require_non_empty("API key id", &api_key.id)?;
        if !ids.insert(api_key.id.clone()) {
            return Err(ConfigError::Compile(format!(
                "duplicate API key id `{}`",
                api_key.id
            )));
        }

        let secret = Zeroizing::new(api_key.key);
        require_non_empty("API key", secret.as_str())?;
        if api_key.allowed_api_formats.is_empty() {
            return Err(ConfigError::Compile(format!(
                "API key `{}` must allow at least one API format",
                api_key.id
            )));
        }
        if api_key.permissions.is_empty() {
            return Err(ConfigError::Compile(format!(
                "API key `{}` must grant at least one permission",
                api_key.id
            )));
        }

        if !api_key.enabled {
            continue;
        }

        let hash = ApiKeyHash::from_secret(secret.as_str());
        let principal = Arc::new(CompiledApiKey::new(
            Arc::from(api_key.id),
            api_key.allowed_api_formats.into_iter().collect(),
            api_key.permissions.into_iter().collect(),
        ));
        if compiled.insert(hash, principal).is_some() {
            return Err(ConfigError::Compile(
                "duplicate active API key secret".to_owned(),
            ));
        }
    }

    Ok(compiled)
}

fn compile_channels(
    channels: Vec<ChannelConfig>,
) -> Result<HashMap<String, Arc<CompiledChannel>>, ConfigError> {
    let mut compiled = HashMap::with_capacity(channels.len());
    let mut ids = HashSet::with_capacity(channels.len());

    for channel in channels {
        require_non_empty("channel id", &channel.id)?;
        if !ids.insert(channel.id.clone()) {
            return Err(ConfigError::Compile(format!(
                "duplicate channel id `{}`",
                channel.id
            )));
        }

        let base_url = parse_base_url(&channel.id, &channel.base_url)?;
        let upstream_auth = match channel.upstream_bearer_token {
            Some(token) => {
                let token = Zeroizing::new(token);
                require_non_empty("upstream bearer token", token.as_str())?;
                HeaderValue::from_str(token.as_str()).map_err(|_| {
                    ConfigError::Compile(format!(
                        "upstream bearer token for channel `{}` is not a valid HTTP header value",
                        channel.id
                    ))
                })?;
                UpstreamAuth::Bearer(Arc::from(token.as_str()))
            }
            None => UpstreamAuth::None,
        };

        if channel.enabled {
            let id = Arc::<str>::from(channel.id.clone());
            compiled.insert(
                channel.id,
                Arc::new(CompiledChannel::new(
                    id,
                    channel.api_format,
                    base_url,
                    upstream_auth,
                )),
            );
        }
    }

    Ok(compiled)
}

fn compile_model_rules(
    model_rules: Vec<ModelRuleConfig>,
    channels: &HashMap<String, Arc<CompiledChannel>>,
) -> Result<HashMap<ModelRouteKey, Arc<CompiledModelRule>>, ConfigError> {
    let mut compiled = HashMap::with_capacity(model_rules.len());

    for model_rule in model_rules {
        require_non_empty("client model", &model_rule.client_model)?;
        require_non_empty("upstream model", &model_rule.upstream_model)?;
        require_non_empty("model rule channel id", &model_rule.channel_id)?;
        if !model_rule.enabled {
            continue;
        }

        let channel = channels.get(&model_rule.channel_id).ok_or_else(|| {
            ConfigError::Compile(format!(
                "enabled model rule for `{}` references missing or disabled channel `{}`",
                model_rule.client_model, model_rule.channel_id
            ))
        })?;
        if channel.api_format() != model_rule.api_format {
            return Err(ConfigError::Compile(format!(
                "model rule `{}` and channel `{}` use different API formats",
                model_rule.client_model,
                channel.id()
            )));
        }

        let key = ModelRouteKey::new(
            model_rule.api_format,
            Arc::<str>::from(model_rule.client_model.clone()),
        );
        let rule = Arc::new(CompiledModelRule::new(
            Arc::from(model_rule.client_model),
            model_rule.api_format,
            Arc::from(model_rule.upstream_model),
            Arc::clone(channel),
        ));
        if compiled.insert(key, rule).is_some() {
            return Err(ConfigError::Compile(
                "duplicate enabled model rule for the same client model and API format".to_owned(),
            ));
        }
    }

    Ok(compiled)
}

fn parse_base_url(channel_id: &str, value: &str) -> Result<Url, ConfigError> {
    let url = Url::parse(value).map_err(|_| {
        ConfigError::Compile(format!("channel `{channel_id}` has an invalid base_url"))
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ConfigError::Compile(format!(
            "channel `{channel_id}` base_url must be an http(s) URL without credentials, query, or fragment"
        )));
    }
    Ok(url)
}

fn require_non_empty(field: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() {
        return Err(ConfigError::Compile(format!("{field} must not be empty")));
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

    const BASE_CONFIG: &str = r#"
[server]
host = "127.0.0.1"
port = 3000
max_request_body_bytes = 1048576

[database]
url = "postgres://gateway:gateway@127.0.0.1/gateway"
max_connections = 10
connect_timeout_seconds = 5

[upstream]
connect_timeout_seconds = 10
response_header_timeout_seconds = 30
stream_idle_timeout_seconds = 90

[runtime_config]
reload_interval_seconds = 30

[observability]
filter = "info"

[[api_keys]]
id = "client-one"
key = "client-secret"
allowed_api_formats = ["open_ai_chat_completions", "open_ai_responses"]
permissions = ["proxy", "models.read"]

[[channels]]
id = "chat-upstream"
api_format = "open_ai_chat_completions"
base_url = "https://chat.example.test/api"
upstream_bearer_token = "upstream-secret"

[[channels]]
id = "responses-upstream"
api_format = "open_ai_responses"
base_url = "https://responses.example.test"

[[model_rules]]
client_model = "shared-model"
api_format = "open_ai_chat_completions"
upstream_model = "chat-model-2025"
channel_id = "chat-upstream"

[[model_rules]]
client_model = "shared-model"
api_format = "open_ai_responses"
upstream_model = "responses-model-2025"
channel_id = "responses-upstream"
"#;

    fn compile(value: &str) -> GatewayConfig {
        toml::from_str::<AppConfig>(value)
            .unwrap()
            .compile()
            .unwrap()
    }

    #[test]
    fn compiled_snapshot_authenticates_and_redacts_client_secrets() {
        let gateway = compile(BASE_CONFIG);
        let snapshot = gateway.runtime.snapshot();

        let principal = snapshot.authenticate("client-secret").unwrap();
        assert_eq!(principal.id(), "client-one");
        assert!(snapshot.authenticate("wrong-secret").is_none());

        let debug_output = format!("{snapshot:?}");
        assert!(!debug_output.contains("client-secret"));
        assert!(!debug_output.contains("upstream-secret"));
    }

    #[test]
    fn same_model_is_kept_separate_by_api_format() {
        let gateway = compile(BASE_CONFIG);
        let snapshot = gateway.runtime.snapshot();

        assert_eq!(
            snapshot
                .model_rule(ApiFormat::OpenAiChatCompletions, "shared-model")
                .unwrap()
                .upstream_model(),
            "chat-model-2025"
        );
        assert_eq!(
            snapshot
                .model_rule(ApiFormat::OpenAiResponses, "shared-model")
                .unwrap()
                .upstream_model(),
            "responses-model-2025"
        );
        assert!(
            snapshot
                .model_rule(ApiFormat::OpenAiChatCompletions, "responses-model-2025")
                .is_none()
        );
    }

    #[test]
    fn rejects_model_rule_with_channel_in_another_format() {
        let invalid = BASE_CONFIG.replace(
            "channel_id = \"chat-upstream\"\n\n[[model_rules]]\nclient_model = \"shared-model\"\napi_format = \"open_ai_responses\"\nupstream_model = \"responses-model-2025\"\nchannel_id = \"responses-upstream\"",
            "channel_id = \"chat-upstream\"\n\n[[model_rules]]\nclient_model = \"shared-model\"\napi_format = \"open_ai_responses\"\nupstream_model = \"responses-model-2025\"\nchannel_id = \"chat-upstream\"",
        );

        let error = match toml::from_str::<AppConfig>(&invalid).unwrap().compile() {
            Ok(_) => panic!("configuration should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("different API formats"));
    }

    #[test]
    fn rejects_duplicate_enabled_model_rules() {
        let invalid = format!(
            "{BASE_CONFIG}\n[[model_rules]]\nclient_model = \"shared-model\"\napi_format = \"open_ai_chat_completions\"\nupstream_model = \"other-model\"\nchannel_id = \"chat-upstream\"\n"
        );

        let error = match toml::from_str::<AppConfig>(&invalid).unwrap().compile() {
            Ok(_) => panic!("configuration should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("duplicate enabled model rule"));
    }

    #[test]
    fn snapshot_replacement_keeps_inflight_snapshot_valid() {
        let gateway = compile(BASE_CONFIG);
        let before = gateway.runtime.snapshot();
        let replacement = compile(&BASE_CONFIG.replace("chat-model-2025", "new-chat-model"));

        gateway
            .runtime
            .replace_snapshot(replacement.runtime.snapshot());
        let after = gateway.runtime.snapshot();

        assert_eq!(
            before
                .model_rule(ApiFormat::OpenAiChatCompletions, "shared-model")
                .unwrap()
                .upstream_model(),
            "chat-model-2025"
        );
        assert_eq!(
            after
                .model_rule(ApiFormat::OpenAiChatCompletions, "shared-model")
                .unwrap()
                .upstream_model(),
            "new-chat-model"
        );
    }
}
