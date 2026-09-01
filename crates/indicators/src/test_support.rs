//! Shared test-only helpers used by every indicator's `#[cfg(test)]` module.

use senken_core::UnixNanos;
use senken_series::{Bar, Volume};

use crate::indicator::Indicator;

/// Builds a minimal bar with the given OHLCV fields; `ts_open` is
/// irrelevant to every indicator in this crate (none of them read it), so
/// it is always the epoch, and the optional trade/quote-volume/taker-buy
/// fields are always `None`.
pub(crate) fn bar(open: i64, high: i64, low: i64, close: i64, volume: i64) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open,
        high,
        low,
        close,
        volume: Volume::Real(volume),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// Asserts two `f64`s are within a tolerance tight enough to catch a wrong
/// formula but loose enough to absorb ordinary floating-point rounding.
pub(crate) fn assert_approx_eq(actual: f64, expected: f64) {
    let diff = (actual - expected).abs();
    assert!(
        diff < 1e-9,
        "expected {expected}, got {actual} (difference {diff})"
    );
}

/// Feeds `bars` into `live` one bar per call — as a real-time subscriber
/// would receive them — and into `backfill` via one loop over the whole
/// slice — as replaying stored history would — then asserts `extract`
/// reports the same thing from both afterwards.
///
/// This crate has no second, from-scratch computation path:
/// both loops below call the exact same [`Indicator::handle_bar`]. This
/// test exists to keep it that way — it would fail the moment a future
/// batch-only shortcut computed a different answer than the incremental
/// path it was meant to match.
pub(crate) fn assert_live_matches_backfill<I: Indicator, T: PartialEq + std::fmt::Debug>(
    mut live: I,
    mut backfill: I,
    bars: &[Bar],
    extract: impl Fn(&I) -> T,
) {
    for bar in bars {
        // One bar "arriving" at a time, exactly as a live subscription
        // would deliver it.
        live.handle_bar(bar);
    }
    for bar in bars {
        // The same history, replayed through the same method, as a
        // backfill would.
        backfill.handle_bar(bar);
    }
    assert_eq!(
        extract(&live),
        extract(&backfill),
        "live and backfill diverged for the same bars"
    );
}
