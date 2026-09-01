//! [`BarCache`] — a byte-bounded LRU of decoded bars
//! .
//!
//! One table serves both roles (page cache —
//! `(SeriesKey, file)` — and derived cache — `(SeriesKey, target_spec,
//! range, generation)`): `senken-store` abstracts individual files behind
//! [`senken_store::Store::read_range`], so this crate never sees a
//! filename to key a *page* cache on, only the `(SeriesKey, TimeRange)` a
//! caller actually asked for — and a [`senken_series::SeriesKey`] already
//! carries `target_spec`. The two roles stay distinguishable by
//! `derived_from`: empty for a direct store read (a "page"), one or more
//! `(base, generation)` pairs for an aggregation folded from those series
//! at those generations (a "derived" entry) — more than one
//! when a stitched request folded segments from different
//! candidate specs.
//!
//! Entries are `Arc<[Bar]>`, not `Arc<RecordBatch>` as first
//! illustrates — deliberately: Arrow stays confined to `senken-store` (plan
//! Part C1), and this crate does not depend on it (see [`crate::source`]'s
//! module docs for the same substitution made for the fetch port). The
//! eviction property D17 actually cares about — evicting drops only *the
//! cache's* strong reference, so a chart still reading a batch keeps it
//! alive — holds identically for `Arc<[Bar]>`.

use std::sync::{Arc, Mutex, PoisonError};

use lru::LruCache;
use senken_core::TimeRange;
use senken_series::{Bar, SeriesKey};

use crate::generation::GenerationTracker;

/// One cached read or aggregation.
#[derive(Clone)]
pub(crate) struct CachedBars {
    pub(crate) bars: Arc<[Bar]>,
    /// For a derived entry: every exact series (always [`Origin::Venue`],
    /// it was folded from, and the generation
    /// ([`GenerationTracker`]) each had at computation time. More than one
    /// entry when a stitched request folded segments from
    /// different candidate specs — a write to *any* of them must
    /// invalidate the whole cached result, since each contributed rows to
    /// it. Empty for bars read directly from their own exact series —
    /// nothing was folded, so there is nothing to go stale relative to
    /// beyond that series' own coverage, which write invalidation
    /// ([`BarCache::invalidate_key`]) already handles.
    ///
    /// [`Origin::Venue`]: senken_series::Origin::Venue
    pub(crate) derived_from: Vec<(SeriesKey, u64)>,
}

impl CachedBars {
    /// An approximation of this entry's memory cost, for the cache's byte
    /// budget. Not exact — byte accounting is
    /// approximate by construction, since an evicted-but-still-`Arc`-held
    /// entry cannot be un-counted without the pinning/lease bookkeeping
    /// this design deliberately avoids — just proportional to what a
    /// `Bar` actually costs.
    fn approx_bytes(&self) -> usize {
        self.bars.len() * std::mem::size_of::<Bar>()
    }
}

/// Cache hit/miss/eviction counters ("an explicit setting with
/// metrics ... not a buried constant").
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheMetrics {
    /// Bytes currently held by live entries.
    pub used_bytes: usize,
    /// The configured byte budget.
    pub max_bytes: usize,
    /// Successful `BarCache::get` lookups.
    pub hits: u64,
    /// Lookups that found nothing (or found a stale generation).
    pub misses: u64,
    /// Entries removed to stay under `max_bytes`.
    pub evictions: u64,
}

struct State {
    lru: LruCache<(SeriesKey, TimeRange), CachedBars>,
    used_bytes: usize,
    max_bytes: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
}

/// A byte-bounded LRU keyed by exactly the `(SeriesKey, TimeRange)` a
/// caller asked for (the key shape — no sub-range stitching).
pub(crate) struct BarCache {
    state: Mutex<State>,
}

impl BarCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            state: Mutex::new(State {
                lru: LruCache::unbounded(),
                used_bytes: 0,
                max_bytes,
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
        }
    }

    /// Looks up `(key, range)`, treating a derived entry whose recorded
    /// generation no longer matches its dependency's *current* generation
    /// (per `generations`) as a miss rather than returning
    /// stale data.
    pub(crate) fn get(
        &self,
        key: &SeriesKey,
        range: TimeRange,
        generations: &GenerationTracker,
    ) -> Option<CachedBars> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let found = state.lru.get(&(key.clone(), range)).cloned();
        let Some(entry) = found else {
            state.misses += 1;
            return None;
        };
        let any_dependency_moved_on = entry
            .derived_from
            .iter()
            .any(|(depends_on, computed_at)| generations.current(depends_on) != *computed_at);
        if any_dependency_moved_on {
            state.misses += 1;
            return None;
        }
        state.hits += 1;
        Some(entry)
    }

    /// Inserts (or replaces) the entry for `(key, range)`, evicting the
    /// least-recently-used entries until back under the byte budget.
    pub(crate) fn insert(&self, key: SeriesKey, range: TimeRange, entry: CachedBars) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let bytes = entry.approx_bytes();
        if let Some(old) = state.lru.put((key, range), entry) {
            state.used_bytes = state.used_bytes.saturating_sub(old.approx_bytes());
        }
        state.used_bytes += bytes;
        while state.used_bytes > state.max_bytes {
            let Some((_, evicted)) = state.lru.pop_lru() else {
                break;
            };
            state.used_bytes = state.used_bytes.saturating_sub(evicted.approx_bytes());
            state.evictions += 1;
        }
    }

    /// Drops every cached entry for `key`, regardless of range — used after
    /// a write to that exact series, since any previously cached read of
    /// it may now be missing rows the new write added.
    pub(crate) fn invalidate_key(&self, key: &SeriesKey) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let to_remove: Vec<(SeriesKey, TimeRange)> = state
            .lru
            .iter()
            .filter(|((k, _), _)| k == key)
            .map(|((k, range), _)| (k.clone(), *range))
            .collect();
        for k in to_remove {
            if let Some(v) = state.lru.pop(&k) {
                state.used_bytes = state.used_bytes.saturating_sub(v.approx_bytes());
            }
        }
    }

    /// A snapshot of this cache's metrics.
    pub(crate) fn metrics(&self) -> CacheMetrics {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        CacheMetrics {
            used_bytes: state.used_bytes,
            max_bytes: state.max_bytes,
            hits: state.hits,
            misses: state.misses,
            evictions: state.evictions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BarCache, CachedBars};
    use crate::generation::GenerationTracker;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Bar, BarSpec, BarUnit, Origin, SeriesKey};
    use std::sync::Arc;

    fn key() -> SeriesKey {
        SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Derived,
            BarSpec::new(1, BarUnit::Hour),
        )
    }

    fn depends_on() -> SeriesKey {
        SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        )
    }

    fn range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(UnixNanos::from_nanos(start), UnixNanos::from_nanos(end)).unwrap()
    }

    fn bar() -> Bar {
        Bar {
            ts_open: UnixNanos::from_nanos(0),
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
    fn a_miss_is_reported_and_a_hit_returns_the_same_bars() {
        let cache = BarCache::new(1_000_000);
        let generations = GenerationTracker::default();
        assert!(cache.get(&key(), range(0, 10), &generations).is_none());

        let bars: Arc<[Bar]> = Arc::from(vec![bar()]);
        cache.insert(
            key(),
            range(0, 10),
            CachedBars {
                bars: Arc::clone(&bars),
                derived_from: Vec::new(),
            },
        );
        let hit = cache.get(&key(), range(0, 10), &generations).unwrap();
        assert_eq!(hit.bars.len(), 1);
        let metrics = cache.metrics();
        assert_eq!(metrics.hits, 1);
        assert_eq!(metrics.misses, 1);
    }

    #[test]
    fn a_stale_generation_is_treated_as_a_miss() {
        let cache = BarCache::new(1_000_000);
        let generations = GenerationTracker::default();
        generations.bump(&depends_on()); // generation now 1
        cache.insert(
            key(),
            range(0, 10),
            CachedBars {
                bars: Arc::from(vec![bar()]),
                derived_from: vec![(depends_on(), 1)],
            },
        );
        assert!(
            cache.get(&key(), range(0, 10), &generations).is_some(),
            "recorded generation matches current: must hit"
        );

        generations.bump(&depends_on()); // generation now 2, entry recorded 1
        assert!(
            cache.get(&key(), range(0, 10), &generations).is_none(),
            "a write to the dependency after the entry was computed must invalidate it"
        );
    }

    #[test]
    fn eviction_stays_under_the_byte_budget() {
        let one_entry_bytes = CachedBars {
            bars: Arc::from(vec![bar()]),
            derived_from: Vec::new(),
        }
        .approx_bytes();
        let cache = BarCache::new(one_entry_bytes);
        let generations = GenerationTracker::default();

        cache.insert(
            key(),
            range(0, 1),
            CachedBars {
                bars: Arc::from(vec![bar()]),
                derived_from: Vec::new(),
            },
        );
        cache.insert(
            key(),
            range(1, 2),
            CachedBars {
                bars: Arc::from(vec![bar()]),
                derived_from: Vec::new(),
            },
        );

        assert!(
            cache.get(&key(), range(0, 1), &generations).is_none(),
            "the least-recently-used entry must be evicted to stay under budget"
        );
        assert!(cache.get(&key(), range(1, 2), &generations).is_some());
        assert_eq!(cache.metrics().evictions, 1);
    }

    #[test]
    fn invalidate_key_drops_every_range_for_that_series_only() {
        let cache = BarCache::new(1_000_000);
        let generations = GenerationTracker::default();
        let other = SeriesKey::new(
            "binance-spot",
            "ETHUSDT",
            Origin::Derived,
            BarSpec::new(1, BarUnit::Hour),
        );
        cache.insert(
            key(),
            range(0, 1),
            CachedBars {
                bars: Arc::from(vec![bar()]),
                derived_from: Vec::new(),
            },
        );
        cache.insert(
            other.clone(),
            range(0, 1),
            CachedBars {
                bars: Arc::from(vec![bar()]),
                derived_from: Vec::new(),
            },
        );

        cache.invalidate_key(&key());

        assert!(cache.get(&key(), range(0, 1), &generations).is_none());
        assert!(
            cache.get(&other, range(0, 1), &generations).is_some(),
            "an unrelated series' cache entry must survive"
        );
    }
}
