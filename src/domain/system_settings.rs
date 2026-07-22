//! Validated process-wide forwarding and channel-automation settings carried
//! by runtime snapshots.

use std::{sync::Arc, time::Duration};

use regex::Regex;
use reqwest::header::HeaderName;

use super::ApiFormat;

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

/// A compiled source used to extract a bounded session-affinity value from an
/// authenticated proxy request.
#[derive(Clone, Debug)]
pub enum SessionAffinityKeySource {
    RequestHeader(HeaderName),
    JsonPointer(Arc<str>),
}

/// One immutable, prevalidated session-affinity rule.
#[derive(Clone, Debug)]
pub struct SessionAffinityRule {
    name: Arc<str>,
    fingerprint: [u8; 32],
    api_formats: Arc<[ApiFormat]>,
    model_regex: Arc<[Regex]>,
    key_sources: Arc<[SessionAffinityKeySource]>,
    value_regex: Option<Regex>,
    ttl: Duration,
}

impl SessionAffinityRule {
    #[must_use]
    pub fn new(
        name: Arc<str>,
        fingerprint: [u8; 32],
        api_formats: Arc<[ApiFormat]>,
        model_regex: Arc<[Regex]>,
        key_sources: Arc<[SessionAffinityKeySource]>,
        value_regex: Option<Regex>,
        ttl: Duration,
    ) -> Self {
        Self {
            name,
            fingerprint,
            api_formats,
            model_regex,
            key_sources,
            value_regex,
            ttl,
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }

    #[must_use]
    pub fn key_sources(&self) -> &[SessionAffinityKeySource] {
        &self.key_sources
    }

    #[must_use]
    pub const fn ttl(&self) -> Duration {
        self.ttl
    }

    #[must_use]
    pub fn matches_request(&self, api_format: ApiFormat, model: &str) -> bool {
        self.api_formats.contains(&api_format)
            && (self.model_regex.is_empty()
                || self
                    .model_regex
                    .iter()
                    .any(|pattern| pattern.is_match(model)))
    }

    #[must_use]
    pub fn matches_value(&self, value: &str) -> bool {
        self.value_regex
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(value))
    }
}

/// Immutable global policy for process-local, successful-channel session
/// affinity.
#[derive(Clone, Debug)]
pub struct SessionAffinitySettings {
    enabled: bool,
    max_entries: usize,
    default_ttl: Duration,
    rules: Arc<[SessionAffinityRule]>,
}

impl SessionAffinitySettings {
    #[must_use]
    pub fn new(
        enabled: bool,
        max_entries: usize,
        default_ttl: Duration,
        rules: Arc<[SessionAffinityRule]>,
    ) -> Self {
        Self {
            enabled,
            max_entries,
            default_ttl,
            rules,
        }
    }

    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub const fn max_entries(&self) -> usize {
        self.max_entries
    }

    #[must_use]
    pub const fn default_ttl(&self) -> Duration {
        self.default_ttl
    }

    #[must_use]
    pub fn rules(&self) -> &[SessionAffinityRule] {
        &self.rules
    }
}

impl Default for SessionAffinitySettings {
    fn default() -> Self {
        Self::new(false, 100_000, Duration::from_secs(3_600), Arc::from([]))
    }
}

/// Immutable global forwarding policy published with each runtime snapshot.
#[derive(Clone, Debug)]
pub struct SystemRuntimeSettings {
    upstream_timeouts: UpstreamTimeoutDefaults,
    passive_health: PassiveHealthSettings,
    automatic_disable: AutomaticDisableSettings,
    scheduled_testing: ScheduledTestingSettings,
    session_affinity: SessionAffinitySettings,
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
            session_affinity: SessionAffinitySettings::default(),
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
            session_affinity: SessionAffinitySettings::default(),
        }
    }

    #[must_use]
    pub fn new_with_all(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
        automatic_disable: AutomaticDisableSettings,
        scheduled_testing: ScheduledTestingSettings,
        session_affinity: SessionAffinitySettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            passive_health,
            automatic_disable,
            scheduled_testing,
            session_affinity,
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

    #[must_use]
    pub fn session_affinity(&self) -> &SessionAffinitySettings {
        &self.session_affinity
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
