//! Bounded client and parser for the externally maintained models.dev catalog.

use std::{collections::HashSet, time::Duration};

use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::{Url, redirect::Policy};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{
    domain::{AdvancedBilling, LongContextTier},
    runtime_config::ModelsSyncConfig,
};

#[derive(Clone)]
pub struct ModelsDevClient {
    client: reqwest::Client,
    url: Url,
    max_response_bytes: usize,
    max_model_metadata_bytes: usize,
}

impl ModelsDevClient {
    pub fn new(config: &ModelsSyncConfig) -> Result<Self, ModelsDevError> {
        let url = Url::parse(&config.api_url).map_err(|_| ModelsDevError::InvalidConfiguration)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .redirect(Policy::none())
            .build()
            .map_err(|_| ModelsDevError::InvalidConfiguration)?;
        Ok(Self {
            client,
            url,
            max_response_bytes: config.max_response_bytes,
            max_model_metadata_bytes: config.max_model_metadata_bytes,
        })
    }

    pub async fn fetch_catalog(
        &self,
        provider_ids: &[String],
    ) -> Result<ModelsDevCatalog, ModelsDevError> {
        let selected_providers = provider_ids.iter().collect::<HashSet<_>>();
        let response = self
            .client
            .get(self.url.clone())
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|_| ModelsDevError::Unavailable)?;
        if !response.status().is_success() {
            return Err(ModelsDevError::Unavailable);
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| ModelsDevError::Unavailable)?;
            if body
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > self.max_response_bytes)
            {
                return Err(ModelsDevError::ResponseTooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        let document: Value =
            serde_json::from_slice(&body).map_err(|_| ModelsDevError::InvalidCatalog)?;
        parse_catalog(document, &selected_providers, self.max_model_metadata_bytes)
    }
}

#[derive(Clone, Serialize)]
pub struct ModelsDevCatalog {
    pub fetched_at: DateTime<Utc>,
    pub models: Vec<ModelsDevModel>,
    pub excluded_missing_prices: usize,
    pub excluded_invalid_models: usize,
    pub excluded_oversized_metadata: usize,
}

impl ModelsDevCatalog {
    pub fn find(&self, provider_id: &str, model_id: &str) -> Option<&ModelsDevModel> {
        self.models
            .iter()
            .find(|model| model.provider_id == provider_id && model.model_id == model_id)
    }

    pub fn select(
        &self,
        selections: &[ModelsDevSelection],
    ) -> Result<Vec<ModelsDevModel>, ModelsDevError> {
        let mut requested = HashSet::new();
        let mut selected = Vec::with_capacity(selections.len());
        for selection in selections {
            let key = (&selection.provider_id, &selection.model_id);
            if !requested.insert(key) {
                return Err(ModelsDevError::InvalidSelection);
            }
            let model = self
                .find(&selection.provider_id, &selection.model_id)
                .ok_or(ModelsDevError::InvalidSelection)?;
            selected.push(model.clone());
        }
        Ok(selected)
    }
}

#[derive(Clone, Serialize)]
pub struct ModelsDevModel {
    pub provider_id: String,
    pub provider_name: String,
    pub model_id: String,
    pub display_name: String,
    pub input_unit_price: Decimal,
    pub cached_input_unit_price: Decimal,
    pub cache_write_unit_price: Decimal,
    pub output_unit_price: Decimal,
    pub advanced_billing: AdvancedBilling,
    #[serde(skip_serializing)]
    pub source_payload: Value,
}

#[derive(Clone)]
pub struct ModelsDevSelection {
    pub provider_id: String,
    pub model_id: String,
}

fn parse_catalog(
    document: Value,
    selected_providers: &HashSet<&String>,
    max_model_metadata_bytes: usize,
) -> Result<ModelsDevCatalog, ModelsDevError> {
    let providers = document.as_object().ok_or(ModelsDevError::InvalidCatalog)?;
    let mut models = Vec::new();
    let mut identities = HashSet::new();
    let mut excluded_missing_prices = 0;
    let mut excluded_invalid_models = 0;
    let mut excluded_oversized_metadata = 0;

    for (provider_key, provider) in providers {
        let Some(provider_object) = provider.as_object() else {
            continue;
        };
        let provider_id = provider_object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(provider_key)
            .to_owned();
        if !selected_providers.is_empty() && !selected_providers.contains(&provider_id) {
            continue;
        }
        let provider_name = provider_object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&provider_id)
            .to_owned();
        let Some(provider_models) = provider_object.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_key, model) in provider_models {
            let Some(model_object) = model.as_object() else {
                excluded_invalid_models += 1;
                continue;
            };
            let model_id = model_object
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(model_key)
                .to_owned();
            let display_name = model_object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&model_id)
                .to_owned();
            if provider_id.is_empty()
                || provider_id.len() > 200
                || provider_name.is_empty()
                || provider_name.len() > 200
                || model_id.is_empty()
                || model_id.len() > 300
                || display_name.is_empty()
                || display_name.len() > 300
                || !identities.insert((provider_id.clone(), model_id.clone()))
            {
                excluded_invalid_models += 1;
                continue;
            }
            let Some(cost) = model_object.get("cost").and_then(Value::as_object) else {
                excluded_missing_prices += 1;
                continue;
            };
            let (Some(input_unit_price), Some(output_unit_price)) =
                (decimal(cost.get("input")), decimal(cost.get("output")))
            else {
                excluded_missing_prices += 1;
                continue;
            };
            let cached_input_unit_price = decimal(cost.get("cache_read")).unwrap_or(Decimal::ZERO);
            let cache_write_unit_price = decimal(cost.get("cache_write")).unwrap_or(Decimal::ZERO);
            if [
                &input_unit_price,
                &cached_input_unit_price,
                &cache_write_unit_price,
                &output_unit_price,
            ]
            .into_iter()
            .any(Decimal::is_sign_negative)
            {
                excluded_invalid_models += 1;
                continue;
            }
            let advanced_billing = match parse_advanced_billing(cost) {
                Ok(value) => value,
                Err(()) => {
                    excluded_invalid_models += 1;
                    continue;
                }
            };
            let source_payload = json!({
                "source": "models.dev",
                "provider_id": provider_id,
                "provider_name": provider_name,
                "model": model,
            });
            if serde_json::to_vec(&source_payload)
                .is_ok_and(|encoded| encoded.len() <= max_model_metadata_bytes)
            {
                models.push(ModelsDevModel {
                    provider_id: provider_id.clone(),
                    provider_name: provider_name.clone(),
                    model_id,
                    display_name,
                    input_unit_price,
                    cached_input_unit_price,
                    cache_write_unit_price,
                    output_unit_price,
                    advanced_billing,
                    source_payload,
                });
            } else {
                excluded_oversized_metadata += 1;
            }
        }
    }
    models.sort_by(|left, right| {
        (&left.provider_id, &left.model_id).cmp(&(&right.provider_id, &right.model_id))
    });
    Ok(ModelsDevCatalog {
        fetched_at: Utc::now(),
        models,
        excluded_missing_prices,
        excluded_invalid_models,
        excluded_oversized_metadata,
    })
}

fn parse_advanced_billing(cost: &Map<String, Value>) -> Result<AdvancedBilling, ()> {
    let long_context_tiers = match cost.get("tiers") {
        Some(Value::Array(tiers)) if !tiers.is_empty() => {
            let mut parsed = tiers
                .iter()
                .map(parse_context_tier)
                .collect::<Result<Vec<_>, _>>()?;
            parsed.sort_unstable_by_key(|tier| tier.input_tokens_threshold);
            if parsed
                .windows(2)
                .any(|pair| pair[0].input_tokens_threshold == pair[1].input_tokens_threshold)
            {
                return Err(());
            }
            parsed
        }
        Some(Value::Array(_)) | None => match cost.get("context_over_200k") {
            Some(value) if cost.get("tiers").is_none() => {
                vec![parse_legacy_context_tier(value)?]
            }
            Some(_) | None => Vec::new(),
        },
        Some(_) => return Err(()),
    };
    Ok(AdvancedBilling {
        long_context_tiers,
        request_multipliers: Vec::new(),
    })
}

fn parse_context_tier(value: &Value) -> Result<LongContextTier, ()> {
    let object = value.as_object().ok_or(())?;
    let tier = object.get("tier").and_then(Value::as_object).ok_or(())?;
    if tier
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "context")
    {
        return Err(());
    }
    let threshold = tier
        .get("size")
        .and_then(Value::as_i64)
        .filter(|size| *size > 0)
        .ok_or(())?;
    parse_long_context_prices(object, threshold)
}

fn parse_legacy_context_tier(value: &Value) -> Result<LongContextTier, ()> {
    parse_long_context_prices(value.as_object().ok_or(())?, 200_000)
}

fn parse_long_context_prices(
    object: &Map<String, Value>,
    threshold: i64,
) -> Result<LongContextTier, ()> {
    let input_unit_price = decimal(object.get("input")).ok_or(())?;
    let output_unit_price = decimal(object.get("output")).ok_or(())?;
    let cached_input_unit_price = decimal(object.get("cache_read")).unwrap_or(Decimal::ZERO);
    let cache_write_unit_price = decimal(object.get("cache_write")).unwrap_or(Decimal::ZERO);
    if [
        input_unit_price,
        cached_input_unit_price,
        cache_write_unit_price,
        output_unit_price,
    ]
    .into_iter()
    .any(|price| price.is_sign_negative())
    {
        return Err(());
    }
    Ok(LongContextTier {
        input_tokens_threshold: threshold,
        input_unit_price,
        cached_input_unit_price,
        cache_write_unit_price,
        output_unit_price: Some(output_unit_price),
    })
}

fn decimal(value: Option<&Value>) -> Option<Decimal> {
    let value = value?;
    match value {
        Value::Number(number) => number.to_string().parse().ok(),
        Value::String(string) => string.parse().ok(),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum ModelsDevError {
    #[error("models.dev client configuration is invalid")]
    InvalidConfiguration,
    #[error("models.dev is unavailable")]
    Unavailable,
    #[error("models.dev response exceeds the configured limit")]
    ResponseTooLarge,
    #[error("models.dev returned an invalid catalog")]
    InvalidCatalog,
    #[error("requested models.dev selections are invalid or unavailable")]
    InvalidSelection,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::parse_catalog;

    #[test]
    fn parser_keeps_only_complete_nonnegative_priced_models() {
        let catalog = parse_catalog(
            json!({
                "provider": {
                    "id": "provider",
                    "name": "Provider",
                    "models": {
                        "complete": {
                            "id": "complete",
                            "name": "Complete",
                            "cost": {
                                "input": 1,
                                "output": 2,
                                "tiers": [
                                    {
                                        "input": 3,
                                        "output": 4,
                                        "cache_read": 0.3,
                                        "cache_write": 0.6,
                                        "tier": {"type": "context", "size": 32000}
                                    },
                                    {
                                        "input": 5,
                                        "output": 6,
                                        "cache_read": 0.5,
                                        "tier": {"type": "context", "size": 128000}
                                    }
                                ],
                                "context_over_200k": {
                                    "input": 99,
                                    "output": 99
                                }
                            }
                        },
                        "missing": {
                            "id": "missing",
                            "cost": {"input": 1}
                        },
                        "negative": {
                            "id": "negative",
                            "cost": {"input": -1, "output": 2}
                        }
                    }
                }
            }),
            &HashSet::new(),
            1_024,
        )
        .unwrap();

        assert_eq!(catalog.models.len(), 1);
        assert_eq!(catalog.models[0].model_id, "complete");
        assert_eq!(
            catalog.models[0]
                .advanced_billing
                .long_context_tiers
                .iter()
                .map(|tier| (
                    tier.input_tokens_threshold,
                    tier.input_unit_price,
                    tier.output_unit_price,
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    32_000,
                    rust_decimal::Decimal::from(3),
                    Some(rust_decimal::Decimal::from(4)),
                ),
                (
                    128_000,
                    rust_decimal::Decimal::from(5),
                    Some(rust_decimal::Decimal::from(6)),
                ),
            ]
        );
        assert_eq!(
            catalog.models[0].advanced_billing.long_context_tiers[1].cache_write_unit_price,
            rust_decimal::Decimal::ZERO
        );
        assert_eq!(catalog.excluded_missing_prices, 1);
        assert_eq!(catalog.excluded_invalid_models, 1);
    }

    #[test]
    fn parser_supports_legacy_context_over_200k_when_tiers_are_absent() {
        let catalog = parse_catalog(
            json!({
                "provider": {
                    "models": {
                        "legacy": {
                            "id": "legacy",
                            "cost": {
                                "input": 1,
                                "output": 2,
                                "context_over_200k": {
                                    "input": 3,
                                    "output": 4,
                                    "cache_read": 0.3
                                }
                            }
                        }
                    }
                }
            }),
            &HashSet::new(),
            4_096,
        )
        .unwrap();

        let tier = &catalog.models[0].advanced_billing.long_context_tiers[0];
        assert_eq!(tier.input_tokens_threshold, 200_000);
        assert_eq!(tier.input_unit_price, rust_decimal::Decimal::from(3));
        assert_eq!(tier.output_unit_price, Some(rust_decimal::Decimal::from(4)));
    }

    #[test]
    fn parser_rejects_duplicate_or_malformed_context_tiers() {
        let catalog = parse_catalog(
            json!({
                "provider": {
                    "models": {
                        "duplicate": {
                            "id": "duplicate",
                            "cost": {
                                "input": 1,
                                "output": 2,
                                "tiers": [
                                    {"input": 3, "output": 4, "tier": {"type": "context", "size": 32000}},
                                    {"input": 5, "output": 6, "tier": {"type": "context", "size": 32000}}
                                ]
                            }
                        },
                        "malformed": {
                            "id": "malformed",
                            "cost": {
                                "input": 1,
                                "output": 2,
                                "tiers": {}
                            }
                        }
                    }
                }
            }),
            &HashSet::new(),
            4_096,
        )
        .unwrap();

        assert!(catalog.models.is_empty());
        assert_eq!(catalog.excluded_invalid_models, 2);
    }

    #[test]
    fn parser_excludes_oversized_raw_model_metadata() {
        let catalog = parse_catalog(
            json!({
                "provider": {
                    "models": {
                        "complete": {
                            "id": "complete",
                            "cost": {"input": 1, "output": 2},
                            "description": "metadata larger than one byte"
                        }
                    }
                }
            }),
            &HashSet::new(),
            1,
        )
        .unwrap();

        assert!(catalog.models.is_empty());
        assert_eq!(catalog.excluded_oversized_metadata, 1);
    }
}
