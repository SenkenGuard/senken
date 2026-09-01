//! [`Stochastic`] — the stochastic oscillator: `%K` and its `%D` moving
//! average.

use std::collections::VecDeque;

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The stochastic oscillator.
///
/// `%K` compares the current close to the high/low range of the last
/// `k_period` bars; `%D` is a simple moving average of `%K` over
/// `d_period` values. Reports two values, so — like [`Macd`](crate::Macd)
///   — it has its own accessors rather than one shared `value()`.
///
/// Needs `k_period + d_period - 1` bars to initialize: `k_period` to
/// produce the first `%K`, then `d_period` more `%K` values (the first of
/// which is that same bar) to fill `%D`'s own window.
#[derive(Debug, Clone)]
pub struct Stochastic {
    k_period: usize,
    d_period: usize,
    window: VecDeque<(f64, f64)>,
    k_values: VecDeque<f64>,
    k: f64,
    d: f64,
    has_inputs: bool,
    initialized: bool,
}

impl Stochastic {
    /// Creates a new [`Stochastic`] with the given `%K` and `%D` periods.
    ///
    /// # Panics
    ///
    /// Panics if either period is zero.
    #[must_use]
    pub fn new(k_period: usize, d_period: usize) -> Self {
        assert!(k_period > 0, "Stochastic::new requires k_period > 0");
        assert!(d_period > 0, "Stochastic::new requires d_period > 0");
        Self {
            k_period,
            d_period,
            window: VecDeque::with_capacity(k_period),
            k_values: VecDeque::with_capacity(d_period),
            k: 0.0,
            d: 0.0,
            has_inputs: false,
            initialized: false,
        }
    }

    /// The current `%K`, on a `0..=100` scale. Meaningless before the
    /// `k_period`-bar window is full.
    #[must_use]
    pub fn k(&self) -> f64 {
        self.k
    }

    /// The current `%D`: a simple moving average of `%K`. Meaningless
    /// before [`initialized`](Indicator::initialized).
    #[must_use]
    pub fn d(&self) -> f64 {
        self.d
    }
}

impl Indicator for Stochastic {
    fn name(&self) -> String {
        "Stochastic".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    fn initialized(&self) -> bool {
        self.initialized
    }

    fn snapshot(&self) -> Box<dyn Indicator> {
        Box::new(self.clone())
    }

    fn handle_bar(&mut self, bar: &Bar) {
        let high = scaled_to_f64(bar.high);
        let low = scaled_to_f64(bar.low);
        let close = scaled_to_f64(bar.close);
        self.has_inputs = true;

        self.window.push_back((high, low));
        if self.window.len() > self.k_period {
            self.window.pop_front();
        }
        if self.window.len() < self.k_period {
            return;
        }

        let highest = self
            .window
            .iter()
            .map(|&(h, _)| h)
            .fold(f64::NEG_INFINITY, f64::max);
        let lowest = self
            .window
            .iter()
            .map(|&(_, l)| l)
            .fold(f64::INFINITY, f64::min);
        let range = highest - lowest;
        // A flat window (every high and low equal) has no range to divide
        // by; `50.0` is the same "unmoving series" convention `Rsi` uses.
        self.k = if range <= 0.0 {
            50.0
        } else {
            100.0 * (close - lowest) / range
        };

        self.k_values.push_back(self.k);
        if self.k_values.len() > self.d_period {
            self.k_values.pop_front();
        }
        if self.k_values.len() >= self.d_period {
            self.d = self.k_values.iter().sum::<f64>() / self.d_period as f64;
            self.initialized = true;
        }
    }

    fn reset(&mut self) {
        self.window.clear();
        self.k_values.clear();
        self.k = 0.0;
        self.d = 0.0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Stochastic;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `k_period = 3`, `d_period = 2` over bars
    /// `(H, L, C)`: `(10, 5, 8)`, `(12, 6, 9)`, `(11, 7, 10)`, `(9, 6, 8)`.
    ///
    /// - After bar 3, the window is `{(10,5), (12,6), (11,7)}`:
    ///   highest = 12, lowest = 5, range = 7,
    ///   `%K = 100 * (10 - 5) / 7 = 500/7`. Only one `%K` value exists, so
    ///   `%D` (which needs two) is not ready.
    /// - After bar 4, the window drops bar 1 and is
    ///   `{(12,6), (11,7), (9,6)}`: highest = 12, lowest = 6, range = 6,
    ///   `%K = 100 * (8 - 6) / 6 = 200/6`.
    ///   `%D = (500/7 + 200/6) / 2 = 52.380952380952...`.
    #[test]
    fn stochastic_matches_a_hand_computed_value() {
        let mut stoch = Stochastic::new(3, 2);
        stoch.handle_bar(&bar(0, 10, 5, 8, 0));
        assert!(!stoch.initialized());
        stoch.handle_bar(&bar(0, 12, 6, 9, 0));
        assert!(!stoch.initialized());

        stoch.handle_bar(&bar(0, 11, 7, 10, 0));
        assert!(!stoch.initialized(), "%K window is full but %D's is not");
        assert_approx_eq(stoch.k(), 500.0 / 7.0);

        stoch.handle_bar(&bar(0, 9, 6, 8, 0));
        assert!(stoch.initialized());
        assert_approx_eq(stoch.k(), 200.0 / 6.0);
        assert_approx_eq(stoch.d(), f64::midpoint(500.0 / 7.0, 200.0 / 6.0));
    }

    #[test]
    fn reset_returns_stochastic_to_its_pre_input_state() {
        let mut stoch = Stochastic::new(2, 2);
        stoch.handle_bar(&bar(0, 10, 5, 8, 0));
        stoch.handle_bar(&bar(0, 12, 6, 9, 0));
        stoch.handle_bar(&bar(0, 11, 7, 10, 0));
        assert!(stoch.initialized());

        stoch.reset();
        assert!(!stoch.has_inputs());
        assert!(!stoch.initialized());
        assert_approx_eq(stoch.k(), 0.0);
        assert_approx_eq(stoch.d(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_stochastic_output() {
        let bars = [
            bar(0, 10, 5, 8, 0),
            bar(0, 12, 6, 9, 0),
            bar(0, 11, 7, 10, 0),
            bar(0, 9, 6, 8, 0),
            bar(0, 13, 8, 12, 0),
        ];
        assert_live_matches_backfill(
            Stochastic::new(3, 2),
            Stochastic::new(3, 2),
            &bars,
            |stoch| (stoch.k(), stoch.d(), stoch.initialized()),
        );
    }
}
