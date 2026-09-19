//! The MVP property this crate exists to protect and, until this file,
//! had no test at all: bars already written to `senken-store` are a
//! user's own data, and a venue's own `bars()` refusing every fetch —
//! because an admin turned that plugin off — must never make history
//! already on disk unreadable.
//!
//! `senken-runtime` is the layer that actually installs a live on/off
//! switch around a real `BarSource`
//! (`senken_runtime::plugin_gate::GatedBarSource`, not visible here —
//! `senken-loader` sits below `senken-runtime` in the dependency graph and
//! must not depend on it). What this test proves is the half of the
//! property this crate is itself responsible for: `SeriesLoader::resolve`
//! never needs to call a rejecting source at all once the requested range
//! is already covered on disk. `DisabledSource` below stands in for a
//! plugin whose gate is off, exactly the way `PanicSource` in
//! `crate::loader`'s own tests stands in for "a source `plan()` must never
//! call".

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos};
use senken_loader::{BarSource, FetchError, SeriesLoaderBuilder, SystemClock};
use senken_series::{Anchor, Bar, BarSpec, BarUnit, Origin, SeriesKey, Volume};
use senken_store::Store;
use tempfile::TempDir;

const SOURCE_ID: &str = "disabled-venue";
const SYMBOL: &str = "TESTUSD";

fn m1() -> BarSpec {
    BarSpec::new(1, BarUnit::Minute)
}

fn key() -> SeriesKey {
    SeriesKey::new(SOURCE_ID, SYMBOL, Origin::Venue, m1())
}

fn range(start_secs: i64, end_secs: i64) -> TimeRange {
    TimeRange::new(
        UnixNanos::from_secs(start_secs).unwrap(),
        UnixNanos::from_secs(end_secs).unwrap(),
    )
    .unwrap()
}

fn bar(ts_secs: i64) -> Bar {
    Bar {
        ts_open: UnixNanos::from_secs(ts_secs).unwrap(),
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: Volume::Real(1),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// Rejects every fetch, always — the same answer a real venue plugin gives
/// once its admin-controlled gate is off
/// (`senken_marketdata::SourceError::rejected("this venue is currently
/// disabled")`, mirrored here through this crate's own, smaller
/// `FetchError`). Counts its calls so the test can prove `resolve` never
/// needed to make one.
struct DisabledSource {
    calls: AtomicUsize,
}

#[async_trait]
impl BarSource for DisabledSource {
    fn source_id(&self) -> &str {
        SOURCE_ID
    }

    fn max_rows(&self) -> usize {
        1_000
    }

    async fn bars(
        &self,
        _symbol: &str,
        _spec: BarSpec,
        _range: TimeRange,
    ) -> Result<Vec<Bar>, FetchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(FetchError::Rejected(
            "this venue is currently disabled".to_owned(),
        ))
    }
}

#[tokio::test]
async fn bars_already_on_disk_stay_readable_while_the_venues_own_fetch_rejects_everything() {
    let dir = TempDir::new().unwrap();
    let store = Store::new(dir.path());
    store.init().unwrap();

    // Write history the way a completed backfill would have left it — the
    // user's own data, already durable, before the venue is ever disabled.
    let stored_range = range(0, 600);
    let stored_bars: Vec<Bar> = (0..600).step_by(60).map(bar).collect();
    store
        .write(&key(), Anchor::UTC, 0, 0, stored_range, &stored_bars)
        .unwrap();

    let source = Arc::new(DisabledSource {
        calls: AtomicUsize::new(0),
    });
    let loader = SeriesLoaderBuilder::new(
        store,
        Arc::clone(&source) as Arc<dyn BarSource>,
        Arc::new(SystemClock),
        m1(),
    )
    .build();

    // The real read path a chart or API handler calls, not a direct
    // Parquet read: `resolve()` is `senken-loader`'s own
    // materialised-bars contract.
    let resolved = loader
        .resolve(&key(), stored_range, Anchor::UTC)
        .await
        .unwrap();

    assert_eq!(
        resolved.bars.len(),
        stored_bars.len(),
        "every already-written bar must come back, disabled venue or not"
    );
    assert!(
        resolved.missing.is_empty(),
        "a fully-covered range must resolve with nothing left to fetch"
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        0,
        "resolve() must never have called the disabled source's own bars() \
         to serve a range that was already on disk"
    );

    // The other half of the property, made explicit rather than assumed:
    // the venue really is refusing, so the assertions above are not
    // passing merely because resolve() happens to succeed either way.
    let err = source
        .bars(SYMBOL, m1(), range(600, 660))
        .await
        .unwrap_err();
    assert!(
        matches!(err, FetchError::Rejected(_)),
        "the disabled venue must still refuse a fresh fetch: {err:?}"
    );
}
