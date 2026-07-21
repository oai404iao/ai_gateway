//! Built-in benchmark profiles shared by the orchestrator, client, and Mock.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

pub const CLIENT_API_KEY: &str = "perf-client-key";
pub const UPSTREAM_API_KEY: &str = "perf-upstream-key";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKind {
    ChatCompletions,
    Responses,
}

impl ApiKind {
    pub const fn path(self) -> &'static str {
        match self {
            Self::ChatCompletions => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
        }
    }

    pub const fn database_name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "open_ai_chat_completions",
            Self::Responses => "open_ai_responses",
        }
    }

    pub const fn short_name(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat",
            Self::Responses => "responses",
        }
    }
}

impl fmt::Display for ApiKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.short_name())
    }
}

impl FromStr for ApiKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "chat" | "chat_completions" => Ok(Self::ChatCompletions),
            "responses" => Ok(Self::Responses),
            _ => Err(format!(
                "unsupported API format {value:?}; expected chat or responses"
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MockMode {
    Json,
    Sse,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MockConfig {
    pub mode: MockMode,
    pub response_delay_ms: u64,
    pub ttft_ms: u64,
    pub chunk_interval_ms: u64,
    pub chunk_count: usize,
}

impl MockConfig {
    pub const fn instant_json() -> Self {
        Self {
            mode: MockMode::Json,
            response_delay_ms: 0,
            ttft_ms: 0,
            chunk_interval_ms: 0,
            chunk_count: 1,
        }
    }

    pub const fn delayed_json(response_delay_ms: u64) -> Self {
        Self {
            mode: MockMode::Json,
            response_delay_ms,
            ttft_ms: 0,
            chunk_interval_ms: 0,
            chunk_count: 1,
        }
    }

    pub const fn sse(ttft_ms: u64, chunk_interval_ms: u64, chunk_count: usize) -> Self {
        Self {
            mode: MockMode::Sse,
            response_delay_ms: 0,
            ttft_ms,
            chunk_interval_ms,
            chunk_count,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.chunk_count == 0 {
            return Err("mock chunk_count must be greater than zero".into());
        }
        if self.chunk_count > 10_000 {
            return Err("mock chunk_count must not exceed 10000".into());
        }
        Ok(())
    }
}

impl Default for MockConfig {
    fn default() -> Self {
        Self::instant_json()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Scenario {
    pub name: String,
    pub model: String,
    pub api_kind: ApiKind,
    pub streamed: bool,
    pub concurrency: usize,
    pub warmup_seconds: u64,
    pub duration_seconds: u64,
    pub timeout_seconds: u64,
    pub mock: MockConfig,
}

impl Scenario {
    fn new(
        name: &str,
        api_kind: ApiKind,
        streamed: bool,
        load: LoadPlan,
        mock: MockConfig,
    ) -> Self {
        Self {
            name: name.into(),
            model: format!("perf-{name}"),
            api_kind,
            streamed,
            concurrency: load.concurrency,
            warmup_seconds: load.warmup_seconds,
            duration_seconds: load.duration_seconds,
            timeout_seconds: load.timeout_seconds,
            mock,
        }
    }
}

#[derive(Clone, Copy)]
struct LoadPlan {
    concurrency: usize,
    warmup_seconds: u64,
    duration_seconds: u64,
    timeout_seconds: u64,
}

const fn load(
    concurrency: usize,
    warmup_seconds: u64,
    duration_seconds: u64,
    timeout_seconds: u64,
) -> LoadPlan {
    LoadPlan {
        concurrency,
        warmup_seconds,
        duration_seconds,
        timeout_seconds,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileName {
    Quick,
    Standard,
}

impl ProfileName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
        }
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProfileName {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "quick" => Ok(Self::Quick),
            "standard" => Ok(Self::Standard),
            _ => Err(format!(
                "unsupported profile {value:?}; expected quick or standard"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: ProfileName,
    pub scenarios: Vec<Scenario>,
}

pub fn profile(name: ProfileName) -> Profile {
    match name {
        ProfileName::Quick => Profile {
            name,
            scenarios: vec![
                Scenario::new(
                    "chat-json-fast",
                    ApiKind::ChatCompletions,
                    false,
                    load(32, 2, 5, 10),
                    MockConfig::instant_json(),
                ),
                Scenario::new(
                    "responses-json-fast",
                    ApiKind::Responses,
                    false,
                    load(32, 2, 5, 10),
                    MockConfig::instant_json(),
                ),
                Scenario::new(
                    "chat-sse-short",
                    ApiKind::ChatCompletions,
                    true,
                    load(32, 2, 5, 15),
                    MockConfig::sse(25, 5, 5),
                ),
                Scenario::new(
                    "responses-sse-short",
                    ApiKind::Responses,
                    true,
                    load(32, 2, 5, 15),
                    MockConfig::sse(25, 5, 5),
                ),
            ],
        },
        ProfileName::Standard => Profile {
            name,
            scenarios: vec![
                Scenario::new(
                    "chat-json-fast",
                    ApiKind::ChatCompletions,
                    false,
                    load(128, 10, 30, 15),
                    MockConfig::instant_json(),
                ),
                Scenario::new(
                    "responses-json-fast",
                    ApiKind::Responses,
                    false,
                    load(128, 10, 30, 15),
                    MockConfig::instant_json(),
                ),
                Scenario::new(
                    "chat-json-50ms",
                    ApiKind::ChatCompletions,
                    false,
                    load(256, 10, 30, 15),
                    MockConfig::delayed_json(50),
                ),
                Scenario::new(
                    "responses-json-50ms",
                    ApiKind::Responses,
                    false,
                    load(256, 10, 30, 15),
                    MockConfig::delayed_json(50),
                ),
                Scenario::new(
                    "chat-sse-short",
                    ApiKind::ChatCompletions,
                    true,
                    load(256, 10, 30, 30),
                    MockConfig::sse(100, 20, 20),
                ),
                Scenario::new(
                    "responses-sse-short",
                    ApiKind::Responses,
                    true,
                    load(256, 10, 30, 30),
                    MockConfig::sse(100, 20, 20),
                ),
            ],
        },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{ApiKind, MockMode, ProfileName, profile};

    #[test]
    fn quick_profile_covers_both_formats_and_response_modes() {
        let profile = profile(ProfileName::Quick);
        let formats = profile
            .scenarios
            .iter()
            .map(|scenario| scenario.api_kind)
            .collect::<HashSet<_>>();
        let modes = profile
            .scenarios
            .iter()
            .map(|scenario| scenario.mock.mode)
            .collect::<HashSet<_>>();

        assert_eq!(
            formats,
            HashSet::from([ApiKind::ChatCompletions, ApiKind::Responses])
        );
        assert_eq!(modes, HashSet::from([MockMode::Json, MockMode::Sse]));
    }

    #[test]
    fn profile_models_are_unique_and_need_no_alias_rewrite() {
        for name in [ProfileName::Quick, ProfileName::Standard] {
            let profile = profile(name);
            let models = profile
                .scenarios
                .iter()
                .map(|scenario| scenario.model.as_str())
                .collect::<HashSet<_>>();
            assert_eq!(models.len(), profile.scenarios.len());
        }
    }
}
