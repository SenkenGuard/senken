//! Swap, charged per open position once for every trading day it is held
//! through rollover.
//!
//! Two things here are routinely got wrong and are worth stating. A long
//! and a short on the same symbol almost always carry **different** rates,
//! often with opposite signs — that is the FX carry mechanic, not a
//! rounding artefact. And the triple-charge day is
//! `SYMBOL_SWAP_ROLLOVER3DAYS`, a per-symbol broker setting: Wednesday is
//! a common choice for covering the weekend value date, not a platform
//! rule, so it is read rather than assumed.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_trade::{PositionSide, TradeError};

/// Nanoseconds in one day.
const DAY_NANOS: i64 = 86_400 * 1_000_000_000;
/// 1970-01-01 was a Thursday, so day 0 is weekday 3 counting from Monday.
const EPOCH_WEEKDAY: i64 = 3;

/// How swap is calculated for one symbol.
///
/// `ENUM_SYMBOL_SWAP_MODE`'s cases. Closed on purpose: a mode this build
/// has not been taught should fail to compile at every site that charges
/// swap rather than silently taking whichever branch came last, because
/// the difference between "points" and "annual interest" is orders of
/// magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapMode {
    /// No swap at all.
    Disabled,
    /// A number of points, applied against the position's volume.
    Points,
    /// A money amount per lot, in the account's deposit currency.
    CurrencyDeposit,
    /// An annual interest rate applied to the position's current value.
    InterestCurrent,
    /// An annual interest rate applied to the position's open price.
    InterestOpen,
}

/// The broker's swap configuration for one symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapTerms {
    /// Which formula applies.
    pub mode: SwapMode,
    /// The rate a long position is charged, per lot per night. Negative is
    /// a charge, positive a credit — brokers publish both.
    pub long_rate: i64,
    /// The rate a short position is charged. Routinely a different number
    /// from `long_rate`, and often the opposite sign.
    pub short_rate: i64,
    /// Scale the two rates are expressed at.
    pub rate_scale: u8,
    /// Which weekday the broker charges three days of swap on, counting
    /// Monday as 0. `None` when the broker charges none.
    pub rollover3_weekday: Option<u8>,
    /// Units of the instrument in one lot, for the modes that need it.
    pub contract_size: i64,
}

/// Which weekday `instant` falls on, Monday as 0.
#[must_use]
pub fn weekday(instant: UnixNanos) -> u8 {
    let days = instant.as_nanos().div_euclid(DAY_NANOS);
    u8::try_from((days + EPOCH_WEEKDAY).rem_euclid(7)).unwrap_or(0)
}

/// How many days of swap accrue between `from` and `to`.
///
/// One per rollover crossed, except the broker's three-day weekday, which
/// counts for three. A range that crosses no rollover accrues nothing, so
/// reading an account twice in one day does not charge it twice.
#[must_use]
pub fn swap_days(terms: SwapTerms, from: UnixNanos, to: UnixNanos) -> i64 {
    let first = from.as_nanos().div_euclid(DAY_NANOS);
    let last = to.as_nanos().div_euclid(DAY_NANOS);
    if last <= first {
        return 0;
    }
    ((first + 1)..=last)
        .map(|day| {
            let weekday = u8::try_from((day + EPOCH_WEEKDAY).rem_euclid(7)).unwrap_or(0);
            if terms.rollover3_weekday == Some(weekday) {
                3
            } else {
                1
            }
        })
        .sum()
}

/// Swap charged on one position for `days` of rollover, at [`CASH_SCALE`].
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn swap_for(
    terms: SwapTerms,
    side: PositionSide,
    lots: Scaled,
    price: Scaled,
    days: i64,
) -> Result<i64, TradeError> {
    if matches!(terms.mode, SwapMode::Disabled) || days == 0 {
        return Ok(0);
    }
    let rate = i128::from(match side {
        PositionSide::Long => terms.long_rate,
        PositionSide::Short => terms.short_rate,
    });
    let lots_units = i128::from(lots.value);
    let days = i128::from(days);

    let (raw, from_scale) = match terms.mode {
        SwapMode::Disabled => (0, CASH_SCALE),
        SwapMode::CurrencyDeposit => (
            rate * lots_units * days,
            terms.rate_scale.saturating_add(lots.scale),
        ),
        SwapMode::Points => (
            rate * lots_units * i128::from(terms.contract_size) * days,
            terms.rate_scale.saturating_add(lots.scale),
        ),
        // An annual rate, so a day is a 360th of it — the day-count
        // convention brokers use for FX swap.
        SwapMode::InterestCurrent | SwapMode::InterestOpen => (
            rate * lots_units * i128::from(terms.contract_size) * i128::from(price.value) * days
                / (360 * 100),
            terms
                .rate_scale
                .saturating_add(lots.scale)
                .saturating_add(price.scale),
        ),
    };
    rescale(raw, from_scale, CASH_SCALE)
}
