//! How a simulated fill is priced, and when a resting order becomes one.
//!
//! Shared by every settlement model: slippage always runs against the
//! trader, and a resting order's trigger condition does not depend on what
//! kind of book is behind it.

use senken_core::decimal::Scaled;
use senken_trade::{OrderAmendment, OrderKind, OrderSide, TradeError};

use crate::money::slip;

/// The terms an account trades under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Terms {
    /// Fee charged on every fill, in basis points of notional.
    pub fee_bps: i64,
    /// How far a market order fills from the mark, in basis points.
    pub slippage_bps: i64,
    /// Notional multiple of margin an account may hold.
    pub leverage: i64,
}

/// Where a market order actually fills, given the mark.
///
/// Slippage runs against the trader in both directions — a buy fills above
/// the mark, a sell below it. A simulator that split the spread would
/// flatter every strategy run through it.
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn market_fill_price(
    mark: Scaled,
    side: OrderSide,
    terms: Terms,
) -> Result<Scaled, TradeError> {
    slip(mark, terms.slippage_bps, side.sign())
}

/// Whether a resting order's condition is met by `mark`.
///
/// A limit buy fills when the market comes down to it, a limit sell when it
/// comes up. A stop is the mirror image: it exists to be triggered by the
/// market moving *against* the position, so a stop buy triggers on the way
/// up.
#[must_use]
pub fn is_triggered(kind: OrderKind, side: OrderSide, mark: Scaled) -> bool {
    let Some(reference) = (match kind {
        OrderKind::Limit { price } => Some((price, true)),
        OrderKind::Stop { trigger } | OrderKind::StopLimit { trigger, .. } => {
            Some((trigger, false))
        }
        // `Market` has nothing to wait for, and `OrderKind` is
        // `#[non_exhaustive]`: a kind the kernel has not been taught never
        // fills rather than filling on a guessed condition.
        _ => None,
    }) else {
        return false;
    };
    let (level, is_limit) = reference;
    let Some(mark) = mark.rescale(level.scale) else {
        return false;
    };
    match (side, is_limit) {
        (OrderSide::Buy, true) | (OrderSide::Sell, false) => mark.value <= level.value,
        (OrderSide::Sell, true) | (OrderSide::Buy, false) => mark.value >= level.value,
    }
}

/// Applies an amendment's supplied prices onto `kind`, keeping whichever
/// field the amendment left `None` unchanged.
///
/// Every field `amendment` carries is assumed to already fit `kind` — the
/// engine refuses a limit price for a market order, or a trigger for a
/// plain limit, before this is ever called. `Market` has no price to amend
/// at all, and `OrderKind` is `#[non_exhaustive]`: a kind this build has not
/// been taught is left exactly as it was rather than guessed at.
#[must_use]
pub fn apply_amendment(kind: OrderKind, amendment: OrderAmendment) -> OrderKind {
    match kind {
        OrderKind::Limit { price } => OrderKind::Limit {
            price: amendment.limit_price.unwrap_or(price),
        },
        OrderKind::Stop { trigger } => OrderKind::Stop {
            trigger: amendment.trigger_price.unwrap_or(trigger),
        },
        OrderKind::StopLimit { trigger, price } => OrderKind::StopLimit {
            trigger: amendment.trigger_price.unwrap_or(trigger),
            price: amendment.limit_price.unwrap_or(price),
        },
        other => other,
    }
}
