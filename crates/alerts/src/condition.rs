//! [`Condition`] — the third element of an alert's `(series key, indicator
//! spec, condition)` triple.
//!
//! A condition names which scalar an indicator reports
//! ([`IndicatorField`], re-exported from `senken-indicators`, which owns
//! the dynamic build-and-read contract this crate's [`crate::ConcreteIndicator`]
//! reuses — needed because [`senken_indicators::Indicator`] itself has no
//! `value() -> f64`, since three of the ten built-ins report more than one
//! number per bar), how to compare it ([`Comparator`]), and the threshold
//! to compare against.
//!
//! The threshold is `f64`, on the indicator side of the boundary:
//! an indicator's own output is a display/decision value, fractional by
//! nature, never money — so a threshold compared directly against one is
//! too. It is **not** a price in the instrument's own decimal notation
//! unless the field being compared is itself a raw close price passed
//! through unconverted; callers must express it in whatever unit the
//! chosen field already reports in (an RSI threshold is plain 0–100, a
//! price-crossing threshold must be given in the bar's own scaled-integer
//! representation, since `senken-indicators` never divides one out — see
//! that crate's `convert::scaled_to_f64`).

use serde::{Deserialize, Serialize};

pub use senken_indicators::IndicatorField;

/// How a [`Condition`] compares an indicator field's current value (and,
/// for the crossing variants, its immediately preceding one) against a
/// threshold.
///
/// The crossing variants need history precisely because "crossing" is a
/// statement about two consecutive closed bars, not one — a value that has
/// always been above a threshold never *crosses* it. [`Condition::check`]
/// is the only place that history is threaded through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Comparator {
    /// Fires while the value is strictly greater than the threshold.
    GreaterThan,
    /// Fires while the value is strictly less than the threshold.
    LessThan,
    /// Fires on the bar where the value moves from at-or-below to strictly
    /// above the threshold.
    CrossesAbove,
    /// Fires on the bar where the value moves from at-or-above to strictly
    /// below the threshold.
    CrossesBelow,
}

/// `(field, comparator, threshold)` — the whole condition half of an
/// alert's `(series key, indicator spec, condition)` triple.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// Which of the indicator's own numbers this condition reads.
    pub field: IndicatorField,
    /// How the field's value is compared against `threshold`.
    pub comparator: Comparator,
    /// The value compared against, in whatever unit `field` already
    /// reports in (see this module's docs).
    pub threshold: f64,
}

impl Condition {
    /// Whether this condition fires given the field's value on the bar that
    /// just closed and, for the crossing comparators, its value on the
    /// previous closed bar (`None` if this is the field's first-ever
    /// reading, in which case a crossing can never be declared — there is
    /// nothing to have crossed from).
    #[must_use]
    pub fn check(&self, previous: Option<f64>, current: f64) -> bool {
        match self.comparator {
            Comparator::GreaterThan => current > self.threshold,
            Comparator::LessThan => current < self.threshold,
            Comparator::CrossesAbove => {
                previous.is_some_and(|p| p <= self.threshold) && current > self.threshold
            }
            Comparator::CrossesBelow => {
                previous.is_some_and(|p| p >= self.threshold) && current < self.threshold
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Comparator, Condition, IndicatorField};

    fn condition(comparator: Comparator, threshold: f64) -> Condition {
        Condition {
            field: IndicatorField::Value,
            comparator,
            threshold,
        }
    }

    #[test]
    fn greater_than_fires_whenever_the_current_value_exceeds_the_threshold() {
        let c = condition(Comparator::GreaterThan, 70.0);
        assert!(c.check(None, 71.0));
        assert!(c.check(Some(80.0), 75.0), "no history needed, still above");
        assert!(!c.check(Some(60.0), 70.0), "equal is not greater");
    }

    #[test]
    fn crosses_above_fires_only_on_the_bar_that_moves_from_at_or_below_to_above() {
        let c = condition(Comparator::CrossesAbove, 68_800.0);
        assert!(
            !c.check(None, 69_000.0),
            "no previous reading — nothing to have crossed from"
        );
        assert!(
            !c.check(Some(69_100.0), 69_500.0),
            "already above on the previous bar — not a crossing"
        );
        assert!(
            c.check(Some(68_700.0), 68_900.0),
            "moved from at-or-below to above — a genuine crossing"
        );
        assert!(
            !c.check(Some(68_900.0), 68_850.0),
            "moving back down is not crossing above"
        );
    }

    #[test]
    fn crosses_below_fires_only_on_the_bar_that_moves_from_at_or_above_to_below() {
        let c = condition(Comparator::CrossesBelow, 176.40);
        assert!(!c.check(None, 175.0));
        assert!(c.check(Some(176.50), 176.10));
        assert!(
            !c.check(Some(176.10), 175.90),
            "already below — not a crossing"
        );
    }
}
