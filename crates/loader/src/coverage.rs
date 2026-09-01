//! [`CoverageCache`] — `SeriesKey → ranges`, invalidated on write (plan
//! its third cache).
//!
//! [`senken_store::Store::coverage`] is already cheap (a directory listing,
//! no file ever opened), but the resolution ladder calls it repeatedly —
//! once per candidate spec, for every request — so this still avoids a
//! directory listing per candidate per request. Invalidation is exact
//! rather than time-based: [`crate::SeriesLoader`] calls
//! [`Self::invalidate`] itself immediately after every committed write, so
//! there is no window where this cache can disagree with the store it
//! mirrors.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use senken_core::TimeRange;
use senken_series::{Anchor, SeriesKey};
use senken_store::{Store, StoreError};

/// The anchor is part of a series' identity for Day-and-above specs (plan
/// the anchor) but is not a [`SeriesKey`] field, so it rides along as part
/// of this cache's key.
type CoverageKey = (SeriesKey, i64);

fn anchor_bits(anchor: Anchor) -> i64 {
    anchor.offset_nanos()
}

#[derive(Default)]
pub(crate) struct CoverageCache {
    entries: Mutex<HashMap<CoverageKey, Vec<TimeRange>>>,
}

impl CoverageCache {
    /// `store`'s coverage for `key`/`anchor`, from cache if present.
    ///
    /// # Errors
    /// Whatever [`Store::coverage`] fails with, uncached.
    pub(crate) fn get(
        &self,
        store: &Store,
        key: &SeriesKey,
        anchor: Anchor,
    ) -> Result<Vec<TimeRange>, StoreError> {
        let cache_key = (key.clone(), anchor_bits(anchor));
        if let Some(hit) = self
            .entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&cache_key)
        {
            return Ok(hit.clone());
        }
        let coverage = store.coverage(key, anchor)?;
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(cache_key, coverage.clone());
        Ok(coverage)
    }

    /// Drops every cached anchor's coverage for `key` — call immediately
    /// after a committed write to that series, before anything else can
    /// observe stale coverage.
    pub(crate) fn invalidate(&self, key: &SeriesKey) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(k, _), _| k != key);
    }
}

#[cfg(test)]
mod tests {
    use super::CoverageCache;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Anchor, BarSpec, BarUnit, Origin, SeriesKey};
    use senken_store::Store;
    use tempfile::TempDir;

    fn key() -> SeriesKey {
        SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        )
    }

    fn bar(ts_open_secs: i64) -> senken_series::Bar {
        senken_series::Bar {
            ts_open: UnixNanos::from_secs(ts_open_secs).unwrap(),
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

    #[test]
    fn a_write_after_a_cached_lookup_is_observed_once_invalidated() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let cache = CoverageCache::default();

        assert_eq!(cache.get(&store, &key(), Anchor::UTC).unwrap(), Vec::new());

        let range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(60).unwrap(),
        )
        .unwrap();
        store
            .write(&key(), Anchor::UTC, 0, 0, range, &[bar(0)])
            .unwrap();

        // Without invalidation this would still report the pre-write
        // (empty) coverage.
        cache.invalidate(&key());
        assert_eq!(cache.get(&store, &key(), Anchor::UTC).unwrap(), vec![range]);
    }
}
