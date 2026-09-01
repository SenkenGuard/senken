//! Folding a live price stream into completed and provisional bars.
//!
//! A tick is not a bar. Keeping this fold beside [`PriceUpdate`](crate::PriceUpdate)
//! gives every live consumer the same OHLCV semantics, instead of asking each
//! API or browser client to make a subtly different forming candle.

use senken_series::{Anchor, Bar, BarSpec, Volume, bucket_start};

use crate::PriceUpdate;

/// One bucket's running OHLCV fold.
#[derive(Debug)]
struct OpenBucket {
    start: senken_core::UnixNanos,
    open: i64,
    high: i64,
    low: i64,
    close: i64,
    volume: i64,
}

impl OpenBucket {
    fn bar(&self) -> Bar {
        Bar {
            ts_open: self.start,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: Volume::Real(self.volume),
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }
}

/// Folds a stream of ticks into bars for one specification.
///
/// [`push`](Self::push) returns only a completed bar: the bucket in progress
/// remains available through [`forming`](Self::forming) and is never passed
/// off as confirmed data.
#[derive(Debug)]
pub struct TickBarBuilder {
    spec: BarSpec,
    open: Option<OpenBucket>,
}

impl TickBarBuilder {
    /// Creates a builder with no open bucket.
    #[must_use]
    pub fn new(spec: BarSpec) -> Self {
        Self { spec, open: None }
    }

    /// Returns the current, still-forming bar.
    #[must_use]
    pub fn forming(&self) -> Option<Bar> {
        self.open.as_ref().map(OpenBucket::bar)
    }

    /// Folds one tick, returning the previous bucket only once a later one
    /// begins. Ticks older than the open bucket are ignored.
    pub fn push(&mut self, tick: &PriceUpdate) -> Option<Bar> {
        let start = bucket_start(tick.ts, self.spec, Anchor::UTC);
        let Some(open) = self.open.as_mut() else {
            self.open = Some(OpenBucket {
                start,
                open: tick.price,
                high: tick.price,
                low: tick.price,
                close: tick.price,
                volume: tick.qty.real().unwrap_or(0),
            });
            return None;
        };

        if start < open.start {
            return None;
        }
        if start == open.start {
            open.high = open.high.max(tick.price);
            open.low = open.low.min(tick.price);
            open.close = tick.price;
            open.volume = open.volume.saturating_add(tick.qty.real().unwrap_or(0));
            return None;
        }

        let closed = open.bar();
        self.open = Some(OpenBucket {
            start,
            open: tick.price,
            high: tick.price,
            low: tick.price,
            close: tick.price,
            volume: tick.qty.real().unwrap_or(0),
        });
        Some(closed)
    }
}

#[cfg(test)]
mod tests {
    use super::TickBarBuilder;
    use crate::PriceUpdate;
    use senken_core::UnixNanos;
    use senken_series::{BarSpec, BarUnit, Volume};

    fn tick(secs: i64, price: i64, qty: i64) -> PriceUpdate {
        PriceUpdate {
            ts: UnixNanos::from_secs(secs).unwrap(),
            price,
            price_scale: 2,
            qty: Volume::Real(qty),
            qty_scale: 0,
        }
    }

    #[test]
    fn a_forming_bar_is_not_reported_as_closed() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        assert!(builder.push(&tick(0, 100, 3)).is_none());
        assert!(builder.push(&tick(10, 110, 5)).is_none());

        let forming = builder.forming().unwrap();
        assert_eq!(forming.close, 110);
        assert_eq!(forming.volume, Volume::Real(8));

        let closed = builder.push(&tick(60, 120, 7)).unwrap();
        assert_eq!(closed, forming);
    }

    #[test]
    fn a_bucket_with_only_intra_bar_ticks_is_never_emitted() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        assert!(builder.push(&tick(0, 100, 0)).is_none());
        assert!(builder.push(&tick(10, 110, 0)).is_none());
        assert!(builder.push(&tick(59, 105, 0)).is_none());
    }

    #[test]
    fn an_out_of_order_tick_does_not_corrupt_the_open_bucket() {
        let mut builder = TickBarBuilder::new(BarSpec::new(1, BarUnit::Minute));
        builder.push(&tick(65, 500, 1));
        assert!(builder.push(&tick(5, 1, 1)).is_none());

        let closed = builder.push(&tick(130, 1, 1)).unwrap();
        assert_eq!(closed.close, 500);
        assert_eq!(closed.volume, Volume::Real(1));
    }
}
