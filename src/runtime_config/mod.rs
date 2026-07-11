//! Immutable runtime configuration snapshots and TOML loading.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use arc_swap::ArcSwap;
use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, Deserialize)]
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
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub max_request_body_bytes: usize,
}

#[derive(Clone, Debug, Deserialize)]
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

/// Atomically replaces immutable configuration snapshots after control-plane changes.
pub struct RuntimeConfig {
    current: ArcSwap<AppConfig>,
}

impl RuntimeConfig {
    pub fn new(initial: AppConfig) -> Self {
        Self {
            current: ArcSwap::from_pointee(initial),
        }
    }

    pub fn snapshot(&self) -> Arc<AppConfig> {
        self.current.load_full()
    }

    pub fn replace(&self, next: AppConfig) {
        self.current.store(Arc::new(next));
    }
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
}
