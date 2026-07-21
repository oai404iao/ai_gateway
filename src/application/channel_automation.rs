//! Asynchronous automatic-disable reporting and bounded error-keyword matching.

use std::{str, sync::Arc, time::Duration};

use bytes::Bytes;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

use crate::domain::{AutomaticDisableSettings, AutomaticDisableTrigger};

use super::{ControlPlaneCoordinator, ControlPlaneError};

/// Error response bytes inspected for configured keywords. The data is kept
/// only in memory while a response stream is forwarded and is never logged.
const ERROR_KEYWORD_SCAN_LIMIT_BYTES: usize = 64 * 1_024;
const AUTOMATIC_DISABLE_QUEUE_CAPACITY: usize = 1_024;
const AUTOMATIC_DISABLE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AutomaticDisableService {
    sender: mpsc::Sender<AutomaticDisableRequest>,
}

impl AutomaticDisableService {
    #[must_use]
    pub(crate) fn new(sender: mpsc::Sender<AutomaticDisableRequest>) -> Self {
        Self { sender }
    }

    /// Never waits on persistence from the forwarding path.
    pub fn try_report(&self, channel_id: Uuid, trigger: AutomaticDisableTrigger) {
        match self.sender.try_send(AutomaticDisableRequest {
            channel_id,
            trigger,
        }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    channel_id = %channel_id,
                    reason = "queue_full",
                    "automatic-disable event dropped"
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::error!(
                    channel_id = %channel_id,
                    reason = "queue_closed",
                    "automatic-disable event dropped"
                );
            }
        }
    }
}

pub(crate) struct AutomaticDisableRequest {
    pub(crate) channel_id: Uuid,
    pub(crate) trigger: AutomaticDisableTrigger,
}

/// Owns the bounded background consumer for automatic-disable state changes.
pub struct AutomaticDisableWorker {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl AutomaticDisableWorker {
    #[must_use]
    pub fn start(coordinator: ControlPlaneCoordinator) -> (AutomaticDisableService, Self) {
        let (sender, mut receiver) = mpsc::channel(AUTOMATIC_DISABLE_QUEUE_CAPACITY);
        let (shutdown, mut shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    request = receiver.recv() => match request {
                        Some(request) => apply_disable(&coordinator, request).await,
                        None => return,
                    },
                    _ = &mut shutdown_requested => {
                        receiver.close();
                        while let Some(request) = receiver.recv().await {
                            apply_disable(&coordinator, request).await;
                        }
                        return;
                    }
                }
            }
        });
        (
            AutomaticDisableService::new(sender),
            Self { shutdown, task },
        )
    }

    pub async fn shutdown(self) {
        let Self { shutdown, mut task } = self;
        let _ = shutdown.send(());
        match timeout(AUTOMATIC_DISABLE_SHUTDOWN_TIMEOUT, &mut task).await {
            Ok(Ok(())) => tracing::info!("automatic-disable worker stopped"),
            Ok(Err(error)) => {
                tracing::error!(%error, "automatic-disable worker terminated unexpectedly")
            }
            Err(_) => {
                tracing::warn!(
                    "automatic-disable worker did not drain before shutdown deadline; aborting"
                );
                task.abort();
                let _ = task.await;
            }
        }
    }
}

async fn apply_disable(coordinator: &ControlPlaneCoordinator, request: AutomaticDisableRequest) {
    match coordinator
        .automatically_disable_channel(request.channel_id, request.trigger)
        .await
    {
        Ok(true) => {}
        Ok(false) => tracing::debug!(
            channel_id = %request.channel_id,
            "automatic-disable event no longer matched the current policy"
        ),
        Err(error) => log_automation_error(request.channel_id, error),
    }
}

fn log_automation_error(channel_id: Uuid, error: ControlPlaneError) {
    tracing::error!(
        channel_id = %channel_id,
        error = %error,
        "automatic-disable state transition failed"
    );
}

/// Matches configured error keywords across arbitrary upstream response
/// chunks without retaining more than a small rolling window.
pub struct ErrorKeywordMatcher {
    keywords: Vec<(Arc<str>, String)>,
    carry: Vec<u8>,
    rolling: String,
    rolling_limit: usize,
    remaining_bytes: usize,
}

impl ErrorKeywordMatcher {
    #[must_use]
    pub fn new(settings: &AutomaticDisableSettings) -> Option<Self> {
        if !settings.enabled() || settings.error_message_keywords().is_empty() {
            return None;
        }
        let keywords = settings
            .error_message_keywords()
            .iter()
            .map(|keyword| (Arc::clone(keyword), keyword.to_lowercase()))
            .collect::<Vec<_>>();
        let rolling_limit = keywords
            .iter()
            .map(|(_, normalized)| normalized.len())
            .max()
            .unwrap_or(0)
            .saturating_mul(4)
            .max(16);
        Some(Self {
            keywords,
            carry: vec![],
            rolling: String::new(),
            rolling_limit,
            remaining_bytes: ERROR_KEYWORD_SCAN_LIMIT_BYTES,
        })
    }

    pub fn observe(&mut self, bytes: &Bytes) -> Option<AutomaticDisableTrigger> {
        if self.remaining_bytes == 0 || bytes.is_empty() {
            return None;
        }
        let accepted_len = bytes.len().min(self.remaining_bytes);
        self.remaining_bytes -= accepted_len;
        self.carry.extend_from_slice(&bytes[..accepted_len]);

        loop {
            match str::from_utf8(&self.carry) {
                Ok(text) => {
                    let text = text.to_owned();
                    self.carry.clear();
                    return self.match_text(&text);
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    if valid_up_to > 0 {
                        let text = String::from_utf8_lossy(&self.carry[..valid_up_to]).into_owned();
                        if let Some(trigger) = self.match_text(&text) {
                            self.carry.clear();
                            return Some(trigger);
                        }
                    }
                    match error.error_len() {
                        Some(invalid_len) => {
                            self.carry.drain(..valid_up_to.saturating_add(invalid_len));
                        }
                        None => {
                            self.carry.drain(..valid_up_to);
                            return None;
                        }
                    }
                }
            }
        }
    }

    fn match_text(&mut self, text: &str) -> Option<AutomaticDisableTrigger> {
        self.rolling.push_str(&text.to_lowercase());
        let matched = self
            .keywords
            .iter()
            .find(|(_, normalized)| self.rolling.contains(normalized))
            .map(|(configured, _)| {
                AutomaticDisableTrigger::ErrorMessageKeyword(Arc::clone(configured))
            });
        trim_to_suffix(&mut self.rolling, self.rolling_limit);
        matched
    }
}

fn trim_to_suffix(value: &mut String, maximum_bytes: usize) {
    if value.len() <= maximum_bytes {
        return;
    }
    let mut index = value.len().saturating_sub(maximum_bytes);
    while !value.is_char_boundary(index) {
        index += 1;
    }
    value.drain(..index);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bytes::Bytes;

    use super::ErrorKeywordMatcher;
    use crate::domain::{AutomaticDisableSettings, AutomaticDisableTrigger};

    #[test]
    fn keyword_matching_spans_utf8_response_chunks() {
        let settings =
            AutomaticDisableSettings::new(true, Arc::from([]), vec![Arc::from("余额不足")].into());
        let mut matcher = ErrorKeywordMatcher::new(&settings).unwrap();
        assert_eq!(
            matcher.observe(&Bytes::from_static("余额不".as_bytes())),
            None
        );
        assert_eq!(
            matcher.observe(&Bytes::from_static("足".as_bytes())),
            Some(AutomaticDisableTrigger::ErrorMessageKeyword(Arc::from(
                "余额不足"
            )))
        );
    }
}
