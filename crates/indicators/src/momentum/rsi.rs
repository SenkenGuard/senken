//! [`Rsi`] — Wilder's relative strength index.

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// Wilder's relative strength index over `period` bar-to-bar changes.
///
/// The first `period` changes are seeded as a plain average (matching
/// Wilder's own worked example); every change after that is folded in with
/// Wilder's smoothing: `avg = (avg * (period - 1) + latest) / period`.
///
/// Needs `period + 1` bars to initialize, one more than most of this
/// crate's other indicators: the first bar establishes a starting close but
/// produces no *change* to average, since a change needs two closes.
#[derive(Debug, Clone)]
pub struct Rsi {
    period: usize,
    prev_close: Option<f64>,
    avg_gain: f64,
    avg_loss: f64,
    deltas_seen: usize,
    has_inputs: bool,
    initialized: bool,
}

impl Rsi {
    /// Creates a new [`Rsi`] over `period` changes.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "Rsi::new requires period > 0");
        Self {
            period,
            prev_close: None,
            avg_gain: 0.0,
            avg_loss: 0.0,
            deltas_seen: 0,
            has_inputs: false,
            initialized: false,
        }
    }

    /// The current RSI, on a `0..=100` scale. Meaningless before
    /// [`initialized`](Indicator::initialized).
    ///
    /// `50.0` is returned for the degenerate case of no losses and no gains
    /// at all (an unmoving series); `100.0` for gains with literally zero
    /// losses to divide by.
    #[must_use]
    pub fn value(&self) -> f64 {
        if self.avg_loss <= 0.0 {
            if self.avg_gain <= 0.0 { 50.0 } else { 100.0 }
        } else {
            let rs = self.avg_gain / self.avg_loss;
            100.0 - 100.0 / (1.0 + rs)
        }
    }
}

impl Indicator for Rsi {
    fn name(&self) -> String {
        "Rsi".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    fn initialized(&self) -> bool {
        self.initialized
    }

    fn handle_bar(&mut self, bar: &Bar) {
        let price = scaled_to_f64(bar.close);
        self.has_inputs = true;
        if let Some(prev) = self.prev_close {
            let change = price - prev;
            let gain = change.max(0.0);
            let loss = (-change).max(0.0);
            if self.deltas_seen < self.period {
                self.avg_gain += gain;
                self.avg_loss += loss;
                self.deltas_seen += 1;
                if self.deltas_seen == self.period {
                    let period = self.period as f64;
                    self.avg_gain /= period;
                    self.avg_loss /= period;
                    self.initialized = true;
                }
            } else {
                let period = self.period as f64;
                let period_minus_one = period - 1.0;
                self.avg_gain = self.avg_gain.mul_add(period_minus_one, gain) / period;
                self.avg_loss = self.avg_loss.mul_add(period_minus_one, loss) / period;
            }
        }
        self.prev_close = Some(price);
    }

    fn reset(&mut self) {
        self.prev_close = None;
        self.avg_gain = 0.0;
        self.avg_loss = 0.0;
        self.deltas_seen = 0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Rsi;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `period = 3` over closes `10, 12, 11, 13`:
    /// changes are `+2, -1, +2`, so the seeded average gain is
    /// `(2 + 0 + 2) / 3 = 4/3` and the seeded average loss is
    /// `(0 + 1 + 0) / 3 = 1/3`. `RS = (4/3) / (1/3) = 4`, so
    /// `RSI = 100 - 100 / (1 + 4) = 80`.
    ///
    /// One more close, `12` (a change of `-1`): Wilder smoothing gives
    /// `avg_gain = (4/3 * 2 + 0) / 3 = 8/9`,
    /// `avg_loss = (1/3 * 2 + 1) / 3 = 5/9`, `RS = 8/5 = 1.6`, so
    /// `RSI = 100 - 100 / 2.6 = 61.538461538461...`.
    #[test]
    fn rsi_matches_a_hand_computed_value() {
        let mut rsi = Rsi::new(3);
        rsi.handle_bar(&bar(0, 0, 0, 10, 0));
        assert!(!rsi.initialized(), "no change observed yet");
        rsi.handle_bar(&bar(0, 0, 0, 12, 0));
        assert!(!rsi.initialized(), "one of three changes seen");
        rsi.handle_bar(&bar(0, 0, 0, 11, 0));
        assert!(!rsi.initialized(), "two of three changes seen");
        rsi.handle_bar(&bar(0, 0, 0, 13, 0));
        assert!(rsi.initialized(), "all three seed changes seen");
        assert_approx_eq(rsi.value(), 80.0);

        rsi.handle_bar(&bar(0, 0, 0, 12, 0));
        assert_approx_eq(rsi.value(), 100.0 - 100.0 / 2.6);
    }

    #[test]
    fn has_inputs_is_true_before_a_change_can_be_measured() {
        let mut rsi = Rsi::new(2);
        assert!(!rsi.has_inputs());
        rsi.handle_bar(&bar(0, 0, 0, 100, 0));
        assert!(rsi.has_inputs());
        assert!(!rsi.initialized(), "a single close has no change yet");
    }

    #[test]
    fn reset_returns_rsi_to_its_pre_input_state() {
        let mut rsi = Rsi::new(2);
        rsi.handle_bar(&bar(0, 0, 0, 10, 0));
        rsi.handle_bar(&bar(0, 0, 0, 11, 0));
        rsi.handle_bar(&bar(0, 0, 0, 12, 0));
        assert!(rsi.initialized());

        rsi.reset();
        assert!(!rsi.has_inputs());
        assert!(!rsi.initialized());
        assert_approx_eq(rsi.value(), 50.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_rsi_output() {
        let bars = [
            bar(0, 0, 0, 10, 0),
            bar(0, 0, 0, 12, 0),
            bar(0, 0, 0, 11, 0),
            bar(0, 0, 0, 13, 0),
            bar(0, 0, 0, 12, 0),
        ];
        assert_live_matches_backfill(Rsi::new(3), Rsi::new(3), &bars, |rsi| {
            (rsi.value(), rsi.initialized())
        });
    }
}
