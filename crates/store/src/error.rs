//! [`StoreError`] — everything that can go wrong reading or writing a
//! series.

use std::io;
use std::path::PathBuf;

use senken_core::UnixNanos;
use senken_series::BarSpec;

/// Why a call into `senken-store` failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The underlying atomic-write layer (`senken-storage`) failed.
    #[error(transparent)]
    Storage(#[from] senken_storage::StorageError),

    /// Listing or otherwise inspecting a directory failed for a reason
    /// other than "it does not exist yet" (which is not an error — an
    /// unfetched series simply has no coverage).
    #[error("reading directory {path}: {source}")]
    Io {
        /// The directory that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: io::Error,
    },

    /// A write was rejected outright rather than silently accepted or
    /// merely warned about. Every variant here is a rows- or
    /// range-level defect that would otherwise poison every downstream
    /// reader — binary search, streaming merge, and aggregation all
    /// assume the invariants these enforce.
    #[error(transparent)]
    Rejected(#[from] WriteAssertionError),

    /// The Arrow/Parquet write or read path failed. Only constructible
    /// with the `parquet` feature enabled.
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),

    /// An Arrow array/schema operation failed. Only constructible with the
    /// `parquet` feature enabled.
    #[cfg(feature = "parquet")]
    #[error(transparent)]
    Arrow(#[from] arrow::error::ArrowError),
}

/// Why a write was rejected. Reject, never warn: a bad bar
/// here poisons every aggregation and every binary search built on top of
/// it, silently and much later.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WriteAssertionError {
    /// `bars` was empty. There is nothing to derive a file's range from,
    /// and an empty file would just be a zero-row placeholder with no
    /// purpose a caller could not express by simply not writing.
    #[error("refusing to write an empty batch of bars")]
    EmptyBatch,

    /// A bar's `ts_open` does not fall on a multiple of its spec's
    /// interval (below [`senken_series::BarUnit::Day`] — see
    /// [`WriteAssertionError::AnchorMismatch`] for Day and above).
    #[error("{ts_open} is not aligned to {spec}")]
    Misaligned {
        /// The offending bar's open time.
        ts_open: UnixNanos,
        /// The spec it was supposed to align to.
        spec: BarSpec,
    },

    /// A Day-or-above bar's `ts_open` does not match the anchor its path
    /// token declares (the anchor is part of a
    /// persisted Day-or-above series' identity).
    #[error("{ts_open} does not fall on a {spec} boundary anchored at offset {offset_nanos}ns")]
    AnchorMismatch {
        /// The offending bar's open time.
        ts_open: UnixNanos,
        /// The spec it was supposed to align to.
        spec: BarSpec,
        /// The anchor's UTC offset, in nanoseconds.
        offset_nanos: i64,
    },

    /// Two consecutive bars (after sorting is not performed — order is the
    /// caller's/M5.3 "timestamps not strictly increasing") did
    /// not strictly increase. Covers duplicates too: `next == previous` is
    /// rejected by the same check.
    #[error("timestamps are not strictly increasing: {previous} is not before {next}")]
    NotStrictlyIncreasing {
        /// The earlier of the two offending timestamps.
        previous: UnixNanos,
        /// The later (or equal, or earlier — hence the error) timestamp.
        next: UnixNanos,
    },

    /// A bar's `ts_open` falls outside the range declared for the file
    /// being written.
    #[error("{ts_open} is outside the declared range [{range_start}, {range_end})")]
    OutOfDeclaredRange {
        /// The offending bar's open time.
        ts_open: UnixNanos,
        /// The declared range's inclusive start.
        range_start: UnixNanos,
        /// The declared range's exclusive end.
        range_end: UnixNanos,
    },

    /// `high < low` for one bar — physically impossible OHLC data.
    #[error("high ({high}) is less than low ({low}) at {ts_open}")]
    HighBelowLow {
        /// The offending bar's open time.
        ts_open: UnixNanos,
        /// The reported high.
        high: i64,
        /// The reported low.
        low: i64,
    },

    /// `open` or `close` fell outside `[low, high]` for one bar.
    #[error("{field} ({value}) is outside [low, high] = [{low}, {high}] at {ts_open}")]
    OutsideLowHigh {
        /// The offending bar's open time.
        ts_open: UnixNanos,
        /// Which field was out of range.
        field: &'static str,
        /// The offending value.
        value: i64,
        /// The bar's reported low.
        low: i64,
        /// The bar's reported high.
        high: i64,
    },

    /// The new file's declared range overlaps an existing file's range
    /// without fully containing it. Files are immutable and
    /// non-overlapping; a partial overlap can only be
    /// resolved by compaction, which this stage provides only as an
    /// interface placeholder — never on the write path.
    #[error(
        "declared range [{new_start}, {new_end}) partially overlaps an existing file's \
         [{existing_start}, {existing_end}) without containing it"
    )]
    OverlapsExistingCoverage {
        /// The new file's declared start.
        new_start: UnixNanos,
        /// The new file's declared end.
        new_end: UnixNanos,
        /// The existing file's declared start.
        existing_start: UnixNanos,
        /// The existing file's declared end.
        existing_end: UnixNanos,
    },
}
