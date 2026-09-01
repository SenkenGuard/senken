//! Chunk-keyed single-flight fetching and gap
//! splitting.
//!
//! # The key is the chunk, never the request
//!
//! Two charts on the same symbol at different timeframes (M15 and H1) both
//! derive from the same stored M1 and will discover the *same* M1 gap.
//! Keyed on the request, `M15/[t0,t1)` and `H1/[t0,t1)` are different keys
//! and both fetch, doubling the venue spend. Keyed on the fetch chunk —
//! `(source, symbol, fetch_spec, chunk_range)`, where `fetch_spec` is
//! whichever spec is actually being fetched from the venue (the plan's
//! "`base_spec`") — they collapse into one job both await. This module uses
//! the same `tokio::sync::OnceCell` single-flight shape the instrument
//! registry already uses (`senken-marketdata`'s `MarketData::catalog_of`):
//! a cell holds only the *success* value, so a failed fetch leaves the cell
//! empty and the next caller retries rather than replaying a cached error.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use senken_core::TimeRange;
use senken_series::{Bar, BarSpec};
use tokio::sync::OnceCell;

use crate::source::FetchError;

/// The single-flight key: everything that must match for two
/// requests to legitimately share one fetch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ChunkKey {
    pub(crate) source_id: Box<str>,
    pub(crate) symbol: Box<str>,
    pub(crate) fetch_spec: BarSpec,
    pub(crate) chunk_range: TimeRange,
}

type ChunkCell = Arc<OnceCell<Arc<[Bar]>>>;

/// The single-flight guard shared by every job on one [`crate::SeriesLoader`].
#[derive(Default)]
pub(crate) struct ChunkSingleFlight {
    cells: Mutex<HashMap<ChunkKey, ChunkCell>>,
    /// How many times a chunk's fetch closure actually *ran* — as opposed
    /// to how many times a caller *asked* for one. The required test for
    /// M6.2 (two concurrent requests at different timeframes issue exactly
    /// one fetch) asserts on this counter rather than merely trusting the
    /// design, per the plan's own instruction to "prove the count".
    fetch_starts: AtomicU64,
}

impl ChunkSingleFlight {
    /// How many distinct fetch closures have run so far.
    pub(crate) fn fetch_starts(&self) -> u64 {
        self.fetch_starts.load(Ordering::SeqCst)
    }

    fn cell_for(&self, key: &ChunkKey) -> ChunkCell {
        let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cell) = cells.get(key) {
            return Arc::clone(cell);
        }
        let cell = ChunkCell::default();
        cells.insert(key.clone(), Arc::clone(&cell));
        cell
    }

    /// Runs `fetch` for `key`'s chunk, or joins whoever is already running
    /// it — `fetch` itself runs at most once per chunk while a result is
    /// pending or already cached in the cell it raced to create.
    pub(crate) async fn fetch_or_join<F, Fut>(
        &self,
        key: ChunkKey,
        fetch: F,
    ) -> Result<Arc<[Bar]>, FetchError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<Bar>, FetchError>>,
    {
        let cell = self.cell_for(&key);
        let result = cell
            .get_or_try_init(|| async {
                self.fetch_starts.fetch_add(1, Ordering::SeqCst);
                fetch().await.map(|bars| Arc::from(bars.into_boxed_slice()))
            })
            .await
            .map(Arc::clone);

        // Once resolved, drop this chunk's cell so a later, unrelated
        // request naming the same nominal range (which should not
        // ordinarily happen once the chunk is written and coverage
        // reflects it, but is not prevented by construction) starts a
        // fresh fetch rather than replaying a cached result forever, and
        // so this map does not grow without bound over a long session.
        let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = cells.get(&key)
            && Arc::ptr_eq(current, &cell)
        {
            cells.remove(&key);
        }

        result
    }
}

/// Splits `gap` into consecutive, non-overlapping chunks no wider than
/// `max_span_nanos` (chunk size follows the venue's own page
/// size — e.g. `max_rows * fetch_spec.duration_nanos()`).
///
/// # Panics
/// Never: `max_span_nanos <= 0` degenerates to one chunk per
/// smallest-representable step rather than looping forever, since a
/// misconfigured caller must not hang the loader.
pub(crate) fn split_into_chunks(gap: TimeRange, max_span_nanos: i64) -> Vec<TimeRange> {
    use std::time::Duration;

    let step_nanos = u64::try_from(max_span_nanos).unwrap_or(1).max(1);
    let mut chunks = Vec::new();
    let mut cursor = gap.start();
    while cursor < gap.end() {
        let next = cursor
            .checked_add(Duration::from_nanos(step_nanos))
            .unwrap_or(gap.end());
        let end = next.min(gap.end());
        match TimeRange::new(cursor, end) {
            // `end > cursor` always holds here: either `next` advanced by
            // at least one nanosecond and was then clamped down to no less
            // than `cursor` by `.min(gap.end())` (safe because the loop
            // condition already guarantees `cursor < gap.end()`), or the
            // checked-add overflowed and `end` fell back to `gap.end()`,
            // which the same loop condition guarantees is still `> cursor`.
            Some(chunk) => chunks.push(chunk),
            None => break,
        }
        cursor = end;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::{ChunkKey, ChunkSingleFlight, split_into_chunks};
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Bar, BarSpec, BarUnit};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(UnixNanos::from_nanos(start), UnixNanos::from_nanos(end)).unwrap()
    }

    fn bar(ts: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_nanos(ts),
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

    fn key(chunk_range: TimeRange) -> ChunkKey {
        ChunkKey {
            source_id: "binance-spot".into(),
            symbol: "BTCUSDT".into(),
            fetch_spec: BarSpec::new(1, BarUnit::Minute),
            chunk_range,
        }
    }

    #[tokio::test]
    async fn concurrent_requests_for_the_same_chunk_run_the_fetch_once() {
        let flight = Arc::new(ChunkSingleFlight::default());
        let calls = Arc::new(AtomicU32::new(0));
        let range = range(0, 1_000);

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let flight = Arc::clone(&flight);
            let calls = Arc::clone(&calls);
            let key = key(range);
            tasks.push(tokio::spawn(async move {
                flight
                    .fetch_or_join(key, || async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        tokio::task::yield_now().await;
                        Ok(vec![bar(0)])
                    })
                    .await
            }));
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(flight.fetch_starts(), 1);
    }

    #[tokio::test]
    async fn a_failed_fetch_leaves_the_cell_empty_for_a_retry() {
        let flight = ChunkSingleFlight::default();
        let range = range(0, 1_000);

        let first = flight
            .fetch_or_join(key(range), || async {
                Err(crate::FetchError::Transient("boom".to_owned()))
            })
            .await;
        assert!(first.is_err());

        let second = flight
            .fetch_or_join(key(range), || async { Ok(vec![bar(0)]) })
            .await;
        assert!(
            second.is_ok(),
            "a failed chunk must be retryable, not poisoned forever"
        );
        assert_eq!(flight.fetch_starts(), 2);
    }

    #[test]
    fn split_into_chunks_tiles_a_gap_with_no_overlap_or_leftover() {
        let gap = range(0, 250);
        let chunks = split_into_chunks(gap, 100);
        assert_eq!(
            chunks,
            vec![range(0, 100), range(100, 200), range(200, 250)]
        );
    }

    #[test]
    fn split_into_chunks_of_an_empty_gap_is_empty() {
        let gap = range(10, 10);
        assert_eq!(split_into_chunks(gap, 100), Vec::new());
    }

    #[test]
    fn split_into_chunks_with_a_span_wider_than_the_gap_is_one_chunk() {
        let gap = range(0, 50);
        assert_eq!(split_into_chunks(gap, 1_000), vec![gap]);
    }
}
