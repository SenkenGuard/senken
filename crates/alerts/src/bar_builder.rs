//! [`TickBarBuilder`] — folding live [`PriceUpdate`] ticks into closed
//! [`Bar`]s.
//!
//! `senken-subscription`'s own docs are explicit that a [`PriceUpdate`] is
//! "not a bar", and that building one was out of scope at the time
//!   — a consumer that wants bars from live ticks has to build them
//! itself. An alert is exactly that consumer ("evaluated on each new bar"), so this is that missing piece, scoped to this crate.
//!
//! # Never emitting a forming bar
//!
//! A bar is only knowable at
//! `ts_open + interval` — evaluating an indicator against a bar that is
//! still forming is the classic false-positive this feature must avoid
//! (the scope note). [`TickBarBuilder::push`] follows exactly
//! the discipline `senken_series::Aggregator::push` already uses for
//! folding finer bars into coarser ones: **a bucket is only ever emitted
//! when a later tick arrives whose bucket has moved on**, never speculatively
//! from the bucket currently in progress. There is no `finish`-style escape
//! hatch here (unlike `Aggregator`, which offers one for a caller that
//! knows history has ended) — a live tick stream never "ends", so there is
//! nothing to flush.
//!
//! This is event-driven, not timer-driven: a stalled tick stream near a
//! bucket boundary delays that bucket's close until the next tick arrives,
//! rather than forcing it closed on a wall-clock timeout. That is a
//! deliberate, narrower property than a real venue feed would want
//! eventually, but it is the same property `Aggregator` itself already has
//! (it has no notion of a timeout either), and it means this builder reads
//! no clock of its own — every timestamp it ever sees came from the
//! [`PriceUpdate`] a real `VenueConnection` decoded, never a direct
//! `SystemTime`/`Instant` read (the "no wall-clock reads on the
//! market-data or replay path").

use senken_series::{Bar, BarSpec, bucket_start};
use senken_subscription::PriceUpdate;

/// One bucket's running OHLC fold, in progress.
///
/// Volume is always `0` and the optional fields are always `None`: a
/// last-trade tick carries no size at all (`PriceUpdate`'s own docs), so
/// there is nothing to sum into them. A caller that needs volume must read
/// it from the stored series instead — this builder exists only to give an
/// indicator a live OHLC close to react to.
#[derive(Debug)]
struct OpenBucket {
    start: senken_core::UnixNanos,
    open: i64,
    high: i64,
    low: i64,
    close: i64,
}

/// Folds a stream of [`PriceUpdate`] ticks for one instrument into closed
/// [`Bar`]s of `spec`, holding at most one bucket in progress — the live
/// counterpart to `senken_series::Aggregator`, but folding raw ticks
/// instead of finer bars.
#[derive(Debug)]
pub struct TickBarBuilder {
    spec: BarSpec,
    open: Option<OpenBucket>,
}

impl TickBarBuilder {
    /// A fresh builder for `spec`, with no bucket yet in progress.
    #[must_use]
    pub fn new(spec: BarSpec) -> Self {
        Self { spec, open: None }
    }

    /// Folds one more tick in, emitting the previous bucket if `tick`
    /// starts a new one.
    ///
    /// A tick that lands in the bucket already in progress only updates
    /// that bucket's high/low/close (open and start are fixed from the
    /// bucket's first tick) and never returns a bar — see the module docs
    /// for why this never speculatively closes the bucket currently open.
    /// A tick timestamped **before** the current bucket's start (a
    /// venue-supplied tick arriving out of order) is dropped rather than
    /// corrupting the bucket in progress.
    pub fn push(&mut self, tick: &PriceUpdate) -> Option<Bar> {
        let bucket_start = bucket_start(tick.ts, self.spec, senken_series::Anchor::UTC);

        let Some(open) = self.open.as_mut() else {
            self.open = Some(OpenBucket {
                start: bucket_start,
                open: tick.price,
                high: tick.price,
                low: tick.price,
                close: tick.price,
            });
            return None;
        };

        if bucket_start < open.start {
            // Out of order relative to the bucket already in progress —
            // dropped, exactly like `Aggregator`'s own treatment of an
            // out-of-order input.
            return None;
        }

        if bucket_start == open.start {
            open.high = open.high.max(tick.price);
            open.low = open.low.min(tick.price);
            open.close = tick.price;
            return None;
        }

        // `tick` starts a new, later bucket: the one in progress is now
        // fully behind us in time and can be safely closed.
        let closed = Bar {
            ts_open: open.start,
            open: open.open,
            high: open.high,
            low: open.low,
            close: open.close,
            volume: 0,
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        };
        self.open = Some(OpenBucket {
            start: bucket_start,
            open: tick.price,
            high: tick.price,
            low: tick.price,
            close: tick.price,
        });
        Some(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::TickBarBuilder;
    use senken_core::UnixNanos;
    use senken_series::{BarSpec, BarUnit};
    use senken_subscription::PriceUpdate;

    fn tick(secs: i64, price: i64) -> PriceUpdate {
        PriceUpdate {
            ts: UnixNanos::from_secs(secs).unwrap(),
            price,
            price_scale: 2,
            qty: 0,
            qty_scale: 0,
        }
    }

    #[test]
    fn a_bucket_with_only_intra_bar_ticks_is_never_emitted() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        // All within [0, 60).
        assert!(builder.push(&tick(0, 100)).is_none());
        assert!(builder.push(&tick(10, 110)).is_none());
        assert!(
            builder.push(&tick(59, 105)).is_none(),
            "still the same forming bucket — must not be emitted yet"
        );
    }

    #[test]
    fn a_tick_starting_the_next_bucket_closes_the_previous_one_with_the_correct_ohlc() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        builder.push(&tick(0, 100));
        builder.push(&tick(10, 130)); // high
        builder.push(&tick(20, 90)); // low
        let bar = builder
            .push(&tick(61, 999)) // starts minute 1 — closes minute 0
            .expect("the first bucket must close once a later one starts");

        assert_eq!(bar.ts_open, UnixNanos::from_secs(0).unwrap());
        assert_eq!(bar.open, 100, "open is the bucket's first tick");
        assert_eq!(bar.high, 130);
        assert_eq!(bar.low, 90);
        assert_eq!(bar.close, 90, "close is the bucket's last tick");
        assert_eq!(bar.volume, 0, "ticks carry no size to sum");
    }

    #[test]
    fn an_out_of_order_tick_is_dropped_and_does_not_corrupt_the_open_bucket() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        builder.push(&tick(65, 500)); // opens minute 1
        assert!(
            builder.push(&tick(5, 1)).is_none(),
            "a tick for the already-passed minute 0 must be dropped"
        );
        let bar = builder.push(&tick(130, 1)).unwrap(); // opens minute 2, closes minute 1
        assert_eq!(
            bar.close, 500,
            "the out-of-order tick must not have touched minute 1's close"
        );
    }
}
