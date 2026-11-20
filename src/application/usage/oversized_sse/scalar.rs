//! `serde_json::Value` constraints beyond the streaming parser's JSON grammar.

use serde_json::Number;

#[derive(Default)]
pub(super) struct StringEscapes {
    phase: EscapePhase,
    needs_low: bool,
}

#[derive(Default)]
enum EscapePhase {
    #[default]
    Text,
    Escape,
    Hex {
        unit: u16,
        remaining: u8,
    },
}

impl StringEscapes {
    pub(super) fn push(&mut self, mut bytes: &[u8]) -> bool {
        while let Some((&byte, rest)) = bytes.split_first() {
            match &mut self.phase {
                EscapePhase::Text => {
                    if self.needs_low && byte != b'\\' {
                        return false;
                    }
                    let Some(slash) = bytes.iter().position(|&b| b == b'\\') else {
                        return true;
                    };
                    bytes = &bytes[slash + 1..];
                    self.phase = EscapePhase::Escape;
                    continue;
                }
                EscapePhase::Escape => {
                    if byte == b'u' {
                        self.phase = EscapePhase::Hex {
                            unit: 0,
                            remaining: 4,
                        };
                    } else if self.needs_low {
                        return false;
                    } else {
                        self.phase = EscapePhase::Text;
                    }
                }
                EscapePhase::Hex { unit, remaining } => {
                    let Some(digit) = char::from(byte).to_digit(16) else {
                        return false;
                    };
                    *unit = (*unit << 4) | digit as u16;
                    *remaining -= 1;
                    if *remaining == 0 {
                        if self.needs_low != (0xdc00..=0xdfff).contains(unit) {
                            return false;
                        }
                        self.needs_low = (0xd800..=0xdbff).contains(unit);
                        self.phase = EscapePhase::Text;
                    }
                }
            }
            bytes = rest;
        }
        true
    }

    pub(super) fn complete(&self) -> bool {
        matches!(self.phase, EscapePhase::Text) && !self.needs_low
    }
}

#[derive(Default)]
pub(super) struct JsonNumber {
    literal: Vec<u8>,
    long: Option<Decimal>,
}

impl JsonNumber {
    pub(super) fn push(&mut self, bytes: &[u8]) {
        if let Some(long) = &mut self.long {
            long.push(bytes);
        } else if self.literal.len().saturating_add(bytes.len()) <= 64 {
            self.literal.extend_from_slice(bytes);
        } else {
            let mut long = Decimal::default();
            long.push(&self.literal);
            long.push(bytes);
            self.literal.clear();
            self.long = Some(long);
        }
    }

    pub(super) fn finish(&mut self) -> Option<Number> {
        if let Some(long) = self.long.take() {
            return long.finish();
        }
        let number = serde_json::from_slice(&self.literal).ok();
        self.literal.clear();
        number
    }
}

// The default serde_json number parser keeps a u64 significand, counts discarded
// integer digits in the decimal exponent, and stops extending an overflowing fraction.
// Rebuild that bounded representation so serde_json itself decides the f64 range.
#[derive(Default)]
struct Decimal {
    significand: u64,
    power: i64,
    explicit: u32,
    negative: bool,
    negative_exponent: bool,
    truncated: bool,
    phase: DecimalPhase,
}

#[derive(Default, PartialEq)]
enum DecimalPhase {
    #[default]
    Integer,
    Fraction,
    Exponent,
}

impl Decimal {
    fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match byte {
                b'-' if self.phase == DecimalPhase::Exponent => self.negative_exponent = true,
                b'-' => self.negative = true,
                b'+' => (),
                b'.' => {
                    self.phase = DecimalPhase::Fraction;
                    self.truncated = false;
                }
                b'e' | b'E' => self.phase = DecimalPhase::Exponent,
                _ => {
                    let digit = byte - b'0'; // Token grammar is checked before fragment delivery.
                    if self.phase == DecimalPhase::Exponent {
                        self.explicit = self
                            .explicit
                            .saturating_mul(10)
                            .saturating_add(u32::from(digit));
                    } else if !self.truncated
                        && let Some(n) = self
                            .significand
                            .checked_mul(10)
                            .and_then(|n| n.checked_add(u64::from(digit)))
                    {
                        self.significand = n;
                        if self.phase == DecimalPhase::Fraction {
                            self.power = self.power.saturating_sub(1);
                        }
                    } else {
                        self.truncated = true;
                        if self.phase == DecimalPhase::Integer {
                            self.power = self.power.saturating_add(1);
                        }
                    }
                }
            }
        }
    }

    fn finish(self) -> Option<Number> {
        let power = if self.explicit > i32::MAX as u32 {
            if !self.negative_exponent && self.significand != 0 {
                return None;
            }
            -400
        } else {
            let exp = i64::from(self.explicit);
            self.power
                .saturating_add(if self.negative_exponent { -exp } else { exp })
                .clamp(-400, 400)
        };
        let sign = if self.negative { "-" } else { "" };
        serde_json::from_str(&format!("{sign}{}e{power}", self.significand)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_long_numbers_match_serde_json_with_bounded_storage() {
        let zeroes = "0".repeat(20_000);
        let nines = "9".repeat(20_000);
        let mut literals = vec![
            format!("123{zeroes}e-20000"),
            format!("0.{zeroes}123e20001"),
            format!("-{nines}.55e-19999"),
            format!("{nines}e999999999999999999999"),
            format!("1e{zeroes}400"),
            format!("0e{nines}"),
            format!("-1e-{nines}"),
            format!("18446744073709551616{zeroes}.12345e-19999"),
        ];
        for n in [
            "1.7976931348623155",
            "1.7976931348623157",
            "1.7976931348623158",
            "1.7976931348623160",
            "1.8446744073709551616",
        ] {
            for e in [-309, 289, 307, 308, 309] {
                literals.push(format!("{n}{zeroes}e{e}"));
            }
        }
        for literal in literals {
            let expected = serde_json::from_str::<Number>(&literal).ok();
            for chunk in [1, 19, 64, 127, 8192] {
                let mut number = JsonNumber::default();
                for bytes in literal.as_bytes().chunks(chunk) {
                    number.push(bytes);
                    assert!(number.literal.len() <= 64);
                }
                assert_eq!(
                    number.finish(),
                    expected,
                    "mantissa prefix: {:.19}, chunk: {chunk}",
                    literal
                );
            }
        }
    }

    #[test]
    fn surrogate_validation_crosses_every_fragment_boundary() {
        for suffix in [
            r"\uD800",
            r"\uDF00",
            r"\uDBff\udc00",
            r"\\uD800",
            r"\uD800a",
            r"\uD800\\uDC00",
            r"\uD800\uDFFF\ud800",
        ] {
            let text = format!("{}{suffix}", "x".repeat(65_537));
            let valid = serde_json::from_str::<String>(&format!("\"{text}\"")).is_ok();
            for chunk in [1, 2, 3, 4, 5, 6, 7, 12, 8192] {
                let mut s = StringEscapes::default();
                assert_eq!(
                    text.as_bytes().chunks(chunk).all(|b| s.push(b)) && s.complete(),
                    valid,
                    "{suffix}, chunk: {chunk}"
                );
            }
        }
    }
}
