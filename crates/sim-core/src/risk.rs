//! What a simulated account's risk state is, and what the system does when
//! it is breached.
//!
//! The two are deliberately separate. MetaTrader's margin call **closes
//! nothing** — it only blocks new positions while the margin level stays
//! below its threshold — while its stop out closes losing positions one at
//! a time until the level recovers. A model that collapsed both into "the
//! account is in trouble" could not reproduce either.

use senken_core::decimal::Scaled;

/// What an account's risk looks like right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskState {
    /// Cash, excluding unrealised profit.
    pub balance: i64,
    /// Balance plus the unrealised profit of every open position.
    pub equity: i64,
    /// Margin held against every open position.
    pub margin_used: i64,
    /// `100 × equity / margin_used`, the single percentage margin call and
    /// stop out are both measured against.
    ///
    /// `None` when no margin is held. Reporting infinity, or a zero that
    /// reads as "fully margin called", would both be wrong for an account
    /// that simply has nothing open.
    pub margin_level: Option<Scaled>,
    /// Which threshold, if any, the level has crossed.
    pub breach: Option<RiskBreach>,
}

/// A threshold the account's risk has crossed.
///
/// Not `#[non_exhaustive]`: a third kind of breach should fail to compile
/// everywhere that decides what to do about one, rather than falling into
/// a catch-all that treats it like an existing kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskBreach {
    /// New positions are refused; nothing is closed.
    ///
    /// This is MetaTrader's **margin call** exactly: it blocks opening
    /// while the level is under the broker's call threshold, and it never
    /// touches an open position.
    OpeningBlocked,
    /// The system closes positions itself until the level recovers.
    ///
    /// MetaTrader's **stop out**, and a crypto venue's liquidation. What
    /// gets closed, and in what order, is the model's own rule — MT5
    /// closes the biggest loser first and repeats.
    ForcedClosure,
}

/// One position the system closed on the account's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcedClose {
    /// Which position was closed, in the model's own book.
    pub position: String,
    /// The price it was closed at.
    pub price: Scaled,
    /// What closing it realised.
    pub realized: i64,
    /// Why the system closed it.
    pub reason: RiskBreach,
}
