//! [`GenerationTracker`] — a generation counter per series.
//!
//! This is what stops a backfill from silently leaving another chart's
//! derived bar wrong — the failure mode called out as the
//! dangerous one *because nothing errors*: chart B caches a derived H1
//! folded from stored M1; chart A then fills a gap inside that M1 range;
//! B's cached H1 is now missing rows and nothing about reading it fails.
//! Every committed write bumps the written series' own counter; a derived
//! cache entry records the counter's value for the series it was folded
//! from at compute time, and a later mismatch means "the data underneath
//! this changed since I computed it" — invalidate, don't trust it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use senken_series::SeriesKey;

/// One atomic counter per exact series identity (`source_id`, `symbol`,
/// `origin`, `spec` — a [`SeriesKey`]). Cheap to share: cloning out a
/// counter handle is one `Arc` clone, and lookups never block a writer to a
/// different series.
#[derive(Debug, Default)]
pub(crate) struct GenerationTracker {
    counters: Mutex<HashMap<SeriesKey, Arc<AtomicU64>>>,
}

impl GenerationTracker {
    fn counter_for(&self, key: &SeriesKey) -> Arc<AtomicU64> {
        let mut counters = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(counter) = counters.get(key) {
            return Arc::clone(counter);
        }
        let counter = Arc::new(AtomicU64::new(0));
        counters.insert(key.clone(), Arc::clone(&counter));
        counter
    }

    /// `key`'s current generation. `0` for a series this tracker has never
    /// seen a write for.
    pub(crate) fn current(&self, key: &SeriesKey) -> u64 {
        self.counter_for(key).load(Ordering::SeqCst)
    }

    /// Records a committed write to `key`. Every derived cache entry that
    /// recorded an earlier generation for this exact series is now stale.
    pub(crate) fn bump(&self, key: &SeriesKey) {
        self.counter_for(key).fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::GenerationTracker;
    use senken_series::{BarSpec, BarUnit, Origin, SeriesKey};

    fn key(symbol: &str) -> SeriesKey {
        SeriesKey::new(
            "binance-spot",
            symbol,
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        )
    }

    #[test]
    fn a_never_written_series_starts_at_generation_zero() {
        let tracker = GenerationTracker::default();
        assert_eq!(tracker.current(&key("BTCUSDT")), 0);
    }

    #[test]
    fn bump_increments_only_the_named_series() {
        let tracker = GenerationTracker::default();
        tracker.bump(&key("BTCUSDT"));
        tracker.bump(&key("BTCUSDT"));
        assert_eq!(tracker.current(&key("BTCUSDT")), 2);
        assert_eq!(
            tracker.current(&key("ETHUSDT")),
            0,
            "an unrelated series must not observe another series' writes"
        );
    }
}
