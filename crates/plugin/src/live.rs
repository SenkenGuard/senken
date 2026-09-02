//! Shared decoding for a venue's live feed.
//!
//! Every [`VenueProtocol`](senken_subscription::VenueProtocol) turns venue
//! text into a [`PriceUpdate`](senken_subscription::PriceUpdate) or a
//! [`QuoteUpdate`](senken_subscription::QuoteUpdate), and the arithmetic to
//! do that is identical across venues while getting it wrong is quiet.
//! Two rules in particular are easy to lose one venue at a time, so they
//! live here once:
//!
//! - **A quote's two sides must share one scale.** `QuoteUpdate::new`
//!   rejects a bid of `"77590"` beside an ask of `"77590.5"`, and a caller
//!   that scales each side on its own hands it exactly that whenever the
//!   book's two sides happen to have different trailing digits — so the
//!   quote is dropped, silently, on the frames where the spread matters
//!   most. [`quote`](crate::live::quote) puts both sides on one scale before constructing it.
//! - **Scientific notation is a venue's choice, not a malformed value.**
//!   HTX reports `1.8E-4` for a size, so every value goes through
//!   [`senken_core::plain_decimal`] before its digits are counted.

use senken_core::{UnixNanos, parse_scaled, plain_decimal};
use senken_series::Volume;
use senken_subscription::{PriceUpdate, QuoteUpdate};

/// One venue-reported decimal as `(value, fractional digits)`.
///
/// Returns `None` when the text is not a decimal this project can hold
/// exactly — including a value with more fractional digits than an `i64`
/// can carry. Dropping it is the deliberate answer: rounding a price is
/// how money is lost quietly.
#[must_use]
pub fn scaled(value: &str) -> Option<(i64, u8)> {
    let plain = plain_decimal(value)?;
    let scale = senken_core::decimal_places(&plain);
    Some((parse_scaled(&plain, scale)?, scale))
}

/// The scale both `values` can share, or `None` when no single scale holds
/// every one of them exactly.
#[must_use]
fn shared_scale<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<u8> {
    let mut scale = 0u8;
    for value in values {
        let plain = plain_decimal(value)?;
        scale = scale.max(senken_core::decimal_places(&plain));
    }
    Some(scale)
}

/// One value re-read at `scale`, or `None` if it does not fit exactly.
#[must_use]
fn at_scale(value: &str, scale: u8) -> Option<i64> {
    parse_scaled(&plain_decimal(value)?, scale)
}

/// A last-trade update decoded from the venue's own `price` and `qty` text.
///
/// The price is required; the size is best effort. A venue whose size is
/// too precise to hold exactly still has a price worth publishing, and
/// dropping the whole tick for it would stall a chart on a value no
/// indicator needed — BingX reports sizes with nineteen fractional digits,
/// which is the case that found this. The size then arrives as
/// [`Volume::Absent`] — "the venue did not report a size this project can
/// hold" — never as a zero or a rounded stand-in.
#[must_use]
pub fn trade(ts: UnixNanos, price: &str, qty: &str) -> Option<PriceUpdate> {
    let (price, price_scale) = scaled(price)?;
    let (qty, qty_scale) = if let Some((value, scale)) = scaled(qty) {
        (Volume::Real(value), scale)
    } else {
        tracing::warn!(
            size = qty,
            "a live trade's size does not fit an exact scaled integer; publishing the price without it"
        );
        (Volume::Absent, 0)
    };
    Some(PriceUpdate {
        ts,
        price,
        price_scale,
        qty,
        qty_scale,
    })
}

/// A last-trade update for a venue that reports no size on a trade.
///
/// [`Volume::Absent`] rather than a zero: a bar with no volume and a bar
/// whose volume the venue did not report are different facts, and an
/// indicator reading the second as the first reports a flat line as if it
/// were real.
#[must_use]
pub fn trade_without_size(ts: UnixNanos, price: &str) -> Option<PriceUpdate> {
    let (price, price_scale) = scaled(price)?;
    Some(PriceUpdate {
        ts,
        price,
        price_scale,
        qty: Volume::Absent,
        qty_scale: 0,
    })
}

/// A best-bid-and-offer update with both sides forced onto one scale.
#[must_use]
pub fn quote(
    ts: UnixNanos,
    bid: &str,
    ask: &str,
    bid_size: &str,
    ask_size: &str,
) -> Option<QuoteUpdate> {
    let price_scale = shared_scale([bid, ask])?;
    let qty_scale = shared_scale([bid_size, ask_size])?;
    QuoteUpdate::new(
        ts,
        (at_scale(bid, price_scale)?, price_scale),
        (at_scale(ask, price_scale)?, price_scale),
        (at_scale(bid_size, qty_scale)?, qty_scale),
        (at_scale(ask_size, qty_scale)?, qty_scale),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::{quote, scaled, trade};
    use senken_core::UnixNanos;
    use senken_series::Volume;

    fn ts() -> UnixNanos {
        UnixNanos::from_millis(1_788_335_009_482).unwrap()
    }

    #[test]
    fn a_price_keeps_every_digit_the_venue_reported() {
        assert_eq!(scaled("77606.8"), Some((776_068, 1)));
        assert_eq!(scaled("0.00504905"), Some((504_905, 8)));
    }

    /// HTX reports trade sizes as `1.8E-4`. Counting the fractional digits
    /// of that literal gives 1, so a decoder that skips normalisation
    /// stores 1.8 units where the venue meant 0.00018.
    #[test]
    fn scientific_notation_is_normalised_before_its_digits_are_counted() {
        assert_eq!(scaled("1.8E-4"), Some((18, 5)));
        assert_eq!(scaled("4.5e3"), Some((4_500, 0)));
    }

    #[test]
    fn a_trade_carries_its_size_as_a_scaled_integer() {
        let update = trade(ts(), "77606.8", "0.000712").unwrap();
        assert_eq!(update.price, 776_068);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.qty, Volume::Real(712));
        assert_eq!(update.qty_scale, 6);
    }

    /// The bug this module exists to close: `QuoteUpdate::new` refuses two
    /// sides at different scales, so scaling each side independently drops
    /// every quote whose bid and ask have different trailing digits.
    #[test]
    fn a_quote_whose_sides_have_different_digits_is_still_delivered() {
        let update = quote(ts(), "77590", "77590.5", "0.5", "1.25").unwrap();
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.bid, 775_900);
        assert_eq!(update.ask, 775_905);
        assert_eq!(update.qty_scale, 2);
        assert_eq!(update.bid_size, 50);
        assert_eq!(update.ask_size, 125);
    }

    /// KuCoin has reported a 24h volume with twenty fractional digits.
    /// At that scale the integer no longer fits an `i64`, and the answer is
    /// to drop the value, never to round it down to something that fits.
    #[test]
    fn a_value_too_precise_to_hold_exactly_is_dropped_rather_than_rounded() {
        assert_eq!(scaled("77606.80000000000000000001"), None);
    }

    /// A price is what a chart cannot do without; a size it can. Dropping
    /// the whole tick because the venue's size is too precise would stall
    /// the price on a value no indicator needed.
    #[test]
    fn a_size_too_precise_to_hold_still_publishes_its_price() {
        let update = trade(ts(), "77495.49", "0.00744395815952752451234567890").unwrap();
        assert_eq!(update.price, 7_749_549);
        assert_eq!(update.qty, Volume::Absent);
    }

    #[test]
    fn a_price_too_precise_to_hold_yields_no_tick_at_all() {
        assert!(trade(ts(), "77495.4900000000000000001", "1").is_none());
    }

    #[test]
    fn text_that_is_not_a_decimal_yields_nothing() {
        assert_eq!(scaled(""), None);
        assert_eq!(scaled("n/a"), None);
    }
}
