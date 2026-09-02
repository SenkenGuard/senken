//! Commission, kept separate from swap and from market profit.
//!
//! MetaTrader gives every deal three distinct fields — `DEAL_COMMISSION`,
//! `DEAL_SWAP` and `DEAL_PROFIT` — and does not merge them, because a
//! trader legitimately wants the venue's fee, the funding cost and the
//! market result apart from each other. This module keeps that separation.
//!
//! MT5 publishes no commission *formula*: it is configured per symbol
//! group inside the broker's own server, and the platform only gives a
//! slot to report the resulting number into. So this is a configurable
//! model rather than "the MT5 commission formula", which would be an
//! invention.

use senken_core::decimal::Scaled;
use senken_sim_core::money::{basis_points, notional, rescale};
use senken_trade::TradeError;

/// How a broker charges commission.
///
/// Closed on purpose: a new model should fail to compile at every site
/// that charges one, rather than silently taking whichever branch came
/// last and billing the trader by a rule nobody chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommissionModel {
    /// None charged; the cost is in a widened spread instead, which is how
    /// many retail brokers price a standard account.
    None,
    /// A flat amount per lot traded, at the account's cash scale.
    PerLot {
        /// Charged per lot, each way.
        amount: i64,
    },
    /// Basis points of the traded notional.
    Notional {
        /// Basis points charged.
        bps: i64,
    },
}

/// Commission on one deal, at the account's cash scale.
///
/// Always a cost, never a credit: a negative commission is not a thing a
/// broker charges, and returning one would quietly pay the trader for
/// trading.
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn commission_for(
    model: CommissionModel,
    lots: Scaled,
    price: Scaled,
    contract_size: i64,
) -> Result<i64, TradeError> {
    let charged = match model {
        CommissionModel::None => 0,
        CommissionModel::PerLot { amount } => {
            rescale(i128::from(amount) * i128::from(lots.value), lots.scale, 0)?
        }
        CommissionModel::Notional { bps } => {
            let units = Scaled::new(lots.scale, lots.value.saturating_mul(contract_size));
            basis_points(notional(price, units)?, bps)?
        }
    };
    Ok(charged.abs())
}
