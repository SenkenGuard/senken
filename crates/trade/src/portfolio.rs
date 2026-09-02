//! What an account is worth right now: its positions and its balances.
//!
//! Both are read from the adapter on every request and never cached here.
//! The venue behind an account is the system of record for them; a second
//! copy in Senken's own database could only ever be a copy that disagrees,
//! and the way it would disagree is by being stale at exactly the moment
//! someone needed it to be right.

use senken_core::UnixNanos;
use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use serde::{Deserialize, Serialize};

use crate::id::{PositionId, TradeAccountId};

/// Which way a position is exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionSide {
    /// Gains when the price rises.
    Long,
    /// Gains when the price falls.
    Short,
}

impl PositionSide {
    /// `+1` for a long, `-1` for a short.
    #[must_use]
    pub fn sign(self) -> i64 {
        match self {
            Self::Long => 1,
            Self::Short => -1,
        }
    }
}

/// Whose money backs a position's margin.
///
/// Crypto derivative venues let a trader choose per position, and the
/// choice decides what liquidation means: an isolated position can be
/// liquidated on its own without touching anything else, while cross
/// positions share the account's balance and, on the venues that document
/// it, a symbol's long and short share a single liquidation price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MarginMode {
    /// Margin allocated to this position alone. Liquidating it leaves the
    /// account's other positions untouched.
    Isolated,
    /// Margin drawn from the account's whole balance, shared with every
    /// other cross position.
    Cross,
}

/// One open position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    /// This position, distinctly from any other on the same instrument.
    ///
    /// A hedging account holds several at once and the instrument alone
    /// cannot name one; see [`PositionId`].
    pub id: PositionId,
    /// The account holding it.
    pub account_id: TradeAccountId,
    /// The instrument.
    pub instrument: InstrumentId,
    /// Long or short.
    pub side: PositionSide,
    /// Size held, in the adapter's own
    /// [`quantity_unit`](crate::AdapterCapabilities::quantity_unit).
    pub quantity: Scaled,
    /// Volume-weighted average price the position was opened at.
    pub average_entry: Scaled,
    /// The price it is currently marked at, when one is available. `None`
    /// is reported honestly rather than substituting the entry price, which
    /// would show a real position at a flat zero profit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<Scaled>,
    /// Profit if closed at [`mark_price`](Self::mark_price), in the
    /// account's own currency. `None` whenever the mark is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl: Option<Scaled>,
    /// Profit already banked on this instrument.
    pub realized_pnl: Scaled,
    /// The price at which this position closes itself at a loss, when the
    /// account has one set.
    ///
    /// At most one, which is the invariant MetaTrader 5 enforces on a
    /// position and which this field makes unrepresentable to violate: a
    /// second stop loss is not a thing that can be constructed. A venue
    /// where stops are free-standing conditional orders instead expresses
    /// them as ordinary orders carrying
    /// [`OrderRequest::reduce_only`](crate::OrderRequest::reduce_only) —
    /// the two mechanisms stay distinct because the venues keep them
    /// distinct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<Scaled>,
    /// The price at which this position closes itself at a profit, when the
    /// account has one set. At most one, for the same reason as
    /// [`stop_loss`](Self::stop_loss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<Scaled>,
    /// Margin the venue is holding against it, where the account uses
    /// margin at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin: Option<Scaled>,
    /// Leverage applied, where the account uses leverage at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leverage: Option<Scaled>,
    /// Whether this position's margin is its own or the account's, where
    /// the venue offers the choice. `None` where it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_mode: Option<MarginMode>,
    /// The price at which the venue would close this position for want of
    /// margin.
    ///
    /// `None` is the honest answer whenever the figure is not known — a
    /// venue that does not publish one, or an account that cannot be
    /// liquidated at all. It is never filled in with an estimate: a trader
    /// who is shown a liquidation price will believe it, and a wrong one
    /// is worse than an absent one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquidation_price: Option<Scaled>,
    /// When the position was first opened.
    pub opened_at: UnixNanos,
}

/// What one asset's balance looks like.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetBalance {
    /// The asset's ticker, as the venue names it (`USDT`, `USD`, `BTC`).
    pub asset: String,
    /// Everything held, free or not.
    pub total: Scaled,
    /// The part that can be spent on a new order.
    pub available: Scaled,
    /// The part held against open orders and margin.
    pub reserved: Scaled,
}

/// An account's money, as its adapter reports it.
///
/// The per-asset rows and the account-level totals both exist because
/// venues differ in which one is the truth: a spot exchange has balances in
/// several assets and no single equity figure, a margin broker has one
/// account currency and no meaningful per-asset split. An adapter fills in
/// what its venue actually has and leaves the rest `None`, rather than
/// synthesising a number that reads as authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountBalances {
    /// The account.
    pub account_id: TradeAccountId,
    /// The currency the account-level figures below are denominated in.
    pub currency: String,
    /// Cash, excluding unrealised profit.
    pub balance: Scaled,
    /// Cash plus unrealised profit.
    pub equity: Scaled,
    /// Unrealised profit across every open position.
    pub unrealized_pnl: Scaled,
    /// Profit already banked on this account, across every instrument and
    /// every position it has ever held.
    ///
    /// An account total rather than a position field, because a realised
    /// profit records something that already happened and a position that
    /// has been closed is not somewhere a completed fact can live. Reading
    /// it off positions loses every profit the moment the position that
    /// earned it goes flat.
    pub realized_pnl: Scaled,
    /// Margin currently held against positions and orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_used: Option<Scaled>,
    /// Margin still available to open with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_available: Option<Scaled>,
    /// Equity as a percentage of margin held — MetaTrader's own figure,
    /// and the one its margin call and stop out are thresholds on. `None`
    /// for an account that holds no margin, where the ratio has no
    /// denominator rather than an infinite value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_level: Option<Scaled>,
    /// Per-asset rows, for venues that have them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<AssetBalance>,
}

/// Whether an adapter can currently be used, and why not when it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdapterHealth {
    /// Reachable and authenticated.
    Connected,
    /// Reachable but not usable — bad credentials, a restricted account.
    Degraded {
        /// What is wrong, in words a user can act on.
        reason: String,
    },
    /// Not reachable at all.
    Disconnected {
        /// What is wrong, in words a user can act on.
        reason: String,
    },
}

impl AdapterHealth {
    /// `true` only when the adapter is fully usable.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }

    /// Builds a [`Disconnected`](Self::Disconnected).
    pub fn disconnected(reason: impl Into<String>) -> Self {
        Self::Disconnected {
            reason: reason.into(),
        }
    }

    /// Builds a [`Degraded`](Self::Degraded).
    pub fn degraded(reason: impl Into<String>) -> Self {
        Self::Degraded {
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AdapterHealth, PositionSide};

    #[test]
    fn a_position_side_signs_profit_the_way_exposure_runs() {
        assert_eq!(PositionSide::Long.sign(), 1);
        assert_eq!(PositionSide::Short.sign(), -1);
    }

    #[test]
    fn only_a_fully_connected_adapter_reports_itself_connected() {
        assert!(AdapterHealth::Connected.is_connected());
        assert!(!AdapterHealth::degraded("api key lacks trade scope").is_connected());
        assert!(!AdapterHealth::disconnected("connection refused").is_connected());
    }

    #[test]
    fn health_serialises_its_reason_alongside_a_readable_state_tag() {
        let json = serde_json::to_string(&AdapterHealth::degraded("read-only key")).unwrap();
        assert_eq!(json, r#"{"state":"degraded","reason":"read-only key"}"#);
    }
}
