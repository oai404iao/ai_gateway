//! Model-level advanced billing configuration and compiled matching rules.

use std::sync::Arc;

use chrono::{DateTime, Datelike, Timelike, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

const MAX_JSON_POINTER_BYTES: usize = 512;
const MAX_MATCH_VALUE_BYTES: usize = 4 * 1024;
const MAX_TIME_MULTIPLIERS: usize = 32;
const MAX_TIME_MULTIPLIER_LABEL_CHARS: usize = 80;
const DAYS_PER_WEEK: usize = 7;
const MINUTES_PER_DAY: usize = 24 * 60;
const MINUTES_PER_WEEK: usize = DAYS_PER_WEEK * MINUTES_PER_DAY;

/// Persisted model-level billing configuration. Long-context prices replace
/// the base input prices when the reported input-token count reaches a tier;
/// request multipliers are matched against the original client JSON body; and
/// recurring UTC windows multiply all selected unit prices.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdvancedBilling {
    #[serde(default)]
    pub long_context_tiers: Vec<LongContextTier>,
    #[serde(default)]
    pub request_multipliers: Vec<RequestBillingMultiplier>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_multipliers: Vec<TimeBillingMultiplier>,
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

/// A UTC weekday accepted by a recurring model price window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl BillingWeekday {
    pub const ALL: [Self; DAYS_PER_WEEK] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    fn index(self) -> usize {
        match self {
            Self::Monday => 0,
            Self::Tuesday => 1,
            Self::Wednesday => 2,
            Self::Thursday => 3,
            Self::Friday => 4,
            Self::Saturday => 5,
            Self::Sunday => 6,
        }
    }

    fn bit(self) -> u8 {
        1 << self.index()
    }
}

fn all_billing_weekdays() -> Vec<BillingWeekday> {
    BillingWeekday::ALL.to_vec()
}

fn is_all_billing_weekdays(weekdays: &[BillingWeekday]) -> bool {
    weekdays.len() == DAYS_PER_WEEK
        && BillingWeekday::ALL
            .iter()
            .all(|weekday| weekdays.contains(weekday))
}

/// One recurring weekly UTC price window. Weekdays identify the UTC day on
/// which the window starts. The start is inclusive and the end is exclusive;
/// a start later than the end wraps into the following UTC day.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBillingMultiplier {
    pub label: String,
    #[serde(
        default = "all_billing_weekdays",
        skip_serializing_if = "is_all_billing_weekdays"
    )]
    pub weekdays: Vec<BillingWeekday>,
    pub start_time: String,
    pub end_time: String,
    pub multiplier: Decimal,
}

#[derive(Clone, Copy, Debug)]
struct CompiledTimeBillingMultiplier {
    start_weekday_mask: u8,
    start_minute: u16,
    end_minute: u16,
    multiplier: Decimal,
}

impl CompiledTimeBillingMultiplier {
    fn starts_on(self, weekday: u8) -> bool {
        self.start_weekday_mask & (1 << weekday) != 0
    }

    fn matches(self, weekday: u8, minute: u16) -> bool {
        if self.start_minute < self.end_minute {
            self.starts_on(weekday) && (self.start_minute..self.end_minute).contains(&minute)
        } else {
            (minute >= self.start_minute && self.starts_on(weekday))
                || (minute < self.end_minute && self.starts_on((weekday + 6) % DAYS_PER_WEEK as u8))
        }
    }
}

/// Immutable, validated advanced-billing policy retained in a route snapshot.
#[derive(Clone, Debug)]
pub struct CompiledAdvancedBilling {
    long_context_tiers: Arc<[LongContextTier]>,
    request_multipliers: Arc<[RequestBillingMultiplier]>,
    time_multipliers: Arc<[CompiledTimeBillingMultiplier]>,
    maximum_request_multiplier: Decimal,
    maximum_time_multiplier: Decimal,
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

        if value.time_multipliers.len() > MAX_TIME_MULTIPLIERS {
            return Err(AdvancedBillingError);
        }
        let mut covered_minutes = [false; MINUTES_PER_WEEK];
        let mut compiled_time_multipliers = Vec::with_capacity(value.time_multipliers.len());
        for (index, rule) in value.time_multipliers.iter().enumerate() {
            if rule.label.is_empty()
                || rule.label.trim() != rule.label
                || rule.label.chars().count() > MAX_TIME_MULTIPLIER_LABEL_CHARS
                || rule.multiplier.is_sign_negative()
                || value.time_multipliers[..index]
                    .iter()
                    .any(|previous| previous.label == rule.label)
            {
                return Err(AdvancedBillingError);
            }
            let start_minute = parse_utc_time(&rule.start_time).ok_or(AdvancedBillingError)?;
            let end_minute = parse_utc_time(&rule.end_time).ok_or(AdvancedBillingError)?;
            if start_minute == end_minute {
                return Err(AdvancedBillingError);
            }
            let mut start_weekday_mask = 0_u8;
            for weekday in &rule.weekdays {
                let bit = weekday.bit();
                if start_weekday_mask & bit != 0 {
                    return Err(AdvancedBillingError);
                }
                start_weekday_mask |= bit;
            }
            if start_weekday_mask == 0 {
                return Err(AdvancedBillingError);
            }
            let compiled = CompiledTimeBillingMultiplier {
                start_weekday_mask,
                start_minute,
                end_minute,
                multiplier: rule.multiplier,
            };
            let duration_minutes = if start_minute < end_minute {
                end_minute - start_minute
            } else {
                MINUTES_PER_DAY as u16 - start_minute + end_minute
            };
            for weekday in &rule.weekdays {
                let week_start = weekday.index() * MINUTES_PER_DAY + usize::from(start_minute);
                for offset in 0..usize::from(duration_minutes) {
                    let covered = &mut covered_minutes[(week_start + offset) % MINUTES_PER_WEEK];
                    if *covered {
                        return Err(AdvancedBillingError);
                    }
                    *covered = true;
                }
            }
            compiled_time_multipliers.push(compiled);
        }

        let maximum_request_multiplier = value
            .request_multipliers
            .iter()
            .filter(|rule| rule.multiplier > Decimal::ONE)
            .try_fold(Decimal::ONE, |total, rule| {
                total.checked_mul(rule.multiplier)
            })
            .ok_or(AdvancedBillingError)?;
        let maximum_time_multiplier = value
            .time_multipliers
            .iter()
            .map(|rule| rule.multiplier)
            .filter(|multiplier| *multiplier > Decimal::ONE)
            .max()
            .unwrap_or(Decimal::ONE);

        Ok(Self {
            long_context_tiers: Arc::from(value.long_context_tiers),
            request_multipliers: Arc::from(value.request_multipliers),
            time_multipliers: Arc::from(compiled_time_multipliers),
            maximum_request_multiplier,
            maximum_time_multiplier,
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

    /// Selects the one matching weekly UTC window for the logical request
    /// start time. Valid policies cannot contain overlapping windows.
    #[must_use]
    pub fn time_multiplier(&self, started_at: DateTime<Utc>) -> Decimal {
        let weekday = started_at.weekday().num_days_from_monday() as u8;
        let minute = (started_at.hour() * 60 + started_at.minute()) as u16;
        self.time_multipliers
            .iter()
            .find(|rule| rule.matches(weekday, minute))
            .map_or(Decimal::ONE, |rule| rule.multiplier)
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

    #[must_use]
    pub fn maximum_time_multiplier(&self) -> Decimal {
        self.maximum_time_multiplier
    }
}

impl Default for CompiledAdvancedBilling {
    fn default() -> Self {
        Self {
            long_context_tiers: Arc::from([]),
            request_multipliers: Arc::from([]),
            time_multipliers: Arc::from([]),
            maximum_request_multiplier: Decimal::ONE,
            maximum_time_multiplier: Decimal::ONE,
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

fn parse_utc_time(value: &str) -> Option<u16> {
    let bytes = value.as_bytes();
    if bytes.len() != 5
        || bytes[2] != b':'
        || !bytes[0..2].iter().all(u8::is_ascii_digit)
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let hour = u16::from(bytes[0] - b'0') * 10 + u16::from(bytes[1] - b'0');
    let minute = u16::from(bytes[3] - b'0') * 10 + u16::from(bytes[4] - b'0');
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use rust_decimal::Decimal;
    use serde_json::json;

    use super::{
        AdvancedBilling, BillingWeekday, CompiledAdvancedBilling, LongContextTier,
        RequestBillingMultiplier, TimeBillingMultiplier,
    };

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().unwrap()
    }

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
            time_multipliers: vec![],
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
                time_multipliers: vec![],
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
                time_multipliers: vec![],
            })
            .is_err()
        );
    }

    #[test]
    fn selects_weekly_utc_windows_with_half_open_and_overnight_boundaries() {
        let billing = CompiledAdvancedBilling::compile(AdvancedBilling {
            time_multipliers: vec![
                TimeBillingMultiplier {
                    label: "morning peak".into(),
                    weekdays: BillingWeekday::ALL.to_vec(),
                    start_time: "01:00".into(),
                    end_time: "04:00".into(),
                    multiplier: Decimal::from(2),
                },
                TimeBillingMultiplier {
                    label: "overnight valley".into(),
                    weekdays: BillingWeekday::ALL.to_vec(),
                    start_time: "22:00".into(),
                    end_time: "00:30".into(),
                    multiplier: Decimal::new(5, 1),
                },
            ],
            ..AdvancedBilling::default()
        })
        .unwrap();

        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T00:29:59Z")),
            Decimal::new(5, 1)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T00:30:00Z")),
            Decimal::ONE
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T01:00:00Z")),
            Decimal::from(2)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T03:59:59Z")),
            Decimal::from(2)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T04:00:00Z")),
            Decimal::ONE
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-17T22:00:00Z")),
            Decimal::new(5, 1)
        );
        assert_eq!(billing.maximum_time_multiplier(), Decimal::from(2));
    }

    #[test]
    fn limits_windows_to_their_selected_utc_start_weekdays() {
        let billing = CompiledAdvancedBilling::compile(AdvancedBilling {
            time_multipliers: vec![TimeBillingMultiplier {
                label: "weekday overnight peak".into(),
                weekdays: vec![
                    BillingWeekday::Monday,
                    BillingWeekday::Tuesday,
                    BillingWeekday::Wednesday,
                    BillingWeekday::Thursday,
                    BillingWeekday::Friday,
                ],
                start_time: "22:00".into(),
                end_time: "02:00".into(),
                multiplier: Decimal::from(2),
            }],
            ..AdvancedBilling::default()
        })
        .unwrap();

        assert_eq!(
            billing.time_multiplier(utc("2026-08-21T23:59:59Z")),
            Decimal::from(2)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-22T01:59:59Z")),
            Decimal::from(2)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-22T02:00:00Z")),
            Decimal::ONE
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-22T23:00:00Z")),
            Decimal::ONE
        );
    }

    #[test]
    fn rejects_invalid_or_overlapping_weekly_utc_windows() {
        let invalid_times = [
            ("1:00", "04:00"),
            ("24:00", "04:00"),
            ("01:60", "04:00"),
            ("04:00", "04:00"),
        ];
        for (start_time, end_time) in invalid_times {
            assert!(
                CompiledAdvancedBilling::compile(AdvancedBilling {
                    time_multipliers: vec![TimeBillingMultiplier {
                        label: "invalid".into(),
                        weekdays: BillingWeekday::ALL.to_vec(),
                        start_time: start_time.into(),
                        end_time: end_time.into(),
                        multiplier: Decimal::ONE,
                    }],
                    ..AdvancedBilling::default()
                })
                .is_err()
            );
        }

        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                time_multipliers: vec![
                    TimeBillingMultiplier {
                        label: "overnight".into(),
                        weekdays: vec![BillingWeekday::Monday],
                        start_time: "22:00".into(),
                        end_time: "02:00".into(),
                        multiplier: Decimal::ONE,
                    },
                    TimeBillingMultiplier {
                        label: "overlap".into(),
                        weekdays: vec![BillingWeekday::Tuesday],
                        start_time: "01:00".into(),
                        end_time: "03:00".into(),
                        multiplier: Decimal::ONE,
                    },
                ],
                ..AdvancedBilling::default()
            })
            .is_err()
        );

        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                time_multipliers: vec![TimeBillingMultiplier {
                    label: "no weekdays".into(),
                    weekdays: vec![],
                    start_time: "01:00".into(),
                    end_time: "02:00".into(),
                    multiplier: Decimal::ONE,
                }],
                ..AdvancedBilling::default()
            })
            .is_err()
        );
        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                time_multipliers: vec![TimeBillingMultiplier {
                    label: "duplicate weekday".into(),
                    weekdays: vec![BillingWeekday::Monday, BillingWeekday::Monday],
                    start_time: "01:00".into(),
                    end_time: "02:00".into(),
                    multiplier: Decimal::ONE,
                }],
                ..AdvancedBilling::default()
            })
            .is_err()
        );
    }

    #[test]
    fn allows_equal_clock_windows_on_disjoint_weekdays() {
        CompiledAdvancedBilling::compile(AdvancedBilling {
            time_multipliers: vec![
                TimeBillingMultiplier {
                    label: "monday".into(),
                    weekdays: vec![BillingWeekday::Monday],
                    start_time: "01:00".into(),
                    end_time: "04:00".into(),
                    multiplier: Decimal::from(2),
                },
                TimeBillingMultiplier {
                    label: "tuesday".into(),
                    weekdays: vec![BillingWeekday::Tuesday],
                    start_time: "01:00".into(),
                    end_time: "04:00".into(),
                    multiplier: Decimal::from(3),
                },
            ],
            ..AdvancedBilling::default()
        })
        .unwrap();
    }

    #[test]
    fn wraps_sunday_windows_into_monday_and_rejects_actual_overlap() {
        let billing = CompiledAdvancedBilling::compile(AdvancedBilling {
            time_multipliers: vec![TimeBillingMultiplier {
                label: "sunday overnight".into(),
                weekdays: vec![BillingWeekday::Sunday],
                start_time: "22:00".into(),
                end_time: "02:00".into(),
                multiplier: Decimal::from(2),
            }],
            ..AdvancedBilling::default()
        })
        .unwrap();
        assert_eq!(
            billing.time_multiplier(utc("2026-08-24T01:59:59Z")),
            Decimal::from(2)
        );
        assert_eq!(
            billing.time_multiplier(utc("2026-08-24T02:00:00Z")),
            Decimal::ONE
        );

        assert!(
            CompiledAdvancedBilling::compile(AdvancedBilling {
                time_multipliers: vec![
                    TimeBillingMultiplier {
                        label: "sunday overnight".into(),
                        weekdays: vec![BillingWeekday::Sunday],
                        start_time: "22:00".into(),
                        end_time: "02:00".into(),
                        multiplier: Decimal::ONE,
                    },
                    TimeBillingMultiplier {
                        label: "monday overlap".into(),
                        weekdays: vec![BillingWeekday::Monday],
                        start_time: "01:00".into(),
                        end_time: "03:00".into(),
                        multiplier: Decimal::ONE,
                    },
                ],
                ..AdvancedBilling::default()
            })
            .is_err()
        );
    }

    #[test]
    fn legacy_advanced_billing_documents_default_to_all_weekdays() {
        let billing: AdvancedBilling = serde_json::from_value(json!({
            "long_context_tiers": [],
            "request_multipliers": [],
            "time_multipliers": [{
                "label": "legacy daily peak",
                "start_time": "01:00",
                "end_time": "04:00",
                "multiplier": "2"
            }]
        }))
        .unwrap();

        assert_eq!(
            billing.time_multipliers[0].weekdays,
            BillingWeekday::ALL.to_vec()
        );
        assert_eq!(
            serde_json::to_value(billing).unwrap(),
            json!({
                "long_context_tiers": [],
                "request_multipliers": [],
                "time_multipliers": [{
                    "label": "legacy daily peak",
                    "start_time": "01:00",
                    "end_time": "04:00",
                    "multiplier": "2"
                }]
            })
        );
    }
}
