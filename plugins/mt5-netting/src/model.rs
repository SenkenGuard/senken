//! The netting account as a [`SettlementModel`](senken_sim_core::SettlementModel).

use senken_core::decimal::Scaled;
use senken_sim_core::money::{basis_points, notional, weighted_average};
use senken_sim_core::{FillContext, Settled, SettlementModel};
use senken_trade::{OrderSide, PositionSide, TradeError};

use crate::book::{NetPosition, NettingBook, Transition};

/// A MetaTrader 5 netting account's settlement rule.
#[derive(Debug, Clone, Copy)]
pub struct Netting {
    /// Commission charged on every fill, in basis points of notional.
    pub fee_bps: i64,
}

impl Netting {
    /// Folds one fill in and says which transition it caused.
    ///
    /// Separate from [`SettlementModel::settle`] so a caller that wants to
    /// know *what happened* — a history writer, a test — can have it,
    /// without widening the shared seam with a concept only netting has.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    pub fn apply(
        &self,
        book: &mut NettingBook,
        fill: &FillContext<'_>,
    ) -> Result<(Settled, Transition), TradeError> {
        let fee = basis_points(notional(fill.price, fill.quantity)?, self.fee_bps)?;
        let opening = match fill.side {
            OrderSide::Buy => PositionSide::Long,
            OrderSide::Sell => PositionSide::Short,
        };
        let settled = |realized: i64| Settled {
            fill_price: fill.price,
            fee,
            realized,
        };

        let Some(existing) = book.positions.get(fill.instrument).cloned() else {
            let ticket = book.next_ticket;
            book.next_ticket += 1;
            book.positions.insert(
                fill.instrument.clone(),
                NetPosition {
                    ticket,
                    // A fresh position's identifier is its opening
                    // ticket, and from here it never changes again.
                    identifier: ticket,
                    side: opening,
                    volume: fill.quantity,
                    entry: fill.price,
                    opened_at: fill.now,
                },
            );
            return Ok((settled(0), Transition::Opened));
        };

        if existing.side == opening {
            // Same direction: volume-weighted average, and the ticket does
            // not change. MetaTrader is explicit that an add keeps the
            // ticket, which is why history stays grouped across it.
            book.positions.insert(
                fill.instrument.clone(),
                NetPosition {
                    volume: Scaled::new(
                        existing.volume.scale,
                        existing.volume.value.saturating_add(fill.quantity.value),
                    ),
                    entry: weighted_average(
                        existing.entry,
                        existing.volume,
                        fill.price,
                        fill.quantity,
                    )?,
                    ..existing
                },
            );
            return Ok((settled(0), Transition::Added));
        }

        let closed = fill.quantity.value.min(existing.volume.value);
        let realized = close_pnl(
            existing.side,
            existing.entry,
            fill.price,
            Scaled::new(fill.quantity.scale, closed),
        )?;
        book.realized = book.realized.saturating_add(realized);
        let remaining = fill.quantity.value - closed;

        if remaining > 0 {
            // Reversal. The ticket becomes this order's own, because the
            // exposure that now exists was opened by it — but the
            // identifier survives, because every deal that ever belonged
            // to this position is keyed on it. The entry is this deal's
            // own price, not a weighted average: the closed leg and the
            // opened leg are priced independently even though MetaTrader
            // books them as one deal. And the open time resets, since the
            // surviving exposure did not exist at the old one.
            let ticket = book.next_ticket;
            book.next_ticket += 1;
            book.positions.insert(
                fill.instrument.clone(),
                NetPosition {
                    ticket,
                    identifier: existing.identifier,
                    side: opening,
                    volume: Scaled::new(fill.quantity.scale, remaining),
                    entry: fill.price,
                    opened_at: fill.now,
                },
            );
            return Ok((settled(realized), Transition::Reversed));
        }

        if existing.volume.value == closed {
            book.positions.remove(fill.instrument);
            return Ok((settled(realized), Transition::Closed));
        }

        // Partial reduce: profit is booked, and the entry price is left
        // alone. Moving it to the closing price would silently rewrite the
        // cost basis of everything still open.
        book.positions.insert(
            fill.instrument.clone(),
            NetPosition {
                volume: Scaled::new(existing.volume.scale, existing.volume.value - closed),
                ..existing
            },
        );
        Ok((settled(realized), Transition::Reduced))
    }
}

impl SettlementModel for Netting {
    type Book = NettingBook;

    fn settle(&self, book: &mut Self::Book, fill: &FillContext<'_>) -> Result<Settled, TradeError> {
        self.apply(book, fill).map(|(settled, _)| settled)
    }
}

/// Profit from closing `quantity` of a position opened at `entry` at
/// `exit`.
fn close_pnl(
    side: PositionSide,
    entry: Scaled,
    exit: Scaled,
    quantity: Scaled,
) -> Result<i64, TradeError> {
    let Some(exit) = exit.rescale(entry.scale) else {
        return Err(TradeError::InvalidRequest(
            "the entry and exit prices cannot be compared at one scale".to_owned(),
        ));
    };
    let difference = i128::from(exit.value) - i128::from(entry.value);
    let directed = match side {
        PositionSide::Long => difference,
        PositionSide::Short => -difference,
    };
    senken_sim_core::money::rescale(
        directed * i128::from(quantity.value),
        entry.scale.saturating_add(quantity.scale),
        senken_sim_core::money::CASH_SCALE,
    )
}
