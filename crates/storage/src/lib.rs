//! Atomic, versioned JSON storage rooted in a single data directory.
//!
//! See the crate README for an overview. The API is synchronous on purpose:
//! it makes no assumptions about an async runtime, so it can be used from a
//! plain binary, a test, or wrapped in `spawn_blocking` by an async caller.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tracing::info;

mod error;

pub use crate::error::StorageError;

/// A value together with the schema version it was written under and when.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Snapshot<T> {
    /// Layout version of `data`. Bump it whenever `T` changes incompatibly.
    pub schema_version: u32,
    /// When the snapshot was taken.
    pub created_at: DateTime<Utc>,
    /// The payload.
    pub data: T,
}

impl<T> Snapshot<T> {
    /// Wraps `data`, stamping it with `schema_version` and the current time.
    #[must_use]
    pub fn new(schema_version: u32, data: T) -> Self {
        Self {
            schema_version,
            created_at: Utc::now(),
            data,
        }
    }

    /// Time elapsed since the snapshot was taken. Never negative.
    #[must_use]
    pub fn age(&self) -> Duration {
        Utc::now()
            .signed_duration_since(self.created_at)
            .to_std()
            .unwrap_or(Duration::ZERO)
    }

    /// `true` once the snapshot is at least `ttl` old.
    #[must_use]
    pub fn is_stale(&self, ttl: Duration) -> bool {
        self.age() >= ttl
    }
}

/// The on-disk shape of a [`Snapshot`], with `data` left unparsed so the
/// version can be checked before paying to deserialise the payload.
#[derive(Deserialize)]
struct SnapshotHeader<'a> {
    schema_version: u32,
    created_at: DateTime<Utc>,
    #[serde(borrow)]
    data: &'a RawValue,
}

/// A data directory. All operations take paths relative to it.
///
/// # Examples
///
/// ```
/// use senken_storage::{Snapshot, Storage};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let dir = tempfile::tempdir()?;
/// let storage = Storage::new(dir.path());
/// storage.init()?;
///
/// storage.write_snapshot("prices/btc.json", &Snapshot::new(1, vec![42_u64]))?;
/// let back = storage
///     .read_snapshot::<Vec<u64>>("prices/btc.json", 1)?
///     .expect("just written");
/// assert_eq!(back.data, [42]);
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Storage {
    data_dir: PathBuf,
}

impl Storage {
    /// Points at `data_dir` without touching the filesystem. Call [`init`](Self::init)
    /// before writing.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// The directory every relative path resolves against.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Creates the data directory (and parents) if missing.
    ///
    /// # Errors
    /// [`StorageError::Io`] if the directory cannot be created.
    pub fn init(&self) -> Result<(), StorageError> {
        fs::create_dir_all(&self.data_dir).map_err(|source| StorageError::Io {
            path: self.data_dir.clone(),
            source,
        })?;
        info!(data_dir = %self.data_dir.display(), "storage ready");
        Ok(())
    }

    fn resolve(&self, rel: impl AsRef<Path>) -> Result<PathBuf, StorageError> {
        let rel = rel.as_ref();
        // `has_root`, not `is_absolute`. On Windows a path is absolute only
        // with a prefix *and* a root, so `/etc/passwd` is not absolute
        // there — yet `Path::join` still "replaces everything except for the
        // prefix (if any) of self" for a rooted path, landing the result at
        // `C:\etc\passwd`, outside the data directory. `has_root` is exactly
        // the predicate for "joining this discards my base", which is the
        // question this guard is actually asking, and it agrees with
        // `is_absolute` on Unix.
        if rel.has_root() {
            return Err(StorageError::NotRelative(rel.to_path_buf()));
        }
        if rel
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(StorageError::Escapes(rel.to_path_buf()));
        }
        Ok(self.data_dir.join(rel))
    }

    /// `true` if `rel` names an existing regular file.
    ///
    /// # Errors
    /// If `rel` is absolute or escapes the data directory.
    pub fn exists(&self, rel: impl AsRef<Path>) -> Result<bool, StorageError> {
        Ok(self.resolve(rel)?.is_file())
    }

    /// Reads a whole file. A missing file is `Ok(None)`, not an error.
    ///
    /// # Errors
    /// If `rel` is invalid or the read fails for any reason other than the
    /// file not existing.
    pub fn read_bytes(&self, rel: impl AsRef<Path>) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.resolve(rel)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(StorageError::Io { path, source }),
        }
    }

    /// Writes a whole file atomically: readers see either the old contents or
    /// the new, never a partial write. Parent directories are created.
    ///
    /// # Errors
    /// If `rel` is invalid or any filesystem step fails. On failure the
    /// temporary file is removed and the destination is untouched.
    pub fn write_bytes(&self, rel: impl AsRef<Path>, bytes: &[u8]) -> Result<(), StorageError> {
        let path = self.resolve(rel)?;

        let parent = path.parent().unwrap_or(&self.data_dir);
        fs::create_dir_all(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })?;

        let tmp = temp_path_for(&path);
        let write_tmp = || -> std::io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(bytes)?;
            // `sync_all` is fsync on Linux and F_FULLFSYNC on macOS —
            // Apple's only true durability barrier, slower but correct.
            // Writes here are rare and callers run them off the hot path.
            file.sync_all()
        };
        if let Err(source) = write_tmp() {
            let _ = fs::remove_file(&tmp);
            return Err(StorageError::Io { path: tmp, source });
        }

        if let Err(source) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(StorageError::Io {
                path: path.clone(),
                source,
            });
        }

        // The rename is only durable once the directory entry is flushed.
        sync_dir(parent).map_err(|source| StorageError::Io {
            path: parent.to_path_buf(),
            source,
        })
    }

    /// Deletes a file. A missing file is not an error.
    ///
    /// # Errors
    /// If `rel` is invalid or the removal fails for any reason other than the
    /// file not existing.
    pub fn remove(&self, rel: impl AsRef<Path>) -> Result<(), StorageError> {
        let path = self.resolve(rel)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::Io { path, source }),
        }
    }

    /// Reads and parses a JSON file. A missing file is `Ok(None)`.
    ///
    /// # Errors
    /// [`StorageError::Decode`] if the file exists but is not valid `T`;
    /// otherwise as [`read_bytes`](Self::read_bytes).
    pub fn read_json<T: DeserializeOwned>(
        &self,
        rel: impl AsRef<Path>,
    ) -> Result<Option<T>, StorageError> {
        let path = self.resolve(&rel)?;
        let Some(bytes) = self.read_bytes(&rel)? else {
            return Ok(None);
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| StorageError::Decode { path, source })
    }

    /// Serialises `value` as compact JSON and writes it atomically.
    ///
    /// Compact, not pretty: these files are read by machines on every warm
    /// start, and the whitespace costs real bytes and parse time at that
    /// frequency. Pipe through `jq` to look inside one.
    ///
    /// # Errors
    /// [`StorageError::Encode`] if `value` cannot be serialised; otherwise as
    /// [`write_bytes`](Self::write_bytes).
    pub fn write_json<T: Serialize + ?Sized>(
        &self,
        rel: impl AsRef<Path>,
        value: &T,
    ) -> Result<(), StorageError> {
        let path = self.resolve(&rel)?;
        let bytes =
            serde_json::to_vec(value).map_err(|source| StorageError::Encode { path, source })?;
        self.write_bytes(&rel, &bytes)
    }

    /// Reads a [`Snapshot`] written by [`write_snapshot`](Self::write_snapshot).
    ///
    /// The version is checked *before* the payload is parsed, so a snapshot in
    /// an old layout is reported as [`StorageError::SchemaMismatch`] rather
    /// than as a confusing decode failure. The file is never modified.
    ///
    /// # Errors
    /// [`StorageError::SchemaMismatch`] when the on-disk version differs from
    /// `expected_version`; [`StorageError::Decode`] when the file is not a
    /// snapshot of `T`; otherwise as [`read_bytes`](Self::read_bytes).
    pub fn read_snapshot<T: DeserializeOwned>(
        &self,
        rel: impl AsRef<Path>,
        expected_version: u32,
    ) -> Result<Option<Snapshot<T>>, StorageError> {
        let path = self.resolve(&rel)?;
        let Some(bytes) = self.read_bytes(&rel)? else {
            return Ok(None);
        };

        let header: SnapshotHeader<'_> =
            serde_json::from_slice(&bytes).map_err(|source| StorageError::Decode {
                path: path.clone(),
                source,
            })?;

        if header.schema_version != expected_version {
            return Err(StorageError::SchemaMismatch {
                path,
                found: header.schema_version,
                expected: expected_version,
            });
        }

        let data = serde_json::from_str(header.data.get())
            .map_err(|source| StorageError::Decode { path, source })?;

        Ok(Some(Snapshot {
            schema_version: header.schema_version,
            created_at: header.created_at,
            data,
        }))
    }

    /// Writes a [`Snapshot`] atomically.
    ///
    /// # Errors
    /// As [`write_json`](Self::write_json).
    pub fn write_snapshot<T: Serialize>(
        &self,
        rel: impl AsRef<Path>,
        snapshot: &Snapshot<T>,
    ) -> Result<(), StorageError> {
        self.write_json(rel, snapshot)
    }
}

/// A sibling path that is unique per process *and* per call, so two threads
/// writing the same file never share a temp file.
fn temp_path_for(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    path.with_file_name(format!(".{file_name}.{}.{seq}.tmp", std::process::id()))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    // Directories cannot be opened for syncing on Windows; rename is already
    // atomic there with respect to other readers.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        n: u32,
    }

    fn temp_storage() -> (TempDir, Storage) {
        let dir = TempDir::new().unwrap();
        let s = Storage::new(dir.path());
        s.init().unwrap();
        (dir, s)
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let (_dir, s) = temp_storage();
        let got: Option<Sample> = s.read_json("a/b/c.json").unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn write_then_read_creates_parent_dirs() {
        let (_dir, s) = temp_storage();
        let v = Sample {
            name: "btc".into(),
            n: 7,
        };
        s.write_json("marketdata/sources/binance-spot/instruments.json", &v)
            .unwrap();

        let got: Option<Sample> = s
            .read_json("marketdata/sources/binance-spot/instruments.json")
            .unwrap();
        assert_eq!(got, Some(v));
    }

    #[test]
    fn writes_land_inside_data_dir_and_leave_no_temp_files() {
        let (_dir, s) = temp_storage();
        s.write_json(
            "x/y.json",
            &Sample {
                name: "a".into(),
                n: 1,
            },
        )
        .unwrap();
        assert!(s.data_dir().join("x/y.json").is_file());

        let leftovers: Vec<_> = fs::read_dir(s.data_dir().join("x"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn concurrent_writers_to_one_path_never_share_a_temp_file() {
        let paths: HashSet<_> = (0..64)
            .map(|_| temp_path_for(Path::new("/x/instruments.json")))
            .collect();
        assert_eq!(paths.len(), 64);
    }

    #[test]
    fn concurrent_writes_leave_a_complete_file() {
        let (_dir, s) = temp_storage();
        let s = std::sync::Arc::new(s);
        let handles: Vec<_> = (0..8_u32)
            .map(|i| {
                let s = std::sync::Arc::clone(&s);
                std::thread::spawn(move || {
                    let payload = vec![i; 50_000];
                    s.write_json("shared.json", &payload).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let got: Vec<u32> = s.read_json("shared.json").unwrap().unwrap();
        assert_eq!(got.len(), 50_000);
        assert!(got.iter().all(|&v| v == got[0]), "interleaved write");
    }

    #[test]
    fn json_on_disk_is_compact() {
        let (_dir, s) = temp_storage();
        s.write_json(
            "compact.json",
            &Sample {
                name: "btc".into(),
                n: 7,
            },
        )
        .unwrap();
        let bytes = s.read_bytes("compact.json").unwrap().unwrap();
        assert!(
            !bytes.contains(&b'\n'),
            "cache files are parsed on every warm start; no pretty-printing"
        );
    }

    #[test]
    fn corrupt_json_is_a_decode_error_not_none() {
        let (_dir, s) = temp_storage();
        s.write_bytes("bad.json", b"{ not json").unwrap();
        let err = s.read_json::<Sample>("bad.json").unwrap_err();
        assert!(matches!(err, StorageError::Decode { .. }));
    }

    #[test]
    fn absolute_and_parent_paths_are_rejected() {
        let (_dir, s) = temp_storage();
        assert!(matches!(
            s.read_json::<Sample>("/etc/passwd").unwrap_err(),
            StorageError::NotRelative(_)
        ));
        assert!(matches!(
            s.read_json::<Sample>("../escape.json").unwrap_err(),
            StorageError::Escapes(_)
        ));
        assert!(matches!(
            s.read_json::<Sample>("ok/../../escape.json").unwrap_err(),
            StorageError::Escapes(_)
        ));
    }

    /// The property the guard above exists for, asserted directly rather
    /// than through one platform's spelling of it: whatever `resolve`
    /// returns must be inside the data directory.
    ///
    /// The test above is written in terms of *which error* each input
    /// produces, which made it silently platform-specific — `/etc/passwd`
    /// is not absolute on Windows, so it escaped there while the Unix
    /// assertions kept passing. This one holds on every platform because it
    /// checks the outcome instead: a hostile input is either rejected, or
    /// it resolves somewhere it is allowed to be. Inputs that are merely
    /// odd filenames on one platform and rooted paths on another satisfy it
    /// either way.
    #[test]
    fn no_input_ever_resolves_outside_the_data_dir() {
        let (_dir, s) = temp_storage();
        for hostile in [
            "/etc/passwd",
            "\\windows\\system32\\config\\sam",
            "../escape.json",
            "ok/../../escape.json",
            "C:/Windows/system.ini",
            "//server/share/file",
        ] {
            match s.resolve(hostile) {
                Err(_) => {}
                Ok(resolved) => assert!(
                    resolved.starts_with(s.data_dir()),
                    "{hostile:?} resolved to {}, outside {}",
                    resolved.display(),
                    s.data_dir().display()
                ),
            }
        }
    }

    #[test]
    fn snapshot_with_matching_version_is_read() {
        let (_dir, s) = temp_storage();
        let snap = Snapshot::new(1, vec![1_u32, 2, 3]);
        s.write_snapshot("snap.json", &snap).unwrap();

        let got: Snapshot<Vec<u32>> = s.read_snapshot("snap.json", 1).unwrap().unwrap();
        assert_eq!(got.data, vec![1, 2, 3]);
        assert_eq!(got.created_at, snap.created_at);
    }

    #[test]
    fn snapshot_with_other_version_is_reported_and_left_in_place() {
        let (_dir, s) = temp_storage();
        s.write_snapshot("snap.json", &Snapshot::new(1, vec![1_u32]))
            .unwrap();

        let err = s.read_snapshot::<Vec<u32>>("snap.json", 2).unwrap_err();
        assert!(matches!(
            err,
            StorageError::SchemaMismatch {
                found: 1,
                expected: 2,
                ..
            }
        ));
        assert!(s.exists("snap.json").unwrap(), "read must never delete");
    }

    #[test]
    fn version_is_checked_before_the_payload_is_parsed() {
        let (_dir, s) = temp_storage();
        // `data` is not a Vec<u32>; a version mismatch must still win.
        s.write_bytes(
            "snap.json",
            br#"{"schema_version":1,"created_at":"2026-01-01T00:00:00Z","data":{"old":"shape"}}"#,
        )
        .unwrap();
        let err = s.read_snapshot::<Vec<u32>>("snap.json", 2).unwrap_err();
        assert!(matches!(err, StorageError::SchemaMismatch { .. }));
    }

    #[test]
    fn missing_snapshot_is_not_an_error() {
        let (_dir, s) = temp_storage();
        let got: Option<Snapshot<Vec<u32>>> = s.read_snapshot("missing/snapshot.json", 1).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn snapshot_age_comes_from_created_at() {
        let snap = Snapshot::new(1, ());
        assert!(!snap.is_stale(Duration::from_hours(1)));
        assert!(snap.is_stale(Duration::ZERO));
    }

    #[test]
    fn removing_a_missing_file_is_not_an_error() {
        let (_dir, s) = temp_storage();
        s.remove("missing/file.json").unwrap();
    }
}
