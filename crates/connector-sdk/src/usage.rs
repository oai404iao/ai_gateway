//! Pure upstream-interface usage normalization into host-owned token counters.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const USAGE_PARSE: &str = "usage.parse/v1";
pub const MAX_USAGE_BYTES: usize = 64 * 1024;
pub const MAX_USAGE_INTERFACE_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageFormat {
    OpenAiChatCompletions,
    OpenAiResponses,
    OpenAiImages,
    AnthropicMessages,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "parser", rename_all = "snake_case", deny_unknown_fields)]
pub enum UsageDescriptor {
    General { format: UsageFormat },
    Plugin { interface: String },
}

impl UsageDescriptor {
    pub fn validate_bounds(&self) -> bool {
        match self {
            Self::General { .. } => true,
            Self::Plugin { interface } => valid_interface(interface),
        }
    }
}

/// Input totals include cache reads and writes; output totals include reasoning.
/// Cache and reasoning counters are subsets, not additional total tokens.
/// The host retains its existing additional cache-write pricing policy; this
/// contract supplies counts, never prices or settlement instructions.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalUsage {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
}

impl CanonicalUsage {
    pub fn validate(&self) -> bool {
        self.input_tokens >= 0
            && self.cached_input_tokens >= 0
            && self.cache_write_tokens >= 0
            && self.output_tokens >= 0
            && self.reasoning_tokens >= 0
            && self.cached_input_tokens <= self.input_tokens
            && self.cache_write_tokens <= self.input_tokens
            && self.reasoning_tokens <= self.output_tokens
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageParseInput {
    pub operation: String,
    pub interface: String,
}

impl UsageParseInput {
    pub fn validate_bounds(&self, body: &[u8]) -> bool {
        matches!(
            self.operation.as_str(),
            "chat_completion"
                | "responses"
                | "responses-ws"
                | "web_search"
                | "images_generation"
                | "images_edit"
        ) && valid_interface(&self.interface)
            && body.len() <= MAX_USAGE_BYTES
            && serde_json::from_slice::<Value>(body).is_ok_and(|usage| usage.is_object())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageParseOutput {
    pub usage: Option<CanonicalUsage>,
}

impl UsageParseOutput {
    pub fn validate_bounds(&self) -> bool {
        self.usage.as_ref().is_none_or(CanonicalUsage::validate)
    }
}

pub fn parse_general_usage(format: UsageFormat, usage: &Value) -> Option<CanonicalUsage> {
    if !usage.is_object() || !crate::response::serialized_within(usage, MAX_USAGE_BYTES) {
        return None;
    }
    let parsed = match format {
        UsageFormat::AnthropicMessages => {
            let uncached = token(usage.get("input_tokens"))?;
            let cached = optional_token(usage, "cache_read_input_tokens")
                .ok()?
                .unwrap_or(0);
            let written = optional_token(usage, "cache_creation_input_tokens")
                .ok()?
                .unwrap_or(0);
            CanonicalUsage {
                input_tokens: uncached.checked_add(cached)?.checked_add(written)?,
                cached_input_tokens: cached,
                cache_write_tokens: written,
                output_tokens: token(usage.get("output_tokens"))?,
                reasoning_tokens: 0,
            }
        }
        UsageFormat::OpenAiChatCompletions
        | UsageFormat::OpenAiResponses
        | UsageFormat::OpenAiImages => {
            let (input, output, input_details, output_details) = match format {
                UsageFormat::OpenAiChatCompletions => (
                    "prompt_tokens",
                    "completion_tokens",
                    "prompt_tokens_details",
                    "completion_tokens_details",
                ),
                _ => (
                    "input_tokens",
                    "output_tokens",
                    "input_tokens_details",
                    "output_tokens_details",
                ),
            };
            let input_details = details(usage, input_details).ok()?;
            let output_details = details(usage, output_details).ok()?;
            let nested_cached = detail_token(input_details, "cached_tokens").ok()?;
            let cached = if format == UsageFormat::OpenAiChatCompletions {
                let hit = optional_token(usage, "prompt_cache_hit_tokens").ok()?;
                optional_token(usage, "prompt_cache_miss_tokens").ok()?;
                hit.or(nested_cached)
            } else {
                nested_cached
            };
            let written = detail_token(input_details, "cache_write_tokens").ok()?;
            let creation = detail_token(input_details, "cache_creation_tokens").ok()?;
            CanonicalUsage {
                input_tokens: token(usage.get(input))?,
                cached_input_tokens: cached.unwrap_or(0),
                cache_write_tokens: written.or(creation).unwrap_or(0),
                output_tokens: token(usage.get(output))?,
                reasoning_tokens: detail_token(output_details, "reasoning_tokens")
                    .ok()?
                    .unwrap_or(0),
            }
        }
    };
    parsed.validate().then_some(parsed)
}

fn valid_interface(interface: &str) -> bool {
    !interface.is_empty()
        && interface.len() <= MAX_USAGE_INTERFACE_BYTES
        && interface.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-./".contains(&byte)
        })
        && interface.as_bytes()[0].is_ascii_lowercase()
}

fn token(value: Option<&Value>) -> Option<i64> {
    value?.as_i64().filter(|value| *value >= 0)
}

fn optional_token(usage: &Value, key: &str) -> Result<Option<i64>, ()> {
    usage
        .get(key)
        .map(|value| token(Some(value)).ok_or(()))
        .transpose()
}

fn details<'a>(usage: &'a Value, key: &str) -> Result<Option<&'a Value>, ()> {
    match usage.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) if value.is_object() => Ok(Some(value)),
        Some(_) => Err(()),
    }
}

fn detail_token(details: Option<&Value>, key: &str) -> Result<Option<i64>, ()> {
    details.map_or(Ok(None), |details| optional_token(details, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inclusive_openai_totals_and_legacy_alias_precedence_are_preserved() {
        let usage = json!({
            "prompt_tokens":100,"completion_tokens":30,
            "prompt_cache_hit_tokens":40,"prompt_cache_miss_tokens":60,
            "prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":20,"cache_creation_tokens":10},
            "completion_tokens_details":{"reasoning_tokens":8}
        });
        assert_eq!(
            parse_general_usage(UsageFormat::OpenAiChatCompletions, &usage),
            Some(CanonicalUsage {
                input_tokens: 100,
                cached_input_tokens: 40,
                cache_write_tokens: 20,
                output_tokens: 30,
                reasoning_tokens: 8
            })
        );
        for format in [UsageFormat::OpenAiResponses, UsageFormat::OpenAiImages] {
            let usage = json!({"input_tokens":100,"output_tokens":30,
                "input_tokens_details":{"cached_tokens":40,"cache_creation_tokens":20},
                "output_tokens_details":{"reasoning_tokens":8}});
            assert_eq!(
                parse_general_usage(format, &usage),
                Some(CanonicalUsage {
                    input_tokens: 100,
                    cached_input_tokens: 40,
                    cache_write_tokens: 20,
                    output_tokens: 30,
                    reasoning_tokens: 8
                })
            );
        }
    }

    #[test]
    fn cache_details_never_guess_the_upstream_interface() {
        let usage = json!({"input_tokens":2,"output_tokens":4,
            "cache_read_input_tokens":90,"cache_creation_input_tokens":10});
        assert_eq!(
            parse_general_usage(UsageFormat::AnthropicMessages, &usage),
            Some(CanonicalUsage {
                input_tokens: 102,
                cached_input_tokens: 90,
                cache_write_tokens: 10,
                output_tokens: 4,
                reasoning_tokens: 0
            })
        );
        assert_eq!(
            parse_general_usage(UsageFormat::OpenAiResponses, &usage),
            Some(CanonicalUsage {
                input_tokens: 2,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 4,
                reasoning_tokens: 0
            })
        );
        assert!(parse_general_usage(UsageFormat::OpenAiChatCompletions, &usage).is_none());
    }

    #[test]
    fn malformed_or_overflowing_counters_are_unknown() {
        for malformed in [
            json!(-1),
            json!(1.5),
            json!("2"),
            json!(null),
            json!(u64::MAX),
        ] {
            for key in [
                "input_tokens",
                "output_tokens",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
            ] {
                let mut usage = json!({"input_tokens":2,"output_tokens":4});
                usage[key] = malformed.clone();
                assert!(parse_general_usage(UsageFormat::AnthropicMessages, &usage).is_none());
            }
            let mut usage = json!({"prompt_tokens":100,"completion_tokens":30,
                "prompt_tokens_details":{"cached_tokens":40}});
            usage["prompt_cache_hit_tokens"] = malformed.clone();
            assert!(parse_general_usage(UsageFormat::OpenAiChatCompletions, &usage).is_none());
        }
        let usage = json!({"input_tokens":i64::MAX,"output_tokens":1,"cache_read_input_tokens":1});
        assert!(parse_general_usage(UsageFormat::AnthropicMessages, &usage).is_none());
        for usage in [
            json!({"input_tokens":1,"output_tokens":1,"input_tokens_details":{"cached_tokens":2}}),
            json!({"input_tokens":1,"output_tokens":1,"input_tokens_details":{"cache_write_tokens":2}}),
            json!({"input_tokens":1,"output_tokens":1,"output_tokens_details":{"reasoning_tokens":2}}),
            json!({"input_tokens":1}),
        ] {
            assert!(parse_general_usage(UsageFormat::OpenAiResponses, &usage).is_none());
        }
    }

    #[test]
    fn canonical_contract_preserves_existing_individual_subset_invariants() {
        let usage = CanonicalUsage {
            input_tokens: 5,
            cached_input_tokens: 4,
            cache_write_tokens: 4,
            output_tokens: 3,
            reasoning_tokens: 2,
        };
        assert!(usage.validate());
        assert!(UsageParseOutput { usage: Some(usage) }.validate_bounds());
        assert!(
            !UsageParseOutput {
                usage: Some(CanonicalUsage {
                    reasoning_tokens: 4,
                    ..usage
                })
            }
            .validate_bounds()
        );
        assert!(UsageParseOutput { usage: None }.validate_bounds());
    }

    #[test]
    fn custom_parser_contract_is_bounded_and_counter_only() {
        let input = UsageParseInput {
            operation: "responses".into(),
            interface: "anthropic.messages/v1".into(),
        };
        assert!(input.validate_bounds(br#"{"input_tokens":1,"output_tokens":2}"#));
        assert!(!input.validate_bounds(b"[]"));
        assert!(!input.validate_bounds(&vec![b' '; MAX_USAGE_BYTES + 1]));
        for interface in ["", "../provider", "Provider", "provider path"] {
            assert!(
                !UsageDescriptor::Plugin {
                    interface: interface.into()
                }
                .validate_bounds()
            );
        }
        assert!(
            !UsageDescriptor::Plugin {
                interface: "a".repeat(MAX_USAGE_INTERFACE_BYTES + 1)
            }
            .validate_bounds()
        );
        assert!(
            serde_json::from_value::<UsageDescriptor>(
                json!({"parser":"general","format":"anthropic_messages","cost":1})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<UsageParseOutput>(json!({"usage":null,"cost_amount":1}))
                .is_err()
        );
        assert!(
            parse_general_usage(
                UsageFormat::OpenAiResponses,
                &json!({
                    "input_tokens":1,"output_tokens":2,"irrelevant":"x".repeat(MAX_USAGE_BYTES)
                })
            )
            .is_none()
        );
    }
}
