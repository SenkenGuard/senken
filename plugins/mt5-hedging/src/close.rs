//! Closing a ticket: in part, in full, or against an opposite one.

use senken_core::decimal::Scaled;
use senken_trade::TradeError;

use crate::ticket::{HedgingBook, Ticket};

/// What closing part of a ticket left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialClose {
    /// How much was closed.
    pub closed: Scaled,
    /// What remains open on the same ticket, or `None` when the close was
    /// total.
    pub remaining: Option<Scaled>,
}

/// Closes `lots` of the ticket numbered `id`.
///
/// The remainder keeps **the same ticket number and the same open price**.
/// A partial close is not a close and a re-open: the position carries on,
/// which is why its entry price does not move to the closing price and its
/// swap keeps accruing against the original.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the ticket is unknown or the
/// volume asked for is larger than the ticket holds.
pub fn close_partial(
    book: &mut HedgingBook,
    id: u64,
    lots: Scaled,
) -> Result<PartialClose, TradeError> {
    let Some(index) = book.tickets.iter().position(|ticket| ticket.id == id) else {
        return Err(TradeError::InvalidRequest(format!(
            "ticket {id} is not open"
        )));
    };
    let held = book.tickets[index].lots;
    let Some(asked) = lots.rescale(held.scale) else {
        return Err(TradeError::InvalidRequest(
            "the volume asked for cannot be compared with the ticket's at one scale".to_owned(),
        ));
    };
    if asked.value > held.value {
        return Err(TradeError::InvalidRequest(format!(
            "ticket {id} holds less than the volume asked to close"
        )));
    }
    if asked.value == held.value {
        book.tickets.remove(index);
        return Ok(PartialClose {
            closed: held,
            remaining: None,
        });
    }
    let remaining = Scaled::new(held.scale, held.value - asked.value);
    book.tickets[index].lots = remaining;
    Ok(PartialClose {
        closed: asked,
        remaining: Some(remaining),
    })
}

/// Closes one ticket against an opposite one on the same symbol.
///
/// `TRADE_ACTION_CLOSE_BY` — a hedging account can settle a locked pair in
/// one operation rather than sending two market orders, so the trader
/// crosses the spread once instead of twice. The smaller volume closes
/// entirely; whatever the larger ticket has left stays open under its own
/// number.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when either ticket is unknown, they are
/// on different symbols, or they are on the same side — there is nothing
/// to close a position *by* except its opposite.
pub fn close_by(book: &mut HedgingBook, id: u64, against: u64) -> Result<Scaled, TradeError> {
    let find = |book: &HedgingBook, wanted: u64| -> Option<Ticket> {
        book.tickets
            .iter()
            .find(|ticket| ticket.id == wanted)
            .cloned()
    };
    let (Some(first), Some(second)) = (find(book, id), find(book, against)) else {
        return Err(TradeError::InvalidRequest(
            "both tickets must be open to close one by the other".to_owned(),
        ));
    };
    if first.instrument != second.instrument {
        return Err(TradeError::InvalidRequest(
            "a position can only be closed by one on the same symbol".to_owned(),
        ));
    }
    if first.side == second.side {
        return Err(TradeError::InvalidRequest(
            "a position is closed by its opposite, and these are on the same side".to_owned(),
        ));
    }
    let Some(other) = second.lots.rescale(first.lots.scale) else {
        return Err(TradeError::InvalidRequest(
            "the two tickets' volumes cannot be compared at one scale".to_owned(),
        ));
    };
    let matched = Scaled::new(first.lots.scale, first.lots.value.min(other.value));
    close_partial(book, id, matched)?;
    close_partial(book, against, matched)?;
    Ok(matched)
}
