//! How MetaTrader 5 charges margin, and the four figures every terminal
//! shows above the position list.
//!
//! Margin is **per symbol**, not per account: `SYMBOL_TRADE_CALC_MODE`
//! decides the formula, and the broker's own percentage and leverage are
//! inputs to it. A simulator that charged `notional / leverage` for
//! everything would be right for forex and wrong for every CFD and every
//! futures contract.

use senken_core::decimal::Scaled;
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_trade::TradeError;

/// How margin is charged for one symbol.
///
/// These are `ENUM_SYMBOL_CALC_MODE`'s retail cases. Not
/// `#[non_exhaustive]`: a mode this build has not been taught should fail
/// to compile at every site that charges margin, rather than falling
/// through to whichever formula happened to be last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalcMode {
    /// `Lots × ContractSize / Leverage`. Currency pairs.
    Forex,
    /// `Lots × ContractSize / 1`. Leverage is not applied at all.
    ForexNoLeverage,
    /// `Lots × ContractSize × Price × Percentage / 100`. Metals, indices.
    Cfd,
    /// `Lots × ContractSize × Price × Percentage / Leverage`.
    CfdLeverage,
    /// `Lots × InitialMargin × Percentage / 100`. Exchange futures.
    Futures {
        /// The broker's per-contract initial margin, at [`CASH_SCALE`].
        initial_margin: i64,
    },
}

/// The broker-set terms one symbol is margined under.
///
/// Every number here is read from the symbol specification on a real
/// account. None of them is an MT5 platform constant, so none of them has
/// a default this simulator may invent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolMargin {
    /// Which formula applies.
    pub mode: CalcMode,
    /// Units of the base instrument in one lot.
    pub contract_size: i64,
    /// The account's leverage, as a plain multiple.
    pub leverage: i64,
    /// The broker's margin requirement percentage for this symbol.
    pub percentage: i64,
}

/// Margin held against `lots` of a symbol at `price`, at [`CASH_SCALE`].
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn margin_for(terms: SymbolMargin, lots: Scaled, price: Scaled) -> Result<i64, TradeError> {
    let lots_units = i128::from(lots.value);
    let contract = i128::from(terms.contract_size);
    let leverage = i128::from(terms.leverage.max(1));
    let percentage = i128::from(terms.percentage);

    let raw = match terms.mode {
        CalcMode::Forex => lots_units * contract / leverage,
        CalcMode::ForexNoLeverage => lots_units * contract,
        CalcMode::Cfd => lots_units * contract * i128::from(price.value) * percentage / 100,
        CalcMode::CfdLeverage => {
            lots_units * contract * i128::from(price.value) * percentage / leverage
        }
        CalcMode::Futures { initial_margin } => {
            lots_units * i128::from(initial_margin) * percentage / 100
        }
    };

    // Lots carry their own scale, and a price-bearing mode carries the
    // price's as well; both are folded back to the account's cash scale.
    let from_scale = match terms.mode {
        CalcMode::Cfd | CalcMode::CfdLeverage => lots.scale.saturating_add(price.scale),
        CalcMode::Forex | CalcMode::ForexNoLeverage | CalcMode::Futures { .. } => lots.scale,
    };
    rescale(raw, from_scale, CASH_SCALE)
}

/// The four figures an MT5 terminal shows, computed the way MetaTrader
/// defines them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountFigures {
    /// Cash, excluding unrealised profit.
    pub balance: i64,
    /// Balance plus every open position's unrealised profit.
    pub equity: i64,
    /// Margin held against every open position.
    pub margin_used: i64,
    /// `Equity − Margin used`: what is left to open with.
    pub free_margin: i64,
}

impl AccountFigures {
    /// Balance and the totals derived from the open book.
    #[must_use]
    pub fn new(balance: i64, unrealized: i64, margin_used: i64) -> Self {
        let equity = balance.saturating_add(unrealized);
        Self {
            balance,
            equity,
            margin_used,
            free_margin: equity.saturating_sub(margin_used),
        }
    }

    /// `100 × Equity / Margin used`, as a percentage at two decimals.
    ///
    /// `None` when no margin is held: an account with nothing open has no
    /// margin level, and reporting either infinity or a zero that reads as
    /// "fully margin called" would be a different claim than the truth.
    #[must_use]
    pub fn margin_level(&self) -> Option<Scaled> {
        if self.margin_used == 0 {
            return None;
        }
        let level = i128::from(self.equity) * 10_000 / i128::from(self.margin_used);
        i64::try_from(level).ok().map(|value| Scaled::new(2, value))
    }
}
