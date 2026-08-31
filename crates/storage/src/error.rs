use std::path::PathBuf;

/// Everything that can go wrong inside [`Storage`](crate::Storage).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// The caller passed an absolute path; all paths are relative to the data directory.
    #[error("path must be relative to the data directory: {0}")]
    NotRelative(PathBuf),

    /// The caller passed a path containing `..`.
    #[error("path must not escape the data directory: {0}")]
    Escapes(PathBuf),

    /// The underlying filesystem operation failed.
    #[error("io failed at {path}")]
    Io {
        /// Path the operation targeted.
        path: PathBuf,
        /// The failing OS call.
        #[source]
        source: std::io::Error,
    },

    /// A value could not be serialised to JSON.
    #[error("failed to encode json for {path}")]
    Encode {
        /// Destination path.
        path: PathBuf,
        /// Serialisation failure.
        #[source]
        source: serde_json::Error,
    },

    /// File contents were not the JSON shape the caller asked for.
    #[error("failed to decode json from {path}")]
    Decode {
        /// Source path.
        path: PathBuf,
        /// Parse failure.
        #[source]
        source: serde_json::Error,
    },

    /// A snapshot exists but was written under a different schema version.
    ///
    /// The file is left untouched; the caller decides whether to migrate,
    /// discard, or refetch.
    #[error("snapshot at {path} has schema version {found}, expected {expected}")]
    SchemaMismatch {
        /// Snapshot path.
        path: PathBuf,
        /// Version recorded in the file.
        found: u32,
        /// Version the caller expects.
        expected: u32,
    },
}
