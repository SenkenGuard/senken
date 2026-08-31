//! [`BarSource`] — the contract a venue plugin implements to supply bars
//! The bar-fetching port a plugin implements.
//!
//! A real venue implementation satisfies this trait and registers through
//! [`ActivationContext::register_bar_source`](crate::ActivationContext::register_bar_source),
//! mirroring
//! [`register_marketdata_source`](crate::ActivationContext::register_marketdata_source)
//! exactly: `senken-marketdata` stays blind to market types, and this crate
//! stays blind to bar-fetching policy — it only ever sees whatever a
//! plugin decides to register.
//!
//! # A deliberately different trait from `senken-loader`'s
//!
//! `senken-loader`'s resolution ladder predates this trait and
//! was built and tested against its own small internal fetch port
//! (`senken_loader::BarSource`, defined before M7 existed). That port has
//! no `supported()` — the ladder already knows which spec to fetch by the
//! time it calls in, decided once at construction — and a deliberately
//! small `FetchError` rather than `SourceError`, "widening it to carry
//! HTTP status codes or transport detail" being, in that crate's own M6
//! report, explicitly "an M7 concern once a real implementation exists to
//! need them." This is that M7 concern, and this trait is *not* a
//! replacement for that one: they answer different questions for different
//! callers, and they need only not silently drift
//! apart while both exist. `senken_loader::PluginBarSource` is the one
//! documented adapter that lets a `BarSource` registered here also satisfy
//! `senken-loader`'s port — see that type's docs for why the adapter runs
//! in that direction and not the other.
//!
//! # Cross-venue traps
//!
//! A real implementation's module docs must state its answer to all five:
//! sort direction, timestamp representation, closed-candle detection, the
//! *tested* row cap, and pagination direction.
//!
//! # The symbol trap is unrepresentable
//!
//! [`BarSource::bars`] takes a
//! [`SourceSymbol`](senken_marketdata::SourceSymbol), not a plain `&str`:
//! the venue's own identifier (OKX's `BTC-USDT`), never the cross-venue
//! normalised form (`BTCUSDT`) — `senken_marketdata`'s `normalise_symbol`
//! discards separator *position*, so the reverse conversion does not exist
//! in general. Passing the normalised form fails outright on a
//! separator-using venue but **silently succeeds** wherever the two forms
//! coincide (Binance's own wire format already equals its normalised
//! symbol), which makes the mistake look venue-specific and is miserable to
//! diagnose from a bug report alone. A
//! [`SourceSymbol`](senken_marketdata::SourceSymbol) is obtainable only from
//! [`Instrument::source_symbol()`](senken_marketdata::Instrument::source_symbol),
//! so a caller that reaches for the normalised symbol instead gets a compile
//! error, not a runtime one.

use async_trait::async_trait;
use senken_core::TimeRange;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_series::{Bar, BarSpec};

/// Fetches bars for one venue source.
///
/// Implementors talk to exactly one venue market (or, in every plugin's own
/// tests, a wiremock stand-in) and know nothing about caching, gap
/// planning, single-flight or jobs — all of that belongs to whatever
/// resolves against the registered sources (`senken-loader`).
#[async_trait]
pub trait BarSource: Send + Sync {
    /// The source id this fetches for, matching the `source_id` half of
    /// every [`senken_series::SeriesKey`] a caller addresses through it —
    /// e.g. `binance-spot`.
    fn source_id(&self) -> &str;

    /// The bar specs this source can fetch directly from the venue, for a
    /// caller (a symbol/timeframe picker, a registration inspector)
    /// deciding what to offer *before* trying a fetch that would just be
    /// rejected.
    fn supported(&self) -> &[BarSpec];

    /// The largest number of bars one [`Self::bars`] call may return — the
    /// **tested**, not documented, cap (Binance spot silently
    /// caps `limit` at 1000 and returns HTTP 200 for `limit=1500`, losing
    /// data with no error for an implementation that trusted the docs).
    fn max_rows(&self) -> usize;

    /// Fetches every **closed** bar of `spec` for `symbol` inside `range`,
    /// ascending by `ts_open` regardless of the order the venue itself
    /// returns them in.
    ///
    /// `symbol` is the venue-native identifier (see this module's docs on
    /// why it is a [`SourceSymbol`], not a plain `&str`) — obtain it from
    /// [`Instrument::source_symbol()`](senken_marketdata::Instrument::source_symbol),
    /// never from the normalised `Instrument::symbol`.
    ///
    /// An implementation must have already dropped any unclosed candle
    /// (OKX flags one with `confirm == "0"`, verified present
    /// even on the history endpoint; Binance and Bybit carry no such flag,
    /// so closure is determined by comparing a close time — or, for Bybit,
    /// the response's own server `time` — against what "now" means for
    /// this call) before returning; persisting an unclosed candle corrupts
    /// the series permanently, and nothing downstream can detect it.
    ///
    /// # Errors
    /// [`SourceError`], whose [`SourceError::is_retryable`] tells the
    /// caller whether trying again is worth it.
    async fn bars(
        &self,
        symbol: &SourceSymbol,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, SourceError>;
}

impl std::fmt::Debug for dyn BarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarSource")
            .field("source_id", &self.source_id())
            .field("max_rows", &self.max_rows())
            .finish()
    }
}
