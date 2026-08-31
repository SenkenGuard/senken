//! The boundary conversion between [`Bar`](senken_series::Bar)'s
//! scaled-integer fields and the `f64` every indicator in this crate
//! computes with.

/// Converts a scaled-integer price or quantity from a
/// [`Bar`](senken_series::Bar) into the `f64` an indicator computes with.
///
/// This is the hard boundary drawn here, restated here because it is
/// the reason this crate is allowed to use `f64` at all when the rest of
/// the workspace never does: an indicator *value* may be `f64` because it
/// is a display/decision value, fractional by nature (an EMA, an RSI, a
/// standard deviation are none of them money). `Bar`'s own fields stay
/// scaled integers — the scale lives on the series' file metadata, not on
/// the bar — so widening them for indicator arithmetic is this crate's job,
/// not `senken-series`'s.
///
/// The boundary is one-directional and hard: nothing in this crate ever
/// produces an order price. If a future caller ever wants to turn an
/// indicator's `f64` output back into something that trades, it must round
/// that value to the instrument's tick and re-enter the scaled-integer
/// world first — this crate does not do that rounding itself, because it
/// has no notion of an instrument's tick size to round to.
///
/// # Where this stops being exact
///
/// `f64` represents every integer up to **2^53** (9,007,199,254,740,992)
/// exactly, and rounds above it. That is what `clippy::cast_precision_loss`
/// warns about, and the lint is allowed for this crate rather than for the
/// workspace precisely so the limit is a known one rather than a silenced
/// one.
///
/// In practice the headroom is wide but not infinite: a quantity at
/// `qty_scale = 8` passes 2^53 at about 90 million units, which a very
/// large trade in a low-priced asset can reach. Beyond that an indicator's
/// input is rounded — by roughly one part in 10^16, which is far below any
/// threshold a chart or a signal reacts to, and is why this is acceptable
/// for indicator arithmetic and would not be for money.
#[must_use]
pub(crate) fn scaled_to_f64(scaled: i64) -> f64 {
    scaled as f64
}

#[cfg(test)]
mod tests {
    use super::scaled_to_f64;

    #[test]
    fn scaled_to_f64_is_a_plain_widening_conversion() {
        // Every value here is exactly representable in `f64`, so comparing
        // bit patterns (rather than `==`, which invites float rounding
        // bugs elsewhere) proves the conversion is exact, not merely
        // close.
        assert_eq!(scaled_to_f64(0).to_bits(), 0.0_f64.to_bits());
        assert_eq!(scaled_to_f64(150_000).to_bits(), 150_000.0_f64.to_bits());
        assert_eq!(scaled_to_f64(-42).to_bits(), (-42.0_f64).to_bits());
    }

    #[test]
    fn the_exactness_limit_is_two_to_the_fifty_third() {
        // Documented in `scaled_to_f64`'s own docs, asserted here so the
        // claim cannot quietly become false: everything up to 2^53 round
        // trips exactly, and 2^53 + 1 does not.
        const LIMIT: i64 = 1 << 53;
        // Compared in `f64` rather than cast back to `i64`: casting the other
        // way is itself a truncating conversion, and asserting the limit must
        // not depend on the very operation the limit describes.
        // Bit patterns, matching the test above: `assert_eq!` on two `f64`s
        // is what `clippy::float_cmp` exists to stop, and an exactness claim
        // should be exact anyway.
        assert_ne!(
            scaled_to_f64(LIMIT - 1).to_bits(),
            scaled_to_f64(LIMIT).to_bits()
        );
        // 2^53 + 1 has no `f64` representation, so it rounds onto 2^53 — the
        // first integer where the mapping stops being one-to-one.
        assert_eq!(
            scaled_to_f64(LIMIT + 1).to_bits(),
            scaled_to_f64(LIMIT).to_bits()
        );
    }
}
