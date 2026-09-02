//! The hedging book: one ticket per deal, and the stop-out rule that
//! decides which one closes first.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_trade::PositionSide;

/// One open position, as MetaTrader identifies it.
///
/// A ticket is the position. On a hedging account nothing ever re-opens a
/// position by reversing it in place — a buy and a sell on one symbol are
/// two tickets, not one changing sign — so the ticket and the position
/// identifier coincide for the position's whole life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    /// The ticket number, unique for the account's lifetime.
    pub id: u64,
    /// Which symbol it is on.
    pub instrument: InstrumentId,
    /// Long or short. Both can be open on one symbol at once — that is
    /// what hedging means.
    pub side: PositionSide,
    /// Volume in lots.
    pub lots: Scaled,
    /// The price this ticket opened at. A partial close leaves it
    /// unchanged on the remainder.
    pub open_price: Scaled,
    /// The stop loss attached to this ticket, if it has one. At most one,
    /// which is MetaTrader's own invariant.
    pub stop_loss: Option<Scaled>,
    /// The take profit attached to this ticket, if it has one.
    pub take_profit: Option<Scaled>,
    /// Swap accrued against this ticket so far, at the account's cash
    /// scale. Charged per position, per day held through rollover.
    pub swap: i64,
    /// Margin held against this ticket.
    pub margin: i64,
    /// When it opened.
    pub opened_at: UnixNanos,
}

/// The hedging book.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HedgingBook {
    /// Every open ticket, in the order they were opened.
    pub tickets: Vec<Ticket>,
    /// The next ticket number to hand out.
    pub next_ticket: u64,
}

impl HedgingBook {
    /// Every ticket on one symbol — several, on a hedging account, and
    /// possibly on both sides at once.
    pub fn on(&self, instrument: &InstrumentId) -> impl Iterator<Item = &Ticket> {
        self.tickets
            .iter()
            .filter(move |ticket| &ticket.instrument == instrument)
    }

    /// The ticket a stop out closes next: the **biggest loser**.
    ///
    /// MetaTrader closes the largest losing position first, then looks
    /// again, and repeats until the margin level recovers — rather than
    /// closing everything at once or closing in ticket order. On a hedging
    /// account that distinction is visible: a trader locked long and short
    /// has two independent unrealised results, and the profitable leg is
    /// not touched while a losing one remains the largest loser.
    ///
    /// `None` when nothing is open, or when nothing is losing — a stop out
    /// has nothing to close on an account whose positions are all in
    /// profit, however low its margin level has fallen.
    #[must_use]
    pub fn biggest_loser(&self, unrealized: &dyn Fn(&Ticket) -> Option<i64>) -> Option<&Ticket> {
        self.tickets
            .iter()
            .filter_map(|ticket| unrealized(ticket).map(|pnl| (ticket, pnl)))
            .filter(|(_, pnl)| *pnl < 0)
            .min_by_key(|(_, pnl)| *pnl)
            .map(|(ticket, _)| ticket)
    }
}
