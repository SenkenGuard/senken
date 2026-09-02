//! Fixed-point arithmetic for the simulated books.
//!
//! Every function here works in `i128` and lands back on an `i64` at a
//! declared scale. Nothing here touches a float: a simulated balance is
//! still a balance, and a paper account whose arithmetic drifts teaches
//! its user the wrong thing about their strategy.
//!
//! It lives in the kernel rather than in one adapter because a fee
//! rounding fixed in one simulator and not in another is a bug nobody can
//! see, in the one part of this application where a wrong number is
//! money.

use senken_core::decimal::Scaled;
use senken_trade::TradeError;

/// Decimal places the simulator keeps cash, fees and profit at.
///
/// Eight, not two: a fee of four basis points on a small position is far
/// below a cent, and rounding each one to a cent would make the fee either
/// free or a hundred times too big depending on which way it went.
pub const CASH_SCALE: u8 = 8;

/// Basis points in one whole unit.
pub const BPS_DIVISOR: i128 = 10_000;

/// Re-expresses `value` from scale `from` to scale `to`, truncating toward
/// zero when it narrows.
///
/// Truncation is stated rather than hidden: the discarded digits are below
/// `CASH_SCALE`, i.e. under a hundred-millionth of the account currency,
/// and truncating toward zero means the direction of the error does not
/// depend on the sign of the value.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the result does not fit an `i64`.
pub fn rescale(value: i128, from: u8, to: u8) -> Result<i64, TradeError> {
    let shifted = if to >= from {
        value
            .checked_mul(pow10(u32::from(to - from))?)
            .ok_or_else(|| TradeError::invalid("value is too large to represent"))?
    } else {
        value / pow10(u32::from(from - to))?
    };
    i64::try_from(shifted).map_err(|_| TradeError::invalid("value is too large to represent"))
}

fn pow10(exponent: u32) -> Result<i128, TradeError> {
    10_i128
        .checked_pow(exponent)
        .ok_or_else(|| TradeError::invalid("value has an unusable scale"))
}

/// `price × quantity`, at [`CASH_SCALE`].
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the product does not fit.
pub fn notional(price: Scaled, quantity: Scaled) -> Result<i64, TradeError> {
    let product = i128::from(price.value) * i128::from(quantity.value);
    rescale(product, price.scale + quantity.scale, CASH_SCALE)
}

/// `amount × bps / 10000`, at [`CASH_SCALE`], always non-negative.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the result does not fit.
pub fn basis_points(amount: i64, bps: i64) -> Result<i64, TradeError> {
    let scaled = i128::from(amount.abs()) * i128::from(bps) / BPS_DIVISOR;
    i64::try_from(scaled).map_err(|_| TradeError::invalid("fee is too large to represent"))
}

/// Moves `price` by `bps` basis points in the direction `sign` gives.
///
/// This is the simulator's whole slippage model, and it is deliberately the
/// simplest one that is honest: a taker always pays a little worse than the
/// mark, never better. Modelling a real book would need depth the simulator
/// does not have, and pretending to have it would be worse than saying so.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the result does not fit.
pub fn slip(price: Scaled, bps: i64, sign: i64) -> Result<Scaled, TradeError> {
    let delta = i128::from(price.value) * i128::from(bps) / BPS_DIVISOR;
    let moved = i128::from(price.value) + delta * i128::from(sign);
    let value = i64::try_from(moved.max(1))
        .map_err(|_| TradeError::invalid("price is too large to represent"))?;
    Ok(Scaled::new(price.scale, value))
}

/// The volume-weighted average of `(price_a, qty_a)` and `(price_b, qty_b)`,
/// at `price_a`'s scale.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the scales cannot be reconciled or
/// the result does not fit.
pub fn weighted_average(
    price_a: Scaled,
    qty_a: Scaled,
    price_b: Scaled,
    qty_b: Scaled,
) -> Result<Scaled, TradeError> {
    let total = i128::from(qty_a.value) + i128::from(qty_b.value);
    if total == 0 {
        return Ok(price_a);
    }
    let weighted = i128::from(price_a.value) * i128::from(qty_a.value)
        + i128::from(price_b.value) * i128::from(qty_b.value);
    let value = i64::try_from(weighted / total)
        .map_err(|_| TradeError::invalid("price is too large to represent"))?;
    Ok(Scaled::new(price_a.scale, value))
}

#[cfg(test)]
mod tests {
    use super::{CASH_SCALE, basis_points, notional, rescale, slip, weighted_average};
    use senken_core::decimal::Scaled;

    #[test]
    fn a_notional_multiplies_the_two_scales_and_lands_on_the_cash_scale() {
        // 0.25 BTC at 68_420.00 is 17_105.00.
        let value = notional(Scaled::new(2, 6_842_000), Scaled::new(3, 250)).unwrap();
        assert_eq!(value, 1_710_500_000_000);
        assert_eq!(
            rescale(i128::from(value), CASH_SCALE, 2).unwrap(),
            1_710_500
        );
    }

    #[test]
    fn a_fee_in_basis_points_is_computed_without_a_float() {
        // Four basis points of 17_105.00 is 6.842.
        let fee = basis_points(1_710_500_000_000, 4).unwrap();
        assert_eq!(fee, 684_200_000);
    }

    #[test]
    fn a_fee_is_never_negative_even_on_a_negative_amount() {
        assert_eq!(
            basis_points(-1_000_000_000, 10).unwrap(),
            basis_points(1_000_000_000, 10).unwrap()
        );
    }

    #[test]
    fn slippage_always_costs_the_taker_whichever_way_they_trade() {
        let mark = Scaled::new(2, 10_000);
        let buy = slip(mark, 5, 1).unwrap();
        let sell = slip(mark, 5, -1).unwrap();
        assert!(buy.value > mark.value, "a buyer pays above the mark");
        assert!(sell.value < mark.value, "a seller receives below the mark");
    }

    #[test]
    fn a_weighted_average_is_the_size_weighted_one_not_the_midpoint() {
        // 3 at 100 and 1 at 200 averages 125, not 150.
        let avg = weighted_average(
            Scaled::new(2, 10_000),
            Scaled::new(0, 3),
            Scaled::new(2, 20_000),
            Scaled::new(0, 1),
        )
        .unwrap();
        assert_eq!(avg, Scaled::new(2, 12_500));
    }

    #[test]
    fn narrowing_a_scale_truncates_toward_zero_in_both_directions() {
        assert_eq!(rescale(199, 2, 0).unwrap(), 1);
        assert_eq!(rescale(-199, 2, 0).unwrap(), -1);
    }
}
