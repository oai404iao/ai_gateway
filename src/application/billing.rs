//! Shared immutable request-billing calculations.

use rust_decimal::Decimal;
use serde_json::Value;

use crate::domain::{
    CompiledAdvancedBilling, ModelPriceSnapshot, RequestBilling, RequestPriceSnapshot, RequestUsage,
};

use super::usage::ResponseUsage;

/// Resolves model-level request billing rules against the validated request
/// body after client policy filters have run.
pub(crate) fn request_billing_multiplier(
    advanced_billing: &CompiledAdvancedBilling,
    body: &[u8],
) -> Decimal {
    if !advanced_billing.has_request_multipliers() {
        return Decimal::ONE;
    }
    let request =
        serde_json::from_slice::<Value>(body).expect("caller supplies a validated JSON body");
    request_billing_multiplier_for_value(advanced_billing, &request)
}

pub(crate) fn request_billing_multiplier_for_value(
    advanced_billing: &CompiledAdvancedBilling,
    request: &Value,
) -> Decimal {
    if !advanced_billing.has_request_multipliers() {
        return Decimal::ONE;
    }
    advanced_billing.request_multiplier(request)
}

/// Captures immutable price facts, parsed usage, and the derived request cost.
pub(crate) fn request_billing(
    snapshot: &ModelPriceSnapshot,
    advanced_billing: &CompiledAdvancedBilling,
    billing_multiplier: Decimal,
    request_billing_multiplier: Decimal,
    usage: Option<ResponseUsage>,
    total_duration_ms: i32,
    ttft_ms: Option<i32>,
) -> RequestBilling {
    let usage = usage.map(|usage| RequestUsage {
        input_tokens: usage.input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_tokens: usage.cache_write_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_tokens,
    });
    let (input_unit_price, cached_input_unit_price, cache_write_unit_price, output_unit_price) =
        usage.as_ref().map_or(
            (
                snapshot.input_unit_price(),
                snapshot.cached_input_unit_price(),
                snapshot.cache_write_unit_price(),
                snapshot.output_unit_price(),
            ),
            |usage| {
                advanced_billing.prices(
                    usage.input_tokens,
                    snapshot.input_unit_price(),
                    snapshot.cached_input_unit_price(),
                    snapshot.cache_write_unit_price(),
                    snapshot.output_unit_price(),
                )
            },
        );
    let billing_multiplier = billing_multiplier
        .checked_mul(request_billing_multiplier)
        .expect("compiled request billing multiplier fits");
    let price = RequestPriceSnapshot {
        currency: snapshot.currency().to_owned(),
        price_unit_tokens: snapshot.price_unit_tokens(),
        price_effective_at: snapshot.price_effective_at(),
        input_unit_price: effective_unit_price(input_unit_price, billing_multiplier),
        cached_input_unit_price: effective_unit_price(cached_input_unit_price, billing_multiplier),
        cache_write_unit_price: effective_unit_price(cache_write_unit_price, billing_multiplier),
        output_unit_price: effective_unit_price(output_unit_price, billing_multiplier),
    };
    let cost_amount = usage.as_ref().map(|usage| calculate_cost(usage, &price));
    let output_tokens_per_second = usage.and_then(|usage| {
        let ttft_ms = ttft_ms?;
        (usage.output_tokens > 0).then(|| {
            let generation_ms = total_duration_ms.saturating_sub(ttft_ms).max(1);
            (Decimal::from(usage.output_tokens) * Decimal::from(1_000_i64)
                / Decimal::from(generation_ms))
            .round_dp(4)
        })
    });
    RequestBilling {
        usage,
        price,
        cost_amount,
        output_tokens_per_second,
    }
}

fn effective_unit_price(price: Decimal, billing_multiplier: Decimal) -> Decimal {
    price
        .checked_mul(billing_multiplier)
        .expect("compiled channel billing price multiplication fits")
        .round_dp(12)
}

pub(crate) fn calculate_cost(usage: &RequestUsage, price: &RequestPriceSnapshot) -> Decimal {
    let unit = Decimal::from(price.price_unit_tokens);
    let non_cached_input = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
    ((Decimal::from(non_cached_input) * price.input_unit_price
        + Decimal::from(usage.cached_input_tokens) * price.cached_input_unit_price
        + Decimal::from(usage.cache_write_tokens) * price.cache_write_unit_price
        + Decimal::from(usage.output_tokens) * price.output_unit_price)
        / unit)
        .round_dp(8)
}
