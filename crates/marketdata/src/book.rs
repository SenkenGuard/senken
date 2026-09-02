//! A fixed-depth order-book snapshot from a venue that reports one.
//!
//! Unlike a live quote, a [`BookSnapshot`] is not something a
//! caller leases from an always-open connection: it is one venue-reported
//! instant, fetched fresh on request. A book maintained locally from venue
//! deltas — reconciling out-of-order updates, replaying a resync after a
//! dropped connection — is real, ongoing complexity that is not worth
//! paying for a panel that only ever needs "the picture right now"; nothing
//! in this crate merges two snapshots or tracks a sequence number across
//! them.

use senken_core::UnixNanos;

use crate::instrument::SourceSymbol;
use crate::source::SourceError;

/// One resting order level, either side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookLevel {
    /// Level price, at the snapshot's [`BookSnapshot::price_scale`].
    pub price: i64,
    /// Size resting at this level, at the snapshot's
    /// [`BookSnapshot::qty_scale`].
    pub size: i64,
}

/// Why a [`BookSnapshot`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    /// The two sides reported levels at different price scales.
    #[error("bid and ask levels must use the same price scale")]
    PriceScaleMismatch,
    /// The two sides reported levels at different quantity scales.
    #[error("bid and ask levels must use the same quantity scale")]
    QuantityScaleMismatch,
}

/// A fixed-depth snapshot of one instrument's resting orders, both sides at
/// one venue-reported instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookSnapshot {
    /// When the venue reported this snapshot.
    pub ts: UnixNanos,
    /// Resting bids, best price first.
    pub bids: Vec<BookLevel>,
    /// Resting asks, best price first.
    pub asks: Vec<BookLevel>,
    /// Shared fractional-digit scale of every level's `price`.
    pub price_scale: u8,
    /// Shared fractional-digit scale of every level's `size`.
    pub qty_scale: u8,
}

impl BookSnapshot {
    /// Constructs a snapshot, rejecting a bid side and ask side that cannot
    /// share one scale — the same invariant `senken_subscription::QuoteUpdate::new`
    /// enforces for a top-of-book quote, generalised to a whole ladder.
    ///
    /// # Errors
    /// Returns [`BookError::PriceScaleMismatch`] or
    /// [`BookError::QuantityScaleMismatch`] when the two sides' own scales
    /// disagree.
    pub fn new(
        ts: UnixNanos,
        bids: Vec<BookLevel>,
        bid_price_scale: u8,
        bid_qty_scale: u8,
        asks: Vec<BookLevel>,
        ask_price_scale: u8,
        ask_qty_scale: u8,
    ) -> Result<Self, BookError> {
        if bid_price_scale != ask_price_scale {
            return Err(BookError::PriceScaleMismatch);
        }
        if bid_qty_scale != ask_qty_scale {
            return Err(BookError::QuantityScaleMismatch);
        }
        Ok(Self {
            ts,
            bids,
            asks,
            price_scale: bid_price_scale,
            qty_scale: bid_qty_scale,
        })
    }
}

/// A source that can fetch a fixed-depth order-book snapshot for one
/// instrument.
///
/// Registered independently of `senken_subscription::QuoteSource` and any bar source —
/// a venue implements this only when it actually has book depth to serve.
/// There is deliberately no default method that answers with an empty book:
/// a venue with no implementation here has **no** capability, not an empty
/// one, and the difference must be visible to whatever reports capabilities
/// to a client.
#[async_trait::async_trait]
pub trait BookSource: Send + Sync {
    /// The source id this serves depth for — e.g. `okx-spot`, the same
    /// left half of an [`InstrumentId`](crate::InstrumentId) a caller
    /// addresses through it.
    ///
    /// Present for the same reason
    /// `senken_plugin::BarSource::source_id` is: a plugin hands the runtime
    /// a registration, not a map entry, so the source it serves has to be
    /// something the implementation states rather than something the caller
    /// remembers to key it by.
    fn source_id(&self) -> &str;

    /// Fetches a fresh snapshot for `symbol`, at most `depth` levels per
    /// side. Never a locally-maintained book updated from deltas — every
    /// call is a fresh request to the venue.
    ///
    /// # Errors
    /// Returns [`SourceError`] on the usual terms a venue adapter reports
    /// through it: transport failure, a non-success HTTP status, an
    /// application-level rejection, or a response that does not decode.
    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError>;
}

impl std::fmt::Debug for dyn BookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookSource")
            .field("source_id", &self.source_id())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{BookError, BookLevel, BookSnapshot};
    use senken_core::UnixNanos;

    fn level(price: i64, size: i64) -> BookLevel {
        BookLevel { price, size }
    }

    #[test]
    fn a_snapshot_rejects_bid_and_ask_levels_with_different_price_scales() {
        let result = BookSnapshot::new(
            UnixNanos::EPOCH,
            vec![level(1, 1)],
            1,
            0,
            vec![level(10, 1)],
            2,
            0,
        );

        assert_eq!(result.unwrap_err(), BookError::PriceScaleMismatch);
    }

    #[test]
    fn a_snapshot_rejects_bid_and_ask_levels_with_different_quantity_scales() {
        let result = BookSnapshot::new(
            UnixNanos::EPOCH,
            vec![level(1, 1)],
            1,
            0,
            vec![level(2, 10)],
            1,
            1,
        );

        assert_eq!(result.unwrap_err(), BookError::QuantityScaleMismatch);
    }

    #[test]
    fn a_snapshot_with_matching_scales_is_constructed() {
        let snapshot = BookSnapshot::new(
            UnixNanos::EPOCH,
            vec![level(100, 5)],
            2,
            3,
            vec![level(101, 4)],
            2,
            3,
        )
        .unwrap();

        assert_eq!(snapshot.price_scale, 2);
        assert_eq!(snapshot.qty_scale, 3);
        assert_eq!(snapshot.bids, vec![level(100, 5)]);
        assert_eq!(snapshot.asks, vec![level(101, 4)]);
    }
}
