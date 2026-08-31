//! A number as venues actually send it.

use std::fmt;

use senken_marketdata::decimal::{increment_from_precision, parse_increment, plain_decimal};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};

/// A numeric field that a venue may send as a JSON string (`"0.01"`), a
/// JSON number (`0.01`), or a JSON number in scientific notation (`1e-06`).
///
/// Exchange APIs are wildly inconsistent here — the same concept arrives as
/// a quoted decimal on one venue, a bare float on the next, and a count of
/// decimal places on a third. `Num` absorbs the first two and normalises
/// them to plain decimal text; [`increment_from_precision`] handles the
/// third.
///
/// Floats are rendered with Rust's shortest round-tripping `Display`, which
/// reproduces every tick and step size venues publish exactly. Arithmetic
/// still happens only in fixed point, through
/// [`increment`](Self::increment).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Num(String);

impl Num {
    /// The value as plain decimal text, never in scientific notation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` when the venue sent nothing at all (an empty string).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The `(scale, size)` pair this value implies as a price tick or
    /// quantity step, per the fixed-point contract on
    /// [`Instrument`](senken_marketdata::Instrument).
    ///
    /// `None` when the value is not a usable increment — empty, unparseable,
    /// or non-positive.
    #[must_use]
    pub fn increment(&self) -> Option<(u8, i64)> {
        parse_increment(&self.0)
    }

    /// The value read as a count of decimal places, for venues that report
    /// precision instead of an increment.
    ///
    /// `None` when the value is not a non-negative integer.
    #[must_use]
    pub fn precision(&self) -> Option<(u8, i64)> {
        let digits: u32 = self.0.parse().ok()?;
        Some(increment_from_precision(digits))
    }

    /// The value as an integer, for counts and epoch timestamps. A decimal
    /// point truncates rather than failing, since venues sometimes send
    /// `"1.0"` where they mean `1`.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        let text = self.0.trim();
        let text = text.split_once('.').map_or(text, |(whole, _)| whole);
        text.parse().ok()
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl From<&str> for Num {
    fn from(value: &str) -> Self {
        Self(plain_decimal(value).map(Into::into).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NumVisitor;

        impl<'de> Visitor<'de> for NumVisitor {
            type Value = Num;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a number, as a JSON number or a decimal string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Num, E> {
                Ok(Num::from(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Num, E> {
                Ok(Num(value.to_string()))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Num, E> {
                Ok(Num(value.to_string()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Num, E> {
                if value.is_finite() {
                    // `Display` for f64 is the shortest round-tripping form
                    // and never uses an exponent, so this stays plain text.
                    Ok(Num(value.to_string()))
                } else {
                    Err(E::custom("number is not finite"))
                }
            }

            fn visit_none<E: de::Error>(self) -> Result<Num, E> {
                Ok(Num::default())
            }

            fn visit_unit<E: de::Error>(self) -> Result<Num, E> {
                Ok(Num::default())
            }

            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Num, D::Error> {
                d.deserialize_any(self)
            }
        }

        deserializer.deserialize_any(NumVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::Num;

    fn parse(json: &str) -> Num {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_number_arrives_as_a_string_or_a_number() {
        assert_eq!(parse(r#""0.01""#).as_str(), "0.01");
        assert_eq!(parse("0.01").as_str(), "0.01");
        assert_eq!(parse("100").as_str(), "100");
        assert_eq!(parse(r#""100""#).as_str(), "100");
    }

    #[test]
    fn scientific_notation_is_normalised() {
        // BingX sends a spot step size as 1e-06.
        assert_eq!(parse("1e-06").as_str(), "0.000001");
        assert_eq!(parse(r#""1e-06""#).as_str(), "0.000001");
        // Deribit sends option strikes as 6.9e4.
        assert_eq!(parse("6.9e4").as_str(), "69000");
    }

    #[test]
    fn increments_follow_the_fixed_point_contract() {
        assert_eq!(parse(r#""0.01""#).increment(), Some((2, 1)));
        assert_eq!(parse("0.0001").increment(), Some((4, 1)));
        assert_eq!(parse("1e-06").increment(), Some((6, 1)));
        assert_eq!(parse(r#""0.5""#).increment(), Some((1, 5)));
        assert_eq!(parse("0").increment(), None, "a zero tick is meaningless");
        assert_eq!(parse(r#""""#).increment(), None);
    }

    #[test]
    fn precision_counts_become_increments() {
        assert_eq!(parse("2").precision(), Some((2, 1)));
        assert_eq!(parse(r#""6""#).precision(), Some((6, 1)));
        assert_eq!(parse(r#""0.5""#).precision(), None);
    }

    #[test]
    fn integers_survive_a_decimal_point() {
        assert_eq!(
            parse(r#""1788508800000""#).as_i64(),
            Some(1_788_508_800_000)
        );
        assert_eq!(parse("100").as_i64(), Some(100));
        assert_eq!(parse("1.0").as_i64(), Some(1));
        assert_eq!(parse(r#""""#).as_i64(), None);
    }

    #[test]
    fn absent_values_are_empty_not_an_error() {
        assert!(parse("null").is_empty());
        assert!(parse(r#""""#).is_empty());
        assert!(parse("null").increment().is_none());
    }
}
