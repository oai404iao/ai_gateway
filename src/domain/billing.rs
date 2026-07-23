//! Model-level advanced billing configuration and compiled matching rules.

use std::sync::Arc;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const MAX_JSON_POINTER_BYTES: usize = 512;
const MAX_MATCH_VALUE_BYTES: usize = 4 * 1024;

/// Persisted model-level billing configuration. Long-context prices replace
/// the base input prices when the reported input-token count reaches a tier;
/// request multipliers are matched against the original client JSON body.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdvancedBilling {
    #[serde(default)]
    pub long_context_tiers: Vec<LongContextTier>,
    #[serde(default)]
    pub request_multipliers: Vec<RequestBillingMultiplier>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LongContextTier {
    pub input_tokens_threshold: i64,
    pub input_unit_price: Decimal,
    pub cached_input_unit_price: Decimal,
    pub cache_write_unit_price: Decimal,
    /// Omission preserves the model's base output price for compatibility
    /// with policies created before output-tier pricing was supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_unit_price: Option<Decimal>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBillingMultiplier {
    pub json_pointer: String,
    pub value: Value,
    pub multiplier: Decimal,
}

/// Immutable, validated advanced-billing policy retained in a route snapshot.
#[derive(Clone, Debug)]
pub struct CompiledAdvancedBilling {
    long_context_tiers: Arc<[LongContextTier]>,
    request_multipliers: Arc<[RequestBillingMultiplier]>,
    maximum_request_multiplier: Decimal,
}

impl CompiledAdvancedBilling {
    /// Compiles one model's persisted policy. The caller must retain the
    /// result in an immutable runtime snapshot.
    pub fn compile(value: AdvancedBilling) -> Result<Self, AdvancedBillingError> {
        let mut previous_threshold = 0_i64;
        for tier in &value.long_context_tiers {
            if tier.input_tokens_threshold <= 0
                || tier.input_tokens_threshold <= previous_threshold
                || [
                    tier.input_unit_price,
                    tier.cached_input_unit_price,
                    tier.cache_write_unit_price,
                ]
                .into_iter()
                .any(|price| price.is_sign_negative())
                || tier
                    .output_unit_price
                    .is_some_and(|price| price.is_sign_negative())
            {
                return Err(AdvancedBillingError);
            }
            previous_threshold = tier.input_tokens_threshold;
        }

        for (index, rule) in value.request_multipliers.iter().enumerate() {
            if !valid_json_pointer(&rule.json_pointer)
                || rule.json_pointer.len() > MAX_JSON_POINTER_BYTES
                || rule.multiplier.is_sign_negative()
                || serde_json::to_vec(&rule.value)
                    .map_or(true, |encoded| encoded.len() > MAX_MATCH_VALUE_BYTES)
            {
                return Err(AdvancedBillingError);
            }
            if value.request_multipliers[..index].iter().any(|previous| {
                previous.json_pointer == rule.json_pointer && previous.value == rule.value
            }) {
                return Err(AdvancedBillingError);
            }
        }

        let maximum_request_multiplier = value
            .request_multipliers
            .iter()
            .filter(|rule| rule.multiplier > Decimal::ONE)
            .try_fold(Decimal::ONE, |total, rule| {
                total.checked_mul(rule.multiplier)
            })
            .ok_or(AdvancedBillingError)?;

        Ok(Self {
            long_context_tiers: Arc::from(value.long_context_tiers),
            request_multipliers: Arc::from(value.request_multipliers),
            maximum_request_multiplier,
        })
    }

    #[must_use]
    pub fn prices(
        &self,
        input_tokens: i64,
        base_input_unit_price: Decimal,
        base_cached_input_unit_price: Decimal,
        base_cache_write_unit_price: Decimal,
        base_output_unit_price: Decimal,
    ) -> (Decimal, Decimal, Decimal, Decimal) {
        self.long_context_tiers
            .iter()
            .rev()
            .find(|tier| input_tokens >= tier.input_tokens_threshold)
            .map_or(
                (
                    base_input_unit_price,
                    base_cached_input_unit_price,
                    base_cache_write_unit_price,
                    base_output_unit_price,
                ),
                |tier| {
                    (
                        tier.input_unit_price,
                        tier.cached_input_unit_price,
                        tier.cache_write_unit_price,
                        tier.output_unit_price.unwrap_or(base_output_unit_price),
                    )
                },
            )
    }

    /// Multiplies all exact JSON Pointer matches. The request body is the
    /// unmodified client payload, not the later transformed upstream body.
    #[must_use]
    pub fn request_multiplier(&self, request: &Value) -> Decimal {
        self.request_multipliers
            .iter()
            .filter(|rule| request.pointer(&rule.json_pointer) == Some(&rule.value))
            .fold(Decimal::ONE, |total, rule| {
                total
                    .checked_mul(rule.multiplier)
                    .expect("compiled request billing multiplier product fits")
            })
    }

    #[must_use]
    pub fn has_request_multipliers(&self) -> bool {
        !self.request_multipliers.is_empty()
    }

    #[must_use]
    pub fn price_candidates(
        &self,
        base_input_unit_price: Decimal,
        base_cached_input_unit_price: Decimal,
        base_cache_write_unit_price: Decimal,
        output_unit_price: Decimal,
    ) -> Vec<Decimal> {
        let mut candidates = vec![
            base_input_unit_price,
            base_cached_input_unit_price,
            base_cache_write_unit_price,
            output_unit_price,
        ];
        for tier in &*self.long_context_tiers {
            candidates.extend([
                tier.input_unit_price,
                tier.cached_input_unit_price,
                tier.cache_write_unit_price,
            ]);
            if let Some(output_unit_price) = tier.output_unit_price {
                candidates.push(output_unit_price);
            }
        }
        candidates
    }

    #[must_use]
    pub fn maximum_request_multiplier(&self) -> Decimal {
        self.maximum_request_multiplier
    }
}

impl Default for CompiledAdvancedBilling {
    fn default() -> Self {
        Self {
            long_context_tiers: Arc::from([]),
            request_multipliers: Arc::from([]),
            maximum_request_multiplier: Decimal::ONE,
        }
    }
}

/// A deliberately value-free validation error for persisted billing policy.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("invalid advanced billing configuration")]
pub struct AdvancedBillingError;

fn valid_json_pointer(pointer: &str) -> bool {
    if pointer.is_empty() {
        return true;
    }
    if !pointer.starts_with('/') {
        return false;
    }
    let bytes = pointer.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'~' {
            let Some(next) = bytes.get(index + 1) else {
                return false;
            };
            if *next != b'0' && *next != b'1' {
                return false;
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use serde_json::json;

    use super::{
        AdvancedBilling, CompiledAdvancedBilling, LongContextTier, RequestBillingMultiplier,
    };

    #[test]
    fn selects_the_highest_matching_context_tier_and_combines_request_rules() {
        let billing = CompiledAdvancedBilling::compile(AdvancedBilling {
            long_context_tiers: vec![
                LongContextTier {
                    input_tokens_threshold: 10,
                    input_unit_price: Decimal::new(2, 0),
                    cached_input_unit_price: Decimal::new(3, 0),
                    cache_write_unit_price: Decimal::new(4, 0),
                    output_unit_price: Some(Decimal::new(8, 0)),
                },
                LongContextTier {
                    input_tokens_threshold: 100,
                    input_unit_price: Decimal::new(5, 0),
                    cached_input_unit_price: Decimal::new(6, 0),
                    cache_write_unit_price: Decimal::new(7, 0),
                    output_unit_price: Some(Decimal::new(9, 0)),
                },
            ],
            request_multipliers: vec![
                RequestBillingMultiplier {
                    json_pointer: "/reasoning/effort".into(),
                    value: json!("high"),
                    multiplier: Decimal::new(2, 0),
                },
                RequestBillingMultiplier {
                    json_pointer: "/background".into(),
                    value: json!(true),
                    multiplier: Decimal::new(15, 1),
                },
            ],
        })
        .unwrap();

        assert_eq!(
            billing.prices(100, Decimal::ONE, Decimal::ONE, Decimal::ONE, Decimal::ONE,),
            (
                Decimal::new(5, 0),
                Decimal::new(6, 0),
                Decimal::new(7, 0),
                Decimal::new(9, 0),
            )
        );
        assert_eq!(
            billing.request_multiplier(&json!({
                "reasoning": {"effort": "high"},
                "background": true,
            })),
            Decimal::from(3_i64)
        );
    }

    #[test]
    fn rejects_unsorted_tiers_and_invalid_pointer_escapes() {
        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                long_context_tiers: vec![
                    LongContextTier {
                        input_tokens_threshold: 100,
                        input_unit_price: Decimal::ONE,
                        cached_input_unit_price: Decimal::ONE,
                        cache_write_unit_price: Decimal::ONE,
                        output_unit_price: None,
                    },
                    LongContextTier {
                        input_tokens_threshold: 10,
                        input_unit_price: Decimal::ONE,
                        cached_input_unit_price: Decimal::ONE,
                        cache_write_unit_price: Decimal::ONE,
                        output_unit_price: None,
                    },
                ],
                request_multipliers: vec![],
            })
            .is_err()
        );
        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                long_context_tiers: vec![],
                request_multipliers: vec![RequestBillingMultiplier {
                    json_pointer: "/bad~2pointer".into(),
                    value: json!(true),
                    multiplier: Decimal::ONE,
                }],
            })
            .is_err()
        );
    }
}
