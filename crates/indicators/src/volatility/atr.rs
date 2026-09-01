//! [`Atr`] — Wilder's average true range.

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// Wilder's average true range over `period` bars.
///
/// True range is `max(high - low, |high - prev_close|, |low - prev_close|)`
///   — or plain `high - low` for the first bar, which has no previous close.
/// Unlike [`Rsi`](crate::Rsi), that means the first bar already produces a
/// usable true range, so `Atr` initializes after exactly `period` bars, not
/// `period + 1`. The first `period` true ranges are seeded as a plain
/// average; every one after that is folded in with Wilder's smoothing:
/// `atr = (atr * (period - 1) + latest) / period`.
#[derive(Debug, Clone)]
pub struct Atr {
    period: usize,
    prev_close: Option<f64>,
    value: f64,
    tr_sum: f64,
    tr_seen: usize,
    has_inputs: bool,
    initialized: bool,
}

impl Atr {
    /// Creates a new [`Atr`] over `period` bars.
    ///
    /// # Panics
    ///
    /// Panics if `period` is zero.
    #[must_use]
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "Atr::new requires period > 0");
        Self {
            period,
            prev_close: None,
            value: 0.0,
            tr_sum: 0.0,
            tr_seen: 0,
            has_inputs: false,
            initialized: false,
        }
    }

    /// The current average true range. Meaningless before
    /// [`initialized`](Indicator::initialized).
    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }
}

impl Indicator for Atr {
    fn name(&self) -> String {
        "Atr".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    fn initialized(&self) -> bool {
        self.initialized
    }

    fn handle_bar(&mut self, bar: &Bar) {
        let high = scaled_to_f64(bar.high);
        let low = scaled_to_f64(bar.low);
        let close = scaled_to_f64(bar.close);
        self.has_inputs = true;

        let true_range = match self.prev_close {
            None => high - low,
            Some(prev) => (high - low)
                .max((high - prev).abs())
                .max((low - prev).abs()),
        };

        if self.tr_seen < self.period {
            self.tr_sum += true_range;
            self.tr_seen += 1;
            if self.tr_seen == self.period {
                self.value = self.tr_sum / self.period as f64;
                self.initialized = true;
            }
        } else {
            let period = self.period as f64;
            self.value = self.value.mul_add(period - 1.0, true_range) / period;
        }

        self.prev_close = Some(close);
    }

    fn snapshot(&self) -> Box<dyn Indicator> {
        Box::new(self.clone())
    }

    fn reset(&mut self) {
        self.prev_close = None;
        self.value = 0.0;
        self.tr_sum = 0.0;
        self.tr_seen = 0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Atr;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed with `period = 2` over bars `(H, L, C)`:
    ///
    /// - bar 1 `(10, 8, 9)`: no previous close, so `TR = 10 - 8 = 2`.
    /// - bar 2 `(11, 9, 10)`: `TR = max(11-9, |11-9|, |9-9|) = max(2,2,0)
    ///   = 2`. That is the second of two seed values, so
    ///   `ATR = (2 + 2) / 2 = 2.0` and the indicator initializes here.
    /// - bar 3 `(13, 10, 12)`: `TR = max(13-10, |13-10|, |10-10|) = 3`.
    ///   Wilder smoothing: `ATR = (2.0 * 1 + 3) / 2 = 2.5`.
    #[test]
    fn atr_matches_a_hand_computed_value() {
        let mut atr = Atr::new(2);
        atr.handle_bar(&bar(0, 10, 8, 9, 0));
        assert!(!atr.initialized());

        atr.handle_bar(&bar(0, 11, 9, 10, 0));
        assert!(atr.initialized());
        assert_approx_eq(atr.value(), 2.0);

        atr.handle_bar(&bar(0, 13, 10, 12, 0));
        assert!(atr.initialized());
        assert_approx_eq(atr.value(), 2.5);
    }

    #[test]
    fn has_inputs_is_true_from_the_first_bar() {
        let mut atr = Atr::new(3);
        assert!(!atr.has_inputs());
        atr.handle_bar(&bar(0, 10, 8, 9, 0));
        assert!(atr.has_inputs());
        assert!(!atr.initialized(), "only one of three true ranges seen");
    }

    #[test]
    fn reset_returns_atr_to_its_pre_input_state() {
        let mut atr = Atr::new(2);
        atr.handle_bar(&bar(0, 10, 8, 9, 0));
        atr.handle_bar(&bar(0, 11, 9, 10, 0));
        assert!(atr.initialized());

        atr.reset();
        assert!(!atr.has_inputs());
        assert!(!atr.initialized());
        assert_approx_eq(atr.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_atr_output() {
        let bars = [
            bar(0, 10, 8, 9, 0),
            bar(0, 11, 9, 10, 0),
            bar(0, 13, 10, 12, 0),
            bar(0, 12, 9, 10, 0),
        ];
        assert_live_matches_backfill(Atr::new(2), Atr::new(2), &bars, |atr| {
            (atr.value(), atr.initialized())
        });
    }
}
