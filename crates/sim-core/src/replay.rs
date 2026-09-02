//! Settling through a range of bars rather than at one mark.
//!
//! Evaluating a resting order against the single price that happened to be
//! current when someone read the account is not a simulation of resting:
//! a stop fills at the reader's price rather than at the bar that actually
//! touched it, and if nobody looks for an hour, the stop fills an hour
//! late. The trader's real risk was never modelled.
//!
//! Senken already stores bars, which almost nothing in this class of
//! application does. So settlement replays the bars between the book's own
//! `settled_through` and now, in order, and a level is reached by the bar
//! whose high or low actually reached it, at that bar's own time.
//!
//! ## Intrabar order is unknowable, and this says so
//!
//! Within one bar, whether the high or the low came first cannot be
//! recovered from the bar. When a bar would trigger both a stop loss and a
//! take profit, this resolves it the way every serious backtester does —
//! **the worse-for-the-trader side first** — and says so rather than
//! hiding it. Finer bars narrow the window; they never close it.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;

/// One bar, as replay needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayBar {
    /// When the bar opened.
    pub opened_at: UnixNanos,
    /// The highest price it reached.
    pub high: Scaled,
    /// The lowest price it reached.
    pub low: Scaled,
    /// Where it closed.
    pub close: Scaled,
}

/// A price level a position is watching, and which way it is crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    /// The price itself.
    pub price: Scaled,
    /// `true` when the level is reached by the market falling to it — a
    /// long's stop loss, a short's take profit.
    pub from_above: bool,
    /// `true` when reaching it is the bad outcome for the trader.
    pub adverse: bool,
}

impl Level {
    /// Whether `bar` reached this level.
    #[must_use]
    pub fn reached_by(self, bar: &ReplayBar) -> bool {
        let extreme = if self.from_above { bar.low } else { bar.high };
        let Some(extreme) = extreme.rescale(self.price.scale) else {
            return false;
        };
        if self.from_above {
            extreme.value <= self.price.value
        } else {
            extreme.value >= self.price.value
        }
    }
}

/// Which of a position's levels a bar triggers, and in what order.
///
/// Returns them worst-first: when one bar reaches both a stop loss and a
/// take profit, the losing side is taken as having happened first, because
/// the bar cannot say which did and assuming the profitable one would
/// flatter every strategy replayed through it.
#[must_use]
pub fn triggered_in_order(bar: &ReplayBar, levels: &[Level]) -> Vec<Level> {
    let mut hit: Vec<Level> = levels
        .iter()
        .copied()
        .filter(|level| level.reached_by(bar))
        .collect();
    hit.sort_by_key(|level| !level.adverse);
    hit
}

/// The bars in `bars` that fall in `(from, to]`.
///
/// Half-open at the start so a book settled through an instant does not
/// replay the bar it already settled — which is what makes reading twice
/// idempotent rather than charging or filling twice.
pub fn bars_in_range(
    bars: &[ReplayBar],
    from: UnixNanos,
    to: UnixNanos,
) -> impl Iterator<Item = &ReplayBar> {
    bars.iter().filter(move |bar| {
        bar.opened_at.as_nanos() > from.as_nanos() && bar.opened_at.as_nanos() <= to.as_nanos()
    })
}

#[cfg(test)]
mod tests {
    use super::{Level, ReplayBar, bars_in_range, triggered_in_order};
    use senken_core::decimal::Scaled;
    use senken_core::time::UnixNanos;

    fn bar(at: i64, high: i64, low: i64, close: i64) -> ReplayBar {
        ReplayBar {
            opened_at: UnixNanos::from_secs(at).unwrap(),
            high: Scaled::new(0, high),
            low: Scaled::new(0, low),
            close: Scaled::new(0, close),
        }
    }

    fn stop_loss(price: i64) -> Level {
        Level {
            price: Scaled::new(0, price),
            from_above: true,
            adverse: true,
        }
    }

    fn take_profit(price: i64) -> Level {
        Level {
            price: Scaled::new(0, price),
            from_above: false,
            adverse: false,
        }
    }

    #[test]
    fn a_level_is_reached_by_the_bar_that_touched_it_not_by_the_close() {
        // A bar that dipped to 95 and recovered to 105 reached a stop at
        // 98, even though its close never did.
        let spike = bar(1_000, 105, 95, 105);
        assert!(
            stop_loss(98).reached_by(&spike),
            "a stop is hit by the low, not by where the bar happened to close — settling on \
             closes alone would miss every intrabar stop"
        );
    }

    #[test]
    fn a_bar_that_never_reached_the_level_does_not_trigger_it() {
        let quiet = bar(1_000, 105, 99, 104);
        assert!(!stop_loss(98).reached_by(&quiet));
        assert!(!take_profit(110).reached_by(&quiet));
    }

    #[test]
    fn one_bar_reaching_both_levels_takes_the_losing_side_first() {
        // High 112, low 95: this bar reached both. Which came first is not
        // recoverable from it.
        let wild = bar(1_000, 112, 95, 100);
        let order = triggered_in_order(&wild, &[take_profit(110), stop_loss(98)]);

        assert_eq!(order.len(), 2);
        assert!(
            order[0].adverse,
            "the bar cannot say which came first, and assuming the profitable one would \
             flatter every strategy replayed through this"
        );
    }

    #[test]
    fn replaying_a_range_skips_the_bar_already_settled_through() {
        let bars = [bar(1_000, 105, 95, 100), bar(2_000, 106, 96, 101)];
        let replayed: Vec<_> = bars_in_range(
            &bars,
            UnixNanos::from_secs(1_000).unwrap(),
            UnixNanos::from_secs(2_000).unwrap(),
        )
        .collect();

        assert_eq!(
            replayed.len(),
            1,
            "the range is half-open at the start, so a second read settles nothing again"
        );
        assert_eq!(replayed[0].opened_at, UnixNanos::from_secs(2_000).unwrap());
    }

    #[test]
    fn settling_twice_through_the_same_instant_replays_nothing() {
        let bars = [bar(1_000, 105, 95, 100)];
        let at = UnixNanos::from_secs(2_000).unwrap();
        assert_eq!(bars_in_range(&bars, at, at).count(), 0);
    }
}
