use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_alerts::IndicatorSpec;
use senken_plugin_host::{CircuitState, PluginHealth, PluginLogLine, PluginLogSeverity};
use senken_runtime::plugin_host::{DynamicIndicatorState, PluginOrigin};

use super::{TimeRangeDto, VolumeDto};

/// A stored `(name, params)` pair, on the wire — mirrors
/// `senken_alerts::IndicatorSpec` field-for-field, reused (not re-declared)
/// for both alerts and indicator computation, the same ten built-ins either
/// way.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct IndicatorSpecDto {
    /// The indicator's name — see `GET /api/indicators`'s catalogue.
    pub name: String,
    /// The indicator's parameters, as JSON-object text.
    pub params: String,
}

impl From<IndicatorSpec> for IndicatorSpecDto {
    fn from(spec: IndicatorSpec) -> Self {
        Self {
            name: spec.name,
            params: spec.params,
        }
    }
}

impl From<IndicatorSpecDto> for IndicatorSpec {
    fn from(dto: IndicatorSpecDto) -> Self {
        Self {
            name: dto.name,
            params: dto.params,
        }
    }
}

/// One entry in `GET /api/indicators`'s catalogue — the ten built-ins
/// `senken-indicators` implements, described well enough that
/// a client can build a settings form without hard-coding this list a
/// second time (closing, one layer up, the exact "second source of truth"
/// the browser's own indicator maths is replaced to avoid).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorCatalogEntry {
    /// The name to pass as `indicator.name` on `POST /api/indicators/compute`
    /// (and, unchanged, as a stored alert's or workspace layer's indicator
    /// name).
    pub name: String,
    /// Full human-readable indicator name.
    pub title: String,
    /// Compact chart label.
    pub short_title: String,
    /// Legend template using parameter keys in braces.
    pub legend: String,
    /// The JSON object keys `indicator.params` must supply.
    pub params: Vec<IndicatorParamDto>,
    /// The value keys `POST /api/indicators/compute`'s response reports for
    /// this indicator's display-list fields.
    pub plots: Vec<IndicatorPlotDto>,
    /// Scale semantics reported by the indicator itself.
    pub scale: IndicatorScaleDto,
    /// Required bar-volume unit.
    pub requires_real_volume: bool,
    /// Allowed display locations.
    pub placement: IndicatorPlacementDto,
    /// Number of earlier bars used before the requested range.
    pub warmup_bars: u64,
}

/// One parameter exposed by an indicator descriptor.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorParamDto {
    /// Wire parameter key.
    pub name: String,
    /// Either `integer` or `number`.
    pub kind: String,
    /// Default value.
    pub default: IndicatorParamDefaultDto,
    /// Inclusive lower bound when the parameter has one.
    pub min: Option<f64>,
}

/// A parameter default without converting integral values to floating point.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum IndicatorParamDefaultDto {
    /// Integral default.
    Integer(u64),
    /// Fractional indicator-parameter default.
    Number(f64),
}

/// One plot an indicator emits.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorPlotDto {
    /// Wire field key.
    pub field: String,
    /// Display label.
    pub label: String,
    /// Either `line` or `histogram`.
    pub shape: String,
    /// Default CSS colour.
    pub color: String,
}

/// Scaling metadata for indicator output.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum IndicatorScaleDto {
    /// Values share the instrument price scale.
    Price,
    /// Values have their own unscaled range.
    Ratio {
        /// Inclusive lower bound.
        min: f64,
        /// Inclusive upper bound.
        max: f64,
    },
    /// Values share the instrument quantity scale.
    Volume,
    /// The indicator owns its scale.
    Own,
}

/// A chart location an indicator instance may occupy.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndicatorPlacementDto {
    Overlay,
    SubPane,
    Either,
}

/// `POST /api/indicators/compute` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ComputeIndicatorRequest {
    /// The instrument, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
    /// Which indicator to compute, and with what parameters.
    pub indicator: IndicatorSpecDto,
    /// The bar still forming, assembled by the client from live ticks and
    /// therefore not stored anywhere yet.
    ///
    /// Supplied so an indicator's newest point lands on the bar the chart is
    /// actually drawing, instead of stopping one bar behind it. It carries
    /// It carries volume as well as prices: a bar is OHLCV, and an indicator
    /// reading volume is fed from the same stream as one reading price.
    #[serde(default)]
    pub provisional: Option<ProvisionalBarDto>,
}

/// The forming bar a client is drawing, as scaled integers at the
/// instrument's own price scale — the same wire form [`super::BarDto`] uses.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ProvisionalBarDto {
    /// Start of the forming interval, Unix nanoseconds.
    pub ts_open: i64,
    /// First tick price seen in the interval.
    pub open: i64,
    /// Highest tick price seen so far.
    pub high: i64,
    /// Lowest tick price seen so far.
    pub low: i64,
    /// Most recent tick price.
    pub close: i64,
    /// Base-asset volume accumulated from the ticks seen in this interval,
    /// at the instrument's own quantity scale.
    pub volume: VolumeDto,
}

/// One point in an indicator series.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorDrawablePointDto {
    /// The bar this point was computed from, Unix nanoseconds.
    pub ts_open: i64,
    /// The indicator value at this bar.
    pub value: f64,
}

/// A chart coordinate used by a non-series drawable — mirrors
/// `senken_indicators::Point`.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorPointDto {
    /// Unix nanoseconds on the horizontal axis.
    pub time: i64,
    /// A display or decision value on the vertical axis.
    pub value: f64,
}

/// How far a segment or level extends past its anchors — mirrors
/// `senken_indicators::Extend`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndicatorExtendDto {
    /// Only between its anchors.
    None,
    /// Beyond its second anchor.
    Forward,
    /// Before its first anchor.
    Backward,
    /// In both directions.
    Both,
}

/// Where a label sits relative to its anchor — mirrors
/// `senken_indicators::LabelAnchor`. Also used by `dto::workspace`'s own
/// `DrawingKindDto::TextNote` (a text note drawing is a persisted `Label`
/// anchor the same way a computed indicator's own label output is one),
/// reused rather than re-declared a second time with a second casing
/// convention. `Deserialize` is needed for that reuse: this type started
/// out serialize-only (`POST /api/indicators/compute`'s response never
/// reads one back), but a drawing round-trips through `PUT
/// /api/layouts/{id}`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndicatorLabelAnchorDto {
    /// Above the anchor.
    Above,
    /// Below the anchor.
    Below,
    /// Centered on the anchor.
    Center,
}

/// An exact, instrument-scaled price — mirrors `senken_indicators::ScaledPrice`.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorScaledPriceDto {
    /// Integer value at `scale`.
    pub value: i64,
    /// Decimal scale of `value`.
    pub scale: u32,
}

/// A price coordinate carried by a `level` drawable — mirrors
/// `senken_indicators::PriceCoord`. No built-in indicator emits `executable`
/// today: nothing in this crate rounds a value to an instrument's tick, so
/// an executable anchor would currently have nowhere to come from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum IndicatorPriceCoordDto {
    /// A visual annotation chosen on a chart. Not an order price.
    Annotation(f64),
    /// A price which can later be used for execution.
    Executable(IndicatorScaledPriceDto),
}

/// One chart display primitive emitted by an indicator — mirrors
/// `senken_indicators::Drawable` field-for-field, so a variant this crate's
/// indicators emit never needs a translation nobody wrote.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum IndicatorDrawableDto {
    /// A calculated series.
    Series {
        /// Field key from the indicator descriptor.
        field: String,
        /// Rendering shape, e.g. `line` or `histogram`.
        shape: String,
        /// Values in chronological order.
        points: Vec<IndicatorDrawablePointDto>,
    },
    /// A line segment between two points.
    Segment {
        /// First endpoint.
        a: IndicatorPointDto,
        /// Second endpoint.
        b: IndicatorPointDto,
        /// Extension behaviour.
        extend: IndicatorExtendDto,
    },
    /// A horizontal level.
    Level {
        /// Price coordinate.
        price: IndicatorPriceCoordDto,
        /// Extension behaviour.
        extend: IndicatorExtendDto,
    },
    /// A rectangular zone.
    Box {
        /// First corner.
        a: IndicatorPointDto,
        /// Opposite corner.
        b: IndicatorPointDto,
    },
    /// Text at one chart point.
    Label {
        /// Text position.
        at: IndicatorPointDto,
        /// Text content.
        text: String,
        /// Position relative to the anchor.
        anchor: IndicatorLabelAnchorDto,
    },
}

/// One entry in `GET /api/indicators/plugins`'s listing — every dynamic
/// indicator ever registered, regardless of whether it ever finished
/// loading, mirroring `senken_runtime::plugin_host::DynamicIndicatorStatus`
/// field-for-field. Unlike `GET /api/indicators` (which only ever lists
/// what a chart may place right now), this reports a disabled, incompatible
/// or failed-to-load entry too, since re-enabling one is exactly what this
/// endpoint exists to let an operator do — and diagnosing why one broke,
/// via `health`/`logs`, is the other half of that same job.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorPluginDto {
    /// This entry's identity: a real descriptor id once one has been read,
    /// otherwise a content hash of the bytes that failed to load.
    pub id: String,
    /// This plugin's catalogue entry, in the same shape `GET
    /// /api/indicators` reports it in while active, flattened onto this
    /// object exactly the way the pre-existing field always was — `None`
    /// (and so contributing no fields at all) when `state` is
    /// `incompatible` or `failed_to_load`, neither of which ever produced
    /// one (a component that never linked has no descriptor to read).
    #[serde(flatten)]
    pub entry: Option<IndicatorCatalogEntry>,
    /// Where these bytes came from.
    pub origin: IndicatorPluginOriginDto,
    /// Which of the five user-facing states this entry is in right now.
    #[serde(flatten)]
    pub state: IndicatorPluginStateDto,
    /// This plugin's current runtime health — `None` for `incompatible`/
    /// `failed_to_load`, which never got as far as a running `Store` to
    /// have any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<PluginHealthDto>,
    /// This plugin's own ring log, oldest first — empty for the same reason
    /// `health` is `None` for `incompatible`/`failed_to_load`.
    pub logs: Vec<PluginLogLineDto>,
}

/// Where a registered dynamic indicator's bytes came from, on the wire —
/// mirrors `senken_runtime::plugin_host::PluginOrigin`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndicatorPluginOriginDto {
    /// Ships with Senken itself.
    BuiltIn,
    /// Uploaded through the Plugins page, or compiled from indicator-lang
    /// source and registered by the authoring panel's "run" action.
    Uploaded,
    /// Found under the data directory at startup, not this session's own
    /// upload.
    DataDirectory,
}

impl From<PluginOrigin> for IndicatorPluginOriginDto {
    fn from(origin: PluginOrigin) -> Self {
        match origin {
            PluginOrigin::BuiltIn => Self::BuiltIn,
            PluginOrigin::Uploaded => Self::Uploaded,
            PluginOrigin::DataDirectory => Self::DataDirectory,
        }
    }
}

/// Which of the five user-facing states one registered entry is in right
/// now, on the wire — mirrors
/// `senken_runtime::plugin_host::DynamicIndicatorState` field-for-field;
/// see that type's own docs for what each state means and what remedy it
/// calls for.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum IndicatorPluginStateDto {
    /// Loaded, enabled, and not currently tripped — calls go through.
    Active,
    /// Loaded, but a user deliberately turned it off.
    Disabled,
    /// The component names a `senken:plugin-api` version this host does not
    /// support.
    Incompatible {
        /// The version the component itself names.
        found_version: String,
        /// The version this host supports.
        supported_version: String,
    },
    /// The component never loaded at all.
    FailedToLoad {
        /// Why loading failed.
        reason: String,
    },
    /// Loaded successfully at least once, but disabled by its own circuit
    /// breaker after repeated traps. Distinct from `Disabled`: a user did
    /// not choose this, and it stays this way until they explicitly
    /// re-enable it (`POST /api/indicators/plugins/{name}/enabled` with
    /// `enabled: true`) — never on its own, however long it has been
    /// tripped.
    AutoDisabled {
        /// The reason the breaker recorded when it tripped.
        reason: String,
    },
}

impl From<DynamicIndicatorState> for IndicatorPluginStateDto {
    fn from(state: DynamicIndicatorState) -> Self {
        match state {
            DynamicIndicatorState::Active => Self::Active,
            DynamicIndicatorState::Disabled => Self::Disabled,
            DynamicIndicatorState::Incompatible {
                found_version,
                supported_version,
            } => Self::Incompatible {
                found_version,
                supported_version,
            },
            DynamicIndicatorState::FailedToLoad { reason } => Self::FailedToLoad { reason },
            DynamicIndicatorState::AutoDisabled { reason } => Self::AutoDisabled { reason },
        }
    }
}

/// One plugin's runtime health, on the wire — mirrors
/// `senken_plugin_host::PluginHealth` field-for-field.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PluginHealthDto {
    /// Every call that returned a trap, for the lifetime of the plugin.
    pub trap_count: u64,
    /// The subset of `trap_count` caused specifically by exceeding a live
    /// wall-clock deadline.
    pub deadline_exceeded_count: u64,
    /// The highest linear-memory size, in bytes, any single instance of
    /// this plugin was ever granted.
    pub peak_memory_bytes: u64,
    /// Whether the shared circuit breaker currently allows calls through,
    /// and why not if it does not.
    pub circuit: PluginCircuitStateDto,
}

impl From<PluginHealth> for PluginHealthDto {
    fn from(health: PluginHealth) -> Self {
        Self {
            trap_count: health.trap_count,
            deadline_exceeded_count: health.deadline_exceeded_count,
            // `usize` has no infallible `Into<u64>` (its width is
            // platform-defined); `try_from` with a saturating fallback
            // keeps this a checked conversion rather than an `as` cast that
            // would silently wrap on a hypothetical >64-bit platform.
            peak_memory_bytes: u64::try_from(health.peak_memory_bytes).unwrap_or(u64::MAX),
            circuit: health.circuit.into(),
        }
    }
}

/// A circuit breaker's current state, on the wire — mirrors
/// `senken_plugin_host::CircuitState`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum PluginCircuitStateDto {
    /// Calls are allowed through.
    Closed,
    /// Disabled after repeated traps, until a user explicitly re-enables
    /// this plugin — never on its own (see
    /// `senken_plugin_host`'s own circuit-breaker docs).
    Open {
        /// Why the breaker tripped.
        reason: String,
    },
}

impl From<CircuitState> for PluginCircuitStateDto {
    fn from(state: CircuitState) -> Self {
        match state {
            CircuitState::Closed => Self::Closed,
            CircuitState::Open { reason } => Self::Open { reason },
        }
    }
}

/// How urgent one plugin log line is, on the wire — mirrors
/// `senken_plugin_host::PluginLogSeverity`.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginLogSeverityDto {
    /// The guest's own `stdout`, or an ordinary host-recorded note.
    Info,
    /// The guest's own `stderr`, or a host-recorded trap or circuit-breaker
    /// event.
    Warn,
}

impl From<PluginLogSeverity> for PluginLogSeverityDto {
    fn from(severity: PluginLogSeverity) -> Self {
        match severity {
            PluginLogSeverity::Info => Self::Info,
            PluginLogSeverity::Warn => Self::Warn,
        }
    }
}

/// One line in a plugin's ring log, on the wire — mirrors
/// `senken_plugin_host::PluginLogLine`.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PluginLogLineDto {
    /// When this line was recorded, as nanoseconds since the Unix epoch.
    pub timestamp: i64,
    /// How urgent it is.
    pub severity: PluginLogSeverityDto,
    /// The line's own text, already stripped of its trailing newline.
    pub message: String,
}

impl From<PluginLogLine> for PluginLogLineDto {
    fn from(line: PluginLogLine) -> Self {
        Self {
            timestamp: line.timestamp.as_nanos(),
            severity: line.severity.into(),
            message: line.message,
        }
    }
}

/// `POST /api/indicators/plugins/{name}/enabled` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetIndicatorPluginEnabledRequest {
    /// The desired enabled state.
    pub enabled: bool,
}

/// `POST /api/indicators/compile` request body: indicator-lang source, as
/// the authoring panel's editor holds it right now.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CompileIndicatorRequest {
    /// The program, in the language `senken-indicator-lang` implements.
    pub source: String,
}

/// `POST /api/indicators/compile`'s error body for a mistake in the
/// trader's own source (a `senken_indicator_lang::CompileError::Syntax` or
/// `::Type`) — line and column and message exactly as the compiler reports
/// them, never collapsed into the crate's usual one-line [`ErrorBody`](crate::dto::ErrorBody):
/// the authoring panel places this at the offending line, which a flattened
/// string cannot drive.
///
/// Not used for [`senken_indicator_lang::CompileError::Internal`] — that
/// variant names a bug in the compiler, not in anything the trader wrote,
/// so it is reported as an ordinary [`ErrorBody`](crate::dto::ErrorBody)-shaped `500` instead of a
/// line this source never had.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct CompileIndicatorErrorDto {
    /// One-based line the problem starts on.
    pub line: u32,
    /// One-based column the problem starts on.
    pub column: u32,
    /// What was wrong, in the language a trader writing an indicator uses.
    pub message: String,
}

/// `POST /api/indicators/compute` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ComputeIndicatorResponse {
    /// The complete display list for this indicator item.
    pub display: Vec<IndicatorDrawableDto>,
    /// Number of oldest bounded objects discarded before this response.
    pub discarded_objects: usize,
    /// Ranges `display` could not cover because the underlying bars are not
    /// resolvable yet — the same contract `BarRangeResponse::missing` makes.
    pub missing: Vec<TimeRangeDto>,
    /// `true` when the requested start had insufficient earlier bars to
    /// prepare a fully warmed calculation.
    pub warmup_truncated: bool,
}
