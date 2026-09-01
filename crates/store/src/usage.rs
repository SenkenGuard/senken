//! Disk usage accounting and reclamation for the data directory.
//!
//! Deliberately **Arrow-free**, alongside [`crate::paths`] and
//! [`crate::store::Store::coverage`]: reporting what exists on disk, and
//! deleting it, needs no columnar dependency at all — only actually
//! reading a Parquet file's rows does. `cargo check -p senken-store
//! --no-default-features --all-targets` proves this module holds to that
//! boundary.
//!
//! Every number here comes from `fs::metadata` on a real file — never
//! estimated, never cached — and every directory this module walks is
//! reported even when its name fails to decode: an admin reclaiming disk
//! space must see *everything* that is actually there, not only the part
//! this crate recognises.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use senken_core::{path_key, symbol_from_path};
use senken_series::{Anchor, BarSpec, Origin};

use crate::error::StoreError;
use crate::paths::parse_bars_dir_name;
use crate::store::Store;

/// What kind of series one `SeriesUsage` reports on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesKind {
    /// A `bars/{origin}-{spec}[@anchor]` directory whose name decoded
    /// successfully.
    Bars {
        /// Whether this series came from the venue or was locally derived.
        origin: Origin,
        /// The bar timeframe.
        spec: BarSpec,
        /// The calendar anchor Day-and-above series persist as part of
        /// their identity (see [`crate`]'s module docs).
        anchor: Anchor,
    },
    /// The one `trades` directory an instrument can have.
    Trades,
    /// A directory (or file) that does not match either recognised shape —
    /// most likely a `bars/` entry whose name this version of the encoding
    /// cannot parse. Still fully counted: an unreadable name is not a
    /// reason to make its bytes invisible.
    Unrecognised,
}

/// One series' footprint on disk: either a `bars/{dir_name}` directory or
/// the `trades` directory, or — for a directory neither shape recognises —
/// whatever raw entry `usage()` found directly under the instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesUsage {
    /// The on-disk directory name, exactly as it appears under the
    /// instrument (or under its `bars/` subdirectory) — the identifier a
    /// caller passes back to [`Store::delete_series`].
    pub dir_name: String,
    /// What this series is, so far as its name could be decoded.
    pub kind: SeriesKind,
    /// Total bytes of every real file under this series' directory.
    pub bytes: u64,
    /// Total file count under this series' directory.
    pub files: u64,
}

/// One instrument's footprint on disk: its decoded symbol plus every
/// series (bar or trade) stored under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentUsage {
    /// The decoded symbol, or the raw on-disk directory name when it could
    /// not be decoded ([`senken_core::symbol_from_path`] failed) — still
    /// reported, never skipped.
    pub symbol: String,
    /// Total bytes across every series under this instrument.
    pub bytes: u64,
    /// Total file count across every series under this instrument.
    pub files: u64,
    /// Every series under this instrument, sorted by `bytes` descending
    /// (ties broken by `dir_name`).
    pub series: Vec<SeriesUsage>,
}

/// One source's footprint on disk: its decoded source id plus every
/// instrument stored under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUsage {
    /// The decoded source id, or the raw on-disk directory name when it
    /// could not be decoded — still reported, never skipped.
    pub source_id: String,
    /// Total bytes across every instrument under this source.
    pub bytes: u64,
    /// Total file count across every instrument under this source.
    pub files: u64,
    /// Every instrument under this source, sorted by `bytes` descending
    /// (ties broken by `symbol`).
    pub instruments: Vec<InstrumentUsage>,
}

impl Store {
    /// Walks the whole data directory and reports what is on disk, source
    /// by source, instrument by instrument, series by series.
    ///
    /// A missing `sources/` directory (a fresh install that has fetched
    /// nothing yet) reports an empty list, not an error. Every level is
    /// sorted by `bytes` descending — the biggest thing is always first —
    /// with ties broken by name so the order is stable between calls.
    ///
    /// # Errors
    /// [`StoreError::Io`] if a directory exists but cannot be listed or
    /// its entries' metadata cannot be read (permissions, or a filesystem
    /// error mid-walk).
    pub fn usage(&self) -> Result<Vec<SourceUsage>, StoreError> {
        let sources_root = self.data_dir().join("sources");
        let mut sources = Vec::new();
        for entry in read_dir_entries(&sources_root)? {
            let encoded = entry_name(&entry);
            let source_id = symbol_from_path(&encoded).unwrap_or_else(|_| encoded.clone());
            sources.push(source_usage(source_id, &entry.path())?);
        }
        sort_by_bytes_desc(&mut sources, |s| &s.source_id);
        Ok(sources)
    }

    /// Deletes one series (a `bars/{dir_name}` directory, the `trades`
    /// directory, or an unrecognised entry directly under the instrument)
    /// and returns the bytes actually freed.
    ///
    /// Removing the last series under an instrument removes the
    /// now-empty instrument directory too, and — if that was the last
    /// instrument — the now-empty source directory, so the tree never
    /// fills with empty nodes.
    ///
    /// # Errors
    /// [`StoreError::Rejected`]... actually [`StoreError`]'s
    /// `InvalidPathSegment` variant if `source_id`, `symbol` (once
    /// path-key-encoded) or `dir_name` is not a safe single path segment.
    /// [`StoreError::Io`] on an underlying filesystem failure. Deleting
    /// something that does not exist is `Ok(0)`, not an error.
    pub fn delete_series(
        &self,
        source_id: &str,
        symbol: &str,
        dir_name: &str,
    ) -> Result<u64, StoreError> {
        validate_path_segment(dir_name)?;
        let (source_dir, instrument_dir) = self.checked_instrument_dirs(source_id, symbol)?;
        let target = resolve_series_path(&instrument_dir, dir_name);
        let freed = remove_path_and_measure(&target)?;
        if let Some(parent) = target.parent()
            && parent != instrument_dir
        {
            remove_if_empty(parent)?;
        }
        prune_instrument_and_source(&instrument_dir, &source_dir)?;
        Ok(freed)
    }

    /// Deletes an entire instrument (every series it holds) and returns
    /// the bytes actually freed.
    ///
    /// Removes the now-empty source directory too when this was the
    /// instrument's last one.
    ///
    /// # Errors
    /// Same as [`Store::delete_series`].
    pub fn delete_instrument(&self, source_id: &str, symbol: &str) -> Result<u64, StoreError> {
        let (source_dir, instrument_dir) = self.checked_instrument_dirs(source_id, symbol)?;
        let freed = remove_path_and_measure(&instrument_dir)?;
        prune_source_if_empty(&source_dir)?;
        Ok(freed)
    }

    /// Deletes an entire source (every instrument it holds) and returns
    /// the bytes actually freed.
    ///
    /// # Errors
    /// Same as [`Store::delete_series`].
    pub fn delete_source(&self, source_id: &str) -> Result<u64, StoreError> {
        let source_dir = self.checked_source_dir(source_id)?;
        remove_path_and_measure(&source_dir)
    }

    /// Validates `source_id` (once path-key-encoded) and returns the
    /// directory it names under `sources/`.
    fn checked_source_dir(&self, source_id: &str) -> Result<PathBuf, StoreError> {
        let encoded = path_key(source_id);
        validate_path_segment(&encoded)?;
        Ok(self.data_dir().join("sources").join(encoded.as_ref()))
    }

    /// Validates `source_id` and `symbol` (each once path-key-encoded) and
    /// returns `(source_dir, instrument_dir)`.
    fn checked_instrument_dirs(
        &self,
        source_id: &str,
        symbol: &str,
    ) -> Result<(PathBuf, PathBuf), StoreError> {
        let source_dir = self.checked_source_dir(source_id)?;
        let encoded_symbol = path_key(symbol);
        validate_path_segment(&encoded_symbol)?;
        let instrument_dir = source_dir.join("instruments").join(encoded_symbol.as_ref());
        Ok((source_dir, instrument_dir))
    }
}

/// `true` when `segment` is safe to use as exactly one path component: not
/// empty, not `.`/`..`, and free of any path separator. A caller that
/// could pass `..` (chained, all the way past `sources/`) could otherwise
/// reach and delete the accounts database — this is the one check standing
/// between an admin's disk-usage request and that.
fn is_safe_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains('/')
        && !segment.contains('\\')
}

/// Rejects `segment` outright (never sanitises it) when
/// [`is_safe_path_segment`] says no.
fn validate_path_segment(segment: &str) -> Result<(), StoreError> {
    if is_safe_path_segment(segment) {
        Ok(())
    } else {
        Err(StoreError::InvalidPathSegment(segment.to_owned()))
    }
}

/// Where a series named `dir_name` (as reported by [`Store::usage`])
/// actually lives: preferring `bars/{dir_name}` when it exists there (the
/// common case), falling back to the fixed `trades` directory, and
/// finally to a direct child of the instrument directory — matching the
/// three shapes `usage()`'s own walk can report a `SeriesUsage` for.
fn resolve_series_path(instrument_dir: &Path, dir_name: &str) -> PathBuf {
    let under_bars = instrument_dir.join("bars").join(dir_name);
    if under_bars.is_dir() {
        return under_bars;
    }
    if dir_name == "trades" {
        return instrument_dir.join("trades");
    }
    instrument_dir.join(dir_name)
}

/// After a series delete, removes `instrument_dir` if it is now empty,
/// and — only then — `source_dir`'s `instruments` and, if that too is now
/// empty, `source_dir` itself.
fn prune_instrument_and_source(instrument_dir: &Path, source_dir: &Path) -> Result<(), StoreError> {
    if remove_if_empty(instrument_dir)? {
        prune_source_if_empty(source_dir)?;
    }
    Ok(())
}

/// Removes `source_dir`'s `instruments` subdirectory if it is now empty,
/// and `source_dir` itself if that leaves it empty too.
fn prune_source_if_empty(source_dir: &Path) -> Result<(), StoreError> {
    let instruments_dir = source_dir.join("instruments");
    if remove_if_empty(&instruments_dir)? {
        remove_if_empty(source_dir)?;
    }
    Ok(())
}

/// Removes `dir` if (and only if) it exists and is empty. Returns whether
/// it was removed.
fn remove_if_empty(dir: &Path) -> Result<bool, StoreError> {
    match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                fs::remove_dir(dir).map_err(|source| StoreError::Io {
                    path: dir.to_path_buf(),
                    source,
                })?;
                Ok(true)
            } else {
                Ok(false)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(StoreError::Io {
            path: dir.to_path_buf(),
            source,
        }),
    }
}

/// Removes `path` (file or directory, recursively) and returns the total
/// bytes it held, measured *before* removal. A missing path is `Ok(0)`,
/// not an error — the reader who asked for this delete may already be
/// looking at a stale tree, and failing here would tell them nothing they
/// could act on.
fn remove_path_and_measure(path: &Path) -> Result<u64, StoreError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(source) => {
            return Err(StoreError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.is_dir() {
        let (bytes, _files) = dir_usage(path)?;
        fs::remove_dir_all(path).map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(bytes)
    } else {
        fs::remove_file(path).map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(metadata.len())
    }
}

/// One source's usage: `source_dir_path` is `sources/{encoded_source_id}`
/// on disk; `source_id` is what to report (decoded, or the raw name on a
/// decode failure).
fn source_usage(source_id: String, source_dir_path: &Path) -> Result<SourceUsage, StoreError> {
    let instruments_root = source_dir_path.join("instruments");
    let mut instruments = Vec::new();
    for entry in read_dir_entries(&instruments_root)? {
        let encoded = entry_name(&entry);
        let symbol = symbol_from_path(&encoded).unwrap_or_else(|_| encoded.clone());
        instruments.push(instrument_usage(symbol, &entry.path())?);
    }
    sort_by_bytes_desc(&mut instruments, |i| &i.symbol);
    let bytes = instruments.iter().map(|i| i.bytes).sum();
    let files = instruments.iter().map(|i| i.files).sum();
    Ok(SourceUsage {
        source_id,
        bytes,
        files,
        instruments,
    })
}

/// One instrument's usage: `instrument_dir_path` is
/// `sources/{source}/instruments/{encoded_symbol}` on disk.
fn instrument_usage(
    symbol: String,
    instrument_dir_path: &Path,
) -> Result<InstrumentUsage, StoreError> {
    let mut series = Vec::new();
    for entry in read_dir_entries(instrument_dir_path)? {
        let name = entry_name(&entry);
        let entry_path = entry.path();
        if name == "bars" {
            for bars_entry in read_dir_entries(&entry_path)? {
                let bars_name = entry_name(&bars_entry);
                let kind = parse_bars_dir_name(&bars_name).map_or(
                    SeriesKind::Unrecognised,
                    |(origin, spec, anchor)| SeriesKind::Bars {
                        origin,
                        spec,
                        anchor,
                    },
                );
                series.push(series_usage(bars_name, kind, &bars_entry.path())?);
            }
        } else if name == "trades" {
            series.push(series_usage(name, SeriesKind::Trades, &entry_path)?);
        } else {
            // Neither documented shape: still counted, under its own raw
            // name, rather than silently folded into the instrument total
            // with no way for a reader to see what it was.
            series.push(series_usage(name, SeriesKind::Unrecognised, &entry_path)?);
        }
    }
    sort_by_bytes_desc(&mut series, |s| &s.dir_name);
    let bytes = series.iter().map(|s| s.bytes).sum();
    let files = series.iter().map(|s| s.files).sum();
    Ok(InstrumentUsage {
        symbol,
        bytes,
        files,
        series,
    })
}

/// One series' usage: sums every real file under `dir_path`, recursively.
fn series_usage(
    dir_name: String,
    kind: SeriesKind,
    dir_path: &Path,
) -> Result<SeriesUsage, StoreError> {
    let (bytes, files) = dir_usage(dir_path)?;
    Ok(SeriesUsage {
        dir_name,
        kind,
        bytes,
        files,
    })
}

/// Sums the real size (`fs::metadata`) of every file under `path`,
/// recursively, plus how many files that was. A directory contributes no
/// bytes of its own — only the files under it do.
fn dir_usage(path: &Path) -> Result<(u64, u64), StoreError> {
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in read_dir_entries(path)? {
        let metadata = entry.metadata().map_err(|source| StoreError::Io {
            path: entry.path(),
            source,
        })?;
        if metadata.is_dir() {
            let (child_bytes, child_files) = dir_usage(&entry.path())?;
            bytes += child_bytes;
            files += child_files;
        } else {
            bytes += metadata.len();
            files += 1;
        }
    }
    Ok((bytes, files))
}

/// Lists `dir`'s entries, or an empty list when `dir` does not exist —
/// "not fetched" and "fetched nothing" must both read as "nothing here",
/// never as an error, matching [`crate::store::list_range_entries`]'s own
/// rule.
fn read_dir_entries(dir: &Path) -> Result<Vec<fs::DirEntry>, StoreError> {
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| StoreError::Io {
                path: dir.to_path_buf(),
                source,
            }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(StoreError::Io {
            path: dir.to_path_buf(),
            source,
        }),
    }
}

/// A directory entry's own file name, decoded lossily rather than
/// skipped: a name this platform cannot represent as valid UTF-8 is
/// vanishingly unlikely on the platforms this project targets, but even
/// then its bytes must still be counted, not silently dropped from the
/// report.
fn entry_name(entry: &fs::DirEntry) -> String {
    entry.file_name().to_string_lossy().into_owned()
}

/// Sorts `items` by `bytes` descending (the biggest thing first), breaking
/// ties by `key` so the order is stable between calls.
fn sort_by_bytes_desc<T>(items: &mut [T], key: impl Fn(&T) -> &str)
where
    T: HasBytes,
{
    items.sort_by(|a, b| b.bytes().cmp(&a.bytes()).then_with(|| key(a).cmp(key(b))));
}

/// Lets [`sort_by_bytes_desc`] work generically over the three usage
/// structs without repeating the same sort three times.
trait HasBytes {
    fn bytes(&self) -> u64;
}

impl HasBytes for SourceUsage {
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl HasBytes for InstrumentUsage {
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl HasBytes for SeriesUsage {
    fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::{SeriesKind, is_safe_path_segment, validate_path_segment};
    use crate::error::StoreError;
    use crate::store::Store;
    use senken_series::{BarUnit, Origin};
    use std::fs;
    use tempfile::TempDir;

    /// Writes `len` zero bytes at `path`, creating parent directories as
    /// needed.
    fn write_sized(path: &std::path::Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; len]).unwrap();
    }

    #[test]
    fn path_segment_validation_refuses_dot_dot_and_embedded_separators() {
        assert!(!is_safe_path_segment(".."));
        assert!(!is_safe_path_segment("."));
        assert!(!is_safe_path_segment("a/b"));
        assert!(!is_safe_path_segment("a\\b"));
        assert!(!is_safe_path_segment(""));
        assert!(is_safe_path_segment("venue-1m"));
        assert!(is_safe_path_segment("binance-spot"));

        assert!(matches!(
            validate_path_segment(".."),
            Err(StoreError::InvalidPathSegment(s)) if s == ".."
        ));
        assert!(matches!(
            validate_path_segment("a/b"),
            Err(StoreError::InvalidPathSegment(s)) if s == "a/b"
        ));
    }

    #[test]
    fn usage_of_an_empty_data_dir_is_an_empty_list_not_an_error() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.usage().unwrap(), Vec::new());
    }

    /// Builds a small real tree: two sources, one of which has a symbol
    /// that path-key-encodes (`D.O.G.E.`), a bars series, a trades
    /// series, and one directory `usage()` cannot decode as either shape.
    fn build_tree(dir: &std::path::Path) {
        write_sized(
            &dir.join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m/f1.parquet"),
            300,
        );
        write_sized(
            &dir.join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m/f2.parquet"),
            200,
        );
        write_sized(
            &dir.join("sources/binance-spot/instruments/BTCUSDT/trades/f1.parquet"),
            1_000,
        );
        // A symbol that path-key encodes: `D.O.G.E.` -> `D%2EO%2EG%2EE%2E`.
        write_sized(
            &dir.join("sources/okx/instruments/D%2EO%2EG%2EE%2E/bars/venue-1d/f1.parquet"),
            50,
        );
        // An entry `usage()` cannot decode as either `bars` or `trades`.
        write_sized(
            &dir.join("sources/okx/instruments/D%2EO%2EG%2EE%2E/mystery/oddfile"),
            10,
        );
    }

    #[test]
    fn usage_sums_real_bytes_and_decodes_symbols() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let sources = store.usage().unwrap();
        assert_eq!(sources.len(), 2);

        // Sorted by bytes descending: binance-spot (1500) before okx (60).
        assert_eq!(sources[0].source_id, "binance-spot");
        assert_eq!(sources[0].bytes, 1_500);
        assert_eq!(sources[0].files, 3);
        assert_eq!(sources[1].source_id, "okx");
        assert_eq!(sources[1].bytes, 60);
        assert_eq!(sources[1].files, 2);

        let binance = &sources[0];
        assert_eq!(binance.instruments.len(), 1);
        let btc = &binance.instruments[0];
        assert_eq!(btc.symbol, "BTCUSDT");
        assert_eq!(btc.bytes, 1_500);
        assert_eq!(btc.series.len(), 2);
        // Trades (1000 bytes) sorts before the bars series (500 bytes).
        assert_eq!(btc.series[0].dir_name, "trades");
        assert_eq!(btc.series[0].kind, SeriesKind::Trades);
        assert_eq!(btc.series[0].bytes, 1_000);
        assert_eq!(btc.series[1].dir_name, "venue-1m");
        assert_eq!(btc.series[1].bytes, 500);
        assert_eq!(btc.series[1].files, 2);
        assert_eq!(
            btc.series[1].kind,
            SeriesKind::Bars {
                origin: Origin::Venue,
                spec: senken_series::BarSpec::new(1, BarUnit::Minute),
                anchor: senken_series::Anchor::UTC,
            }
        );

        // The dotted symbol decoded back to its original form, and the
        // undecodable `mystery` entry was still counted, not skipped.
        let okx = &sources[1];
        let doge = &okx.instruments[0];
        assert_eq!(doge.symbol, "D.O.G.E.");
        assert_eq!(doge.series.len(), 2);
        let mystery = doge
            .series
            .iter()
            .find(|s| s.dir_name == "mystery")
            .expect("undecodable entry must still be reported");
        assert_eq!(mystery.kind, SeriesKind::Unrecognised);
        assert_eq!(mystery.bytes, 10);
    }

    #[test]
    fn usage_reports_a_raw_undecodable_source_directory_name_rather_than_skipping_it() {
        let dir = TempDir::new().unwrap();
        // `%ZZ` is not a valid percent-escape, so `symbol_from_path` fails
        // and the raw on-disk name must be reported instead.
        write_sized(
            &dir.path()
                .join("sources/%ZZ/instruments/X/trades/f.parquet"),
            5,
        );
        let store = Store::new(dir.path());

        let sources = store.usage().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "%ZZ");
        assert_eq!(sources[0].bytes, 5);
    }

    #[test]
    fn delete_series_frees_bytes_and_prunes_the_now_empty_bars_directory() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let freed = store
            .delete_series("binance-spot", "BTCUSDT", "venue-1m")
            .unwrap();
        assert_eq!(freed, 500);
        assert!(
            !dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/bars")
                .exists(),
            "the now-empty bars/ directory must be pruned"
        );
        // trades/ (still non-empty) and the instrument itself must survive.
        assert!(
            dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/trades")
                .exists()
        );

        let sources = store.usage().unwrap();
        let binance = sources
            .iter()
            .find(|s| s.source_id == "binance-spot")
            .unwrap();
        assert_eq!(binance.bytes, 1_000);
    }

    #[test]
    fn delete_series_of_the_last_series_prunes_instrument_and_source() {
        let dir = TempDir::new().unwrap();
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/trades/f1.parquet"),
            42,
        );
        let store = Store::new(dir.path());

        let freed = store
            .delete_series("binance-spot", "BTCUSDT", "trades")
            .unwrap();
        assert_eq!(freed, 42);
        assert!(!dir.path().join("sources/binance-spot").exists());
        assert_eq!(store.usage().unwrap(), Vec::new());
    }

    #[test]
    fn deleting_a_nonexistent_series_is_ok_zero_not_an_error() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let freed = store
            .delete_series("binance-spot", "BTCUSDT", "venue-9999m")
            .unwrap();
        assert_eq!(freed, 0);
        // The real tree must be untouched.
        assert_eq!(store.usage().unwrap()[1].source_id, "okx");
    }

    #[test]
    fn delete_instrument_frees_every_series_and_prunes_the_source_when_it_was_the_last_one() {
        let dir = TempDir::new().unwrap();
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/trades/f1.parquet"),
            10,
        );
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m/f1.parquet"),
            20,
        );
        let store = Store::new(dir.path());

        let freed = store.delete_instrument("binance-spot", "BTCUSDT").unwrap();
        assert_eq!(freed, 30);
        assert!(!dir.path().join("sources/binance-spot").exists());
    }

    #[test]
    fn delete_instrument_leaves_a_sibling_instrument_and_the_source_alone() {
        let dir = TempDir::new().unwrap();
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/trades/f1.parquet"),
            10,
        );
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/ETHUSDT/trades/f1.parquet"),
            20,
        );
        let store = Store::new(dir.path());

        store.delete_instrument("binance-spot", "BTCUSDT").unwrap();
        assert!(dir.path().join("sources/binance-spot").exists());
        let sources = store.usage().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].instruments.len(), 1);
        assert_eq!(sources[0].instruments[0].symbol, "ETHUSDT");
    }

    #[test]
    fn delete_source_frees_everything_under_it() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let freed = store.delete_source("binance-spot").unwrap();
        assert_eq!(freed, 1_500);
        assert!(!dir.path().join("sources/binance-spot").exists());
        let sources = store.usage().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_id, "okx");
    }

    #[test]
    fn delete_series_rejects_a_traversal_attempt_in_dir_name() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let err = store
            .delete_series("binance-spot", "BTCUSDT", "..")
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidPathSegment(_)));
        // Nothing must have been touched.
        assert!(dir.path().join("sources/binance-spot").exists());
    }

    #[test]
    fn delete_series_rejects_an_embedded_separator_in_dir_name() {
        let dir = TempDir::new().unwrap();
        build_tree(dir.path());
        let store = Store::new(dir.path());

        let err = store
            .delete_series("binance-spot", "BTCUSDT", "a/b")
            .unwrap_err();
        assert!(matches!(err, StoreError::InvalidPathSegment(_)));
    }
}
