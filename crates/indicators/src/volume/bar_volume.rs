//! [`Volume`] — the current bar's traded volume, as a plottable indicator.

use senken_series::Bar;

use crate::convert::scaled_to_f64;
use crate::indicator::Indicator;

/// The traded volume of the most recent bar.
///
/// Every other indicator in this crate accumulates some window of history
/// before it means anything; `Volume` does not — a bar's volume is exactly
/// as meaningful on the very first bar as on the thousandth; there is no
/// average or difference to warm up. It exists as its own [`Indicator`]
/// (rather than callers just reading `bar.volume` themselves) so a volume
/// histogram is placeable as a sub-pane layer the same way every other
/// indicator in this crate is.
#[derive(Debug, Clone)]
pub struct Volume {
    value: f64,
    has_inputs: bool,
}

impl Volume {
    /// Creates a new [`Volume`] with no bar seen yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            value: 0.0,
            has_inputs: false,
        }
    }

    /// The most recent bar's volume. Meaningless before
    /// [`initialized`](Indicator::initialized).
    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }
}

impl Default for Volume {
    fn default() -> Self {
        Self::new()
    }
}

impl Indicator for Volume {
    fn name(&self) -> String {
        "Volume".to_string()
    }

    fn has_inputs(&self) -> bool {
        self.has_inputs
    }

    /// Identical to [`has_inputs`](Indicator::has_inputs) — there is no
    /// warm-up period for a value that is never an average of anything.
    fn initialized(&self) -> bool {
        self.has_inputs
    }

    fn handle_bar(&mut self, bar: &Bar) {
        self.value = scaled_to_f64(bar.volume);
        self.has_inputs = true;
    }

    fn reset(&mut self) {
        self.value = 0.0;
        self.has_inputs = false;
    }
}

#[cfg(test)]
mod tests {
    use super::Volume;
    use crate::indicator::Indicator;
    use crate::test_support::{assert_approx_eq, assert_live_matches_backfill, bar};

    /// Hand-computed: a bar with `volume = 1234` reports exactly `1234.0`.
    #[test]
    fn volume_reports_the_bars_volume_exactly() {
        let mut volume = Volume::new();
        assert!(!volume.initialized());

        volume.handle_bar(&bar(0, 0, 0, 0, 1234));
        assert!(volume.initialized());
        assert_approx_eq(volume.value(), 1234.0);

        volume.handle_bar(&bar(0, 0, 0, 0, 5));
        assert_approx_eq(volume.value(), 5.0);
    }

    #[test]
    fn reset_returns_volume_to_its_pre_input_state() {
        let mut volume = Volume::new();
        volume.handle_bar(&bar(0, 0, 0, 0, 1234));
        assert!(volume.initialized());

        volume.reset();
        assert!(!volume.has_inputs());
        assert!(!volume.initialized());
        assert_approx_eq(volume.value(), 0.0);
    }

    #[test]
    fn live_and_backfill_produce_identical_volume_output() {
        let bars = [
            bar(0, 0, 0, 0, 10),
            bar(0, 0, 0, 0, 20),
            bar(0, 0, 0, 0, 30),
        ];
        assert_live_matches_backfill(Volume::new(), Volume::new(), &bars, |volume| {
            (volume.value(), volume.initialized())
        });
    }
}
