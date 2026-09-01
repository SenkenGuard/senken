//! [`SeriesData`] — the bar-fetching counterpart to
//! [`senken_marketdata::MarketData`] (
//! "`senken-runtime` still wires only `MarketDataSource`s").
//!
//! # Why this is not simply "one more thing `MarketData` does"
//!
//! [`senken_loader::SeriesLoader`] is built around exactly **one**
//! [`senken_loader::BarSource`] (the design: nothing in the
//! resolution ladder takes a source id at fetch time to route a call
//! against several venues from inside one loader). Wiring N registered
//! venues therefore means building N loaders, not one loader registering N
//! sources the way [`senken_marketdata::MarketData::register_source`] does.
//! [`SeriesData`] is the small registry that holds one [`SeriesLoader`] per
//! `source_id` and is what [`crate::Runtime::series`] actually returns —
//! mirroring `Runtime::marketdata()` in spirit (a single accessor fronting
//! every registered source) without pretending the underlying shape is
//! identical.
//!
//! # Closing the symbol trap in the one path that matters
//!
//! [`senken_plugin::BarSource::bars`] takes a
//! [`senken_marketdata::SourceSymbol`], obtainable only from
//! [`senken_marketdata::Instrument::source_symbol`] — that is the
//! compiler-enforced half of it. But [`senken_loader::SeriesLoader`]
//! itself calls through its own, older, symbol-agnostic
//! [`senken_loader::BarSource`] port, driven by
//! `senken_series::SeriesKey::symbol` — documented, unchangeably (that type
//! is its finished, unwidened surface), as the **normalised** symbol. That
//! is the string this crate's loader-facing adapter, [`CatalogBarSource`],
//! actually receives on every fetch.
//!
//! `senken_loader::PluginBarSource` (the loader's own bridge from finding
//! has no instrument catalog to consult, so the best it can do is
//! [`senken_marketdata::SourceSymbol::assume`] — trust, not prove, that
//! whatever string it was handed is already venue-native. Passing it a
//! normalised symbol would be exactly the F7 mistake, one layer down.
//! `senken-runtime` is the layer that actually holds an instrument catalog
//! ("wires plugins → sources → store → loader"), so this is
//! where the real translation belongs: [`CatalogBarSource`] looks up the
//! `(source_id, symbol)` pair in [`senken_marketdata::MarketData`] on every
//! fetch and calls the wrapped [`senken_plugin::BarSource`] with the
//! instrument's own [`senken_marketdata::Instrument::source_symbol`] —
//! never with the raw, normalised string the loader started from. This is
//! why [`SeriesData::build`] does **not** wrap sources in
//! `senken_loader::PluginBarSource`, despite that being the plan's own
//! illustrative M8.1 sketch: doing so would compile, and would work for
//! Binance and Bybit by coincidence (their wire format already equals their
//! normalised symbol), but would send OKX a bare `BTCUSDT` where it needs
//! the dashed `BTC-USDT` — silently wrong in exactly the venue-specific way
//! F7 warns is "miserable to diagnose."

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use senken_core::TimeRange;
use senken_loader::{
    BarSource as LoaderBarSource, FetchError, SeriesLoader, SeriesLoaderBuilder, SystemClock,
};
use senken_marketdata::{InstrumentId, MarketData, MarketDataError};
use senken_plugin::BarSource as PluginBarSource;
use senken_series::{Bar, BarSpec};
use senken_store::Store;

/// One [`SeriesLoader`] per registered [`senken_plugin::BarSource`], keyed
/// by that source's own `source_id` (`binance-spot`, `okx-spot`, ...). See
/// this module's docs for why this is a small registry rather than a single
/// loader the way [`MarketData`] is a single registry of many sources.
pub struct SeriesData {
    loaders: HashMap<String, SeriesLoader>,
}

impl fmt::Debug for SeriesData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SeriesData")
            .field("source_ids", &self.source_ids())
            .finish()
    }
}

impl SeriesData {
    /// Builds one [`SeriesLoader`] per entry in `bar_sources`, all rooted at
    /// `store`'s data directory but each with its own coverage cache, bar
    /// cache and job state.
    ///
    /// A source whose [`senken_plugin::BarSource::supported`] contains no
    /// spec with a fixed duration (chunk sizing needs one — see
    /// [`senken_loader::SeriesLoaderBuilder::build`]'s own `# Panics`) is
    /// skipped with a logged warning rather than panicking the whole
    /// runtime over one malformed plugin.
    #[must_use]
    pub(crate) fn build(
        store: &Store,
        marketdata: &Arc<MarketData>,
        bar_sources: Vec<Arc<dyn PluginBarSource>>,
    ) -> Self {
        let mut loaders = HashMap::with_capacity(bar_sources.len());
        for source in bar_sources {
            let source_id = source.source_id().to_owned();
            let supported = source.supported().to_vec();
            let Some(base_spec) = finest(&supported) else {
                tracing::warn!(
                    source = source_id,
                    "bar source supports no spec with a fixed duration; not building a loader for it"
                );
                continue;
            };
            let finer_specs: Vec<BarSpec> = supported
                .into_iter()
                .filter(|spec| *spec != base_spec)
                .collect();

            let bridge: Arc<dyn LoaderBarSource> = Arc::new(CatalogBarSource {
                inner: source,
                marketdata: Arc::clone(marketdata),
            });
            let loader =
                SeriesLoaderBuilder::new(store.clone(), bridge, Arc::new(SystemClock), base_spec)
                    .finer_specs(finer_specs)
                    .build();
            loaders.insert(source_id, loader);
        }
        Self { loaders }
    }

    /// The loader registered for `source_id`, if a bar source registered
    /// under that id exists and supports at least one fixed-duration spec.
    #[must_use]
    pub fn loader(&self, source_id: &str) -> Option<&SeriesLoader> {
        self.loaders.get(source_id)
    }

    /// Every source id with a registered loader, sorted for stable display.
    #[must_use]
    pub fn source_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.loaders.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }
}

/// The finest fixed-duration spec in `specs` — a loader's own `base_spec`
/// ("the finest spec this loader ever fetches directly from
/// source"). `None` when every supported spec lacks a fixed duration (only
/// [`senken_series::BarUnit::Month`] does).
fn finest(specs: &[BarSpec]) -> Option<BarSpec> {
    specs
        .iter()
        .copied()
        .filter(|spec| spec.duration_nanos().is_some())
        .min_by_key(|spec| spec.duration_nanos().unwrap_or(i64::MAX))
}

/// Bridges a real [`senken_plugin::BarSource`] onto `senken-loader`'s own
/// fetch port, resolving the venue-native symbol from the instrument
/// catalog before every fetch. See this module's own docs for why this
/// exists instead of `senken_loader::PluginBarSource`.
struct CatalogBarSource {
    inner: Arc<dyn PluginBarSource>,
    marketdata: Arc<MarketData>,
}

impl fmt::Debug for CatalogBarSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogBarSource")
            .field("source_id", &self.inner.source_id())
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl LoaderBarSource for CatalogBarSource {
    fn source_id(&self) -> &str {
        self.inner.source_id()
    }

    fn max_rows(&self) -> usize {
        self.inner.max_rows()
    }

    /// `symbol` here is whatever `senken_series::SeriesKey::symbol` the
    /// loader was addressed with — the **normalised** symbol, not
    /// the venue-native one the wrapped source actually needs. This method
    /// exists to close that gap: look the instrument up, then hand the real
    /// [`senken_plugin::BarSource`] its own
    /// [`senken_marketdata::Instrument::source_symbol`] — never the string
    /// this method itself received.
    async fn bars(
        &self,
        symbol: &str,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, FetchError> {
        let source_id = self.inner.source_id();
        let id = InstrumentId::new(source_id, symbol)
            .map_err(|error| FetchError::Rejected(error.to_string()))?;
        let matched = self
            .marketdata
            .instrument(&id)
            .await
            .map_err(|error| translate_catalog(&error))?
            .ok_or_else(|| FetchError::Rejected(format!("no instrument `{id}` in the catalog")))?;

        self.inner
            .bars(&matched.instrument.source_symbol(), spec, range)
            .await
            .map_err(|error| translate_source(&error))
    }
}

/// Maps a [`MarketDataError`] from resolving an instrument onto
/// [`FetchError`] — retryable only when it wraps a retryable
/// [`senken_marketdata::SourceError`] (an instrument catalog fetch that
/// itself hit a transient venue failure); every other case (an unknown
/// source, a malformed id, a cache write failure) cannot be fixed by
/// retrying the same lookup.
fn translate_catalog(error: &MarketDataError) -> FetchError {
    if let MarketDataError::Source(source_error) = error
        && source_error.is_retryable()
    {
        return FetchError::Transient(error.to_string());
    }
    FetchError::Rejected(error.to_string())
}

/// Maps a [`senken_marketdata::SourceError`] from the wrapped
/// [`senken_plugin::BarSource`] onto [`FetchError`], preserving exactly the
/// one bit the loader's retry logic reads:
/// [`senken_marketdata::SourceError::is_retryable`] — the same translation
/// `senken_loader::PluginBarSource` performs for the same reason.
fn translate_source(error: &senken_marketdata::SourceError) -> FetchError {
    if error.is_retryable() {
        FetchError::Transient(error.to_string())
    } else {
        FetchError::Rejected(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::SeriesData;
    use async_trait::async_trait;
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::{Instrument, MarketData, SourceError, SourceSymbol};
    use senken_plugin::BarSource as PluginBarSource;
    use senken_series::{Bar, BarSpec, BarUnit, Origin, SeriesKey};
    use senken_storage::Storage;
    use senken_store::Store;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Records the [`SourceSymbol`] it was actually called with, so a test
    /// can prove `CatalogBarSource` resolved the venue-native form rather
    /// than passing the loader's normalised `SeriesKey::symbol` straight
    /// through.
    struct RecordingBarSource {
        source_id: &'static str,
        supported: Vec<BarSpec>,
        seen: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl PluginBarSource for RecordingBarSource {
        fn source_id(&self) -> &str {
            self.source_id
        }

        fn supported(&self) -> &[BarSpec] {
            &self.supported
        }

        fn max_rows(&self) -> usize {
            1_000
        }

        async fn bars(
            &self,
            symbol: &SourceSymbol,
            spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, SourceError> {
            self.seen.lock().unwrap().push(symbol.as_str().to_owned());
            // One bar per `spec`-aligned bucket in `range`, ascending — just
            // enough for `Store::write` to accept a non-empty batch; the
            // test cares about `symbol`, not the bar content.
            let step = spec
                .duration_nanos()
                .expect("test spec has a fixed duration");
            let mut bars = Vec::new();
            let mut t = range.start().as_nanos();
            while t < range.end().as_nanos() {
                bars.push(Bar {
                    ts_open: UnixNanos::from_nanos(t),
                    open: 1,
                    high: 1,
                    low: 1,
                    close: 1,
                    volume: senken_series::Volume::Real(1),
                    quote_volume: None,
                    trade_count: None,
                    taker_buy_volume: None,
                });
                t += step;
            }
            Ok(bars)
        }
    }

    struct DemoSource;

    #[async_trait]
    impl senken_marketdata::MarketDataSource for DemoSource {
        fn id(&self) -> &'static str {
            "okx-spot"
        }

        fn name(&self) -> &'static str {
            "OKX"
        }

        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(vec![Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")])
        }
    }

    fn m1() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    #[tokio::test]
    async fn a_fetch_resolves_the_venue_native_symbol_from_the_catalog_not_the_normalised_one() {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("data"));
        storage.init().unwrap();
        let mut marketdata = MarketData::new(Arc::new(storage));
        marketdata.register_source(Arc::new(DemoSource)).unwrap();
        let marketdata = Arc::new(marketdata);

        let store = Store::new(dir.path().join("store"));
        store.init().unwrap();

        let source = Arc::new(RecordingBarSource {
            source_id: "okx-spot",
            supported: vec![m1()],
            seen: std::sync::Mutex::new(Vec::new()),
        });

        let series = SeriesData::build(&store, &marketdata, vec![source.clone() as _]);
        let loader = series.loader("okx-spot").expect("okx-spot loader built");

        let key = SeriesKey::new("okx-spot", "BTCUSDT", Origin::Venue, m1());
        let range = TimeRange::new(UnixNanos::EPOCH, UnixNanos::from_secs(60).unwrap()).unwrap();
        let outcome = loader
            .ensure(
                &key,
                range,
                senken_series::Anchor::UTC,
                0,
                0,
                senken_loader::Priority::Visible,
            )
            .wait()
            .await;
        assert!(
            matches!(outcome, senken_loader::JobOutcome::Completed),
            "{outcome:?}"
        );

        let seen = source.seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            ["BTC-USDT"],
            "the wrapped BarSource must see OKX's own dashed identifier, \
             never the normalised `BTCUSDT` SeriesKey::symbol carries"
        );
    }

    #[test]
    fn a_source_supporting_only_month_is_skipped_not_panicked_on() {
        struct MonthOnly;

        #[async_trait]
        impl PluginBarSource for MonthOnly {
            fn source_id(&self) -> &'static str {
                "weird-venue"
            }

            fn supported(&self) -> &[BarSpec] {
                static SPECS: std::sync::OnceLock<Vec<BarSpec>> = std::sync::OnceLock::new();
                SPECS.get_or_init(|| vec![BarSpec::new(1, BarUnit::Month)])
            }

            fn max_rows(&self) -> usize {
                100
            }

            async fn bars(
                &self,
                _symbol: &SourceSymbol,
                _spec: BarSpec,
                _range: TimeRange,
            ) -> Result<Vec<Bar>, SourceError> {
                Ok(Vec::new())
            }
        }

        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("data"));
        storage.init().unwrap();
        let marketdata = Arc::new(MarketData::new(Arc::new(storage)));
        let store = Store::new(dir.path().join("store"));

        let series = SeriesData::build(&store, &marketdata, vec![Arc::new(MonthOnly) as _]);
        assert_eq!(series.source_ids(), Vec::<&str>::new());
    }
}
