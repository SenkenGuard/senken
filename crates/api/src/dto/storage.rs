//! Wire shapes for `GET /api/storage` and `POST /api/storage/delete` —
//! what Senken is holding on disk, and reclaiming it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_store::{InstrumentUsage, SeriesKind, SeriesUsage, SourceUsage};

/// One series' kind, on the wire — a plain string rather than a tagged
/// object, since (unlike `LayerKindDto`) nothing here carries per-kind
/// data the client needs back; the spec/origin/anchor that produced it are
/// already folded into [`StorageSeriesDto::label`].
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StorageSeriesKindDto {
    /// A `bars/{origin}-{spec}[@anchor]` directory whose name decoded.
    Bars,
    /// The one `trades` directory an instrument can have.
    Trades,
    /// A directory `usage()` could not decode as either shape — still
    /// counted, reported under its raw on-disk name.
    Unrecognised,
}

impl From<&SeriesKind> for StorageSeriesKindDto {
    fn from(kind: &SeriesKind) -> Self {
        match kind {
            SeriesKind::Bars { .. } => Self::Bars,
            SeriesKind::Trades => Self::Trades,
            SeriesKind::Unrecognised => Self::Unrecognised,
        }
    }
}

/// The human-readable label for one series — what a person reads, not
/// what a caller passes back (that is [`StorageSeriesDto::id`]).
fn series_label(kind: &SeriesKind, dir_name: &str) -> String {
    match kind {
        SeriesKind::Bars {
            origin,
            spec,
            anchor,
        } => {
            let venue_offset_hours = -anchor.offset_nanos() / 3_600_000_000_000;
            if anchor.offset_nanos() == 0 {
                format!("{spec} · {origin}")
            } else {
                // Cosmetic only (never round-tripped) — a whole-hour
                // rounding is enough to tell two anchors on the same
                // nominal spec apart, the case this exists for at all
                // (see `senken_store::paths`' own docs on why the anchor
                // is part of a Day-or-above series' identity).
                format!("{spec} · {origin} (UTC{venue_offset_hours:+})")
            }
        }
        SeriesKind::Trades => "Trades".to_owned(),
        SeriesKind::Unrecognised => dir_name.to_owned(),
    }
}

/// One series' usage, as reported to a client.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StorageSeriesDto {
    /// The on-disk directory name — pass this back as `series_id` to
    /// delete exactly this series.
    pub id: String,
    /// A human-readable label: the spec and origin for bars (e.g.
    /// `"1m · venue"`), `"Trades"` for trades, or the raw directory name
    /// for anything unrecognised.
    pub label: String,
    /// What this series is.
    pub kind: StorageSeriesKindDto,
    /// Total bytes of every real file under this series' directory.
    pub bytes: u64,
    /// Total file count under this series' directory.
    pub files: u64,
}

impl From<SeriesUsage> for StorageSeriesDto {
    fn from(usage: SeriesUsage) -> Self {
        Self {
            label: series_label(&usage.kind, &usage.dir_name),
            kind: StorageSeriesKindDto::from(&usage.kind),
            id: usage.dir_name,
            bytes: usage.bytes,
            files: usage.files,
        }
    }
}

/// One instrument's usage, as reported to a client.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StorageInstrumentDto {
    /// The decoded symbol (or the raw on-disk directory name when it
    /// could not be decoded — still reported, never skipped).
    pub symbol: String,
    /// Total bytes across every series under this instrument.
    pub bytes: u64,
    /// Total file count across every series under this instrument.
    pub files: u64,
    /// Every series under this instrument, biggest first.
    pub series: Vec<StorageSeriesDto>,
}

impl From<InstrumentUsage> for StorageInstrumentDto {
    fn from(usage: InstrumentUsage) -> Self {
        Self {
            symbol: usage.symbol,
            bytes: usage.bytes,
            files: usage.files,
            series: usage
                .series
                .into_iter()
                .map(StorageSeriesDto::from)
                .collect(),
        }
    }
}

/// One source's usage, as reported to a client.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StorageSourceDto {
    /// The decoded source id (or the raw on-disk directory name when it
    /// could not be decoded — still reported, never skipped).
    pub source_id: String,
    /// Total bytes across every instrument under this source.
    pub bytes: u64,
    /// Total file count across every instrument under this source.
    pub files: u64,
    /// Every instrument under this source, biggest first.
    pub instruments: Vec<StorageInstrumentDto>,
}

impl From<SourceUsage> for StorageSourceDto {
    fn from(usage: SourceUsage) -> Self {
        Self {
            source_id: usage.source_id,
            bytes: usage.bytes,
            files: usage.files,
            instruments: usage
                .instruments
                .into_iter()
                .map(StorageInstrumentDto::from)
                .collect(),
        }
    }
}

/// The market-data half of `GET /api/storage`: every source, instrument
/// and series `senken-store` is holding under `sources/`, plus the totals
/// across all of them.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MarketDataUsageDto {
    /// Total bytes across every source.
    pub total_bytes: u64,
    /// Total file count across every source.
    pub total_files: u64,
    /// Every source, biggest first.
    pub sources: Vec<StorageSourceDto>,
}

impl From<Vec<SourceUsage>> for MarketDataUsageDto {
    fn from(sources: Vec<SourceUsage>) -> Self {
        let total_bytes = sources.iter().map(|s| s.bytes).sum();
        let total_files = sources.iter().map(|s| s.files).sum();
        Self {
            total_bytes,
            total_files,
            sources: sources.into_iter().map(StorageSourceDto::from).collect(),
        }
    }
}

/// One SQLite database this server keeps, reported as a single figure —
/// everything but market data lives in the accounts database, and gets no
/// fake tree of its own the way `senken-store`'s Parquet layout does.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StorageDatabaseDto {
    /// What this database is, for a person reading the report (e.g.
    /// `"Accounts"`).
    pub label: String,
    /// The database's file path on disk.
    pub path: String,
    /// The file's size, plus its `-wal`/`-shm` siblings when present —
    /// SQLite's write-ahead log can hold real, not-yet-checkpointed data.
    pub bytes: u64,
}

/// `GET /api/storage` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct StorageReportDto {
    /// The data directory everything below is rooted under.
    pub data_dir: String,
    /// Market data: Parquet-backed bar and trade series, with their full
    /// source/instrument/series breakdown.
    pub market_data: MarketDataUsageDto,
    /// Every other database this server keeps, each as a single figure.
    pub databases: Vec<StorageDatabaseDto>,
}

/// `POST /api/storage/delete` request body. Naming only `source_id`
/// deletes the whole source; adding `symbol` narrows that to one
/// instrument; adding `series_id` too narrows it to one series.
/// `series_id` with no `symbol` is rejected — there is no whole-source
/// concept of "this one series" to delete.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DeleteStorageRequest {
    /// The source to delete from (or delete entirely, if nothing else is
    /// given).
    pub source_id: String,
    /// Narrows the delete to one instrument under `source_id`.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Narrows the delete to one series under `symbol` — the `id` a
    /// [`StorageSeriesDto`] reported. Requires `symbol` to be given too.
    #[serde(default)]
    pub series_id: Option<String>,
}

/// `POST /api/storage/delete` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DeleteStorageResponse {
    /// Bytes actually freed by the delete.
    pub freed_bytes: u64,
}
