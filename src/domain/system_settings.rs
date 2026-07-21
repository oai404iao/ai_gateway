//! Validated process-wide forwarding settings carried by runtime snapshots.

use std::time::Duration;

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

/// Immutable global forwarding policy published with each runtime snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SystemRuntimeSettings {
    upstream_timeouts: UpstreamTimeoutDefaults,
    passive_health: PassiveHealthSettings,
}

impl SystemRuntimeSettings {
    #[must_use]
    pub const fn new(
        upstream_timeouts: UpstreamTimeoutDefaults,
        passive_health: PassiveHealthSettings,
    ) -> Self {
        Self {
            upstream_timeouts,
            passive_health,
        }
    }

    #[must_use]
    pub const fn upstream_timeouts(self) -> UpstreamTimeoutDefaults {
        self.upstream_timeouts
    }

    #[must_use]
    pub const fn passive_health(self) -> PassiveHealthSettings {
        self.passive_health
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
