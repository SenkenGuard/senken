//! [`Store`] and its Arrow-free half: coverage derived purely from a
//! directory listing — no side table, no file ever opened.
//!
//! [`Store::write`] and [`Store::read_range`] (feature `parquet`) live in
//! `writer.rs`/`reader.rs` as further `impl Store` blocks, so this module
//!   — and everything it depends on — compiles with `default-features =
//! false`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use senken_core::{TimeRange, UnixNanos};
use senken_series::{Anchor, SeriesKey};

use crate::error::StoreError;
use crate::paths::bars_dir;
use crate::range::decode_range;

/// A data directory holding Parquet-backed bar (and
/// trade) series.
///
/// Cheap to construct and clone (it owns only a path and an `Arc`'d lock
/// table); layout and coverage inspection ([`Store::coverage`]) work with
/// no Arrow dependency compiled in at all.
#[derive(Debug, Clone)]
pub struct Store {
    storage: senken_storage::Storage,
    compaction_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl Store {
    /// Points at `data_dir` without touching the filesystem yet.
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            storage: senken_storage::Storage::new(data_dir),
            compaction_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Creates the data directory if it does not already exist.
    ///
    /// # Errors
    /// [`StoreError::Storage`] if the directory cannot be created.
    pub fn init(&self) -> Result<(), StoreError> {
        Ok(self.storage.init()?)
    }

    /// The directory every series lives under.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        self.storage.data_dir()
    }

    /// The underlying atomic-write layer, for the `parquet`-gated write
    /// path to reuse rather than reinvent ("reuse
    /// `senken-storage`'s atomic write discipline"). Unused, hence
    /// `cfg`-gated, when that feature is off.
    #[cfg(feature = "parquet")]
    pub(crate) fn storage(&self) -> &senken_storage::Storage {
        &self.storage
    }

    /// The declared coverage of one bar series: every range named by a
    /// `.parquet` file in its `bars/{origin}-{spec}[@anchor]` directory
    ///, in the order the directory listing returns them —
    /// unsorted; a caller wanting chronological order sorts by
    /// [`TimeRange::start`].
    ///
    /// Derived purely from filenames — **no file is opened**. A series
    /// with no directory yet (nothing ever fetched) reports empty
    /// coverage, which is not an error: "not fetched" and "fetched
    /// nothing" are indistinguishable from the outside, and both are
    /// legitimately "no coverage".
    ///
    /// # Errors
    /// [`StoreError::Io`] if the directory exists but cannot be listed
    /// (permissions, or a non-directory occupying that path).
    pub fn coverage(&self, key: &SeriesKey, anchor: Anchor) -> Result<Vec<TimeRange>, StoreError> {
        let dir = self.data_dir().join(bars_dir(key, anchor));
        Ok(list_range_entries(&dir)?
            .into_iter()
            .map(|(_name, range)| range)
            .collect())
    }

    /// The earliest bar a source has conclusively returned for this exact
    /// series, if a boundary probe has observed one. This is deliberately
    /// separate from coverage: filenames say what was requested, while this
    /// value says that asking before a point has already produced a complete
    /// short response. The key includes spec and anchor, so an M1 boundary
    /// can never be inherited by H1.
    ///
    /// A malformed sidecar is ignored rather than converted into a false
    /// boundary; wasting one later request is safer than hiding history.
    ///
    /// # Errors
    /// Returns [`StoreError::Storage`] when the sidecar cannot be read.
    pub fn earliest_available(
        &self,
        key: &SeriesKey,
        anchor: Anchor,
    ) -> Result<Option<UnixNanos>, StoreError> {
        let rel = earliest_available_path(key, anchor);
        let Some(bytes) = self.storage.read_bytes(rel)? else {
            return Ok(None);
        };
        let Ok(raw) = std::str::from_utf8(&bytes) else {
            return Ok(None);
        };
        Ok(raw.trim().parse::<i64>().ok().map(UnixNanos::from_nanos))
    }

    /// Persists or clears a previously observed series boundary atomically.
    /// `None` is used when a later successful earlier fetch disproves the
    /// heuristic that recorded it.
    ///
    /// # Errors
    /// Returns [`StoreError::Storage`] when the sidecar cannot be written
    /// or removed atomically.
    pub fn record_earliest_available(
        &self,
        key: &SeriesKey,
        anchor: Anchor,
        earliest: Option<UnixNanos>,
    ) -> Result<(), StoreError> {
        let rel = earliest_available_path(key, anchor);
        match earliest {
            Some(value) => self
                .storage
                .write_bytes(rel, value.as_nanos().to_string().as_bytes())?,
            None => self.storage.remove(rel)?,
        }
        Ok(())
    }

    /// The exclusive lock a compactor must hold before touching this
    /// series' files ("Compaction: interface placeholder
    /// only, taking an exclusive per-series lock. Never on the read
    /// path.").
    ///
    /// No compaction logic exists yet — this crate stops at M5 (`Store`
    /// does not merge small files into larger ones; extending coverage
    /// only ever adds one file and unlinks superseded ones, per design
    /// This method reserves the *shape* a future compactor and any
    /// caller that must never run concurrently with one already agree on:
    /// two calls naming the same series (same directory, so same
    /// `(source, symbol, origin, spec, anchor)`) return handles to the
    /// same underlying [`Mutex`]; two calls naming different series never
    /// contend with each other. [`Store::coverage`] and
    /// [`Store::read_range`](Self::read_range) never acquire this lock —
    /// a reader must never be blocked behind a compaction, per the same
    /// design note.
    #[must_use]
    pub fn compaction_lock(&self, key: &SeriesKey, anchor: Anchor) -> Arc<Mutex<()>> {
        let dir = bars_dir(key, anchor);
        let mut locks = self
            .compaction_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(locks.entry(dir).or_insert_with(|| Arc::new(Mutex::new(()))))
    }
}

fn earliest_available_path(key: &SeriesKey, anchor: Anchor) -> String {
    format!("{}/earliest_available", bars_dir(key, anchor))
}

/// Lists `dir` and decodes every `.parquet` entry's filename as a
/// `(filename, TimeRange)` pair, skipping anything that does not decode (a
/// stray file, a subdirectory, an in-progress temp file from
/// `senken-storage`'s atomic write). A missing directory yields an empty
/// list, not an error.
///
/// [`Store::coverage`] (no Arrow) uses this for the ranges alone;
/// `writer.rs` (feature `parquet`) also needs the filenames, to know
/// exactly which files a write supersedes.
pub(crate) fn list_range_entries(dir: &Path) -> Result<Vec<(String, TimeRange)>, StoreError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(StoreError::Io {
                path: dir.to_path_buf(),
                source,
            });
        }
    };

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| StoreError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(range) = decode_range(&name) {
            found.push((name, range));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::Store;
    use crate::range::encode_range;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Anchor, BarSpec, BarUnit, Origin, SeriesKey};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn key() -> SeriesKey {
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

    #[test]
    fn coverage_of_a_never_fetched_series_is_empty_not_an_error() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.coverage(&key(), Anchor::UTC).unwrap(), Vec::new());
    }

    #[test]
    fn coverage_is_derived_from_filenames_with_no_file_opened() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        // Write directory entries by hand — zero-byte files, deliberately
        // not valid Parquet — so a passing `coverage()` call proves it
        // never tried to open or parse their contents.
        let bars_dir = dir
            .path()
            .join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m");
        std::fs::create_dir_all(&bars_dir).unwrap();
        let r = range(0, 60_000_000_000);
        std::fs::write(
            bars_dir.join(format!("{}.parquet", encode_range(r))),
            b"not parquet",
        )
        .unwrap();
        // A non-matching file must be silently skipped, not error out.
        std::fs::write(bars_dir.join("README.txt"), b"ignore me").unwrap();

        let coverage = store.coverage(&key(), Anchor::UTC).unwrap();
        assert_eq!(coverage, vec![r]);
    }

    #[test]
    fn different_anchors_report_independent_coverage() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let utc_key = SeriesKey::new(
            "okx",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Day),
        );
        // OKX's UTC+8 (Hong Kong) day is `Anchor`'s *negative* -8h case —
        // see `spec_token`'s module docs for the sign inversion.
        let utc8 = Anchor::from_offset_nanos(-8 * 3_600_000_000_000);

        let plain_dir = dir
            .path()
            .join("sources/okx/instruments/BTCUSDT/bars/venue-1d");
        let shifted_dir = dir
            .path()
            .join("sources/okx/instruments/BTCUSDT/bars/venue-1d@utc8");
        std::fs::create_dir_all(&plain_dir).unwrap();
        std::fs::create_dir_all(&shifted_dir).unwrap();

        let r = range(0, 86_400_000_000_000);
        std::fs::write(plain_dir.join(format!("{}.parquet", encode_range(r))), b"").unwrap();

        assert_eq!(store.coverage(&utc_key, Anchor::UTC).unwrap(), vec![r]);
        assert_eq!(store.coverage(&utc_key, utc8).unwrap(), Vec::new());
    }

    #[test]
    fn compaction_lock_is_shared_for_the_same_series_and_independent_across_series() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());

        let a = store.compaction_lock(&key(), Anchor::UTC);
        let b = store.compaction_lock(&key(), Anchor::UTC);
        assert!(
            Arc::ptr_eq(&a, &b),
            "two calls for the same series must share one lock"
        );

        let other_key = SeriesKey::new(
            "binance-spot",
            "ETHUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        );
        let c = store.compaction_lock(&other_key, Anchor::UTC);
        assert!(
            !Arc::ptr_eq(&a, &c),
            "different series must not contend on the same lock"
        );

        // Holding `a` must actually block a `try_lock` through `b` — proof
        // this is a real mutex, not two independent handles that merely
        // look equal by pointer.
        let _held = a.lock().unwrap();
        assert!(b.try_lock().is_err());
    }
}
