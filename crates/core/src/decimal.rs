//! Decimal-string helpers for turning venue payloads (`"0.01000000"`) into
//! the fixed-point integers used throughout this project, without ever
//! routing through floating point.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// Number of significant fractional digits in a decimal string.
///
/// Trailing zeros do not count: `"0.01000000"` has two, `"0.10"` has one,
/// `"10"` has none. This is the scale a venue's tick or step implies, and the
/// value a source should store as `price_scale` / `qty_scale`.
///
/// Non-numeric input is not validated here; pair with [`parse_scaled`].
///
/// # Examples
/// ```
/// use senken_core::decimal::decimal_places;
///
/// assert_eq!(decimal_places("0.01000000"), 2);
/// assert_eq!(decimal_places("0.1"), 1);
/// assert_eq!(decimal_places("10"), 0);
/// ```
#[must_use]
pub fn decimal_places(s: &str) -> u8 {
    let Some((_, frac)) = s.trim().split_once('.') else {
        return 0;
    };
    let significant = frac.trim_end_matches('0').len();
    u8::try_from(significant).unwrap_or(u8::MAX)
}

/// Parses a decimal string into an integer at `scale` fractional digits.
///
/// `parse_scaled("1.5", 2) == Some(150)`. Accepts an optional leading `-` and
/// surrounding whitespace. Trailing zeros in the fraction never count against
/// `scale`, so `parse_scaled("0.01000000", 2) == Some(1)`.
///
/// Returns `None` for anything that is not a plain decimal (`1e-8`, `+1`,
/// `1,5`), for more significant fractional digits than `scale` allows, and
/// for values that do not fit in an `i64`.
///
/// # Examples
/// ```
/// use senken_core::decimal::parse_scaled;
///
/// assert_eq!(parse_scaled("1.5", 2), Some(150));
/// assert_eq!(parse_scaled("0.01000000", 2), Some(1));
/// assert_eq!(parse_scaled("0.001", 2), None); // finer than the scale
/// ```
#[must_use]
pub fn parse_scaled(s: &str, scale: u8) -> Option<i64> {
    let s = s.trim();
    let (negative, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };

    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    let frac_part = frac_part.trim_end_matches('0');

    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if frac_part.len() > usize::from(scale) {
        return None;
    }

    let mut value: i64 = 0;
    for digit in int_part.bytes().chain(frac_part.bytes()) {
        if !digit.is_ascii_digit() {
            return None;
        }
        value = value
            .checked_mul(10)?
            .checked_add(i64::from(digit - b'0'))?;
    }
    for _ in frac_part.len()..usize::from(scale) {
        value = value.checked_mul(10)?;
    }

    Some(if negative { -value } else { value })
}

/// Parses a venue increment such as `"0.01000000"` into the `(scale, size)`
/// pair of the fixed-point contract on `Instrument`: the minimal scale
/// the increment implies, and the increment expressed at that scale.
/// `"0.01"` becomes `(2, 1)`; `"0.05"` becomes `(2, 5)`.
///
/// Returns `None` for anything [`parse_scaled`] rejects and for
/// non-positive increments: a zero tick is meaningless.
///
/// # Examples
/// ```
/// use senken_core::decimal::parse_increment;
///
/// assert_eq!(parse_increment("0.01000000"), Some((2, 1)));
/// assert_eq!(parse_increment("0.05"), Some((2, 5)));
/// assert_eq!(parse_increment("0"), None);
/// ```
#[must_use]
pub fn parse_increment(raw: &str) -> Option<(u8, i64)> {
    let scale = decimal_places(raw);
    let size = parse_scaled(raw, scale).filter(|size| *size > 0)?;
    Some((scale, size))
}

/// The `(scale, size)` pair implied by a venue that reports a *precision*
///   — a count of decimal places — instead of an increment.
///
/// Many venues say "price precision 2" where others say "tick 0.01"; both
/// mean the same thing, and this maps the former onto the fixed-point
/// contract of `Instrument`. Precision is clamped to what an `i64` can
/// carry at that scale.
///
/// # Examples
/// ```
/// use senken_core::decimal::increment_from_precision;
///
/// assert_eq!(increment_from_precision(2), (2, 1)); // same as tick "0.01"
/// assert_eq!(increment_from_precision(0), (0, 1)); // whole units
/// ```
#[must_use]
pub fn increment_from_precision(digits: u32) -> (u8, i64) {
    const MAX_SCALE: u32 = 18;
    (u8::try_from(digits.min(MAX_SCALE)).unwrap_or(0), 1)
}

/// Rewrites a decimal in scientific notation (`1e-06`, `1.5E3`) as plain
/// decimal text, leaving anything already plain untouched.
///
/// Venues that serialise increments as JSON numbers can emit exponents —
/// `1e-06` for a step of `0.000001` — which [`parse_scaled`] rejects on
/// purpose. Normalising the text keeps the parser strict while still
/// accepting what those venues send, and never routes through `f64`.
///
/// Returns `None` when `s` is not a decimal at all.
///
/// # Examples
/// ```
/// use senken_core::decimal::plain_decimal;
///
/// assert_eq!(plain_decimal("1e-06").as_deref(), Some("0.000001"));
/// assert_eq!(plain_decimal("1.5e3").as_deref(), Some("1500"));
/// assert_eq!(plain_decimal("0.01").as_deref(), Some("0.01"));
/// assert_eq!(plain_decimal("abc"), None);
/// ```
#[must_use]
pub fn plain_decimal(s: &str) -> Option<Cow<'_, str>> {
    let s = s.trim();
    let Some(exponent_at) = s.find(['e', 'E']) else {
        return is_plain_decimal(s).then_some(Cow::Borrowed(s));
    };

    let (mantissa, exponent) = s.split_at(exponent_at);
    let exponent: i32 = exponent[1..].parse().ok()?;

    let (negative, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa.strip_prefix('+').unwrap_or(mantissa)),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part
        .bytes()
        .chain(frac_part.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }

    // Shift the point through the digit string rather than scaling a float.
    let digits: String = format!("{int_part}{frac_part}");
    let point = i32::try_from(int_part.len()).ok()? + exponent;
    let sign = if negative { "-" } else { "" };

    let shifted = if point <= 0 {
        let leading = usize::try_from(point.unsigned_abs()).ok()?;
        format!("{sign}0.{}{digits}", "0".repeat(leading))
    } else {
        let point = usize::try_from(point).ok()?;
        if point >= digits.len() {
            format!("{sign}{digits}{}", "0".repeat(point - digits.len()))
        } else {
            let (whole, fraction) = digits.split_at(point);
            format!("{sign}{whole}.{fraction}")
        }
    };
    Some(Cow::Owned(shifted))
}

/// `true` when `s` is a plain decimal: an optional `-`, then digits with at
/// most one decimal point, and at least one digit overall.
fn is_plain_decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int_part, frac_part) = s.split_once('.').unwrap_or((s, ""));
    if int_part.is_empty() && frac_part.is_empty() {
        return false;
    }
    !frac_part.contains('.')
        && int_part
            .bytes()
            .chain(frac_part.bytes())
            .all(|b| b.is_ascii_digit())
}

/// Renders a fixed-point integer as a decimal string.
///
/// The inverse of [`parse_scaled`] for the same `scale`.
///
/// # Examples
/// ```
/// use senken_core::decimal::format_scaled;
///
/// assert_eq!(format_scaled(150, 2), "1.50");
/// assert_eq!(format_scaled(1, 8), "0.00000001");
/// ```
#[must_use]
pub fn format_scaled(value: i64, scale: u8) -> String {
    let scale = usize::from(scale);
    let digits = value.unsigned_abs().to_string();
    let sign = if value < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int_part, frac_part) = padded.split_at(padded.len() - scale);
    format!("{sign}{int_part}.{frac_part}")
}

/// A fixed-point quantity that carries its own scale.
///
/// Most of this module treats scale as context the caller already knows —
/// `price_scale` lives on the instrument, `qty_scale` next to it. `Scaled`
/// is for the code that cannot assume that: comparing or rescaling a value
/// whose scale is not implied by where it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scaled {
    /// Decimal places `value` is expressed at.
    pub scale: u8,
    /// The fixed-point integer itself, at `scale`.
    pub value: i64,
}

impl Scaled {
    /// A scaled quantity from its parts.
    #[must_use]
    pub fn new(scale: u8, value: i64) -> Self {
        Self { scale, value }
    }

    /// Re-expresses this quantity at `to`, per [`checked_rescale`].
    #[must_use]
    pub fn rescale(self, to: u8) -> Option<Self> {
        checked_rescale(self.value, self.scale, to).map(|value| Self { scale: to, value })
    }
}

/// Converts `value`, expressed at scale `from`, into the equivalent integer
/// at scale `to`.
///
/// Widening the scale (`to > from`) multiplies by a power of ten and can
/// overflow `i64`; narrowing it (`to < from`) divides, and — like
/// [`parse_scaled`]'s refusal to accept more precision than a scale allows —
/// this function never truncates a value silently. If the digits being
/// dropped are non-zero, the conversion is not exact and this returns
/// `None` rather than rounding.
///
/// # Examples
/// ```
/// use senken_core::decimal::checked_rescale;
///
/// assert_eq!(checked_rescale(150, 2, 4), Some(15_000)); // 1.50 -> 1.5000
/// assert_eq!(checked_rescale(15_000, 4, 2), Some(150)); // 1.5000 -> 1.50, exact
/// assert_eq!(checked_rescale(15_001, 4, 2), None); // 1.5001 does not fit at scale 2
/// ```
#[must_use]
pub fn checked_rescale(value: i64, from: u8, to: u8) -> Option<i64> {
    if to >= from {
        let factor = 10_i64.checked_pow(u32::from(to - from))?;
        value.checked_mul(factor)
    } else {
        let factor = 10_i64.checked_pow(u32::from(from - to))?;
        (value % factor == 0).then_some(value / factor)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Scaled, checked_rescale, decimal_places, format_scaled, parse_increment, parse_scaled,
    };

    #[test]
    fn increments_map_to_minimal_scale_pairs() {
        assert_eq!(parse_increment("0.01000000"), Some((2, 1)));
        assert_eq!(parse_increment("0.00001000"), Some((5, 1)));
        assert_eq!(parse_increment("0.00000001"), Some((8, 1)));
        assert_eq!(parse_increment("1.00000000"), Some((0, 1)));
        assert_eq!(parse_increment("1"), Some((0, 1)));
        assert_eq!(parse_increment("0.1"), Some((1, 1)));
        assert_eq!(parse_increment("0.5"), Some((1, 5)));
        assert_eq!(parse_increment("0.05"), Some((2, 5)));
        assert_eq!(parse_increment("0"), None, "a zero tick is meaningless");
        assert_eq!(parse_increment("1e-8"), None);
        assert_eq!(parse_increment("abc"), None);
    }

    #[test]
    fn format_is_the_inverse_of_parse() {
        assert_eq!(format_scaled(150, 2), "1.50");
        assert_eq!(format_scaled(1, 8), "0.00000001");
        assert_eq!(format_scaled(-150, 2), "-1.50");
        assert_eq!(format_scaled(7, 0), "7");
        assert_eq!(format_scaled(i64::MIN, 2), "-92233720368547758.08");
        for (raw, scale) in [("0.01", 2), ("123.456", 3), ("-0.5", 1), ("42", 0)] {
            let parsed = parse_scaled(raw, scale).unwrap();
            assert_eq!(format_scaled(parsed, scale), raw);
        }
    }

    #[test]
    fn shifts_the_point_right_by_scale() {
        assert_eq!(parse_scaled("0.01000000", 8), Some(1_000_000));
        assert_eq!(parse_scaled("1", 8), Some(100_000_000));
        assert_eq!(parse_scaled("0.00000001", 8), Some(1));
        assert_eq!(parse_scaled("0.5", 8), Some(50_000_000));
        assert_eq!(parse_scaled("100.5", 2), Some(10_050));
        assert_eq!(parse_scaled("10", 0), Some(10));
    }

    #[test]
    fn trailing_zeros_do_not_count_against_scale() {
        assert_eq!(parse_scaled("0.01000000", 2), Some(1));
        assert_eq!(parse_scaled("0.10", 1), Some(1));
        assert_eq!(parse_scaled("1.0", 0), Some(1));
    }

    #[test]
    fn real_values_from_venues() {
        assert_eq!(parse_scaled("0.00001", 5), Some(1));
        assert_eq!(parse_scaled("0.1", 1), Some(1));
        assert_eq!(parse_scaled("0.000001", 6), Some(1));
        assert_eq!(parse_scaled("0.000000000001", 12), Some(1));
    }

    #[test]
    fn rejects_precision_that_does_not_fit() {
        assert_eq!(parse_scaled("0.001", 2), None);
        assert_eq!(parse_scaled("0.123456789", 8), None);
    }

    #[test]
    fn rejects_non_decimal_input() {
        assert_eq!(parse_scaled("abc", 8), None);
        assert_eq!(parse_scaled("", 8), None);
        assert_eq!(parse_scaled(".", 8), None);
        assert_eq!(parse_scaled("1e-8", 8), None);
        assert_eq!(parse_scaled("+1.0", 8), None);
        assert_eq!(parse_scaled("1,5", 8), None);
        assert_eq!(parse_scaled("--1", 8), None);
    }

    #[test]
    fn handles_sign_and_surrounding_space() {
        assert_eq!(parse_scaled("-1.5", 2), Some(-150));
        assert_eq!(parse_scaled("  2.25  ", 2), Some(225));
    }

    #[test]
    fn overflow_yields_none_instead_of_panic() {
        assert_eq!(parse_scaled("99999999999999999999", 0), None);
        assert_eq!(parse_scaled("1", 18), Some(1_000_000_000_000_000_000));
        assert_eq!(parse_scaled("10", 18), None);
    }

    #[test]
    fn never_routes_through_f64() {
        assert_eq!(parse_scaled("0.07", 8), Some(7_000_000));
        assert_eq!(parse_scaled("0.29", 8), Some(29_000_000));
    }

    #[test]
    fn decimal_places_ignores_trailing_zeros() {
        assert_eq!(decimal_places("0.00001"), 5);
        assert_eq!(decimal_places("0.01000000"), 2);
        assert_eq!(decimal_places("0.1"), 1);
        assert_eq!(decimal_places("10"), 0);
        assert_eq!(decimal_places("1.0"), 0);
        assert_eq!(decimal_places("0.000000000001"), 12);
    }

    #[test]
    fn decimal_places_and_parse_scaled_agree() {
        for raw in ["0.01000000", "0.00001000", "1.00000000", "0.1", "100"] {
            let scale = decimal_places(raw);
            assert!(parse_scaled(raw, scale).is_some(), "{raw} at scale {scale}");
        }
    }

    #[test]
    fn rescale_widens_by_multiplying() {
        assert_eq!(
            checked_rescale(150, 2, 2),
            Some(150),
            "same scale is a no-op"
        );
        assert_eq!(checked_rescale(150, 2, 4), Some(15_000));
        assert_eq!(checked_rescale(1, 0, 8), Some(100_000_000));
    }

    #[test]
    fn rescale_narrows_only_when_exact() {
        assert_eq!(checked_rescale(15_000, 4, 2), Some(150));
        assert_eq!(
            checked_rescale(15_001, 4, 2),
            None,
            "1.5001 is not representable at scale 2"
        );
        assert_eq!(checked_rescale(-15_000, 4, 2), Some(-150));
    }

    #[test]
    fn rescale_reports_overflow_instead_of_wrapping() {
        assert_eq!(checked_rescale(i64::MAX, 0, 1), None);
        assert_eq!(
            checked_rescale(1, 0, 255),
            None,
            "10^255 does not fit in an i64 exponent"
        );
    }

    #[test]
    fn scaled_rescale_matches_the_free_function() {
        let price = Scaled::new(2, 150);
        assert_eq!(price.rescale(4), Some(Scaled::new(4, 15_000)));
        assert_eq!(price.rescale(0), None, "1.50 is not a whole number");
    }
}
