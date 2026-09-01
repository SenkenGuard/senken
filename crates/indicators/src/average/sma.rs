//! [`Sma`] — the unweighted rolling average of the last `period` closes.

use std::collections::VecDeque;

use senken_series::Bar;

use crate::average::MovingAverage;
use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The simple moving average of the last `period` closes.
#[derive(Debug, Clone)]
pub struct Sma {
    period: usize,
    window: VecDeque<f64>,
    sum: f64,
    has_inputs: bool,
    initialized: bool,
}

impl Sma {
    /// Creates a new [`Sma`] over `period` bars.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero — a zero-length average is meaningless.
    #[must_use]
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "Sma::new requires period > 0");
        Self {
            period,
            window: VecDeque::with_capacity(period),
            sum: 0.0,
            has_inputs: false,
            initialized: false,
        }
    }
}

impl Indicator for Sma {
    fn name(&self) -> String {
        "Sma".to_string()
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

    fn snapshot(&self) -> Box<dyn Indicator> {
        Box::new(self.clone())
    }

    fn reset(&mut self) {
        self.window.clear();
        self.sum = 0.0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

impl MovingAverage for Sma {
    fn value(&self) -> f64 {
        if self.window.is_empty() {
            0.0
        } else {
            self.sum / self.window.len() as f64
        }
    }

    fn update_raw(&mut self, value: f64) {
        self.has_inputs = true;
        self.window.push_back(value);
        self.sum += value;
        if self.window.len() > self.period
            && let Some(oldest) = self.window.pop_front()
        {
            self.sum -= oldest;
        }
        self.initialized = self.window.len() >= self.period;
    }
}

#[cfg(test)]
mod tests {
    use super::Sma;
    use crate::average::MovingAverage;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed: the mean of `1, 2, 3` is exactly `2.0`, and the mean
    /// of the next rolling window `2, 3, 6` (once `1` has fallen out) is
    /// `11 / 3`.
    #[test]
    fn sma_matches_a_hand_computed_average_and_rolls_the_window() {
        let mut sma = Sma::new(3);
        sma.update_raw(1.0);
        assert!(!sma.initialized(), "only one of three inputs seen");
        sma.update_raw(2.0);
        assert!(!sma.initialized(), "only two of three inputs seen");
        sma.update_raw(3.0);
        assert!(sma.initialized(), "the window is now full");
        assert_approx_eq(sma.value(), 2.0);

        sma.update_raw(6.0);
        assert!(sma.initialized());
        assert_approx_eq(sma.value(), 11.0 / 3.0);
    }

    #[test]
    fn has_inputs_is_true_before_initialized() {
        let mut sma = Sma::new(2);
        assert!(!sma.has_inputs());
        sma.update_raw(5.0);
        assert!(sma.has_inputs());
        assert!(!sma.initialized(), "period is 2, only one input seen");
    }

    #[test]
    fn reset_returns_sma_to_its_pre_input_state() {
        let mut sma = Sma::new(3);
        sma.update_raw(1.0);
        sma.update_raw(2.0);
        sma.update_raw(3.0);
        assert!(sma.initialized());

        sma.reset();
        assert!(!sma.has_inputs());
        assert!(!sma.initialized());
        assert_approx_eq(sma.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_sma_output() {
        let bars = [
            bar(0, 0, 0, 1, 0),
            bar(0, 0, 0, 2, 0),
            bar(0, 0, 0, 3, 0),
            bar(0, 0, 0, 4, 0),
            bar(0, 0, 0, 5, 0),
        ];
        assert_live_matches_backfill(Sma::new(3), Sma::new(3), &bars, |sma| {
            (sma.value(), sma.initialized())
        });
    }
}
