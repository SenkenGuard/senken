//! [`Macd`] — moving average convergence/divergence: a fast and slow EMA of
//! `close`, a signal EMA of their difference, and the histogram between
//! the two.

use senken_series::Bar;

use crate::average::{Ema, MovingAverage};
use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// Moving average convergence/divergence.
///
/// Reports three values every bar, not one — the reason this crate's
/// [`Indicator`] trait has no shared `value()` method:
///
/// - [`macd`](Self::macd) — the fast EMA minus the slow EMA.
/// - [`signal`](Self::signal) — an EMA of the MACD line itself.
/// - [`histogram`](Self::histogram) — `macd - signal`.
///
/// Built entirely out of three [`Ema`]s composed through
/// [`MovingAverage::update_raw`] — the whole indicator is under 40 lines
/// because the incremental discipline it needs already lives in [`Ema`].
#[derive(Debug, Clone)]
pub struct Macd {
    fast: Ema,
    slow: Ema,
    signal: Ema,
    line: f64,
    histogram: f64,
    has_inputs: bool,
}

impl Macd {
    /// Creates a new [`Macd`] with the given fast, slow and signal
    /// periods.
    ///
    /// # Panics
    ///
    /// Panics if any period is zero (via [`Ema::new`]).
    #[must_use]
    pub fn new(fast_period: usize, slow_period: usize, signal_period: usize) -> Self {
        Self {
            fast: Ema::new(fast_period),
            slow: Ema::new(slow_period),
            signal: Ema::new(signal_period),
            line: 0.0,
            histogram: 0.0,
            has_inputs: false,
        }
    }

    /// The MACD line: fast EMA minus slow EMA.
    #[must_use]
    pub fn macd(&self) -> f64 {
        self.line
    }

    /// The signal line: an EMA of the MACD line.
    #[must_use]
    pub fn signal(&self) -> f64 {
        self.signal.value()
    }

    /// The histogram: `macd - signal`.
    #[must_use]
    pub fn histogram(&self) -> f64 {
        self.histogram
    }
}

impl Indicator for Macd {
    fn name(&self) -> String {
        "Macd".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    /// Initialized once the slow EMA is meaningful *and* the signal line
    /// has smoothed over enough MACD values of its own — whichever of the
    /// two takes longer, since both must hold for all three outputs to be
    /// trustworthy at once.
    fn initialized(&self) -> bool {
        self.slow.initialized() && self.signal.initialized()
    }

    fn handle_bar(&mut self, bar: &Bar) {
        let price = scaled_to_f64(bar.close);
        self.has_inputs = true;
        self.fast.update_raw(price);
        self.slow.update_raw(price);
        self.line = self.fast.value() - self.slow.value();
        self.signal.update_raw(self.line);
        self.histogram = self.line - self.signal.value();
    }

    fn snapshot(&self) -> Box<dyn Indicator> {
        Box::new(self.clone())
    }

    fn reset(&mut self) {
        self.fast.reset();
        self.slow.reset();
        self.signal.reset();
        self.line = 0.0;
        self.histogram = 0.0;
        self.has_inputs = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Macd;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `fast = 1` (alpha = 1, so the fast EMA tracks
    /// `close` exactly), `slow = 3` (alpha = 0.5, an exact power of two)
    /// and `signal = 2` (alpha = 2/3) over closes `10, 12, 14`:
    ///
    /// - bar 1: fast = 10, slow = 10, macd = 0, signal = 0.
    /// - bar 2: fast = 12, slow = 0.5*12 + 0.5*10 = 11, macd = 1,
    ///   signal = 2/3*1 + 1/3*0 = 2/3. Slow is still warming up (2 < 3).
    /// - bar 3: fast = 14, slow = 0.5*14 + 0.5*11 = 12.5, macd = 1.5,
    ///   signal = 2/3*1.5 + 1/3*(2/3) = 1 + 2/9 = 11/9,
    ///   histogram = 1.5 - 11/9 = 5/18. The slow EMA reaches its third
    ///   input on this bar and the signal EMA reached its second on the
    ///   bar before, so `initialized()` only becomes true here.
    #[test]
    fn macd_matches_a_hand_computed_value() {
        let mut macd = Macd::new(1, 3, 2);

        macd.handle_bar(&bar(0, 0, 0, 10, 0));
        assert!(!macd.initialized());
        assert_approx_eq(macd.macd(), 0.0);

        macd.handle_bar(&bar(0, 0, 0, 12, 0));
        assert!(!macd.initialized(), "slow EMA has only two of three inputs");
        assert_approx_eq(macd.macd(), 1.0);
        assert_approx_eq(macd.signal(), 2.0 / 3.0);

        macd.handle_bar(&bar(0, 0, 0, 14, 0));
        assert!(macd.initialized());
        assert_approx_eq(macd.macd(), 1.5);
        assert_approx_eq(macd.signal(), 11.0 / 9.0);
        assert_approx_eq(macd.histogram(), 5.0 / 18.0);
    }

    #[test]
    fn reset_returns_macd_to_its_pre_input_state() {
        let mut macd = Macd::new(1, 2, 1);
        macd.handle_bar(&bar(0, 0, 0, 10, 0));
        macd.handle_bar(&bar(0, 0, 0, 12, 0));
        assert!(macd.initialized());

        macd.reset();
        assert!(!macd.has_inputs());
        assert!(!macd.initialized());
        assert_approx_eq(macd.macd(), 0.0);
        assert_approx_eq(macd.signal(), 0.0);
        assert_approx_eq(macd.histogram(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_macd_output() {
        let bars = [
            bar(0, 0, 0, 10, 0),
            bar(0, 0, 0, 12, 0),
            bar(0, 0, 0, 14, 0),
            bar(0, 0, 0, 11, 0),
            bar(0, 0, 0, 13, 0),
        ];
        assert_live_matches_backfill(Macd::new(1, 3, 2), Macd::new(1, 3, 2), &bars, |macd| {
            (
                macd.macd(),
                macd.signal(),
                macd.histogram(),
                macd.initialized(),
            )
        });
    }
}
