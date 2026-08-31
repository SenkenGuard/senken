//! [`AlertEvaluator`] — feeding closed bars through an indicator and
//! deciding whether the alert's [`Condition`] just fired.

use senken_series::Bar;

use crate::condition::Condition;
use crate::error::IndicatorSpecError;
use crate::indicator_spec::ConcreteIndicator;

/// The result of [`AlertEvaluator::on_closed_bar`] firing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fired {
    /// The field's value on the bar that triggered the condition.
    pub value: f64,
    /// The closed bar's own open time — when the condition became true.
    pub bar_ts_open: senken_core::UnixNanos,
}

/// Ties one alert's indicator and [`Condition`] together, and is the only
/// thing in this crate that decides whether an alert has just fired.
///
/// Two rules, both non-negotiable's scope note for this
/// milestone:
///
/// - **Never evaluates a forming bar.** [`on_closed_bar`](Self::on_closed_bar)
///   takes a [`Bar`] that its caller (ordinarily a [`crate::TickBarBuilder`])
///   has already established is closed — this type has no notion of "maybe
///   forming" at all, by construction, rather than a flag that could be
///   passed incorrectly.
/// - **Never fires during warm-up.** The indicator's own
///   [`initialized`](senken_indicators::Indicator::initialized) is checked
///   before the condition is even read — an indicator's first few values
///   are a warm-up artefact, not a value this evaluator will
///   ever compare against a threshold.
#[derive(Debug)]
pub struct AlertEvaluator {
    indicator: ConcreteIndicator,
    condition: Condition,
    /// The field's value on the previous closed bar this evaluator saw
    /// *after* the indicator became initialized — `None` before that,
    /// which is also what makes [`crate::condition::Comparator::CrossesAbove`]/
    /// `CrossesBelow` correctly refuse to fire on an indicator's very first
    /// initialized reading (there is nothing yet to have crossed from).
    last_value: Option<f64>,
}

impl AlertEvaluator {
    /// Builds an evaluator from an already-constructed indicator (see
    /// [`crate::IndicatorSpec::build`]) and the condition to check against
    /// it.
    #[must_use]
    pub fn new(indicator: ConcreteIndicator, condition: Condition) -> Self {
        Self {
            indicator,
            condition,
            last_value: None,
        }
    }

    /// Feeds one **closed** bar into the wrapped indicator and reports
    /// whether the condition just fired.
    ///
    /// Always advances the indicator's own state via `handle_bar` (backfill
    /// and live evaluation are the same call — there is no
    /// second way to feed this evaluator bars), but only reads the
    /// condition once [`initialized`](senken_indicators::Indicator::initialized)
    /// is `true`.
    ///
    /// # Errors
    /// [`IndicatorSpecError::FieldNotReported`] if this evaluator's
    /// condition names a field the wrapped indicator does not report — a
    /// configuration mistake caught here rather than silently comparing the
    /// wrong number forever.
    pub fn on_closed_bar(&mut self, bar: &Bar) -> Result<Option<Fired>, IndicatorSpecError> {
        self.indicator.handle_bar(bar);
        if !self.indicator.initialized() {
            // Warm-up: `has_inputs()` may already be true, but
            // is explicit that this is weaker than `initialized()` and must
            // never be mistaken for it.
            return Ok(None);
        }

        let value = self.indicator.read(self.condition.field)?;
        let fired = self.condition.check(self.last_value, value);
        self.last_value = Some(value);

        Ok(fired.then_some(Fired {
            value,
            bar_ts_open: bar.ts_open,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::AlertEvaluator;
    use crate::condition::{Comparator, Condition, IndicatorField};
    use crate::indicator_spec::ConcreteIndicator;
    use senken_core::UnixNanos;
    use senken_series::Bar;

    fn bar(minute: i64, close: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(minute * 60).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 0,
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }

    fn sma3_above(threshold: f64) -> AlertEvaluator {
        let indicator = ConcreteIndicator::build("Sma", r#"{"period":3}"#).unwrap();
        let condition = Condition {
            field: IndicatorField::Value,
            comparator: Comparator::GreaterThan,
            threshold,
        };
        AlertEvaluator::new(indicator, condition)
    }

    #[test]
    fn does_not_fire_during_indicator_warm_up_even_if_the_raw_input_would_satisfy_the_condition() {
        // An Sma(3) is not initialized until its third bar. Every one of
        // these three closes is already above the threshold on its own
        // terms — proving this evaluator gates on `initialized()`, not on
        // "does the raw price already look right".
        let mut eval = sma3_above(50.0);
        assert_eq!(
            eval.on_closed_bar(&bar(0, 100)).unwrap(),
            None,
            "1st bar: warm-up"
        );
        assert_eq!(
            eval.on_closed_bar(&bar(1, 100)).unwrap(),
            None,
            "2nd bar: warm-up"
        );
        // The 3rd bar completes the SMA(3) window (average = 100), which is
        // also the very first initialized reading — `CrossesAbove` would
        // correctly refuse this (nothing to cross from), but plain
        // `GreaterThan` fires on it, proving warm-up gating and condition
        // evaluation are two separate, correctly-ordered steps.
        let fired = eval.on_closed_bar(&bar(2, 100)).unwrap();
        assert!(fired.is_some(), "3rd bar: now initialized, and 100 > 50");
    }

    #[test]
    fn fires_exactly_once_on_the_bar_where_a_crossing_condition_becomes_true() {
        let indicator = ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap();
        let condition = Condition {
            field: IndicatorField::Value,
            comparator: Comparator::CrossesAbove,
            threshold: 100.0,
        };
        let mut eval = AlertEvaluator::new(indicator, condition);

        assert_eq!(
            eval.on_closed_bar(&bar(0, 90)).unwrap(),
            None,
            "first initialized reading — nothing to cross from yet"
        );
        assert_eq!(
            eval.on_closed_bar(&bar(1, 95)).unwrap(),
            None,
            "still below"
        );
        let fired = eval
            .on_closed_bar(&bar(2, 105))
            .unwrap()
            .expect("moved from below to above — must fire");
        assert_eq!(fired.value.to_bits(), 105.0_f64.to_bits());
        assert_eq!(fired.bar_ts_open, UnixNanos::from_secs(120).unwrap());
        assert_eq!(
            eval.on_closed_bar(&bar(3, 110)).unwrap(),
            None,
            "still above — a crossing condition must not fire again while it stays true"
        );
    }

    #[test]
    fn asking_for_a_field_the_indicator_does_not_report_is_an_error_not_a_silent_wrong_value() {
        let indicator = ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap();
        let condition = Condition {
            field: IndicatorField::MacdLine,
            comparator: Comparator::GreaterThan,
            threshold: 0.0,
        };
        let mut eval = AlertEvaluator::new(indicator, condition);
        assert!(eval.on_closed_bar(&bar(0, 1)).is_err());
    }
}
