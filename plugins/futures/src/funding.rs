//! Funding: a periodic transfer between longs and shorts.
//!
//! Not a fee paid to the exchange. It exists to pull a perpetual's price
//! back toward the underlying, so a long pays when the perpetual trades
//! above the index and is paid when it trades below. It settles straight
//! against the balance at the funding instant — no order, no fill, no fee.
//!
//! The interval is **per symbol and not constant**. Eight hours is the
//! usual default, but venues shorten it during extreme volatility and
//! revert afterwards, so it is read rather than assumed.

use senken_core::time::UnixNanos;
use senken_trade::{PositionSide, TradeError};

/// A symbol's funding configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FundingTerms {
    /// Nanoseconds between funding instants. Read per symbol: a venue may
    /// shorten this during volatility, so a hard-coded eight hours would
    /// silently undercount.
    pub interval_nanos: i64,
    /// The current rate, in basis points of notional. Positive means longs
    /// pay shorts.
    pub rate_bps: i64,
}

/// How many funding instants fall in `(from, to]`.
#[must_use]
pub fn intervals_crossed(terms: FundingTerms, from: UnixNanos, to: UnixNanos) -> i64 {
    if terms.interval_nanos <= 0 {
        return 0;
    }
    let first = from.as_nanos().div_euclid(terms.interval_nanos);
    let last = to.as_nanos().div_euclid(terms.interval_nanos);
    (last - first).max(0)
}

/// What one position pays or receives over `intervals`.
///
/// Negative is paid out of the account, positive is received. A long pays
/// a positive rate and a short receives it — the sign is the whole
/// mechanic, and getting it backwards would reward exactly the traders the
/// funding exists to discourage.
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn funding_for(
    terms: FundingTerms,
    side: PositionSide,
    notional: i64,
    intervals: i64,
) -> Result<i64, TradeError> {
    if intervals == 0 {
        return Ok(0);
    }
    let magnitude =
        i128::from(notional) * i128::from(terms.rate_bps) * i128::from(intervals) / 10_000;
    let directed = match side {
        PositionSide::Long => -magnitude,
        PositionSide::Short => magnitude,
    };
    i64::try_from(directed)
        .map_err(|_| TradeError::InvalidRequest("the funding amount does not fit".to_owned()))
}
