//! [`SymbolMap`] — resolving Senken's normalised symbol back to a venue's
//! own wire form.

use senken_marketdata::InstrumentId;

/// Translates a normalised [`InstrumentId`] into the literal symbol string a
/// venue's subscribe frame expects.
///
/// `InstrumentId::symbol()` is Senken's own normalised, cross-venue form —
/// separators stripped, upper-cased (the "symbol trap" finding:
/// a fetch call must never be handed this form by mistake, only a venue's
/// *native* symbol). OKX's own wire format wants the native form back:
/// subscribing to `BTC-USDT` (with the dash) was confirmed live
/// 2026-08-31, while Senken's own `InstrumentId` for that instrument holds
/// the normalised `BTCUSDT`. Recovering one from the other is not a pure
/// function of the normalised string alone — `senken_venue::normalise_symbol`
/// only strips separators, and which characters were separators to begin
/// with is exactly the fact that got stripped — so this crate does not
/// attempt it. `Instrument::source_symbol` already exists
/// for this: a real deployment resolves one through whatever catalog
/// already tracks it. This trait is that seam; this crate has no catalog of
/// its own to look one up in.
pub trait SymbolMap: Send + Sync + 'static {
    /// The venue-native symbol for `instrument`, or `None` if this map has
    /// no answer for it (an unknown instrument, or one from a different
    /// venue than this map was built for).
    fn source_symbol(&self, instrument: &InstrumentId) -> Option<String>;
}

/// Assumes a venue's own wire symbol is identical to Senken's normalised
/// [`InstrumentId::symbol`] — true only for a venue whose native symbols
/// already contain no separator this crate would otherwise need to
/// reconstruct (unverified for any real venue; **not** true for OKX, whose
/// verified wire form keeps the dash `normalise_symbol` strips). Exists for
/// tests and for a venue that genuinely needs no translation, never as a
/// default assumed correct for a specific venue.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentitySymbolMap;

impl SymbolMap for IdentitySymbolMap {
    fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
        Some(instrument.symbol().to_owned())
    }
}
