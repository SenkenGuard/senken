//! Volume in lots, and the four per-symbol limits an order is checked
//! against before it reaches the book.
//!
//! MetaTrader states volume in lots, not units, and every broker publishes
//! a minimum, a maximum and a step. An order that does not sit exactly on
//! the step is rejected rather than rounded: rounding it would fill the
//! trader at a size they did not ask for, which on a leveraged account is
//! a different risk than the one they sized.

use senken_core::decimal::Scaled;

/// The broker's volume limits for one symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeLimits {
    /// Smallest tradable volume, in lots.
    pub min: Scaled,
    /// Largest volume in a single order, in lots.
    pub max: Scaled,
    /// The increment volume must be an exact multiple of.
    pub step: Scaled,
    /// The most that may be open and pending on this symbol at once,
    /// across every ticket. `None` when the broker sets no such cap.
    pub limit: Option<Scaled>,
}

/// Why a requested volume is not tradable.
///
/// Closed on purpose: a new rejection reason should fail to compile at
/// every site that explains one to a trader, rather than falling into a
/// catch-all that reports the wrong cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeRejection {
    /// Below the symbol's minimum.
    BelowMinimum,
    /// Above the symbol's per-order maximum.
    AboveMaximum,
    /// Not an exact multiple of the step.
    OffStep,
    /// Would take the account past the symbol's total volume limit.
    OverSymbolLimit,
}

/// Checks `lots` against `limits`, with `already_open` already held on the
/// symbol.
///
/// # Errors
/// The specific [`VolumeRejection`], so the message a trader reads names
/// the actual cause rather than "invalid volume".
pub fn check(
    limits: VolumeLimits,
    lots: Scaled,
    already_open: Scaled,
) -> Result<(), VolumeRejection> {
    let at = |value: Scaled| -> i128 {
        // Compared at the finest of the scales involved, so a 0.01 step
        // and a 0.001 volume are still comparable.
        i128::from(value.value) * 10_i128.pow(u32::from(8_u8.saturating_sub(value.scale)))
    };

    if at(lots) < at(limits.min) {
        return Err(VolumeRejection::BelowMinimum);
    }
    if at(lots) > at(limits.max) {
        return Err(VolumeRejection::AboveMaximum);
    }
    let step = at(limits.step);
    if step > 0 && at(lots) % step != 0 {
        return Err(VolumeRejection::OffStep);
    }
    if let Some(limit) = limits.limit
        && at(already_open) + at(lots) > at(limit)
    {
        return Err(VolumeRejection::OverSymbolLimit);
    }
    Ok(())
}
