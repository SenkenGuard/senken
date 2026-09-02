//! One simulated MT5 hedging account: its cash, its tickets, and the two
//! risk events a broker's server applies to it.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::risk::{ForcedClose, RiskBreach, RiskState};
use senken_trade::{PositionSide, TradeError};

use crate::margin::{AccountFigures, SymbolMargin, margin_for};
use crate::ticket::{HedgingBook, Ticket};

/// The two thresholds a broker configures, as percentages of margin level.
///
/// Both are broker-defined, not MetaTrader constants: there is no platform
/// default that applies everywhere, so an account that has not been told
/// its thresholds has none rather than inheriting an invented pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopLevels {
    /// Below this margin level, no new position may be opened. Nothing is
    /// closed.
    pub margin_call: Option<Scaled>,
    /// Below this margin level, the server starts closing losing
    /// positions.
    pub stop_out: Option<Scaled>,
}

/// Profit on one ticket if it closed at `price`, at [`CASH_SCALE`].
///
/// `(close − open) × ContractSize × Lots` for a long, mirrored for a
/// short — MetaTrader's own profit formula for the forex and CFD calc
/// modes.
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn ticket_profit(
    ticket: &Ticket,
    price: Scaled,
    contract_size: i64,
) -> Result<i64, TradeError> {
    let Some(price) = price.rescale(ticket.open_price.scale) else {
        return Err(TradeError::InvalidRequest(
            "the mark and the open price cannot be compared at one scale".to_owned(),
        ));
    };
    let difference = i128::from(price.value) - i128::from(ticket.open_price.value);
    let directed = match ticket.side {
        PositionSide::Long => difference,
        PositionSide::Short => -difference,
    };
    let raw = directed * i128::from(contract_size) * i128::from(ticket.lots.value);
    rescale(
        raw,
        ticket.open_price.scale.saturating_add(ticket.lots.scale),
        CASH_SCALE,
    )
}

/// What one symbol is margined and marked under, as the account knows it.
#[derive(Debug, Clone, Copy)]
pub struct SymbolTerms {
    /// The margin formula and its broker inputs.
    pub margin: SymbolMargin,
    /// The current mark, when one is available.
    pub mark: Option<Scaled>,
}

/// A hedging account's book plus the cash behind it.
#[derive(Debug, Clone, Default)]
pub struct Account {
    /// Cash, excluding unrealised profit.
    pub cash: i64,
    /// Every open ticket.
    pub book: HedgingBook,
}

impl Account {
    /// The account's risk state, given what each symbol is worth now.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    pub fn risk(
        &self,
        levels: StopLevels,
        terms: &dyn Fn(&InstrumentId) -> Option<SymbolTerms>,
    ) -> Result<RiskState, TradeError> {
        let mut unrealized = 0_i64;
        let mut margin_used = 0_i64;

        for ticket in &self.book.tickets {
            let Some(symbol) = terms(&ticket.instrument) else {
                continue;
            };
            margin_used = margin_used.saturating_add(ticket.margin);
            // Swap already charged sits on the ticket and is part of what
            // the position has cost, so equity carries it alongside the
            // market result.
            if let Some(mark) = symbol.mark {
                let profit = ticket_profit(ticket, mark, symbol.margin.contract_size)?;
                unrealized = unrealized.saturating_add(profit);
            }
            unrealized = unrealized.saturating_add(ticket.swap);
        }

        let figures = AccountFigures::new(self.cash, unrealized, margin_used);
        let level = figures.margin_level();
        Ok(RiskState {
            balance: figures.balance,
            equity: figures.equity,
            margin_used: figures.margin_used,
            margin_level: level,
            breach: breach_for(level, levels),
        })
    }

    /// Closes losing positions until the margin level recovers above the
    /// stop-out threshold, biggest loser first.
    ///
    /// This is the loop, not just the choice: MetaTrader closes one
    /// position, looks again, and repeats. Closing everything at once
    /// would liquidate an account that one close would have rescued, and
    /// closing only once would leave it under the threshold.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    pub fn apply_stop_out(
        &mut self,
        levels: StopLevels,
        terms: &dyn Fn(&InstrumentId) -> Option<SymbolTerms>,
        now: UnixNanos,
    ) -> Result<Vec<ForcedClose>, TradeError> {
        let mut closed = Vec::new();
        loop {
            let risk = self.risk(levels, terms)?;
            if risk.breach != Some(RiskBreach::ForcedClosure) {
                break;
            }
            let profit_of = |ticket: &Ticket| -> Option<i64> {
                let symbol = terms(&ticket.instrument)?;
                let mark = symbol.mark?;
                ticket_profit(ticket, mark, symbol.margin.contract_size).ok()
            };
            let Some(worst) = self.book.biggest_loser(&profit_of).map(|ticket| ticket.id) else {
                // Nothing is losing. A stop out closes losing positions, so
                // however low the level has fallen there is nothing for it
                // to do — reporting otherwise would invent a close.
                break;
            };
            let Some(index) = self.book.tickets.iter().position(|t| t.id == worst) else {
                break;
            };
            let ticket = self.book.tickets.remove(index);
            let symbol = terms(&ticket.instrument);
            let price = symbol.and_then(|s| s.mark).unwrap_or(ticket.open_price);
            let realized = symbol
                .map(|s| ticket_profit(&ticket, price, s.margin.contract_size))
                .transpose()?
                .unwrap_or(0)
                .saturating_add(ticket.swap);
            self.cash = self.cash.saturating_add(realized);
            closed.push(ForcedClose {
                position: ticket.id.to_string(),
                price,
                realized,
                reason: RiskBreach::ForcedClosure,
            });
            let _ = now;
        }
        Ok(closed)
    }

    /// Margin this account would hold against `lots` of a symbol, added to
    /// what it already holds.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    pub fn margin_if_opened(
        &self,
        symbol: SymbolTerms,
        lots: Scaled,
        price: Scaled,
    ) -> Result<i64, TradeError> {
        margin_for(symbol.margin, lots, price)
    }
}

/// Which threshold `level` has crossed, if any.
///
/// Stop out is checked first: an account under both thresholds is being
/// closed out, and reporting only that it cannot open would understate
/// what is happening to it.
fn breach_for(level: Option<Scaled>, levels: StopLevels) -> Option<RiskBreach> {
    let level = level?;
    let below = |threshold: Option<Scaled>| -> bool {
        threshold.is_some_and(|threshold| {
            threshold
                .rescale(level.scale)
                .is_some_and(|threshold| level.value < threshold.value)
        })
    };
    if below(levels.stop_out) {
        return Some(RiskBreach::ForcedClosure);
    }
    if below(levels.margin_call) {
        return Some(RiskBreach::OpeningBlocked);
    }
    None
}
