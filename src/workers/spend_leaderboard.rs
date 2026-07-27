//! Periodically rebuilds Console user-spend leaderboard snapshots.

use std::time::Duration;

use tokio::{
    sync::oneshot,
    task::JoinHandle,
    time::{MissedTickBehavior, interval, timeout},
};

use crate::persistence::{RequestLogRepository, SpendLeaderboardRefresh};

const REFRESH_INTERVAL: Duration = Duration::from_secs(15 * 60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Owns the bounded-lag aggregation used by the public Console leaderboard.
/// The first refresh begins immediately after startup; subsequent refreshes
/// run every fifteen minutes.
pub struct SpendLeaderboardWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl SpendLeaderboardWorker {
    #[must_use]
    pub fn start(repository: RequestLogRepository) -> Self {
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut ticker = interval(REFRESH_INTERVAL);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        match repository.refresh_spend_leaderboard_snapshots().await {
                            Ok(SpendLeaderboardRefresh::Updated) => {
                                tracing::debug!(
                                    interval_minutes = 15,
                                    "spend leaderboard snapshots refreshed"
                                );
                            }
                            Ok(SpendLeaderboardRefresh::AlreadyRunning) => {
                                tracing::debug!(
                                    "spend leaderboard refresh already running in another process"
                                );
                            }
                            Err(error) => {
                                tracing::error!(
                                    %error,
                                    "spend leaderboard snapshot refresh failed"
                                );
                            }
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
            Ok(Ok(())) => tracing::info!("spend leaderboard worker stopped"),
            Ok(Err(error)) => {
                tracing::error!(%error, "spend leaderboard worker terminated unexpectedly")
            }
            Err(_) => {
                tracing::warn!(
                    "spend leaderboard worker did not stop before shutdown deadline; aborting"
                );
                task.abort();
                let _ = task.await;
            }
        }
    }
}
