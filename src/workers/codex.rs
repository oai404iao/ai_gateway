//! Same-process Codex OAuth token and quota maintenance.

use std::time::Duration;

use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{MissedTickBehavior, interval, timeout},
};

use crate::application::CodexConnectorService;

const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

pub struct CodexCredentialWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl CodexCredentialWorker {
    #[must_use]
    pub fn start(service: CodexConnectorService) -> Self {
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ticker = interval(MAINTENANCE_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if let Err(error) = service.run_maintenance().await {
                            tracing::warn!(
                                code = error.code(),
                                "Codex credential maintenance pass failed"
                            );
                        }
                    }
                    _ = &mut shutdown_requested => return,
                }
            }
        });
        Self { shutdown, task }
    }

    pub async fn shutdown(self) {
        let Self { shutdown, mut task } = self;
        let _ = shutdown.send(());
        match timeout(SHUTDOWN_TIMEOUT, &mut task).await {
            Ok(Ok(())) => tracing::info!("Codex credential worker stopped"),
            Ok(Err(error)) => {
                tracing::error!(%error, "Codex credential worker terminated unexpectedly")
            }
            Err(_) => {
                tracing::warn!("Codex credential worker did not stop before deadline");
                task.abort();
                let _ = task.await;
            }
        }
    }
}
