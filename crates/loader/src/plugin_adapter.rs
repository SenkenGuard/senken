//! [`PluginBarSource`] — the one documented bridge between
//! [`senken_plugin::BarSource`] and this crate's own [`crate::source::BarSource`]
//!.
//!
//! # Why two traits exist, and why an adapter rather than one shared trait
//!
//! `crate::source::BarSource` is this crate's own fetch port, defined in
//! before the real, plugin-facing trait existed, so the
//! resolution ladder had something to build and test against (an in-memory
//! fake) without any venue network at all. It has no `supported()` — the
//! ladder already knows which spec it is fetching by the time it calls
//! in, fixed once at [`crate::SeriesLoaderBuilder`] construction — and a
//! small [`crate::source::FetchError`] with exactly the one bit of
//! information the ladder's retry logic needs: is this worth trying again.
//!
//! `senken_plugin::BarSource` is the contract a real venue plugin
//! implements and registers through
//! `ActivationContext::register_bar_source`. It needs
//! `supported()` — a caller inspecting a venue, before ever trying a fetch,
//! wants to know what timeframes it offers — and it returns
//! `senken_marketdata::SourceError` because that is what naturally falls
//! out of fetching through a `VenueClient` and decoding JSON, exactly like
//! every existing `MarketDataSource` implementation already does.
//!
//! Finding F6 requires these not to silently drift apart into two
//! definitions that quietly diverge over time. The fix chosen here is
//! **not** to merge them: doing that would force either every plugin
//! author to depend on this crate's cache/single-flight machinery just to
//! name a trait, or force this crate (and every future consumer of it that
//! has nothing to do with plugins) to inherit the instrument-catalog
//! dependency graph. Instead, this module is the single, documented place
//! that lets a `senken_plugin::BarSource` satisfy this crate's own,
//! already-tested port — `supported()` is simply not needed by a fetch
//! call that already knows its spec, and `SourceError` carries strictly
//! more information than `FetchError` needs, so the translation is total
//! and lossless in the direction that matters: [`SourceError::is_retryable`]
//! maps onto exactly the two [`crate::source::FetchError`] variants.
//!
//! # The symbol is *not* verified here
//!
//! `senken_plugin::BarSource::bars` now takes a
//! `senken_marketdata::SourceSymbol`, obtainable in the ordinary case only
//! from `Instrument::source_symbol()` — but this adapter's own `bars` comes
//! from [`crate::source::BarSource`], whose `symbol: &str` parameter
//! predates that type and carries whatever string the caller's
//! `senken_series::SeriesKey` was built with. This module therefore uses
//! [`senken_marketdata::SourceSymbol::assume`] — see that method's docs for
//! exactly which callers may rely on it and why `senken-runtime`, the one
//! real caller in this workspace, deliberately does **not** route through
//! `PluginBarSource` for that reason (`SeriesKey::symbol` is documented as
//! the *normalised* symbol here, not the venue-native one a real venue call
//! needs). This adapter stays a faithful, symbol-agnostic bridge for a
//! standalone consumer that already keys its own `SeriesKey`s by
//! venue-native symbol; it cannot, on its own, close the symbol trap for a
//! caller that does not.

use std::sync::Arc;

use senken_core::TimeRange;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource as PluginTraitBarSource;
use senken_series::{Bar, BarSpec};

use crate::source::{BarSource, FetchError};

/// Adapts an `Arc<dyn senken_plugin::BarSource>` into this crate's own
/// [`BarSource`] port, so a real venue implementation can be passed
/// straight to [`crate::SeriesLoaderBuilder::new`].
///
/// `supported()` is intentionally not exposed here — this crate's port has
/// no place for it (see the module docs) — so a caller that needs it reads
/// it from the wrapped `senken_plugin::BarSource` directly, before
/// wrapping.
#[derive(Clone)]
pub struct PluginBarSource {
    inner: Arc<dyn PluginTraitBarSource>,
}

impl PluginBarSource {
    /// Wraps `inner` for use as this crate's fetch port.
    #[must_use]
    pub fn new(inner: Arc<dyn PluginTraitBarSource>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl BarSource for PluginBarSource {
    fn source_id(&self) -> &str {
        self.inner.source_id()
    }

    fn max_rows(&self) -> usize {
        self.inner.max_rows()
    }

    async fn bars(
        &self,
        symbol: &str,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, FetchError> {
        // See this module's docs: `symbol` is asserted, not proven,
        // venue-native here — this port predates `SourceSymbol` and does
        // not carry the provenance needed to prove it.
        let symbol = SourceSymbol::assume(symbol);
        self.inner
            .bars(&symbol, spec, range)
            .await
            .map_err(|error| translate(&error))
    }
}

/// Maps a [`SourceError`] onto this crate's smaller [`FetchError`],
/// preserving exactly the one bit its retry logic reads:
/// [`SourceError::is_retryable`].
fn translate(error: &SourceError) -> FetchError {
    if error.is_retryable() {
        FetchError::Transient(error.to_string())
    } else {
        FetchError::Rejected(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::source::SourceError;
    use senken_series::{Bar, BarSpec, BarUnit};

    use super::PluginBarSource;
    use crate::source::{BarSource, FetchError};

    fn bar(ts_open_ms: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_millis(ts_open_ms).unwrap(),
            open: 1,
            high: 1,
            low: 1,
            close: 1,
            volume: senken_series::Volume::Real(1),
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }

    fn range() -> TimeRange {
        TimeRange::new(UnixNanos::EPOCH, UnixNanos::from_millis(60_000).unwrap()).unwrap()
    }

    /// A minimal `senken_plugin::BarSource` whose one call either succeeds
    /// with one bar or fails with a caller-chosen `SourceError`, so the
    /// adapter's delegation and error translation can both be observed.
    struct FakePlugin {
        source_id: &'static str,
        supported: Vec<BarSpec>,
        max_rows: usize,
        calls: AtomicUsize,
        fail_with: Option<fn() -> SourceError>,
    }

    #[async_trait::async_trait]
    impl senken_plugin::BarSource for FakePlugin {
        fn source_id(&self) -> &str {
            self.source_id
        }

        fn supported(&self) -> &[BarSpec] {
            &self.supported
        }

        fn max_rows(&self) -> usize {
            self.max_rows
        }

        async fn bars(
            &self,
            _symbol: &SourceSymbol,
            _spec: BarSpec,
            _range: TimeRange,
        ) -> Result<Vec<Bar>, SourceError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(fail_with) = self.fail_with {
                return Err(fail_with());
            }
            Ok(vec![bar(0)])
        }
    }

    #[tokio::test]
    async fn a_successful_fetch_delegates_straight_through() {
        let plugin = Arc::new(FakePlugin {
            source_id: "fake-venue",
            supported: vec![BarSpec::new(1, BarUnit::Minute)],
            max_rows: 1_000,
            calls: AtomicUsize::new(0),
            fail_with: None,
        });
        let adapter = PluginBarSource::new(plugin.clone());

        assert_eq!(adapter.source_id(), "fake-venue");
        assert_eq!(adapter.max_rows(), 1_000);

        let bars = adapter
            .bars("BTCUSDT", BarSpec::new(1, BarUnit::Minute), range())
            .await
            .unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(plugin.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_retryable_source_error_becomes_transient() {
        let plugin = Arc::new(FakePlugin {
            source_id: "fake-venue",
            supported: Vec::new(),
            max_rows: 1,
            calls: AtomicUsize::new(0),
            fail_with: Some(|| SourceError::http(429, "slow down")),
        });
        let adapter = PluginBarSource::new(plugin);

        let error = adapter
            .bars("BTCUSDT", BarSpec::new(1, BarUnit::Minute), range())
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::Transient(_)));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn a_non_retryable_source_error_becomes_rejected() {
        let plugin = Arc::new(FakePlugin {
            source_id: "fake-venue",
            supported: Vec::new(),
            max_rows: 1,
            calls: AtomicUsize::new(0),
            fail_with: Some(|| SourceError::rejected("bad symbol")),
        });
        let adapter = PluginBarSource::new(plugin);

        let error = adapter
            .bars("BTCUSDT", BarSpec::new(1, BarUnit::Minute), range())
            .await
            .unwrap_err();
        assert!(matches!(error, FetchError::Rejected(_)));
        assert!(!error.is_retryable());
    }
}
