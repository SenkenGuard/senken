use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_core::TimeRange;
use senken_loader::{JobSnapshot, Phase, Requirement};
use senken_series::{Bar, Volume};

/// A bar volume and its unit on the wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum VolumeDto {
    /// Base-asset quantity actually traded.
    Real(i64),
    /// Number of price changes, not an asset quantity.
    Tick(u32),
    /// The source did not report volume.
    Absent,
}

impl From<Volume> for VolumeDto {
    fn from(volume: Volume) -> Self {
        match volume {
            Volume::Real(value) => Self::Real(value),
            Volume::Tick(value) => Self::Tick(value),
            Volume::Absent => Self::Absent,
        }
    }
}

impl From<VolumeDto> for Volume {
    fn from(volume: VolumeDto) -> Self {
        match volume {
            VolumeDto::Real(value) => Self::Real(value),
            VolumeDto::Tick(value) => Self::Tick(value),
            VolumeDto::Absent => Self::Absent,
        }
    }
}

/// `?instrument=&spec=&from=&to=` — shared by `GET /api/bars/plan` and
/// `GET /api/bars/range`. `from`/`to` are Unix nanoseconds
/// (`senken_core::UnixNanos`'s own wire representation — see that type's
/// docs — so no unit ambiguity crosses the HTTP boundary).
#[derive(Debug, Deserialize)]
pub(crate) struct BarRangeQuery {
    /// The instrument, `source:symbol`, e.g. `binance-spot:BTCUSDT`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
}

/// A half-open `[from, to)` span of time, on the wire — Unix nanoseconds at
/// both ends, matching [`BarRangeQuery`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct TimeRangeDto {
    /// Inclusive start, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end, Unix nanoseconds.
    pub to: i64,
}

impl From<TimeRange> for TimeRangeDto {
    fn from(range: TimeRange) -> Self {
        Self {
            from: range.start().as_nanos(),
            to: range.end().as_nanos(),
        }
    }
}

/// `GET /api/bars/plan` response body (pure inspection, no side effects — see `senken_loader::SeriesLoader::plan`'s own docs).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarsRequirementDto {
    /// The parts of the requested range already resolvable without
    /// fetching anything.
    pub covered: Vec<TimeRangeDto>,
    /// The parts a call to `POST /api/bars/ensure` would actually need to
    /// fetch.
    pub missing: Vec<TimeRangeDto>,
    /// How many venue-page-sized fetch chunks `missing` splits into.
    pub chunks: u32,
    /// An estimate of how many bars `missing` represents.
    pub estimated_bars: u64,
    /// An estimate, in seconds, of how long fetching `missing` would take —
    /// `None` until this loader has measured its own throughput.
    pub estimate_secs: Option<f64>,
}

impl From<Requirement> for BarsRequirementDto {
    fn from(requirement: Requirement) -> Self {
        Self {
            covered: requirement
                .covered
                .into_iter()
                .map(TimeRangeDto::from)
                .collect(),
            missing: requirement
                .missing
                .into_iter()
                .map(TimeRangeDto::from)
                .collect(),
            chunks: requirement.chunks,
            estimated_bars: requirement.estimated_bars,
            estimate_secs: requirement.estimate.map(|d| d.as_secs_f64()),
        }
    }
}

/// One OHLCV bar, on the wire: plain scaled integers, exactly
/// as stored — the series' price/quantity scale is not carried per bar (see
/// `senken_series::Bar`'s own docs), so a client must already know an
/// instrument's `price_scale`/`qty_scale` (from `senken search`/the catalog)
/// to render these as real prices.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarDto {
    /// The start of the interval this bar covers, Unix nanoseconds.
    pub ts_open: i64,
    /// The first trade price in the interval, at the series' price scale.
    pub open: i64,
    /// The highest trade price in the interval.
    pub high: i64,
    /// The lowest trade price in the interval.
    pub low: i64,
    /// The last trade price in the interval.
    pub close: i64,
    /// Base-asset volume traded in the interval, at the series' quantity
    /// scale.
    pub volume: VolumeDto,
    /// Quote-asset volume, when the venue reports it.
    pub quote_volume: Option<i64>,
    /// Number of trades in the interval, when the venue reports it.
    pub trade_count: Option<u32>,
    /// Base-asset volume bought by takers, when the venue reports it.
    pub taker_buy_volume: Option<i64>,
}

impl From<&Bar> for BarDto {
    fn from(bar: &Bar) -> Self {
        Self {
            ts_open: bar.ts_open.as_nanos(),
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume.into(),
            quote_volume: bar.quote_volume,
            trade_count: bar.trade_count,
            taker_buy_volume: bar.taker_buy_volume,
        }
    }
}

/// `GET /api/bars/range` response body: whatever is already resolvable
/// right now (the "progressive delivery: never blocks on a
/// fetch"), plus what is not — call `POST /api/bars/ensure` to start
/// filling `missing`.
///
/// Carries `price_scale`/`qty_scale` alongside the bars themselves
/// : `BarDto`'s own doc already says a client must already know an
/// instrument's `price_scale`/`qty_scale` to render these as real prices,
/// but nothing in the surface exposes a catalog lookup for
/// them — no `GET /api/instruments` exists, deliberately (this names exactly
/// four route groups: workspaces, bars, indicators, alerts). Rather than
/// leave the charts page with raw scaled integers and no way to turn them
/// into a price, this response carries the one instrument's `price_scale`/
/// `qty_scale` it already resolved to serve the request.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarRangeResponse {
    /// Bars already available, ascending by `ts_open`.
    pub bars: Vec<BarDto>,
    /// Ranges not yet resolvable.
    pub missing: Vec<TimeRangeDto>,
    /// The earliest observed bar the venue made available for this exact
    /// source/symbol/spec, if a complete short response established one.
    /// `None` means the server has not observed the edge yet — it does not
    /// mean history is unbounded.
    pub earliest_available: Option<i64>,
    /// Decimal places in a price — see `senken_marketdata::Instrument`'s own
    /// fixed-point contract. Every `BarDto` field above is `value ×
    /// 10^price_scale`.
    pub price_scale: u8,
    /// Decimal places in a quantity, the same contract for `volume` et al.
    pub qty_scale: u8,
    /// When the bar currently forming closes and the next one opens, as
    /// nanoseconds since the Unix epoch.
    ///
    /// Supplied by the server because bucket boundaries depend on the
    /// series' anchor, which the client does not carry: a venue-supplied
    /// Day-or-above series can roll over hours away from UTC midnight, and a
    /// countdown computed from a UTC floor would be wrong by that much.
    pub next_bar_open_at: i64,
}

/// `POST /api/bars/ensure` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct EnsureBarsRequest {
    /// The instrument, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
    /// `"background"`, `"prefetch"` or `"visible"` (the default) — matches
    /// `senken_loader::Priority`'s three variants exactly.
    #[serde(default)]
    pub priority: Option<String>,
}

/// `POST /api/bars/m1-download` request body.
///
/// Minute bars are requested separately from chart loading because they are
/// the canonical input for replay and simulation, not a prerequisite for
/// rendering a chart at another venue-native interval.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DownloadM1Request {
    /// The instrument, `source:symbol`.
    pub instrument: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
}

/// `POST /api/bars/ensure` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EnsureBarsResponse {
    /// Opaque; pass verbatim to `GET /api/bars/jobs/{job_id}`. Encodes which
    /// loader minted it — `senken_loader::JobId` is unique only within the
    /// loader that assigned it (see that type's own docs), never globally —
    /// so this is not simply that id's decimal text.
    pub job_id: String,
}

/// `GET /api/bars/jobs/{job_id}` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarJobDto {
    /// `"queued"`, `"downloading"`, `"writing"`, `"aggregating"` or
    /// `"done"` — mirrors `senken_loader::Phase`'s own variants.
    pub phase: String,
    /// `"background"`, `"prefetch"` or `"visible"`. A minute-history
    /// download is always background work, so it cannot jump ahead of a
    /// chart range currently being viewed.
    pub priority: String,
    /// How many fetch chunks this job's plan requires in total.
    pub chunks_total: u32,
    /// How many of those chunks have been fetched and written so far.
    pub chunks_done: u32,
    /// How many bars have been written so far, across every chunk.
    pub bars_written: u64,
    /// An estimate of the remaining time in seconds — `None` until at least
    /// one chunk has completed.
    pub estimate_secs: Option<f64>,
    /// Set while a chunk fetch is being retried after a transient failure.
    /// Not the same as the job having failed (see
    /// `senken_loader::JobSnapshot::last_error`'s own docs).
    pub last_error: Option<String>,
}

impl From<JobSnapshot> for BarJobDto {
    fn from(snapshot: JobSnapshot) -> Self {
        Self {
            phase: match snapshot.phase {
                Phase::Queued => "queued",
                Phase::Downloading => "downloading",
                Phase::Writing => "writing",
                Phase::Aggregating => "aggregating",
                Phase::Done => "done",
            }
            .to_owned(),
            priority: match snapshot.priority {
                senken_loader::Priority::Background => "background",
                senken_loader::Priority::Prefetch => "prefetch",
                senken_loader::Priority::Visible => "visible",
            }
            .to_owned(),
            chunks_total: snapshot.chunks_total,
            chunks_done: snapshot.chunks_done,
            bars_written: snapshot.bars_written,
            estimate_secs: snapshot.estimate.map(|d| d.as_secs_f64()),
            last_error: snapshot.last_error,
        }
    }
}
