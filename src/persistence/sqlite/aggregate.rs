//! Exact scale-eight accumulation with the existing SQLx PostgreSQL result-decoding boundary.

use num_bigint::BigInt;
use rust_decimal::{Decimal, MathematicalOps};

use crate::persistence::RepositoryError;

#[derive(Default)]
pub(super) struct CostSum {
    units: BigInt,
}

impl CostSum {
    pub(super) fn add(&mut self, mut amount: Decimal) {
        // Inputs come only from numeric(24,8) columns, so this rescale cannot lose precision.
        amount.rescale(8);
        self.units += BigInt::from(amount.mantissa());
    }

    pub(super) fn finish(&self) -> Result<Decimal, RepositoryError> {
        let digits = self.units.to_string();
        let (negative, digits) = digits
            .strip_prefix('-')
            .map_or((false, digits.as_str()), |digits| (true, digits));
        if digits == "0" {
            return Ok(Decimal::ZERO);
        }
        let padded = format!("{digits:0>width$}", width = digits.len().div_ceil(4) * 4);
        let groups = padded.len() / 4;
        let mut value = Decimal::ZERO;
        for (index, digit) in padded.as_bytes().chunks_exact(4).enumerate() {
            let digit = digit.iter().fold(0u16, |n, d| n * 10 + u16::from(d - b'0'));
            if digit == 0 {
                continue;
            }
            let weight = i64::try_from(groups - index).map_err(|_| overflow())? - 3;
            let power = Decimal::from(10_000)
                .checked_powi(weight)
                .ok_or_else(overflow)?;
            let part = Decimal::from(digit)
                .checked_mul(power)
                .ok_or_else(overflow)?;
            value = value.checked_add(part).ok_or_else(overflow)?;
        }
        // Mirror sqlx-postgres 0.8.6 PgNumeric -> Decimal, only after the exact SUM.
        value.set_sign_negative(negative);
        value.rescale(8);
        Ok(value)
    }
}

fn overflow() -> RepositoryError {
    sqlx::Error::Decode("aggregate is not representable as rust_decimal::Decimal".into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_sum_is_exact_before_result_conversion() {
        let mut sum = CostSum::default();
        let maximum = Decimal::from_str_exact("9999999999999999.99999999").unwrap();
        for _ in 0..80_000 {
            sum.add(maximum);
        }
        assert_eq!(
            sum.finish().unwrap(),
            Decimal::from_str_exact("799999999999999999999.9992").unwrap()
        );
    }
}
