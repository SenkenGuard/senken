//! A best bid and offer update from a live venue.

use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use tokio::sync::watch;

use crate::{Lease, PoolError, SubscriptionPool};

/// Why a [`QuoteUpdate`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuoteError {
    /// The two sides of a quote used different price scales.
    #[error("bid and ask prices must use the same scale")]
    PriceScaleMismatch,
    /// The two sides of a quote used different quantity scales.
    #[error("bid and ask sizes must use the same scale")]
    QuantityScaleMismatch,
}

/// One top-of-book quote with consistent scaled-integer precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteUpdate {
    /// When the venue reported this quote.
    pub ts: UnixNanos,
    /// Best bid, at [`Self::price_scale`] fractional digits.
    pub bid: i64,
    /// Best ask, at [`Self::price_scale`] fractional digits.
    pub ask: i64,
    /// Shared fractional-digit scale of [`Self::bid`] and [`Self::ask`].
    pub price_scale: u8,
    /// Size available at the best bid, at [`Self::qty_scale`] fractional digits.
    pub bid_size: i64,
    /// Size available at the best ask, at [`Self::qty_scale`] fractional digits.
    pub ask_size: i64,
    /// Shared fractional-digit scale of [`Self::bid_size`] and [`Self::ask_size`].
    pub qty_scale: u8,
}

impl QuoteUpdate {
    /// Constructs a quote, rejecting sides that cannot share one scale.
    ///
    /// # Errors
    /// Returns [`QuoteError::PriceScaleMismatch`] or
    /// [`QuoteError::QuantityScaleMismatch`] when the corresponding two
    /// values use different decimal scales.
    pub const fn new(
        ts: UnixNanos,
        bid: (i64, u8),
        ask: (i64, u8),
        bid_size: (i64, u8),
        ask_size: (i64, u8),
    ) -> Result<Self, QuoteError> {
        if bid.1 != ask.1 {
            return Err(QuoteError::PriceScaleMismatch);
        }
        if bid_size.1 != ask_size.1 {
            return Err(QuoteError::QuantityScaleMismatch);
        }
        Ok(Self {
            ts,
            bid: bid.0,
            ask: ask.0,
            price_scale: bid.1,
            bid_size: bid_size.0,
            ask_size: ask_size.0,
            qty_scale: bid_size.1,
        })
    }
}

/// A source that can lease best-bid-and-offer updates independently of a
/// caller's use of last-trade prices.
#[async_trait::async_trait]
pub trait QuoteSource: Send + Sync {
    /// Claims the live stream for `instrument`.
    ///
    /// # Errors
    /// Returns the pool error when the venue connection cannot be opened or
    /// subscribed.
    async fn lease_quote(&self, instrument: InstrumentId) -> Result<QuoteLease, PoolError>;
}

/// A drop-guarded claim on the latest quote for one instrument.
#[must_use = "dropping this immediately releases the quote lease"]
pub struct QuoteLease {
    lease: Lease,
}

impl QuoteLease {
    /// The instrument this quote lease claims.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        self.lease.instrument()
    }

    /// A receiver for this instrument's latest quote.
    #[must_use]
    pub fn updates(&self) -> watch::Receiver<Option<QuoteUpdate>> {
        self.lease.quote_updates()
    }
}

#[async_trait::async_trait]
impl QuoteSource for SubscriptionPool {
    async fn lease_quote(&self, instrument: InstrumentId) -> Result<QuoteLease, PoolError> {
        self.lease(instrument)
            .await
            .map(|lease| QuoteLease { lease })
    }
}

#[cfg(test)]
mod tests {
    use super::{QuoteError, QuoteUpdate};
    use senken_core::UnixNanos;

    #[test]
    fn a_quote_rejects_bid_and_ask_with_different_scales() {
        let result = QuoteUpdate::new(UnixNanos::EPOCH, (1, 1), (10, 2), (1, 0), (1, 0));

        assert_eq!(result, Err(QuoteError::PriceScaleMismatch));
    }

    #[test]
    fn a_quote_rejects_sizes_with_different_scales() {
        let result = QuoteUpdate::new(UnixNanos::EPOCH, (1, 1), (2, 1), (1, 0), (10, 1));

        assert_eq!(result, Err(QuoteError::QuantityScaleMismatch));
    }
}
