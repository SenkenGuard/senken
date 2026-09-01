//! [`PriceUpdate`] — the one thing a [`crate::Lease`] ever receives.

use senken_core::UnixNanos;
use senken_series::Volume;

/// One last-price update for a leased instrument, delivered to every
/// current leaseholder of it ("anything holding a lease receives updates; that is the whole contract").
///
/// **Not a bar.** For kline streams, whether the
/// newest row is closed or still forming is a per-venue fact that must never
/// be assumed — Binance gives no flag at all, OKX's `confirm` field exists
/// only for candles. A `PriceUpdate` sidesteps that trap entirely rather
/// than getting it wrong: it is one instant's last traded price, with no
/// interval to close and therefore nothing that could be mistaken for a
/// closed or forming candle. A consumer that wants a bar still has to build
/// one itself (out of scope here — a
/// derived series is never persisted).
///
/// Carries its own scale rather than assuming one: unlike a stored `Bar`
/// (`senken-series`), whose scale lives in the series' stored file metadata,
/// a live tick has no file backing it, so the scale travels with the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceUpdate {
    /// When the venue reported this price. This crate never reads a wall
    /// clock to produce one: whichever [`crate::VenueConnection`] decoded
    /// the venue's message supplies it, decoded from the venue's own data
    /// where the venue provides one (OKX's public trades channel timestamps
    /// every trade itself, verified live) or, failing that, from that
    /// connection's own `senken_series::Clock` rather than a direct
    /// `SystemTime`/`Instant` read.
    pub ts: UnixNanos,
    /// The last traded price, at `price_scale` fractional digits.
    pub price: i64,
    /// How many of `price`'s digits are fractional. A quoted price `p` is
    /// `price × 10^-price_scale`, the same convention
    /// [`Instrument::price_scale`](senken_marketdata::Instrument::price_scale)
    /// uses.
    pub price_scale: u8,
    /// Base-asset quantity traded at `price`, at `qty_scale` fractional
    /// digits.
    ///
    /// Carried because a bar is OHLC**V**: an indicator reading volume — a
    /// VWAP, a volume histogram — is fed from the same stream as one reading
    /// price, and a tick that dropped its size would force every such
    /// indicator to stop at the last stored bar.
    pub qty: Volume,
    /// How many of `qty`'s digits are fractional.
    pub qty_scale: u8,
}
