//! Validated process-wide forwarding and channel-automation settings carried
//! by runtime snapshots.

use std::{sync::Arc, time::Duration};

/// Global timeout defaults used when a channel has no explicit override.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpstreamTimeoutDefaults {
    connect: Duration,
    response_header: Duration,
    stream_idle: Duration,
}

impl UpstreamTimeoutDefaults {
    #[must_use]
    pub const fn new(connect: Duration, response_header: Duration, stream_idle: Duration) -> Self {
        Self {
            connect,
            response_header,
            stream_idle,
        }
    }

    #[must_use]
    pub const fn connect(self) -> Duration {
        self.connect
    }

    #[must_use]
    pub const fn response_header(self) -> Duration {
        self.response_header
    }

    #[must_use]
    pub const fn stream_idle(self) -> Duration {
        self.stream_idle
    }
}

impl Default for UpstreamTimeoutDefaults {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(10),
            Duration::from_secs(30),
            Duration::from_secs(90),
        )
    }
}

/// Process-wide passive connection-health settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PassiveHealthSettings {
    connection_failure_threshold: u32,
    cooldown: Duration,
}

impl PassiveHealthSettings {
    #[must_use]
    pub const fn new(connection_failure_threshold: u32, cooldown: Duration) -> Self {
        Self {
            connection_failure_threshold,
            cooldown,
        }
    }

    #[must_use]
    pub const fn connection_failure_threshold(self) -> u32 {
        self.connection_failure_threshold
    }

    #[must_use]
    pub const fn cooldown(self) -> Duration {
        self.cooldown
    }
}

impl Default for PassiveHealthSettings {
    fn default() -> Self {
        Self::new(3, Duration::from_secs(30))
    }
}

/// Immutable matching policy for automatically taking a channel out of
/// rotation after a configured upstream error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutomaticDisableSettings {
    enabled: bool,
    error_status_codes: Arc<[u16]>,
    error_message_keywords: Arc<[Arc<str>]>,
}

impl AutomaticDisableSettings {
    #[must_use]
    pub fn new(
        enabled: bool,
        error_status_codes: Arc<[u16]>,
        error_message_keywords: Arc<[Arc<str>]>,
    ) -> Self {
        Self {
            enabled,
            error_status_codes,
            error_message_keywords,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn matches_status(&self, status: u16) -> bool {
        self.enabled && self.error_status_codes.contains(&status)
    }

    #[must_use]
    pub fn error_message_keywords(&self) -> &[Arc<str>] {
        &self.error_message_keywords
    }
}

impl Default for AutomaticDisableSettings {
    fn default() -> Self {
        Self::new(false, Arc::from([]), Arc::from([]))
    }
}

/// The sanitized upstream failure fact that can trigger automatic disabling.
///
/// It intentionally carries only a HTTP status or a configured keyword, never
/// the raw upstream response body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomaticDisableTrigger {
    HttpStatus(u16),
    ErrorMessageKeyword(Arc<str>),
}

impl AutomaticDisableSettings {
    #[must_use]
    pub fn matches_trigger(&self, trigger: &AutomaticDisableTrigger) -> bool {
        if !self.enabled {
            return false;
        }
        match trigger {
            AutomaticDisableTrigger::HttpStatus(status) => self.error_status_codes.contains(status),
            AutomaticDisableTrigger::ErrorMessageKeyword(keyword) => self
                .error_message_keywords
                .iter()
                .any(|candidate| candidate.to_lowercase() == keyword.to_lowercase()),
        }
    }
}

/// Scope for periodic upstream test requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScheduledTestingMode {
    Global,
    FailureOnly,
}

/// Immutable policy controlling periodic direct upstream test requests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledTestingSettings {
    mode: ScheduledTestingMode,
    auto_recover: bool,
    interval: Duration,
    prompt: Arc<str>,
}

impl ScheduledTestingSettings {
    #[must_use]
    pub fn new(
        mode: ScheduledTestingMode,
        auto_recover: bool,
        interval: Duration,
        prompt: Arc<str>,
    ) -> Self {
        Self {
            mode,
            auto_recover,
            interval,
            prompt,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> ScheduledTestingMode {
        self.mode
    }

    #[must_use]
    pub const fn auto_recover(&self) -> bool {
        self.auto_recover
    }

    #[must_use]
    pub const fn interval(&self) -> Duration {
        self.interval
    }

    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

impl Default for ScheduledTestingSettings {
    fn default() -> Self {
        Self::new(
            ScheduledTestingMode::Global,
            true,
            Duration::from_secs(5 * 60),
            Arc::from("reply '1'"),
        )
    }
}

/// Immutable global forwarding policy published with each runtime snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemRuntimeSettings {
    upstream_timeouts: UpstreamTimeoutDefaults,
    passive_health: PassiveHealthSettings,
    automatic_disable: AutomaticDisableSettings,
    scheduled_testing: ScheduledTestingSettings,
}

impl SystemRuntimeSettings {
    #[must_use]
    pub fn new(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            passive_health,
            automatic_disable: AutomaticDisableSettings::default(),
            scheduled_testing: ScheduledTestingSettings::default(),
        }
    }

    #[must_use]
    pub fn new_with_channel_automation(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
        automatic_disable: AutomaticDisableSettings,
        scheduled_testing: ScheduledTestingSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            passive_health,
            automatic_disable,
            scheduled_testing,
        }
    }

    #[must_use]
    pub const fn upstream_timeouts(&self) -> UpstreamTimeoutDefaults {
        self.upstream_timeouts
    }

    #[must_use]
    pub const fn passive_health(&self) -> PassiveHealthSettings {
        self.passive_health
    }

    #[must_use]
    pub fn automatic_disable(&self) -> &AutomaticDisableSettings {
        &self.automatic_disable
    }

    #[must_use]
    pub fn scheduled_testing(&self) -> &ScheduledTestingSettings {
        &self.scheduled_testing
    }
}

impl Default for SystemRuntimeSettings {
    fn default() -> Self {
        Self::new(
            UpstreamTimeoutDefaults::default(),
            PassiveHealthSettings::default(),
        )
    }
}
