//! The per-plugin live on/off switch, installed once by the runtime around
//! every capability a static plugin registers — never by the plugin
//! itself.
//!
//! `senken_plugin::ActivationContext::register_*` only records what a
//! plugin contributes; nothing about that call requires a plugin to also
//! wire up its own enabled/disabled check, and a plugin author who forgot
//! would compile clean and ship a toggle that silently does nothing. This
//! module is the fix: `crate::drain_registrations` wraps every source it
//! takes out of the context with the flag below before handing it to
//! `senken-marketdata`, `senken-loader` or `senken-subscription` — a
//! plugin never sees the wrapper and cannot skip it.
//!
//! The gated behaviour mirrors `plugin_host::DynamicVenueSource` exactly
//! for the two capabilities that type already covers: an instrument
//! catalog goes empty, and a bar fetch is refused. The same reasoning
//! extends to order-book depth (refused, like a bar fetch — see
//! [`GatedBookSource`]) and the live feed (see [`GatedFeedSource`] and
//! [`GatedVenueProtocol`] for why the live feed needs its own argument).
//!
//! A registered [`senken_trade::TradeAdapter`] is deliberately never
//! wrapped here — see `crate::drain_registrations`'s own docs for why
//! turning off a venue's trading capability is not a call this module
//! makes on its own.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use senken_core::TimeRange;
use senken_marketdata::book::{BookSnapshot, BookSource};
use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_marketdata::{Instrument, InstrumentId, SourceSymbol};
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};

/// One plugin's live enabled flag, shared by every capability the runtime
/// wraps for it — flipping this is the entire effect of
/// [`crate::Runtime::set_static_plugin_enabled`].
pub(crate) type PluginGate = Arc<AtomicBool>;

/// A fresh gate, enabled — what a plugin that just activated starts with.
pub(crate) fn new_gate() -> PluginGate {
    Arc::new(AtomicBool::new(true))
}

fn is_enabled(gate: &PluginGate) -> bool {
    gate.load(Ordering::Relaxed)
}

/// Wraps a [`MarketDataSource`] so a disabled plugin's catalog reads back
/// empty — never an error, mirroring `DynamicVenueSource::instruments`
/// exactly: a client asking "what can I chart" gets an honest "nothing
/// right now" rather than a fetch failure to handle.
pub(crate) struct GatedMarketDataSource {
    inner: Arc<dyn MarketDataSource>,
    gate: PluginGate,
}

impl GatedMarketDataSource {
    pub(crate) fn new(inner: Arc<dyn MarketDataSource>, gate: PluginGate) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl MarketDataSource for GatedMarketDataSource {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
        if !is_enabled(&self.gate) {
            return Ok(Vec::new());
        }
        self.inner.instruments().await
    }

    fn is_serving(&self) -> bool {
        // The one that actually takes a venue out of the catalogue: a
        // registry memoises each source's instruments and keeps a disk
        // snapshot of them, so returning an empty list above is never
        // reached again once either is warm. This is asked ahead of both.
        is_enabled(&self.gate) && self.inner.is_serving()
    }
}

/// Wraps a [`BarSource`] so a disabled plugin refuses every fetch —
/// mirroring `DynamicVenueSource::bars` exactly: the server keeps refusing
/// a dead source's own fetches, since hiding it in a client is not
/// enforcement.
pub(crate) struct GatedBarSource {
    inner: Arc<dyn BarSource>,
    gate: PluginGate,
}

impl GatedBarSource {
    pub(crate) fn new(inner: Arc<dyn BarSource>, gate: PluginGate) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl BarSource for GatedBarSource {
    fn source_id(&self) -> &str {
        self.inner.source_id()
    }

    fn supported(&self) -> &[BarSpec] {
        self.inner.supported()
    }

    fn max_rows(&self) -> usize {
        self.inner.max_rows()
    }

    async fn bars(
        &self,
        symbol: &SourceSymbol,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, SourceError> {
        if !is_enabled(&self.gate) {
            return Err(SourceError::rejected("this venue is currently disabled"));
        }
        self.inner.bars(symbol, spec, range).await
    }
}

/// Wraps a [`BookSource`] the same way [`GatedBarSource`] wraps a bar
/// fetch: depth is a stateless per-call request exactly like a bar fetch,
/// with no catalog of its own to report empty, so refusing the call is the
/// whole contract — there is no honest "depth for nothing" answer the way
/// an empty instrument list is for a catalog.
pub(crate) struct GatedBookSource {
    inner: Arc<dyn BookSource>,
    gate: PluginGate,
}

impl GatedBookSource {
    pub(crate) fn new(inner: Arc<dyn BookSource>, gate: PluginGate) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl BookSource for GatedBookSource {
    fn source_id(&self) -> &str {
        self.inner.source_id()
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        if !is_enabled(&self.gate) {
            return Err(SourceError::rejected("this venue is currently disabled"));
        }
        self.inner.book_snapshot(symbol, depth).await
    }
}

/// Wraps a [`FeedSource`] so the *protocol it builds* — not just the
/// factory — keeps checking the flag for as long as the server runs.
///
/// `FeedSource::protocol` is called exactly once, at server startup (see
/// `senken_api::feed::build_feed_pools`), to build the long-lived
/// [`VenueProtocol`] a `SubscriptionPool` holds for the rest of the
/// process. Gating the factory itself would only matter at that one
/// startup call — toggling the plugin afterward would have nothing left to
/// check. Wrapping the *protocol it returns* instead means the same
/// long-lived object the pool already holds keeps consulting this flag on
/// every subscribe and every inbound frame, which is what makes disabling
/// a plugin's live feed take effect without rebuilding the pool.
pub(crate) struct GatedFeedSource {
    inner: Arc<dyn FeedSource>,
    gate: PluginGate,
}

impl GatedFeedSource {
    pub(crate) fn new(inner: Arc<dyn FeedSource>, gate: PluginGate) -> Self {
        Self { inner, gate }
    }
}

impl FeedSource for GatedFeedSource {
    fn source_ids(&self) -> &[String] {
        self.inner.source_ids()
    }

    fn serves_quotes(&self) -> bool {
        self.inner.serves_quotes()
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(GatedVenueProtocol {
            inner: self.inner.protocol(symbols),
            gate: Arc::clone(&self.gate),
        })
    }
}

/// The live half of [`GatedFeedSource`]'s gating — see that type's own
/// docs for why this, not the factory, is where the flag has to live.
///
/// * [`subscribe_frame`](VenueProtocol::subscribe_frame) refuses while
///   disabled, so no *new* lease can attach to a dead venue's socket —
///   mirroring [`GatedBarSource::bars`] refusing a fresh fetch.
/// * [`parse_message`](VenueProtocol::parse_message) reports no updates
///   while disabled, so an already-open subscription simply stops
///   receiving prices rather than erroring — mirroring
///   [`GatedMarketDataSource::instruments`] reporting an empty catalog
///   rather than a fetch failure.
/// * [`unsubscribe_frame`](VenueProtocol::unsubscribe_frame) is **never**
///   gated: a lease already held when the venue is disabled must still be
///   able to release cleanly through its own `Drop` guard
///   (`senken_subscription::Lease` has no other release path), and
///   refusing an unsubscribe would trap the pool's own bookkeeping in a
///   state it can never unwind.
/// * `endpoint`, `decode_binary`, `reply_to` and `keepalive` are
///   connection housekeeping — resolving a dial URL, unwrapping a binary
///   frame, or answering a venue's own heartbeat — not data Senken serves
///   to a client, so they pass straight through regardless of the flag;
///   the flag only ever gates what a client can see, never whether the
///   underlying socket stays healthy.
struct GatedVenueProtocol {
    inner: Arc<dyn VenueProtocol>,
    gate: PluginGate,
}

#[async_trait]
impl VenueProtocol for GatedVenueProtocol {
    fn url(&self) -> &str {
        self.inner.url()
    }

    async fn endpoint(&self) -> Result<String, ConnectionError> {
        self.inner.endpoint().await
    }

    fn venue(&self) -> &str {
        self.inner.venue()
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        if !is_enabled(&self.gate) {
            return Err(ConnectionError::new("this venue is currently disabled"));
        }
        self.inner.subscribe_frame(instrument)
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.inner.unsubscribe_frame(instrument)
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        if !is_enabled(&self.gate) {
            return Vec::new();
        }
        self.inner.parse_message(text)
    }

    fn decode_binary(&self, bytes: &[u8]) -> Option<String> {
        self.inner.decode_binary(bytes)
    }

    fn reply_to(&self, text: &str) -> Option<String> {
        self.inner.reply_to(text)
    }

    fn keepalive(&self) -> Option<(std::time::Duration, String)> {
        self.inner.keepalive()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GatedBarSource, GatedBookSource, GatedFeedSource, GatedMarketDataSource, new_gate,
    };
    use async_trait::async_trait;
    use senken_core::TimeRange;
    use senken_core::UnixNanos;
    use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
    use senken_marketdata::source::{MarketDataSource, SourceError};
    use senken_marketdata::{Instrument, InstrumentId, SourceSymbol};
    use senken_plugin::BarSource;
    use senken_series::{Bar, BarSpec, BarUnit, Volume};
    use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    struct StubMarketData;

    #[async_trait]
    impl MarketDataSource for StubMarketData {
        fn id(&self) -> &'static str {
            "stub"
        }
        fn name(&self) -> &'static str {
            "Stub"
        }
        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(vec![Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")])
        }
    }

    #[tokio::test]
    async fn a_disabled_marketdata_source_reports_an_empty_catalog_not_an_error() {
        let gate = new_gate();
        let gated = GatedMarketDataSource::new(Arc::new(StubMarketData), Arc::clone(&gate));

        assert_eq!(gated.instruments().await.unwrap().len(), 1);

        gate.store(false, Ordering::Relaxed);
        assert_eq!(
            gated.instruments().await.unwrap(),
            Vec::new(),
            "a disabled source's catalog must read back empty, not error"
        );
    }

    struct StubBars;

    #[async_trait]
    impl BarSource for StubBars {
        fn source_id(&self) -> &'static str {
            "stub"
        }
        fn supported(&self) -> &[BarSpec] {
            &[]
        }
        fn max_rows(&self) -> usize {
            1_000
        }
        async fn bars(
            &self,
            _symbol: &SourceSymbol,
            _spec: BarSpec,
            _range: TimeRange,
        ) -> Result<Vec<Bar>, SourceError> {
            Ok(vec![Bar {
                ts_open: UnixNanos::EPOCH,
                open: 1,
                high: 1,
                low: 1,
                close: 1,
                volume: Volume::Real(1),
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            }])
        }
    }

    fn range() -> TimeRange {
        TimeRange::new(UnixNanos::EPOCH, UnixNanos::from_secs(60).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn a_disabled_bar_source_refuses_every_fetch() {
        let gate = new_gate();
        let gated = GatedBarSource::new(Arc::new(StubBars), Arc::clone(&gate));
        let symbol = SourceSymbol::assume("BTC-USDT");
        let spec = BarSpec::new(1, BarUnit::Minute);

        assert_eq!(gated.bars(&symbol, spec, range()).await.unwrap().len(), 1);

        gate.store(false, Ordering::Relaxed);
        let error = gated.bars(&symbol, spec, range()).await.unwrap_err();
        assert!(
            !error.is_retryable(),
            "a disabled venue is not a transient failure"
        );
    }

    struct StubBook;

    #[async_trait]
    impl BookSource for StubBook {
        fn source_id(&self) -> &'static str {
            "stub"
        }
        async fn book_snapshot(
            &self,
            _symbol: &SourceSymbol,
            _depth: usize,
        ) -> Result<BookSnapshot, SourceError> {
            Ok(BookSnapshot::new(
                UnixNanos::EPOCH,
                vec![BookLevel { price: 1, size: 1 }],
                0,
                0,
                vec![BookLevel { price: 2, size: 1 }],
                0,
                0,
            )
            .unwrap())
        }
    }

    #[tokio::test]
    async fn a_disabled_book_source_refuses_every_fetch() {
        let gate = new_gate();
        let gated = GatedBookSource::new(Arc::new(StubBook), Arc::clone(&gate));
        let symbol = SourceSymbol::assume("BTC-USDT");

        assert!(gated.book_snapshot(&symbol, 10).await.is_ok());

        gate.store(false, Ordering::Relaxed);
        assert!(gated.book_snapshot(&symbol, 10).await.is_err());
    }

    struct StubProtocol;

    #[async_trait]
    impl VenueProtocol for StubProtocol {
        fn url(&self) -> &'static str {
            "wss://stub.example"
        }
        fn venue(&self) -> &'static str {
            "stub"
        }
        fn subscribe_frame(&self, _instrument: &InstrumentId) -> Result<String, ConnectionError> {
            Ok("subscribe".to_owned())
        }
        fn unsubscribe_frame(&self, _instrument: &InstrumentId) -> Result<String, ConnectionError> {
            Ok("unsubscribe".to_owned())
        }
        fn parse_message(&self, _text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
            vec![]
        }
    }

    struct StubFeed;

    impl FeedSource for StubFeed {
        fn source_ids(&self) -> &[String] {
            &[]
        }
        fn serves_quotes(&self) -> bool {
            false
        }
        fn protocol(&self, _symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
            Arc::new(StubProtocol)
        }
    }

    struct EmptySymbols;

    impl SymbolMap for EmptySymbols {
        fn source_symbol(&self, _instrument: &InstrumentId) -> Option<String> {
            None
        }
    }

    fn instrument_id() -> InstrumentId {
        InstrumentId::new("stub", "BTCUSDT").unwrap()
    }

    #[test]
    fn a_disabled_feeds_protocol_refuses_new_subscribes_but_still_allows_unsubscribe() {
        let gate = new_gate();
        let feed = GatedFeedSource::new(Arc::new(StubFeed), Arc::clone(&gate));
        let protocol = feed.protocol(Arc::new(EmptySymbols));

        assert!(protocol.subscribe_frame(&instrument_id()).is_ok());

        gate.store(false, Ordering::Relaxed);
        assert!(
            protocol.subscribe_frame(&instrument_id()).is_err(),
            "a disabled venue must refuse a fresh subscribe"
        );
        assert!(
            protocol.unsubscribe_frame(&instrument_id()).is_ok(),
            "an already-held lease must still be able to release while the venue is disabled"
        );
    }

    #[test]
    fn a_disabled_feeds_protocol_reports_no_updates_from_an_incoming_frame() {
        let gate = new_gate();
        let feed = GatedFeedSource::new(Arc::new(StubFeed), Arc::clone(&gate));
        let protocol = feed.protocol(Arc::new(EmptySymbols));

        gate.store(false, Ordering::Relaxed);
        assert!(protocol.parse_message("irrelevant").is_empty());
    }
}
