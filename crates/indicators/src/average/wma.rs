//! [`Wma`] — a rolling average weighted linearly toward the most recent
//! bar.

use std::collections::VecDeque;

use senken_series::Bar;

use crate::average::MovingAverage;
use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The weighted moving average of the last `period` closes, with linear
/// weights `1, 2, ..., period` assigned oldest to newest — the most recent
/// close counts `period` times as much as the oldest one in the window.
///
/// Recomputing the weighted sum from the window on every bar is `O(period)`
/// rather than `O(1)`, the same tradeoff the reference implementation read
/// makes for the same indicator. That is still bounded by
/// `period`, not by how much history has accumulated — a `Wma` over a
/// 10-bar window does the same fixed amount of work on bar five thousand as
/// it does on bar ten.
#[derive(Debug, Clone)]
pub struct Wma {
    period: usize,
    window: VecDeque<f64>,
    has_inputs: bool,
    initialized: bool,
}

impl Wma {
    /// Creates a new [`Wma`] over `period` bars.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "Wma::new requires period > 0");
        Self {
            period,
            window: VecDeque::with_capacity(period),
            has_inputs: false,
            initialized: false,
        }
    }
}

impl Indicator for Wma {
    fn name(&self) -> String {
        "Wma".to_string()
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
        self.window.clear();
        self.has_inputs = false;
        self.initialized = false;
    }
}

impl MovingAverage for Wma {
    fn value(&self) -> f64 {
        let n = self.window.len();
        if n == 0 {
            return 0.0;
        }
        let mut weighted_sum = 0.0;
        let mut weight_total = 0.0;
        for (i, price) in self.window.iter().enumerate() {
            // Oldest entry (i = 0) gets weight 1; the newest (i = n - 1)
            // gets weight n.
            let weight = (i + 1) as f64;
            weighted_sum += weight * price;
            weight_total += weight;
        }
        weighted_sum / weight_total
    }

    fn update_raw(&mut self, value: f64) {
        self.has_inputs = true;
        self.window.push_back(value);
        if self.window.len() > self.period {
            self.window.pop_front();
        }
        self.initialized = self.window.len() >= self.period;
    }
}

#[cfg(test)]
mod tests {
    use super::Wma;
    use crate::average::MovingAverage;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed: with weights `1, 2, 3` over closes `1, 2, 3`, the
    /// weighted sum is `1*1 + 2*2 + 3*3 = 14` over a weight total of `6`,
    /// giving `14 / 6`. Once `4` arrives and `1` falls out of the window,
    /// the weighted sum over `2, 3, 4` is `1*2 + 2*3 + 3*4 = 20`, giving
    /// `20 / 6`.
    #[test]
    fn wma_matches_a_hand_computed_weighted_average() {
        let mut wma = Wma::new(3);
        wma.update_raw(1.0);
        assert!(!wma.initialized());
        wma.update_raw(2.0);
        assert!(!wma.initialized());
        wma.update_raw(3.0);
        assert!(wma.initialized());
        assert_approx_eq(wma.value(), 14.0 / 6.0);

        wma.update_raw(4.0);
        assert!(wma.initialized());
        assert_approx_eq(wma.value(), 20.0 / 6.0);
    }

    #[test]
    fn reset_returns_wma_to_its_pre_input_state() {
        let mut wma = Wma::new(2);
        wma.update_raw(1.0);
        wma.update_raw(2.0);
        assert!(wma.initialized());

        wma.reset();
        assert!(!wma.has_inputs());
        assert!(!wma.initialized());
        assert_approx_eq(wma.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_wma_output() {
        let bars = [
            bar(0, 0, 0, 1, 0),
            bar(0, 0, 0, 2, 0),
            bar(0, 0, 0, 3, 0),
            bar(0, 0, 0, 4, 0),
            bar(0, 0, 0, 5, 0),
        ];
        assert_live_matches_backfill(Wma::new(3), Wma::new(3), &bars, |wma| {
            (wma.value(), wma.initialized())
        });
    }
}
