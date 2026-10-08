use std::{path::PathBuf, sync::OnceLock};

use ai_gateway::connector_plugins::{ConnectorPlugins, PluginConfig};
use sha2::{Digest, Sha256};

pub fn codex_plugins() -> ConnectorPlugins {
    static PLUGINS: OnceLock<ConnectorPlugins> = OnceLock::new();
    PLUGINS
        .get_or_init(|| {
            let path = PathBuf::from(
                std::env::var_os("AI_GATEWAY_TEST_CODEX_PLUGIN")
                    .expect("run scripts/prepare-connector-tests.sh and export its plugin path"),
            );
            let sha256 = Sha256::digest(std::fs::read(&path).unwrap())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            ConnectorPlugins::load(&[PluginConfig {
                id: "codex".into(),
                path,
                sha256,
            }])
            .expect("the test Codex plugin must satisfy the production loader contract")
        })
        .clone()
}
