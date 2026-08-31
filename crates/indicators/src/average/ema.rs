//! [`Ema`] — the exponential moving average, seeded with the first input
//! and updated in constant time thereafter.

use senken_series::Bar;

use crate::average::MovingAverage;
use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The exponential moving average of `close`, with smoothing factor
/// `alpha = 2 / (period + 1)`.
#[derive(Debug, Clone)]
pub struct Ema {
    period: usize,
    alpha: f64,
    value: f64,
    count: usize,
    has_inputs: bool,
    initialized: bool,
}

impl Ema {
    /// Creates a new [`Ema`] over `period` bars.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "Ema::new requires period > 0");
        Self {
            period,
            alpha: 2.0 / (period as f64 + 1.0),
            value: 0.0,
            count: 0,
            has_inputs: false,
            initialized: false,
        }
    }
}

impl Indicator for Ema {
    fn name(&self) -> String {
        "Ema".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    fn initialized(&self) -> bool {
        self.initialized
    }

    fn handle_bar(&mut self, bar: &Bar) {
        self.update_raw(scaled_to_f64(bar.close));
    }

    fn reset(&mut self) {
        self.value = 0.0;
        self.count = 0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

impl MovingAverage for Ema {
    fn value(&self) -> f64 {
        self.value
    }

    fn update_raw(&mut self, value: f64) {
        if self.has_inputs {
            // `mul_add` is one rounding step instead of two, matching the
            // reference implementation.
            self.value = self.alpha.mul_add(value, (1.0 - self.alpha) * self.value);
            self.count += 1;
        } else {
            self.has_inputs = true;
            self.value = value;
            self.count = 1;
        }
        if !self.initialized && self.count >= self.period {
            self.initialized = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Ema;
    use crate::average::MovingAverage;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `period = 3` (`alpha = 0.5`, an exact power of
    /// two, chosen so every step below is exact in binary floating point):
    /// `1 -> 1.0`, `2 -> 1.5`, `3 -> 2.25`. The average only becomes
    /// initialized on the third input.
    #[test]
    fn ema_matches_a_hand_computed_average() {
        let mut ema = Ema::new(3);
        ema.update_raw(1.0);
        assert_approx_eq(ema.value(), 1.0);
        assert!(!ema.initialized());

        ema.update_raw(2.0);
        assert_approx_eq(ema.value(), 1.5);
        assert!(!ema.initialized());

        ema.update_raw(3.0);
        assert_approx_eq(ema.value(), 2.25);
        assert!(ema.initialized());

        ema.update_raw(4.0);
        assert_approx_eq(ema.value(), 3.125);
        assert!(ema.initialized());
    }

    /// `alpha = 1.0` when `period = 1`, so the average tracks the latest
    /// input exactly and is initialized immediately.
    #[test]
    fn period_one_tracks_the_latest_value_exactly() {
        let mut ema = Ema::new(1);
        ema.update_raw(10.0);
        assert!(ema.initialized());
        assert_approx_eq(ema.value(), 10.0);

        ema.update_raw(42.0);
        assert_approx_eq(ema.value(), 42.0);
    }

    #[test]
    fn reset_returns_ema_to_its_pre_input_state() {
        let mut ema = Ema::new(2);
        ema.update_raw(1.0);
        ema.update_raw(2.0);
        assert!(ema.initialized());

        ema.reset();
        assert!(!ema.has_inputs());
        assert!(!ema.initialized());
        assert_approx_eq(ema.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_ema_output() {
        let bars = [
            bar(0, 0, 0, 1, 0),
            bar(0, 0, 0, 2, 0),
            bar(0, 0, 0, 3, 0),
            bar(0, 0, 0, 4, 0),
            bar(0, 0, 0, 5, 0),
        ];
        assert_live_matches_backfill(Ema::new(3), Ema::new(3), &bars, |ema| {
            (ema.value(), ema.initialized())
        });
    }
}
