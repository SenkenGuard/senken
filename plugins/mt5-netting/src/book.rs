//! One position per symbol, and the four transitions a fill can cause.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_trade::PositionSide;
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

/// One netted position.
///
/// It carries **two** identifiers, and the difference is the thing most
/// implementations of netting get wrong. A reversal changes the ticket to
/// the reversing order's own, because the exposure that now exists was
/// opened by that order — but the identifier survives, because everything
/// that groups a position's deal history keys on it. Collapsing them into
/// one field makes a reversal either break history or lie about when the
/// current exposure opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetPosition {
    /// Changes on reversal, to the reversing order's ticket.
    pub ticket: u64,
    /// Survives every transition, including a reversal.
    pub identifier: u64,
    /// Long or short. One or the other, never both.
    pub side: PositionSide,
    /// Volume held.
    pub volume: Scaled,
    /// Volume-weighted average entry, except after a reversal, where it is
    /// the reversing deal's own price.
    pub entry: Scaled,
    /// When the exposure that exists now opened. Resets on reversal.
    pub opened_at: UnixNanos,
}

/// A netting book: one position per symbol, ordered so two runs of the
/// same fills report in the same order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NettingBook {
    /// The positions, at most one per instrument.
    pub positions: BTreeMap<InstrumentId, NetPosition>,
    /// The next ticket number to hand out.
    pub next_ticket: u64,
    /// Realised profit banked across every position this account has held.
    pub realized: i64,
}

/// Which of the four transitions a fill caused.
///
/// Closed on purpose: there is no fifth transition on a netting account,
/// and a new variant should fail to compile everywhere that reports one
/// rather than being folded into an existing case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// A position opened where there was none.
    Opened,
    /// Volume added in the same direction, at a new weighted average.
    Added,
    /// Volume taken off, booking profit, leaving the entry alone.
    Reduced,
    /// The position closed exactly flat.
    Closed,
    /// An opposite fill larger than the position: it closed, and the
    /// remainder opened the other way at this deal's own price.
    Reversed,
}
