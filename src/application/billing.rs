//! Shared immutable request-billing calculations.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::domain::{
    CompiledAdvancedBilling, ModelPriceSnapshot, RequestBilling, RequestLogOutcome,
    RequestPriceSnapshot, RequestUsage,
};

use super::usage::ResponseUsage;

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestBillingFactors {
    started_at: DateTime<Utc>,
    channel_multiplier: Decimal,
    request_multiplier: Decimal,
}

impl RequestBillingFactors {
    pub(crate) fn new(
        started_at: DateTime<Utc>,
        channel_multiplier: Decimal,
        request_multiplier: Decimal,
    ) -> Self {
        Self {
            started_at,
            channel_multiplier,
            request_multiplier,
        }
    }
}

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
    factors: RequestBillingFactors,
    usage: Option<ResponseUsage>,
    total_duration_ms: i32,
    ttft_ms: Option<i32>,
    outcome: RequestLogOutcome,
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
    let time_multiplier = advanced_billing.time_multiplier(factors.started_at);
    let billing_multiplier = factors
        .channel_multiplier
        .checked_mul(factors.request_multiplier)
        .and_then(|multiplier| multiplier.checked_mul(time_multiplier))
        .expect("compiled billing multiplier product fits");
    let price = RequestPriceSnapshot {
        currency: snapshot.currency().to_owned(),
        price_unit_tokens: snapshot.price_unit_tokens(),
        price_effective_at: snapshot.price_effective_at(),
        input_unit_price: effective_unit_price(input_unit_price, billing_multiplier),
        cached_input_unit_price: effective_unit_price(cached_input_unit_price, billing_multiplier),
        cache_write_unit_price: effective_unit_price(cache_write_unit_price, billing_multiplier),
        output_unit_price: effective_unit_price(output_unit_price, billing_multiplier),
    };
    let cost_amount = if outcome.forces_zero_cost() {
        Some(Decimal::ZERO)
    } else {
        usage.as_ref().map(|usage| calculate_cost(usage, &price))
    };
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
        peak_pricing: time_multiplier > Decimal::ONE,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn price() -> RequestPriceSnapshot {
        RequestPriceSnapshot {
            currency: "USD".into(),
            price_unit_tokens: 1,
            price_effective_at: "2026-09-16T00:00:00Z".parse().unwrap(),
            input_unit_price: Decimal::ZERO,
            cached_input_unit_price: Decimal::ZERO,
            cache_write_unit_price: Decimal::ZERO,
            output_unit_price: Decimal::ZERO,
        }
    }

    #[test]
    fn effective_prices_round_midpoints_to_even_at_twelve_places() {
        for (coefficient, expected) in [(5, 2), (15, 8), (25, 12), (35, 18)] {
            assert_eq!(
                effective_unit_price(Decimal::new(coefficient, 12), Decimal::new(5, 1)),
                Decimal::new(expected, 12),
            );
        }
    }

    #[test]
    fn cost_rounds_midpoints_to_even_only_after_summing_and_dividing() {
        let usage = RequestUsage {
            input_tokens: 1,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 1,
            reasoning_tokens: 1,
        };
        for (coefficient, expected) in [(5, 0), (15, 2), (25, 2), (35, 4)] {
            let mut price = price();
            price.price_unit_tokens = 1_000_000;
            price.input_unit_price = Decimal::new(coefficient, 3);
            assert_eq!(calculate_cost(&usage, &price), Decimal::new(expected, 8));
        }
        let mut price = price();
        price.input_unit_price = Decimal::new(5, 9);
        price.output_unit_price = Decimal::new(5, 9);
        assert_eq!(calculate_cost(&usage, &price), Decimal::new(1, 8));
    }

    #[test]
    fn cost_uses_the_rounded_effective_price_not_the_unrounded_product() {
        let price = price();
        let snapshot = ModelPriceSnapshot::new(
            price.currency.into(),
            1,
            price.price_effective_at,
            Decimal::new(7, 12),
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::ZERO,
        );
        let billing = request_billing(
            &snapshot,
            &CompiledAdvancedBilling::default(),
            RequestBillingFactors::new(price.price_effective_at, Decimal::new(5, 1), Decimal::ONE),
            Some(ResponseUsage {
                input_tokens: 15_000,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
            }),
            1,
            None,
            RequestLogOutcome::Succeeded,
        );
        assert_eq!(billing.price.input_unit_price, Decimal::new(4, 12));
        assert_eq!(billing.cost_amount, Some(Decimal::new(6, 8)));
    }

    #[test]
    fn missing_usage_is_unknown_except_for_explicit_failure_or_cancellation() {
        let price = price();
        let snapshot = ModelPriceSnapshot::new(
            price.currency.into(),
            1,
            price.price_effective_at,
            Decimal::ONE,
            Decimal::ONE,
            Decimal::ONE,
            Decimal::ONE,
        );
        for (outcome, expected) in [
            (RequestLogOutcome::Succeeded, None),
            (RequestLogOutcome::Rejected, None),
            (RequestLogOutcome::Failed, Some(Decimal::ZERO)),
            (RequestLogOutcome::Cancelled, Some(Decimal::ZERO)),
        ] {
            let billing = request_billing(
                &snapshot,
                &CompiledAdvancedBilling::default(),
                RequestBillingFactors::new(price.price_effective_at, Decimal::ONE, Decimal::ONE),
                None,
                1,
                None,
                outcome,
            );
            assert_eq!(billing.usage, None);
            assert_eq!(billing.cost_amount, expected, "{outcome:?}");
        }
    }
}
