//! [`BollingerBands`] — a moving average with bands at `k` population
//! standard deviations above and below it.

use std::collections::VecDeque;

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// Bollinger Bands: an overlay of an upper, middle and lower line.
///
/// The middle line is a simple moving average of `close` over `period`
/// bars; the upper and lower lines sit `k` **population** standard
/// deviations (divided by `period`, not `period - 1`) above and below it —
/// the convention Bollinger's own definition and most charting platforms
/// use. Reports three values, so — like [`Macd`](crate::Macd) — it has its
/// own accessors rather than one shared `value()`.
#[derive(Debug, Clone)]
pub struct BollingerBands {
    period: usize,
    k: f64,
    window: VecDeque<f64>,
    sum: f64,
    upper: f64,
    middle: f64,
    lower: f64,
    has_inputs: bool,
    initialized: bool,
}

impl BollingerBands {
    /// Creates a new [`BollingerBands`] over `period` bars, `k` standard
    /// deviations wide.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: usize, k: f64) -> Self {
        assert!(period > 0, "BollingerBands::new requires period > 0");
        Self {
            period,
            k,
            window: VecDeque::with_capacity(period),
            sum: 0.0,
            upper: 0.0,
            middle: 0.0,
            lower: 0.0,
            has_inputs: false,
            initialized: false,
        }
    }

    /// The upper band: `middle + k * standard deviation`.
    #[must_use]
    pub fn upper(&self) -> f64 {
        self.upper
    }

    /// The middle band: the simple moving average of `close`.
    #[must_use]
    pub fn middle(&self) -> f64 {
        self.middle
    }

    /// The lower band: `middle - k * standard deviation`.
    #[must_use]
    pub fn lower(&self) -> f64 {
        self.lower
    }
}

impl Indicator for BollingerBands {
    fn name(&self) -> String {
        "BollingerBands".to_string()
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

        self.window.push_back(price);
        self.sum += price;
        if self.window.len() > self.period
            && let Some(oldest) = self.window.pop_front()
        {
            self.sum -= oldest;
        }
        if self.window.len() < self.period {
            return;
        }

        let n = self.window.len() as f64;
        self.middle = self.sum / n;
        let variance = self
            .window
            .iter()
            .map(|price| (price - self.middle).powi(2))
            .sum::<f64>()
            / n;
        let std_dev = variance.sqrt();
        self.upper = self.k.mul_add(std_dev, self.middle);
        self.lower = self.middle - self.k * std_dev;
        self.initialized = true;
    }

    fn snapshot(&self) -> Box<dyn Indicator> {
        Box::new(self.clone())
    }

    fn reset(&mut self) {
        self.window.clear();
        self.sum = 0.0;
        self.upper = 0.0;
        self.middle = 0.0;
        self.lower = 0.0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::BollingerBands;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `period = 3`, `k = 2.0` over closes `1, 2, 3`:
    /// mean = 2.0; population variance = `((1-2)^2 + (2-2)^2 + (3-2)^2) / 3
    /// = 2/3`; standard deviation = `sqrt(2/3) = sqrt(6)/3 ≈
    /// 0.8164965809277260`. Upper = `2 + 2 * 0.816... ≈ 3.632993`, lower =
    /// `2 - 2 * 0.816... ≈ 0.367007`.
    #[test]
    fn bollinger_bands_match_a_hand_computed_value() {
        let mut bb = BollingerBands::new(3, 2.0);
        bb.handle_bar(&bar(0, 0, 0, 1, 0));
        assert!(!bb.initialized());
        bb.handle_bar(&bar(0, 0, 0, 2, 0));
        assert!(!bb.initialized());
        bb.handle_bar(&bar(0, 0, 0, 3, 0));
        assert!(bb.initialized());

        let std_dev = 6.0_f64.sqrt() / 3.0;
        assert_approx_eq(bb.middle(), 2.0);
        assert_approx_eq(bb.upper(), 2.0 + 2.0 * std_dev);
        assert_approx_eq(bb.lower(), 2.0 - 2.0 * std_dev);
    }

    #[test]
    fn reset_returns_bollinger_bands_to_its_pre_input_state() {
        let mut bb = BollingerBands::new(2, 2.0);
        bb.handle_bar(&bar(0, 0, 0, 1, 0));
        bb.handle_bar(&bar(0, 0, 0, 2, 0));
        assert!(bb.initialized());

        bb.reset();
        assert!(!bb.has_inputs());
        assert!(!bb.initialized());
        assert_approx_eq(bb.middle(), 0.0);
        assert_approx_eq(bb.upper(), 0.0);
        assert_approx_eq(bb.lower(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_bollinger_bands_output() {
        let bars = [
            bar(0, 0, 0, 1, 0),
            bar(0, 0, 0, 2, 0),
            bar(0, 0, 0, 3, 0),
            bar(0, 0, 0, 4, 0),
            bar(0, 0, 0, 5, 0),
        ];
        assert_live_matches_backfill(
            BollingerBands::new(3, 2.0),
            BollingerBands::new(3, 2.0),
            &bars,
            |bb| (bb.upper(), bb.middle(), bb.lower(), bb.initialized()),
        );
    }
}
