use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_alerts::IndicatorSpec;

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

/// One chart display primitive emitted by an indicator.
///
/// The built-ins currently emit `series`; the other variants deliberately
/// share the response shape so an indicator that emits a zone or label does
/// not require a second rendering endpoint.
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
