//! The hedging account as a [`SettlementModel`](senken_sim_core::SettlementModel).
//!
//! Implementing the kernel's seam rather than only calling into it is what
//! makes this system substitutable for the other three: a caller that
//! holds a `SettlementModel` can settle a fill, measure risk, enforce a
//! breach and accrue time without knowing which of the four it has.

use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_sim_core::risk::{ForcedClose, RiskState};
use senken_sim_core::{FillContext, Marks, Settled, SettlementModel};
use senken_trade::{OrderSide, PositionSide, TradeError};

use crate::account::{Account, StopLevels, SymbolTerms};
use crate::commission::{CommissionModel, commission_for};
use crate::margin::{SymbolMargin, margin_for};
use crate::swap::{SwapTerms, swap_days, swap_for};
use crate::ticket::Ticket;

/// A MetaTrader 5 hedging account's settlement rules.
#[derive(Debug, Clone, Copy)]
pub struct Hedging {
    /// How margin is charged for the symbol being traded.
    pub margin: SymbolMargin,
    /// The two thresholds this account's broker applies.
    pub levels: StopLevels,
    /// How swap accrues.
    pub swap: SwapTerms,
    /// How commission is charged.
    pub commission: CommissionModel,
}

impl Hedging {
    fn terms_for(self, marks: &Marks) -> impl Fn(&InstrumentId) -> Option<SymbolTerms> + '_ {
        move |instrument: &InstrumentId| {
            Some(SymbolTerms {
                margin: self.margin,
                mark: marks.get(&instrument.to_string()).copied(),
            })
        }
    }
}

impl SettlementModel for Hedging {
    type Book = Account;

    /// Every deal opens its own ticket. Nothing merges, nothing nets, and
    /// a buy while short does not reduce the short — that is precisely
    /// what separates a hedging account from a netting one.
    fn settle(&self, book: &mut Self::Book, fill: &FillContext<'_>) -> Result<Settled, TradeError> {
        let commission = commission_for(
            self.commission,
            fill.quantity,
            fill.price,
            self.margin.contract_size,
        )?;
        let margin = margin_for(self.margin, fill.quantity, fill.price)?;
        let id = book.book.next_ticket;
        book.book.next_ticket += 1;
        book.book.tickets.push(Ticket {
            id,
            instrument: fill.instrument.clone(),
            side: match fill.side {
                OrderSide::Buy => PositionSide::Long,
                OrderSide::Sell => PositionSide::Short,
            },
            lots: fill.quantity,
            open_price: fill.price,
            stop_loss: None,
            take_profit: None,
            swap: 0,
            margin,
            opened_at: fill.now,
        });
        Ok(Settled {
            fill_price: fill.price,
            fee: commission,
            // Opening a ticket realises nothing: on a hedging account a
            // profit is only realised when a ticket is closed, and no
            // fill ever closes one implicitly.
            realized: 0,
        })
    }

    fn risk(&self, book: &Self::Book, marks: &Marks) -> Result<RiskState, TradeError> {
        book.risk(self.levels, &self.terms_for(marks))
    }

    fn enforce(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        now: UnixNanos,
    ) -> Result<Vec<ForcedClose>, TradeError> {
        let terms = self.terms_for(marks);
        book.apply_stop_out(self.levels, &terms, now)
    }

    fn accrue(
        &self,
        book: &mut Self::Book,
        marks: &Marks,
        from: UnixNanos,
        to: UnixNanos,
    ) -> Result<i64, TradeError> {
        let days = swap_days(self.swap, from, to);
        if days == 0 {
            return Ok(0);
        }
        let mut total = 0_i64;
        for ticket in &mut book.book.tickets {
            let price = marks
                .get(&ticket.instrument.to_string())
                .copied()
                .unwrap_or(ticket.open_price);
            let charged = swap_for(self.swap, ticket.side, ticket.lots, price, days)?;
            ticket.swap = ticket.swap.saturating_add(charged);
            total = total.saturating_add(charged);
        }
        Ok(total)
    }
}
