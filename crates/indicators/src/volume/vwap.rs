//! [`Vwap`] — the cumulative volume-weighted average price.

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The cumulative volume-weighted average price: the running sum of
/// `typical_price * volume` divided by the running sum of `volume`, where
/// `typical_price = (high + low + close) / 3`.
///
/// Accumulates since construction (or the last [`reset`](Indicator::reset))
/// rather than resetting on a session boundary — this crate has no notion
/// of a trading session or anchor point, so that is left to whatever
/// eventually schedules `reset()` calls (out of scope for).
#[derive(Debug, Clone)]
pub struct Vwap {
    cumulative_price_volume: f64,
    cumulative_volume: f64,
    value: f64,
    has_inputs: bool,
    initialized: bool,
}

impl Vwap {
    /// Creates a new [`Vwap`] with no accumulated volume.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cumulative_price_volume: 0.0,
            cumulative_volume: 0.0,
            value: 0.0,
            has_inputs: false,
            initialized: false,
        }
    }

    /// The current volume-weighted average price. Meaningless before
    /// [`initialized`](Indicator::initialized).
    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }
}

impl Default for Vwap {
    fn default() -> Self {
        Self::new()
    }
}

impl Indicator for Vwap {
    fn name(&self) -> String {
        "Vwap".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    /// `false` until cumulative volume is positive — a bar with zero
    /// volume has been "seen" but contributes nothing to compute a
    /// meaningful average from, so it does not initialize `Vwap` on its
    /// own.
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
        let Some(volume) = bar.volume.real().map(scaled_to_f64) else {
            return;
        };
        let typical_price = (high + low + close) / 3.0;

        self.has_inputs = true;
        self.cumulative_price_volume += typical_price * volume;
        self.cumulative_volume += volume;
        if self.cumulative_volume > 0.0 {
            self.value = self.cumulative_price_volume / self.cumulative_volume;
            self.initialized = true;
        }
    }

    fn reset(&mut self) {
        self.cumulative_price_volume = 0.0;
        self.cumulative_volume = 0.0;
        self.value = 0.0;
        self.has_inputs = false;
        self.initialized = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Vwap;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed over bars `(H, L, C, V)`: `(10, 8, 9, 100)`,
    /// `(12, 10, 11, 50)`.
    ///
    /// - bar 1: typical price = `(10+8+9)/3 = 9.0`,
    ///   VWAP = `9.0*100 / 100 = 9.0`.
    /// - bar 2: typical price = `(12+10+11)/3 = 11.0`,
    ///   cumulative price*volume = `900 + 550 = 1450`,
    ///   cumulative volume = `150`, VWAP = `1450/150 = 9.666...`.
    #[test]
    fn vwap_matches_a_hand_computed_value() {
        let mut vwap = Vwap::new();
        vwap.handle_bar(&bar(0, 10, 8, 9, 100));
        assert!(vwap.initialized());
        assert_approx_eq(vwap.value(), 9.0);

        vwap.handle_bar(&bar(0, 12, 10, 11, 50));
        assert_approx_eq(vwap.value(), 1450.0 / 150.0);
    }

    #[test]
    fn a_zero_volume_bar_does_not_initialize_vwap() {
        let mut vwap = Vwap::new();
        assert!(!vwap.has_inputs());
        assert!(!vwap.initialized());

        vwap.handle_bar(&bar(0, 10, 8, 9, 0));
        assert!(vwap.has_inputs());
        assert!(!vwap.initialized(), "no volume yet to weight an average by");

        vwap.handle_bar(&bar(0, 10, 8, 9, 5));
        assert!(vwap.initialized());
    }

    #[test]
    fn reset_returns_vwap_to_its_pre_input_state() {
        let mut vwap = Vwap::new();
        vwap.handle_bar(&bar(0, 10, 8, 9, 100));
        assert!(vwap.initialized());

        vwap.reset();
        assert!(!vwap.has_inputs());
        assert!(!vwap.initialized());
        assert_approx_eq(vwap.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_vwap_output() {
        let bars = [
            bar(0, 10, 8, 9, 100),
            bar(0, 12, 10, 11, 50),
            bar(0, 11, 9, 10, 75),
        ];
        assert_live_matches_backfill(Vwap::new(), Vwap::new(), &bars, |vwap| {
            (vwap.value(), vwap.initialized())
        });
    }
}
