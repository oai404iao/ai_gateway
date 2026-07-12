//! Background control-plane snapshot reloading.

use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio::{
    sync::Mutex,
    time::{MissedTickBehavior, interval},
};

use crate::{
    persistence::{ControlPlaneRepository, RepositoryError},
    runtime_config::{ConfigError, RuntimeConfig, compile_control_plane},
};

#[derive(Clone)]
pub struct ControlPlaneReloader {
    repository: ControlPlaneRepository,
    runtime: Arc<RuntimeConfig>,
    serial: Arc<Mutex<()>>,
}
impl ControlPlaneReloader {
    #[must_use]
    pub fn new(repository: ControlPlaneRepository, runtime: Arc<RuntimeConfig>) -> Self {
        Self {
            repository,
            runtime,
            serial: Arc::new(Mutex::new(())),
        }
    }
    pub async fn reload(&self) -> Result<(), ReloadError> {
        let _guard = self.serial.lock().await;
        let records = self.repository.load().await?;
        let next = Arc::new(compile_control_plane(records)?);
        self.runtime.replace_snapshot(next);
        Ok(())
    }
    pub fn spawn(self, frequency: Duration) {
        tokio::spawn(async move {
            let mut ticker = interval(frequency);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = self.reload().await {
                    tracing::error!(error = %error, "control-plane reload failed; retaining previous snapshot");
                }
            }
        });
    }
}
#[derive(Debug, Error)]
pub enum ReloadError {
    #[error("control-plane load failed")]
    Repository(#[from] RepositoryError),
    #[error("control-plane compilation failed: {0}")]
    Compile(#[from] ConfigError),
}
