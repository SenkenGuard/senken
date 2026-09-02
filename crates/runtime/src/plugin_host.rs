//! Bridges Senken's own domain types to the WIT wire shapes
//! `senken-plugin-host` speaks: the catalog of indicators loaded from
//! uploaded `.wasm` components ([`DynamicIndicators`]), and the catalog of
//! venues loaded the same way ([`DynamicVenues`](crate::plugin_host::DynamicVenues)), presented as ordinary
//! [`senken_marketdata::source::MarketDataSource`]/[`senken_plugin::BarSource`]
//! implementations so the rest of the application never has to know a given
//! source came from a `.wasm` file.
//!
//! This lives here, not in `senken-plugin-host` or `senken-plugin-api`,
//! because it needs a domain crate (`senken-series` for [`senken_series::Bar`],
//! `senken-marketdata` for [`senken_marketdata::instrument::Instrument`], and
//! `senken-indicators` for the exact vocabulary a built-in's own descriptor
//! already uses) and the plugin runtime at the same time.
//! `senken-plugin-api` must never depend on a domain crate — a published
//! SDK must not ship a domain crate's implementation alongside it — and
//! `senken-plugin-host`'s own domain dependency exists for one purpose only
//! (backing the `builtins` WIT import with real indicator state machines,
//! and the `http` import with a real `senken_venue::VenueClient`). Neither
//! crate is the right place for a *second*, unrelated use of both at once,
//! so it lives at the one layer that already depends on everything: the
//! runtime.
//!
//! # Why an uploaded indicator's plotted values line up with a raw scaled
//! integer, not a real price
//!
//! `senken_indicators::convert::scaled_to_f64` (private to that crate, so
//! not linkable from here) is a plain widening cast — it never divides by a
//! price or quantity scale, because the scale lives on a series' own
//! metadata, not on a bar, and every built-in already computes in these
//! unscaled units (a client divides by `10^price_scale` only when it
//! renders a value on screen). A dynamic indicator must compute over the
//! exact same numbers a built-in would see, or "matches the equivalent
//! built-in over the same bars" — this slice's own proof of done — could
//! never hold. [`crate::plugin_host::bar_to_wit`] therefore carries every
//! price and quantity across the boundary at a fixed nominal scale paired
//! with the bar's own raw integer, unchanged: the guest side of
//! `wit/senken.wit`'s `builtins` import already reads a `scaled` value the
//! same way (an "already-extracted price", per that interface's own doc
//! comment), so this is not a shortcut — it is the one convention the
//! whole system already uses.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use senken_core::{TimeRange, UnixNanos};
use senken_indicators::{
    Drawable, Extend, LabelAnchor, ParamDefault, ParamKind, PlotShape, Point, PriceCoord,
    SeriesShape,
};
use senken_marketdata::SourceSymbol;
use senken_marketdata::instrument::{Instrument, InstrumentStatus};
use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_plugin::BarSource;
use senken_plugin_host::{
    Bar as WitBar, BarSpec as WitBarSpec, BarUnit as WitBarUnit, CircuitState,
    CompiledIndicatorInstance, Drawable as WitDrawable, Extend as WitExtend,
    FetchError as WitFetchError, IndicatorDescriptor as WitIndicatorDescriptor,
    LabelAnchor as WitLabelAnchor, LoadedCompiledIndicator, LoadedPlugin, LoadedVenuePlugin,
    ParamKind as WitParamKind, ParamValue as WitParamValue, PlotShape as WitPlotShape,
    PluginHealth, PluginHost, PluginHostError, PluginInstance, PluginLimits, PluginLogLine,
    PriceCoord as WitPriceCoord, Scaled as WitScaled, SeriesShape as WitSeriesShape,
    VenueCallError, VenueError as WitVenueError, VenueInstrument as WitVenueInstrument,
    Volume as WitVolume,
};
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{LimitGroup, VenueClient};
use sha2::{Digest, Sha256};

/// The `scaled.scale` this bridge uses for every price and quantity field —
/// see this module's own doc comment for why the real instrument scale
/// would be the wrong thing to send here.
const NOMINAL_SCALE: u8 = 0;

/// The non-series display-object cap applied to every dynamic indicator.
///
/// `wit/senken.wit`'s `indicator-descriptor` has no field for this — unlike
/// a built-in, whose own `IndicatorDescriptor::max_display_objects` is
/// curated per indicator, a plugin author cannot declare one, so this is a
/// single host policy applied to every uploaded indicator alike. Chosen to
/// match the built-ins' own typical cap (`crates/indicators/src/
/// descriptor.rs`'s `DESCRIPTORS` entries are all `500`), not measured
/// against any real plugin workload.
pub const DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS: usize = 500;

/// One [`DynamicIndicators::spawn`]'d instance's fuel budget for its entire
/// lifetime, generous for the hundreds of bars one `POST
/// /api/indicators/compute` range replays: `wit/senken.wit`'s `builtins`
/// bridge does a handful of floating-point operations per bar, so this
/// ceiling is reached only by a plugin that is genuinely runaway, which is
/// exactly what fuel exists to catch.
const DYNAMIC_INDICATOR_FUEL_BUDGET: u64 = 50_000_000;

/// Turns a non-zero `discarded` count (from a
/// `senken_indicators::DisplayList` built with
/// [`DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS`]) into a rejection.
///
/// A built-in's own display list silently keeps the newest objects and
/// reports how many older ones it dropped (`ComputeIndicatorResponse::
/// discarded_objects`) — acceptable for ten curated indicators whose
/// authors already know the limit. An uploaded, untrusted plugin gets the
/// stronger failure mode this slice's own plan calls for: **rejected with
/// a message**, not a chart that quietly stops drawing some of its
/// objects.
///
/// # Errors
/// [`DynamicIndicatorError::TooManyDisplayObjects`] if `discarded` is
/// greater than zero.
pub fn reject_if_over_display_cap(
    indicator: &str,
    produced: usize,
    discarded: usize,
) -> Result<(), DynamicIndicatorError> {
    if discarded == 0 {
        return Ok(());
    }
    Err(DynamicIndicatorError::TooManyDisplayObjects {
        indicator: indicator.to_owned(),
        produced,
        limit: DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS,
    })
}

/// Converts a [`Bar`] and the [`BarSpec`] its series was resolved under into
/// the WIT `bar` record a plugin's `handle-bar` accepts.
///
/// # Errors
/// [`DynamicIndicatorError::UnsupportedBarUnit`] if `spec.unit` is not one
/// of the six units `wit/senken.wit`'s closed `bar-unit` enum names —
/// unreachable today (`senken_series::BarUnit` currently has exactly those
/// six too) but `BarUnit` is `#[non_exhaustive]` precisely so a future
/// aggregation unit can be added without a schema change, and the WIT
/// enum, frozen by this slice's own scope, cannot grow to match it. A
/// guessed mapping would silently misreport a bar's timeframe to every
/// plugin; refusing is the honest alternative.
pub fn bar_to_wit(bar: &Bar, spec: BarSpec) -> Result<WitBar, DynamicIndicatorError> {
    Ok(WitBar {
        ts_open: bar.ts_open.as_nanos(),
        spec: bar_spec_to_wit(spec)?,
        open: scaled(bar.open),
        high: scaled(bar.high),
        low: scaled(bar.low),
        close: scaled(bar.close),
        volume: volume_to_wit(bar.volume),
        quote_volume: bar.quote_volume.map(scaled),
        trade_count: bar.trade_count,
        taker_buy_volume: bar.taker_buy_volume.map(scaled),
    })
}

const fn scaled(value: i64) -> WitScaled {
    WitScaled {
        scale: NOMINAL_SCALE,
        value,
    }
}

fn bar_spec_to_wit(spec: BarSpec) -> Result<WitBarSpec, DynamicIndicatorError> {
    Ok(WitBarSpec {
        step: spec.step.get(),
        unit: bar_unit_to_wit(spec.unit)?,
    })
}

fn bar_unit_to_wit(unit: BarUnit) -> Result<WitBarUnit, DynamicIndicatorError> {
    Ok(match unit {
        BarUnit::Second => WitBarUnit::Second,
        BarUnit::Minute => WitBarUnit::Minute,
        BarUnit::Hour => WitBarUnit::Hour,
        BarUnit::Day => WitBarUnit::Day,
        BarUnit::Week => WitBarUnit::Week,
        BarUnit::Month => WitBarUnit::Month,
        other => {
            return Err(DynamicIndicatorError::UnsupportedBarUnit(format!(
                "{other:?}"
            )));
        }
    })
}

fn volume_to_wit(volume: Volume) -> WitVolume {
    match volume {
        Volume::Real(value) => WitVolume::Real(scaled(value)),
        Volume::Tick(count) => WitVolume::Tick(count),
        Volume::Absent => WitVolume::Absent,
    }
}

/// The reverse of [`bar_unit_to_wit`] — total, because `wit/senken.wit`'s
/// `bar-unit` enum is a closed, exact copy of `BarUnit`'s six variants
/// today, so every value it can carry has a domain counterpart. Unlike the
/// forward direction, there is no "unit added to `BarUnit` since" case to
/// guard against here: a *guest*-declared unit can never name a seventh
/// case the WIT enum itself does not have.
fn bar_unit_from_wit(unit: WitBarUnit) -> BarUnit {
    match unit {
        WitBarUnit::Second => BarUnit::Second,
        WitBarUnit::Minute => BarUnit::Minute,
        WitBarUnit::Hour => BarUnit::Hour,
        WitBarUnit::Day => BarUnit::Day,
        WitBarUnit::Week => BarUnit::Week,
        WitBarUnit::Month => BarUnit::Month,
    }
}

/// The reverse of [`bar_spec_to_wit`]. `None` only for a `step` of `0` — a
/// venue plugin's own bug, since `wit/senken.wit` cannot express
/// `BarSpec`'s own "always at least one" invariant in its plain `u32`
/// field the way `senken_series::BarSpec::step`'s `NonZeroU32` does at the
/// type level.
fn bar_spec_from_wit(spec: WitBarSpec) -> Option<BarSpec> {
    Some(BarSpec {
        step: NonZeroU32::new(spec.step)?,
        unit: bar_unit_from_wit(spec.unit),
    })
}

/// The reverse of [`volume_to_wit`]. Every raw integer crosses back
/// unchanged, for the same reason [`bar_to_wit`]'s own doc comment states
/// for the forward direction — a venue plugin's `bars` call carries its
/// own venue-native scale per field exactly like a dynamic indicator's
/// `bar` does, and `senken_series::Bar` itself never carries a scale at
/// all (a series' own metadata does, resolved elsewhere), so there is
/// nothing to divide by here.
fn volume_from_wit(volume: WitVolume) -> Volume {
    match volume {
        WitVolume::Real(value) => Volume::Real(value.value),
        WitVolume::Tick(count) => Volume::Tick(count),
        WitVolume::Absent => Volume::Absent,
    }
}

/// Converts one `wit/senken.wit` `bar` (as a [`LoadedVenuePlugin::bars`]
/// call returns it) into a [`Bar`] — the mirror image of [`bar_to_wit`],
/// minus that function's own `spec` argument: a venue plugin's `bar`
/// record carries its own `spec` field, which this bridge does not need
/// (the caller already knows which spec it asked for) and does not
/// validate against it — a venue plugin that answered a different spec
/// than it was asked for is a bug this bridge has no way to detect from
/// one bar alone.
fn bar_from_wit(bar: &WitBar) -> Bar {
    Bar {
        ts_open: UnixNanos::from_nanos(bar.ts_open),
        open: bar.open.value,
        high: bar.high.value,
        low: bar.low.value,
        close: bar.close.value,
        volume: volume_from_wit(bar.volume),
        quote_volume: bar.quote_volume.map(|value| value.value),
        trade_count: bar.trade_count,
        taker_buy_volume: bar.taker_buy_volume.map(|value| value.value),
    }
}

/// One configurable parameter, described in the owned form a plugin loaded
/// at runtime needs — a built-in's own [`senken_indicators::ParamSpec`]
/// borrows `&'static str`/`&'static [_]`, which nothing loaded from an
/// uploaded `.wasm` byte string can produce. [`ParamKind`] and
/// [`ParamDefault`] themselves are reused unchanged: neither borrows
/// anything, so there is nothing about them a dynamic indicator needs to
/// restate.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicParamSpec {
    /// Wire key, matched positionally against the plugin's own declared
    /// order when a call is built for its `constructor`.
    pub name: String,
    /// Parameter shape.
    pub kind: ParamKind,
    /// Default value shown to a client.
    pub default: ParamDefault,
    /// Inclusive lower bound when applicable.
    pub min: Option<f64>,
}

/// One emitted plot, in the same owned form as [`DynamicParamSpec`].
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicPlotSpec {
    /// Wire key — the same string an `on-bar-result`'s `plot-value.field`
    /// carries for this plot.
    pub field: String,
    /// Display label.
    pub label: String,
    /// Default rendering shape.
    pub shape: PlotShape,
    /// Default CSS colour.
    pub color: String,
}

/// A dynamic indicator's descriptor, translated to the same vocabulary a
/// built-in's [`senken_indicators::IndicatorDescriptor`] already uses
/// wherever the WIT contract carries an equivalent field.
///
/// `wit/senken.wit`'s `indicator-descriptor` has no counterpart for
/// `scale`/`requires`/`placement`/`smoothing`/`max_display_objects` — a
/// plugin author cannot declare any of them — so a catalog entry built from
/// this is presented with a stated, conservative default for each instead
/// of an invented one: see `crates/api/src/indicator_handlers.rs`'s own
/// catalog-entry conversion for exactly what those defaults are and why.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicIndicatorInfo {
    /// Identity. Rejected at registration if it collides (case-
    /// insensitively) with a built-in's own id — see [`DynamicIndicators::register`].
    pub id: String,
    /// Human-readable name.
    pub title: String,
    /// Compact label for chart chrome.
    pub short_title: String,
    /// Legend template.
    pub legend: String,
    /// Configurable parameters, in the plugin's own declared order — the
    /// same order [`DynamicIndicators::spawn`] must supply values in, since
    /// the WIT `constructor` takes a plain positional `list<param-value>`.
    pub params: Vec<DynamicParamSpec>,
    /// Values emitted for each initialized bar.
    pub plots: Vec<DynamicPlotSpec>,
}

fn param_kind_from_wit(kind: WitParamKind) -> ParamKind {
    match kind {
        WitParamKind::Integer => ParamKind::Integer,
        WitParamKind::Number => ParamKind::Number,
    }
}

fn param_default_from_wit(value: WitParamValue) -> ParamDefault {
    match value {
        WitParamValue::Integer(value) => ParamDefault::Integer(value),
        WitParamValue::Number(value) => ParamDefault::Number(value),
    }
}

fn plot_shape_from_wit(shape: WitPlotShape) -> PlotShape {
    match shape {
        WitPlotShape::Line => PlotShape::Line,
        WitPlotShape::Histogram => PlotShape::Histogram,
    }
}

fn info_from_descriptor(descriptor: &WitIndicatorDescriptor) -> DynamicIndicatorInfo {
    DynamicIndicatorInfo {
        id: descriptor.id.clone(),
        title: descriptor.title.clone(),
        short_title: descriptor.short_title.clone(),
        legend: descriptor.legend.clone(),
        params: descriptor
            .params
            .iter()
            .map(|param| DynamicParamSpec {
                name: param.name.clone(),
                kind: param_kind_from_wit(param.kind),
                default: param_default_from_wit(param.default),
                min: param.min,
            })
            .collect(),
        plots: descriptor
            .plots
            .iter()
            .map(|plot| DynamicPlotSpec {
                field: plot.field.clone(),
                label: plot.label.clone(),
                shape: plot_shape_from_wit(plot.shape),
                color: plot.color.clone(),
            })
            .collect(),
    }
}

/// The wire key [`DynamicOnBar::plots`] reports a compiled indicator-lang
/// program's single plotted value under. The language has exactly one
/// `plot` expression per program (see `crates/indicator-lang`'s own
/// grammar), so unlike a built-in or a Rust-authored plugin — either of
/// which may declare several named plot fields — there is only ever one
/// field to name, and this is that name.
const COMPILED_INDICATOR_PLOT_FIELD: &str = "value";

/// The default line colour assigned to every compiled indicator-lang
/// program's one plot. The language has no way to declare a colour of its
/// own — see [`synthesize_compiled_info`]'s own doc comment — so every
/// compiled indicator gets the same one; chosen to match
/// `crates/indicators/src/descriptor.rs`'s own `VALUE_LINE`, the exact plot
/// every single-valued built-in a compiled program can call (`Sma`, `Ema`,
/// `Wma`, `Rsi`, `Atr`, `Vwap`) already uses for its own "value" field, so a
/// compiled indicator does not stand out as visually foreign next to one.
const COMPILED_INDICATOR_COLOR: &str = "#f2f2ef";

/// Builds the [`DynamicIndicatorInfo`] for a component compiled from
/// indicator-lang source, from nothing but its own compiled bytes.
///
/// `wit/senken.wit`'s `compiled-indicator` world exports a bare `on-bar`
/// function and nothing else — the language has no syntax for a title, an
/// id, or a parameter (whatever a trader wrote, such as `ema(close, 20)`'s
/// period, is already baked into the compiled bytes rather than left
/// runtime-configurable) — so every field here is synthesised by this
/// bridge rather than read out of the component the way
/// [`info_from_descriptor`] reads a real descriptor:
///
/// - `id` is a content hash of the compiled bytes. `senken_indicator_lang::
///   compile` already guarantees identical source compiles to
///   byte-identical output, so this makes recompiling the same program
///   idempotent — the same "re-uploading the same id replaces the earlier
///   registration" contract [`DynamicIndicators::register`] already
///   documents for an uploaded `.wasm` file — without needing a second
///   channel (a request field, a client-chosen name) this task's own
///   surface does not have room for.
/// - `params` is always empty, for the reason above: there is nothing left
///   to configure at spawn time.
/// - `plots` is always the one field [`COMPILED_INDICATOR_PLOT_FIELD`]
///   names, since the language has exactly one `plot` expression per
///   program.
fn synthesize_compiled_info(wasm: &[u8]) -> DynamicIndicatorInfo {
    let digest = Sha256::digest(wasm);
    let short_hash = digest.iter().take(4).fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    let id = format!("Compiled-{short_hash}");
    DynamicIndicatorInfo {
        title: format!("Compiled indicator {short_hash}"),
        short_title: "Compiled".to_owned(),
        legend: id.clone(),
        id,
        params: Vec::new(),
        plots: vec![DynamicPlotSpec {
            field: COMPILED_INDICATOR_PLOT_FIELD.to_owned(),
            label: "VALUE".to_owned(),
            shape: PlotShape::Line,
            color: COMPILED_INDICATOR_COLOR.to_owned(),
        }],
    }
}

/// The plain magnitude `wit/senken.wit`'s `compiled-indicator` world's
/// `on-bar` takes for volume.
///
/// That world's flat ABI has no channel for [`Volume`]'s real/tick/absent
/// distinction the way `indicator-plugin`'s own `bar` record does (see
/// [`volume_to_wit`]) — a compiled indicator-lang program only ever calls
/// the `volume` built-in with one bare number, exactly as
/// `senken_plugin_host`'s own host-side bridge for that built-in always
/// wraps its incoming `f64` as `Volume::Real` before calling
/// `senken_indicators::Volume::handle_bar`. So both a real traded quantity
/// and a tick count are widened to this one number here, and a bar with no
/// reported volume becomes zero — the honest floor of what this world's
/// ABI can carry, not a shortcut this bridge introduces.
fn bar_volume_magnitude(volume: Volume) -> f64 {
    match volume {
        Volume::Real(value) => scaled_to_f64_lossy(scaled(value)),
        Volume::Tick(count) => f64::from(count),
        Volume::Absent => 0.0,
    }
}

fn point_from_wit(point: senken_plugin_host::PlotPoint) -> Point {
    Point {
        time: point.time,
        value: point.value,
    }
}

fn extend_from_wit(extend: WitExtend) -> Extend {
    match extend {
        WitExtend::None => Extend::None,
        WitExtend::Forward => Extend::Forward,
        WitExtend::Backward => Extend::Backward,
        WitExtend::Both => Extend::Both,
    }
}

fn label_anchor_from_wit(anchor: WitLabelAnchor) -> LabelAnchor {
    match anchor {
        WitLabelAnchor::Above => LabelAnchor::Above,
        WitLabelAnchor::Below => LabelAnchor::Below,
        WitLabelAnchor::Center => LabelAnchor::Center,
    }
}

fn series_shape_from_wit(shape: WitSeriesShape) -> SeriesShape {
    match shape {
        WitSeriesShape::Line => SeriesShape::Line,
        WitSeriesShape::Histogram => SeriesShape::Histogram,
        WitSeriesShape::Area => SeriesShape::Area,
    }
}

fn price_coord_from_wit(price: WitPriceCoord) -> PriceCoord {
    match price {
        WitPriceCoord::Annotation(value) => PriceCoord::Annotation(value),
        // The nominal scale this bridge assigns every crossing (see this
        // module's doc comment) is meaningless as an *executable* price: an
        // order price must be the instrument's real scale, which a plugin
        // has no channel to learn. An executable level from a dynamic
        // indicator is therefore reported as the annotation it actually is
        // rather than a `ScaledPrice` that would silently claim a
        // tradable precision it does not have.
        WitPriceCoord::Executable(value) => PriceCoord::Annotation(scaled_to_f64_lossy(value)),
    }
}

/// Widens a WIT `scaled` back to a plain display number, for the
/// annotation fallback [`price_coord_from_wit`] uses. Never used for
/// anything that trades — see that function's own doc comment.
///
/// `num_traits::cast` rather than a bare `as f64`: the same widening
/// `senken_indicators::convert::scaled_to_f64` performs (exact up to 2^53,
/// which this module's own doc comment already accepts for every other
/// field this bridge carries), but `clippy::cast_precision_loss` is an
/// allowed exception for `senken-indicators` alone, never for this crate —
/// so the conversion goes through a function call instead of the `as`
/// operator, the same way `senken-plugin-host`'s own `bar_field_from_f64`
/// converts the other direction.
fn scaled_to_f64_lossy(value: WitScaled) -> f64 {
    num_traits::cast(value.value).unwrap_or(0.0)
}

/// Converts one `wit/senken.wit` `drawable` into the domain
/// [`Drawable`] every consumer of an indicator's display output already
/// speaks (`senken_indicators::DisplayList`, `crates/api/src/
/// indicator_handlers.rs`'s own wire conversion).
fn drawable_from_wit(drawable: &WitDrawable) -> Drawable {
    match drawable {
        WitDrawable::Series(series) => Drawable::Series {
            field: series.field.clone(),
            shape: series_shape_from_wit(series.shape),
            points: series.points.iter().copied().map(point_from_wit).collect(),
        },
        WitDrawable::Segment(segment) => Drawable::Segment {
            a: point_from_wit(segment.a),
            b: point_from_wit(segment.b),
            extend: extend_from_wit(segment.extend),
        },
        WitDrawable::Level(level) => Drawable::Level {
            price: price_coord_from_wit(level.price),
            extend: extend_from_wit(level.extend),
        },
        WitDrawable::Box(box_drawable) => Drawable::Box {
            a: point_from_wit(box_drawable.a),
            b: point_from_wit(box_drawable.b),
        },
        WitDrawable::Label(label) => Drawable::Label {
            at: point_from_wit(label.at),
            text: label.text.clone(),
            anchor: label_anchor_from_wit(label.anchor),
        },
    }
}

/// What handling one bar produced from a dynamic indicator, translated to
/// the same domain vocabulary [`crate::plugin_host`]'s built-in-facing
/// types already use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DynamicOnBar {
    /// This bar's value for each plot field the indicator reports, keyed by
    /// the plugin's own declared field name (see [`DynamicPlotSpec::field`]).
    pub plots: Vec<(String, f64)>,
    /// Any new display objects this bar produced.
    pub drawables: Vec<Drawable>,
}

/// Why loading, registering or running a dynamic indicator failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DynamicIndicatorError {
    /// The plugin runtime itself refused to load or run the component —
    /// carries `senken_plugin_host::PluginHostError`'s own message.
    #[error(transparent)]
    Host(#[from] PluginHostError),
    /// `id` collides, case-insensitively, with a built-in's own id.
    #[error("`{0}` is already a built-in indicator; a dynamic indicator cannot reuse its id")]
    CollidesWithBuiltin(String),
    /// No plugin is registered under this id.
    #[error("no dynamic indicator plugin is registered as `{0}`")]
    UnknownPlugin(String),
    /// The plugin is registered but currently disabled.
    #[error("the dynamic indicator `{0}` is currently disabled")]
    Disabled(String),
    /// The parameters supplied for `indicator` could not be matched against
    /// its own declared [`DynamicParamSpec`]s.
    #[error("invalid parameters for dynamic indicator `{indicator}`: {reason}")]
    InvalidParams {
        /// The indicator whose parameters could not be read.
        indicator: String,
        /// Why the parameters were rejected.
        reason: String,
    },
    /// A bar's [`BarUnit`] has no counterpart in `wit/senken.wit`'s closed
    /// `bar-unit` enum — see [`bar_to_wit`]'s own doc comment.
    #[error("bar unit {0} has no counterpart in the plugin ABI")]
    UnsupportedBarUnit(String),
    /// The bar range handed to a dynamic indicator produced more display
    /// objects than [`DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS`] allows.
    #[error(
        "dynamic indicator `{indicator}` produced {produced} display objects, over the limit of {limit}"
    )]
    TooManyDisplayObjects {
        /// The indicator that exceeded the limit.
        indicator: String,
        /// How many non-series display objects were discarded past the
        /// limit.
        produced: usize,
        /// [`DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS`].
        limit: usize,
    },
    /// `id` is registered but never finished loading (it is
    /// [`DynamicIndicatorState::Incompatible`] or
    /// [`DynamicIndicatorState::FailedToLoad`]), so it has no enabled flag
    /// for [`DynamicIndicators::set_enabled`] to toggle.
    #[error("`{0}` never finished loading and has nothing to enable or disable")]
    NotToggleable(String),
}

/// Either shape a registered component can take, once
/// [`DynamicIndicators::register`] has proven it loads: a Rust-authored
/// plugin against `indicator-plugin` (with its own real descriptor), or a
/// component compiled from indicator-lang source against the leaner
/// `compiled-indicator` (whose descriptor [`synthesize_compiled_info`]
/// invents, since the component itself carries none).
enum LoadedIndicator {
    Plugin(LoadedPlugin),
    Compiled(LoadedCompiledIndicator),
}

impl LoadedIndicator {
    fn health(&self) -> PluginHealth {
        match self {
            Self::Plugin(plugin) => plugin.health(),
            Self::Compiled(compiled) => compiled.health(),
        }
    }

    fn logs(&self) -> Vec<PluginLogLine> {
        match self {
            Self::Plugin(plugin) => plugin.logs(),
            Self::Compiled(compiled) => compiled.logs(),
        }
    }

    /// Explicitly closes this plugin's circuit breaker — the "re-enable"
    /// remedy [`DynamicIndicators::set_enabled`] applies whenever a caller
    /// asks for `enabled: true`, since the breaker never clears itself (see
    /// `senken_plugin_host::circuit`'s own docs for why a guest trap gets no
    /// timer-based recovery the way a venue's rate limit does).
    fn reset_circuit_breaker(&self) {
        match self {
            Self::Plugin(plugin) => plugin.reset_circuit_breaker(),
            Self::Compiled(compiled) => compiled.reset_circuit_breaker(),
        }
    }
}

/// Where a registered plugin's bytes came from. Each origin implies a
/// different remedy when the plugin ends up
/// [`DynamicIndicatorState::Incompatible`] or
/// [`DynamicIndicatorState::FailedToLoad`] — a user can replace a file they
/// uploaded themselves, but a built-in or a file already sitting in the
/// data directory needs a different conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginOrigin {
    /// Ships with Senken itself.
    BuiltIn,
    /// Uploaded through the Plugins page, or compiled from indicator-lang
    /// source and registered by the authoring panel's "run" action.
    Uploaded,
    /// Found under the data directory at startup, not this session's own
    /// upload.
    DataDirectory,
}

/// Which of the five user-facing states one registered entry is in right
/// now — see this crate's own design record for why these must stay
/// distinguishable rather than collapsed into a single "broken" bucket:
/// each demands a different action from whoever is looking at the Plugins
/// page.
#[derive(Debug, Clone, PartialEq)]
pub enum DynamicIndicatorState {
    /// Loaded, enabled, and not currently tripped — calls go through.
    Active,
    /// Loaded, but a user deliberately turned it off.
    Disabled,
    /// The component names a `senken:plugin-api` version this host does
    /// not support. The fix is recompiling against the version this host
    /// supports (or upgrading Senken), never re-reading a log.
    Incompatible {
        /// The version the component itself names.
        found_version: String,
        /// The version this host supports.
        supported_version: String,
    },
    /// The component never loaded at all — malformed bytes, an unknown
    /// world, a capability it reached for that this host never grants.
    /// Kept and shown rather than discarded, with the reason, so an
    /// uploaded file that failed does not simply vanish.
    FailedToLoad {
        /// Why loading failed.
        reason: String,
    },
    /// Loaded successfully at least once, but disabled by its own circuit
    /// breaker after repeated traps. Distinct from `Disabled`: a user did
    /// not choose this, and re-enabling is their call to make once they
    /// have read why.
    AutoDisabled {
        /// The reason the breaker recorded when it tripped.
        reason: String,
    },
}

/// A successfully loaded registration's own fields, boxed inside
/// [`DynamicIndicatorEntry::Loaded`] so that variant — the only one holding
/// a whole [`LoadedIndicator`] — does not force the other two, much
/// smaller variants to pay for its size in every [`HashMap`] slot.
struct LoadedEntry {
    plugin: LoadedIndicator,
    info: DynamicIndicatorInfo,
    enabled: bool,
    origin: PluginOrigin,
}

enum DynamicIndicatorEntry {
    Loaded(Box<LoadedEntry>),
    /// The component names an unsupported `senken:plugin-api` version.
    /// Keyed by a content hash (see [`content_hash_id`]) since a component
    /// that never linked has no descriptor to read a real id from.
    Incompatible {
        origin: PluginOrigin,
        found_version: String,
        supported_version: String,
    },
    /// The component never loaded at all, under either world this crate
    /// tries. Keyed the same way as `Incompatible`, for the same reason.
    FailedToLoad {
        origin: PluginOrigin,
        reason: String,
    },
}

/// A short, stable identity for a registration this bridge could not turn
/// into a real descriptor: the component's own content hash, so an operator
/// still has something to key a failed entry by, and re-uploading the exact
/// same broken bytes replaces the earlier failed entry rather than piling
/// up duplicates — the same contract [`DynamicIndicators::register`]
/// already documents for a component that *does* load. Shares
/// [`synthesize_compiled_info`]'s own hashing approach for the same reason
/// that function has it.
fn content_hash_id(prefix: &str, wasm: &[u8]) -> String {
    let digest = Sha256::digest(wasm);
    let short_hash = digest.iter().take(4).fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    format!("{prefix}-{short_hash}")
}

/// Indicators loaded at runtime from an uploaded `.wasm` component,
/// alongside `senken_indicators::DESCRIPTORS`'s ten built-ins.
///
/// Disabling an entry removes it from [`Self::catalog`] immediately without
/// discarding the loaded component — enabling it again needs no re-upload.
/// A chart layer that already placed a disabled indicator keeps its own
/// `indicator_name`/`params` regardless (that state lives in
/// `senken_workspace::Layer`, entirely outside this type), which is what
/// lets the client show a placeholder and restore the real thing the moment
/// this catalog offers the id again.
///
/// A registration that never loaded is kept too, not discarded — see
/// [`DynamicIndicatorState`]'s own doc comment for why a failed upload must
/// still show up on the Plugins page rather than silently vanishing.
///
/// Cheap to clone: every clone shares the same [`PluginHost`] and the same
/// registered-entry table.
#[derive(Clone)]
pub struct DynamicIndicators {
    host: PluginHost,
    entries: Arc<RwLock<HashMap<String, DynamicIndicatorEntry>>>,
}

impl std::fmt::Debug for DynamicIndicators {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        f.debug_struct("DynamicIndicators")
            .field("registered", &count)
            .finish_non_exhaustive()
    }
}

/// One registered entry's full status — [`DynamicIndicators::all`]'s own
/// return shape, for the Plugins page: list, detail, per-plugin log and
/// runtime health all read from this one type.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicIndicatorStatus {
    /// This entry's identity: a real descriptor id once one has been read,
    /// otherwise a content hash of the bytes that failed (see this crate's
    /// own `content_hash_id`).
    pub id: String,
    /// The plugin's own descriptor, translated to domain vocabulary —
    /// `None` for [`DynamicIndicatorState::Incompatible`] or
    /// [`DynamicIndicatorState::FailedToLoad`], neither of which ever
    /// produced one.
    pub info: Option<DynamicIndicatorInfo>,
    /// Where these bytes came from.
    pub origin: PluginOrigin,
    /// Which of the five user-facing states this entry is in right now.
    pub state: DynamicIndicatorState,
    /// This plugin's current runtime health — `None` for
    /// `Incompatible`/`FailedToLoad`, which never got as far as a `Store`
    /// to have any.
    pub health: Option<PluginHealth>,
    /// This plugin's own ring log, oldest first — empty for
    /// `Incompatible`/`FailedToLoad` for the same reason `health` is
    /// `None` for them.
    pub logs: Vec<PluginLogLine>,
}

impl DynamicIndicators {
    /// Builds an empty catalog with its own capability-zero, memory-capped
    /// plugin host.
    ///
    /// # Errors
    /// If the underlying `senken_plugin_host::PluginHost` cannot be built —
    /// see that type's own `new` for when that happens.
    pub fn new() -> Result<Self, DynamicIndicatorError> {
        Ok(Self {
            host: PluginHost::new(PluginLimits::default())?,
            entries: Arc::default(),
        })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, DynamicIndicatorEntry>> {
        self.entries.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, DynamicIndicatorEntry>> {
        self.entries.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Loads `wasm` as an [`PluginOrigin::Uploaded`] registration — see
    /// [`Self::register_with_origin`] for the full contract. This is the
    /// entry point both the upload endpoint and the indicator-lang
    /// authoring panel's "run" action already use, and both hand this
    /// bridge nothing but the bytes themselves, so `Uploaded` is the
    /// correct origin for either caller.
    ///
    /// # Errors
    /// See [`Self::register_with_origin`].
    pub fn register(&self, wasm: &[u8]) -> Result<DynamicIndicatorInfo, DynamicIndicatorError> {
        self.register_with_origin(wasm, PluginOrigin::Uploaded)
    }

    /// Loads `wasm`, registering it under its own descriptor id, enabled by
    /// default. Re-uploading the same id replaces the earlier registration
    /// outright — the mechanism a plugin author uses to ship a fixed build.
    ///
    /// `wasm` may implement either `wit/senken.wit` world this crate's own
    /// `senken_plugin_host` can load: a Rust-authored plugin against
    /// `indicator-plugin` is tried first, and only a component that world
    /// rejects (because it does not export the `indicator` interface at
    /// all) is tried again against the leaner `compiled-indicator` world —
    /// what `senken_indicator_lang::compile` produces.
    ///
    /// A component satisfying neither world is never simply dropped: it is
    /// still recorded, as either [`DynamicIndicatorState::Incompatible`] (if
    /// either attempt named an unsupported `senken:plugin-api` version) or
    /// [`DynamicIndicatorState::FailedToLoad`] (both worlds' own reasons,
    /// combined) — see [`DynamicIndicators::all`] to read it back. This
    /// call still fails for the immediate caller either way; the point is
    /// that the *next* look at the catalog still finds it.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::CollidesWithBuiltin`] if the descriptor's id
    /// matches one of the ten built-ins (case-insensitively, the same match
    /// `senken_indicators::descriptor` itself uses) — a dynamic indicator
    /// must never shadow a curated one, since a client resolves a name
    /// against exactly one of the two catalogs; [`DynamicIndicatorError::Host`]
    /// if the component fails to load against both worlds.
    pub fn register_with_origin(
        &self,
        wasm: &[u8],
        origin: PluginOrigin,
    ) -> Result<DynamicIndicatorInfo, DynamicIndicatorError> {
        let (plugin, info) = match self.host.load(wasm) {
            Ok(plugin) => {
                let info = info_from_descriptor(plugin.descriptor());
                (LoadedIndicator::Plugin(plugin), info)
            }
            Err(plugin_error) => match self.host.load_compiled(wasm) {
                Ok(compiled) => (
                    LoadedIndicator::Compiled(compiled),
                    synthesize_compiled_info(wasm),
                ),
                Err(compiled_error) => {
                    return self.record_failed_registration(
                        wasm,
                        origin,
                        &plugin_error,
                        &compiled_error,
                    );
                }
            },
        };
        if senken_indicators::descriptor(&info.id).is_some() {
            return Err(DynamicIndicatorError::CollidesWithBuiltin(info.id));
        }
        let mut entries = self.write();
        entries.insert(
            info.id.clone(),
            DynamicIndicatorEntry::Loaded(Box::new(LoadedEntry {
                plugin,
                info: info.clone(),
                enabled: true,
                origin,
            })),
        );
        Ok(info)
    }

    /// Records a registration that loaded under neither world, as either
    /// `Incompatible` or `FailedToLoad` depending on what the two attempts
    /// actually said — see [`Self::register_with_origin`]'s own doc
    /// comment for why this is recorded rather than only returned.
    fn record_failed_registration(
        &self,
        wasm: &[u8],
        origin: PluginOrigin,
        plugin_error: &PluginHostError,
        compiled_error: &PluginHostError,
    ) -> Result<DynamicIndicatorInfo, DynamicIndicatorError> {
        // Either attempt naming an unsupported version is the more specific,
        // more actionable diagnosis — in practice both name the same
        // version, since both loads see the same bytes.
        for error in [plugin_error, compiled_error] {
            if let PluginHostError::Incompatible { found, supported } = error {
                let id = content_hash_id("Incompatible", wasm);
                self.write().insert(
                    id,
                    DynamicIndicatorEntry::Incompatible {
                        origin,
                        found_version: found.clone(),
                        supported_version: supported.clone(),
                    },
                );
                return Err(DynamicIndicatorError::Host(PluginHostError::Incompatible {
                    found: found.clone(),
                    supported: supported.clone(),
                }));
            }
        }
        let reason = format!(
            "not a valid `indicator-plugin` component ({plugin_error}), and not a valid \
             `compiled-indicator` component either ({compiled_error})"
        );
        let id = content_hash_id("FailedToLoad", wasm);
        self.write().insert(
            id,
            DynamicIndicatorEntry::FailedToLoad {
                origin,
                reason: reason.clone(),
            },
        );
        Err(DynamicIndicatorError::Host(PluginHostError::Load(reason)))
    }

    /// The catalog offered to a chart right now: every registered plugin
    /// currently enabled, in no particular order — the caller (`GET
    /// /api/indicators`) merges this into the built-in list.
    #[must_use]
    pub fn catalog(&self) -> Vec<DynamicIndicatorInfo> {
        self.read()
            .values()
            .filter_map(|entry| match entry {
                DynamicIndicatorEntry::Loaded(loaded) if loaded.enabled => {
                    Some(loaded.info.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Every registered entry, regardless of state — including one that
    /// never loaded at all. See [`DynamicIndicatorStatus`] for the shape.
    #[must_use]
    pub fn all(&self) -> Vec<DynamicIndicatorStatus> {
        self.read()
            .iter()
            .map(|(id, entry)| Self::status_for(id, entry))
            .collect()
    }

    fn status_for(id: &str, entry: &DynamicIndicatorEntry) -> DynamicIndicatorStatus {
        match entry {
            DynamicIndicatorEntry::Loaded(loaded) => {
                let health = loaded.plugin.health();
                let state = if !loaded.enabled {
                    DynamicIndicatorState::Disabled
                } else if let CircuitState::Open { reason } = &health.circuit {
                    DynamicIndicatorState::AutoDisabled {
                        reason: reason.clone(),
                    }
                } else {
                    DynamicIndicatorState::Active
                };
                DynamicIndicatorStatus {
                    id: id.to_owned(),
                    info: Some(loaded.info.clone()),
                    origin: loaded.origin,
                    state,
                    logs: loaded.plugin.logs(),
                    health: Some(health),
                }
            }
            DynamicIndicatorEntry::Incompatible {
                origin,
                found_version,
                supported_version,
            } => DynamicIndicatorStatus {
                id: id.to_owned(),
                info: None,
                origin: *origin,
                state: DynamicIndicatorState::Incompatible {
                    found_version: found_version.clone(),
                    supported_version: supported_version.clone(),
                },
                health: None,
                logs: Vec::new(),
            },
            DynamicIndicatorEntry::FailedToLoad { origin, reason } => DynamicIndicatorStatus {
                id: id.to_owned(),
                info: None,
                origin: *origin,
                state: DynamicIndicatorState::FailedToLoad {
                    reason: reason.clone(),
                },
                health: None,
                logs: Vec::new(),
            },
        }
    }

    /// This id's descriptor, regardless of enabled state — `None` if
    /// nothing was ever registered under it, or if it was but never
    /// finished loading.
    #[must_use]
    pub fn info(&self, id: &str) -> Option<DynamicIndicatorInfo> {
        match self.read().get(id)? {
            DynamicIndicatorEntry::Loaded(loaded) => Some(loaded.info.clone()),
            DynamicIndicatorEntry::Incompatible { .. }
            | DynamicIndicatorEntry::FailedToLoad { .. } => None,
        }
    }

    /// Flips `id`'s enabled flag.
    ///
    /// Setting `enabled: true` also resets this plugin's circuit breaker if
    /// its repeated traps had tripped it — this is the whole "re-enable" a
    /// user performs from the Plugins page once they have read why it broke
    /// (see [`DynamicIndicatorState::AutoDisabled`] and
    /// `senken_plugin_host::circuit`'s own docs: the breaker never clears
    /// itself, on a timer or otherwise, because a guest trap's cause is a
    /// deterministic bug rather than a transient venue failure). Setting
    /// `enabled: false` never touches the breaker; a user's own disable and
    /// the breaker's own trip are recorded independently.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::UnknownPlugin`] if nothing is registered
    /// under `id`; [`DynamicIndicatorError::NotToggleable`] if it is
    /// registered but never finished loading (`Incompatible` or
    /// `FailedToLoad`), so there is no enabled flag to flip.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), DynamicIndicatorError> {
        let mut entries = self.write();
        match entries
            .get_mut(id)
            .ok_or_else(|| DynamicIndicatorError::UnknownPlugin(id.to_owned()))?
        {
            DynamicIndicatorEntry::Loaded(loaded) => {
                loaded.enabled = enabled;
                if enabled {
                    loaded.plugin.reset_circuit_breaker();
                }
                Ok(())
            }
            DynamicIndicatorEntry::Incompatible { .. }
            | DynamicIndicatorEntry::FailedToLoad { .. } => {
                Err(DynamicIndicatorError::NotToggleable(id.to_owned()))
            }
        }
    }

    /// Builds the positional parameter list a `constructor` call needs from
    /// a JSON object keyed by parameter name — the same shape a built-in's
    /// own `{"period": 5}` params string takes.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::InvalidParams`] if `params` is not a JSON
    /// object, or is missing a value for one of `info`'s declared
    /// parameters, or that value does not match the parameter's declared
    /// [`ParamKind`].
    fn params_from_json(
        info: &DynamicIndicatorInfo,
        params: &str,
    ) -> Result<Vec<WitParamValue>, DynamicIndicatorError> {
        let invalid = |reason: String| DynamicIndicatorError::InvalidParams {
            indicator: info.id.clone(),
            reason,
        };
        let json: serde_json::Value =
            serde_json::from_str(params).map_err(|error| invalid(error.to_string()))?;
        info.params
            .iter()
            .map(|spec| {
                let value = json
                    .get(&spec.name)
                    .ok_or_else(|| invalid(format!("missing `{}`", spec.name)))?;
                match spec.kind {
                    ParamKind::Integer => {
                        value.as_u64().map(WitParamValue::Integer).ok_or_else(|| {
                            invalid(format!("`{}` must be a non-negative integer", spec.name))
                        })
                    }
                    ParamKind::Number => value
                        .as_f64()
                        .map(WitParamValue::Number)
                        .ok_or_else(|| invalid(format!("`{}` must be a number", spec.name))),
                }
            })
            .collect()
    }

    /// Spawns a fresh instance of the enabled indicator `id`, with
    /// `params_json` matched positionally against its own declared
    /// parameters.
    ///
    /// Bounded by [`senken_plugin_host::ExecutionMode::Backtest`]: a
    /// compute call replays a bar range exactly the way a backtest does,
    /// and the same fixed fuel budget on every call is what keeps a replay
    /// of the same range costing the same regardless of the machine it
    /// runs on.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::UnknownPlugin`] if `id` was never
    /// registered; [`DynamicIndicatorError::Disabled`] if it is currently
    /// disabled; [`DynamicIndicatorError::InvalidParams`] if `params_json`
    /// does not match its declared parameters; [`DynamicIndicatorError::Host`]
    /// if `id` never finished loading (`Incompatible` or `FailedToLoad`), or
    /// if its circuit breaker is open, or the constructor call traps.
    pub fn spawn(
        &self,
        id: &str,
        params_json: &str,
    ) -> Result<DynamicIndicatorInstance, DynamicIndicatorError> {
        let entries = self.read();
        let (plugin, info, enabled) = match entries
            .get(id)
            .ok_or_else(|| DynamicIndicatorError::UnknownPlugin(id.to_owned()))?
        {
            DynamicIndicatorEntry::Loaded(loaded) => (&loaded.plugin, &loaded.info, loaded.enabled),
            DynamicIndicatorEntry::Incompatible {
                found_version,
                supported_version,
                ..
            } => {
                return Err(DynamicIndicatorError::Host(PluginHostError::Incompatible {
                    found: found_version.clone(),
                    supported: supported_version.clone(),
                }));
            }
            DynamicIndicatorEntry::FailedToLoad { reason, .. } => {
                return Err(DynamicIndicatorError::Host(PluginHostError::Load(
                    reason.clone(),
                )));
            }
        };
        if !enabled {
            return Err(DynamicIndicatorError::Disabled(id.to_owned()));
        }
        // A compiled indicator-lang program's own `info.params` is always
        // empty (see `synthesize_compiled_info`), so this validates
        // `params_json` against that empty list — trivially satisfied by
        // any JSON object — the same way it does for a real plugin with
        // declared parameters. There is nothing further to pass a compiled
        // component's spawn call: it takes none.
        let params = Self::params_from_json(info, params_json)?;
        let mode = senken_plugin_host::ExecutionMode::Backtest {
            fuel: DYNAMIC_INDICATOR_FUEL_BUDGET,
        };
        let kind = match plugin {
            LoadedIndicator::Plugin(plugin) => {
                DynamicInstanceKind::Plugin(plugin.spawn(&params, mode)?)
            }
            LoadedIndicator::Compiled(compiled) => {
                DynamicInstanceKind::Compiled(compiled.spawn(mode)?)
            }
        };
        Ok(DynamicIndicatorInstance {
            kind,
            info: info.clone(),
        })
    }
}

/// Which of the two loaded shapes a [`DynamicIndicatorInstance`] is
/// actually running — mirrors [`LoadedIndicator`] one level down, at the
/// spawned-instance stage.
enum DynamicInstanceKind {
    Plugin(PluginInstance),
    Compiled(CompiledIndicatorInstance),
}

/// One running dynamic-indicator instance, spawned by
/// [`DynamicIndicators::spawn`].
#[derive(Debug)]
pub struct DynamicIndicatorInstance {
    kind: DynamicInstanceKind,
    info: DynamicIndicatorInfo,
}

impl std::fmt::Debug for DynamicInstanceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plugin(instance) => f.debug_tuple("Plugin").field(instance).finish(),
            Self::Compiled(instance) => f.debug_tuple("Compiled").field(instance).finish(),
        }
    }
}

impl DynamicIndicatorInstance {
    /// This instance's own descriptor.
    #[must_use]
    pub fn info(&self) -> &DynamicIndicatorInfo {
        &self.info
    }

    /// Feeds one bar into this instance. Bars must be handed to this in
    /// chronological order, oldest first — the same requirement every
    /// built-in's own [`senken_indicators::Indicator::handle_bar`] states,
    /// inherited here because `wit/senken.wit` states it too, for both
    /// `handle-bar` and `on-bar`.
    ///
    /// # Errors
    /// Whatever the underlying instance's own call returns — a trap, an
    /// open circuit breaker — wrapped in [`DynamicIndicatorError::Host`];
    /// [`DynamicIndicatorError::UnsupportedBarUnit`] if `spec.unit` has no
    /// counterpart in `wit/senken.wit`'s closed `bar-unit` enum (see
    /// [`bar_to_wit`]) — checked even for a compiled indicator, which does
    /// not carry a `bar-spec` across its own flat ABI, so every dynamic
    /// indicator rejects the same bar units consistently rather than one
    /// kind silently accepting what the other refuses.
    pub fn handle_bar(
        &mut self,
        bar: &Bar,
        spec: BarSpec,
    ) -> Result<DynamicOnBar, DynamicIndicatorError> {
        match &mut self.kind {
            DynamicInstanceKind::Plugin(instance) => {
                let result = instance.handle_bar(bar_to_wit(bar, spec)?)?;
                Ok(DynamicOnBar {
                    plots: result
                        .plots
                        .into_iter()
                        .map(|plot| (plot.field, plot.value))
                        .collect(),
                    drawables: result.drawables.iter().map(drawable_from_wit).collect(),
                })
            }
            DynamicInstanceKind::Compiled(instance) => {
                // Proves this bar's own unit is one `wit/senken.wit` knows
                // about, exactly as the plugin path above does, even though
                // `on-bar`'s flat signature never actually carries it.
                bar_spec_to_wit(spec)?;
                let value = instance.on_bar(
                    scaled_to_f64_lossy(scaled(bar.open)),
                    scaled_to_f64_lossy(scaled(bar.high)),
                    scaled_to_f64_lossy(scaled(bar.low)),
                    scaled_to_f64_lossy(scaled(bar.close)),
                    bar_volume_magnitude(bar.volume),
                )?;
                Ok(DynamicOnBar {
                    plots: vec![(COMPILED_INDICATOR_PLOT_FIELD.to_owned(), value)],
                    drawables: Vec::new(),
                })
            }
        }
    }

    /// Whether this instance has seen enough bars for its output to be
    /// meaningful.
    ///
    /// A compiled indicator-lang program has no way to report this —
    /// `wit/senken.wit`'s `compiled-indicator` world has no counterpart to
    /// `indicator-plugin`'s own `initialized` method, since the language
    /// has no notion of warm-up separate from the value a built-in already
    /// returns on every bar — so this always reports `true` for one. Every
    /// built-in a compiled program calls already computes its real value
    /// from the first bar it sees, the same value `senken_indicators`
    /// itself would report before its own `initialized()` turns `true`,
    /// which is exactly what `crates/indicator-lang`'s own equivalence
    /// tests check bar-for-bar without ever gating on that flag.
    ///
    /// # Errors
    /// See [`Self::handle_bar`].
    pub fn initialized(&mut self) -> Result<bool, DynamicIndicatorError> {
        match &mut self.kind {
            DynamicInstanceKind::Plugin(instance) => Ok(instance.initialized()?),
            DynamicInstanceKind::Compiled(_) => Ok(true),
        }
    }
}

/// Converts one `wit/senken.wit` `instrument` into the domain [`Instrument`]
/// every `senken-marketdata` consumer already speaks.
///
/// Spot only, always `InstrumentStatus::Trading`: `wit/senken.wit`'s own
/// `venue.instrument` record carries neither a `kind` nor a `status` field
/// (see that record's own doc comment for why — a derivative's contract
/// terms are real weight this boundary does not carry yet), so every
/// dynamic venue instrument is presented as an ordinary, currently-trading
/// spot pair. A plugin author with a delisted or halted symbol to report
/// has no channel for that today; extending `venue.instrument` to carry it
/// is a real, separate piece of work, not a gap this bridge can paper over.
fn instrument_from_wit(instrument: WitVenueInstrument) -> Instrument {
    Instrument::spot(
        instrument.symbol,
        instrument.source_symbol,
        instrument.base,
        instrument.quote,
    )
    .with_name(instrument.name)
    .with_status(InstrumentStatus::Trading)
    .with_price_increment((instrument.price_scale, instrument.tick_size))
    .with_qty_increment((instrument.qty_scale, instrument.step_size))
}

/// Restates a [`VenueCallError`] as the [`SourceError`] every
/// `MarketDataSource`/`BarSource` caller already knows how to handle.
///
/// [`VenueCallError::Host`] — a trap, an open circuit breaker, or a load
/// failure this bridge did not expect to see again after the plugin already
/// loaded once — is always reported as [`SourceError::Rejected`], never
/// [`SourceError::Transport`]: a guest trap is a deterministic bug in
/// compiled code, not a transient network condition, so telling a caller
/// "retry me" would be dishonest (`SourceError::is_retryable` returns
/// `false` for `Rejected`, exactly the answer this case needs).
fn source_error_from_venue_call_error(error: VenueCallError) -> SourceError {
    match error {
        VenueCallError::Host(host_error) => SourceError::rejected(host_error.to_string()),
        VenueCallError::Venue(WitVenueError::Fetch(WitFetchError::Transport(message))) => {
            SourceError::transport(message)
        }
        VenueCallError::Venue(WitVenueError::Fetch(WitFetchError::Http((status, body)))) => {
            SourceError::http(status, body)
        }
        VenueCallError::Venue(WitVenueError::Fetch(WitFetchError::Rejected(reason))) => {
            SourceError::rejected(reason)
        }
        VenueCallError::Venue(WitVenueError::Decode(message)) => SourceError::decode(message),
        VenueCallError::Venue(WitVenueError::Rejected(reason)) => SourceError::rejected(reason),
        // `VenueCallError` is `#[non_exhaustive]` (a future variant this
        // bridge has not seen must not fail to compile here); restated as
        // `Rejected` rather than guessed at more specifically, the same
        // fallback `VenueCallError::Host` above already uses.
        other => SourceError::rejected(other.to_string()),
    }
}

/// Runs `call` — one of [`LoadedVenuePlugin`]'s own blocking methods — on
/// Tokio's blocking thread pool, so an `async fn` caller
/// ([`DynamicVenueSource`]'s own `MarketDataSource`/`BarSource` impls)
/// never blocks its own executor on a call that may run a real network
/// fetch to completion (see `senken_plugin_host::http_host`'s own docs for
/// why that fetch is itself a plain blocking call, not something this
/// function could `.await` directly).
///
/// A panic inside `call` (never expected — every guest-facing path in
/// `senken-plugin-host` already turns a trap into `Err`) surfaces as
/// [`VenueCallError::Host`] rather than propagating, so one broken venue
/// plugin cannot take an unrelated caller's task down with it.
async fn run_venue_call<T>(
    call: impl FnOnce() -> Result<T, VenueCallError> + Send + 'static,
) -> Result<T, VenueCallError>
where
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(call).await {
        Ok(result) => result,
        Err(join_error) => Err(VenueCallError::Host(PluginHostError::Trap(format!(
            "venue plugin call task did not complete: {join_error}"
        )))),
    }
}

/// Why loading, registering or calling a dynamic venue failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DynamicVenueError {
    /// The plugin runtime itself refused to load or run the component.
    #[error(transparent)]
    Host(#[from] PluginHostError),
    /// The component loaded, but a call this bridge makes once at
    /// registration time (`supported-specs`, `max-rows`) failed.
    #[error(transparent)]
    Call(#[from] VenueCallError),
    /// No venue is registered under this id.
    #[error("no dynamic venue plugin is registered as `{0}`")]
    UnknownPlugin(String),
    /// `id` is registered but never finished loading, so it has no enabled
    /// flag for [`DynamicVenues::set_enabled`] to toggle — mirrors
    /// [`DynamicIndicatorError::NotToggleable`] exactly.
    #[error("`{0}` never finished loading and has nothing to enable or disable")]
    NotToggleable(String),
    /// This registry's own shared `reqwest::Client` could not be built.
    #[error("could not build the HTTP client every dynamic venue shares: {0}")]
    HttpClientInit(String),
}

/// A registered venue's identity, returned by a successful
/// [`DynamicVenues::register`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicVenueInfo {
    /// This venue's stable id — the source half of every instrument id it
    /// contributes, exactly like a compiled-in [`MarketDataSource::id`].
    pub id: String,
    /// Human-readable name.
    pub name: String,
}

/// One registered venue's own live, shared state.
///
/// Held behind an [`Arc`] by both the [`DynamicVenues`] registry entry and
/// every [`DynamicVenueSource`] handed out from it, so enabling or
/// disabling a venue from the Plugins page is observed immediately by
/// every `MarketDataSource`/`BarSource` handle already registered into
/// `senken-marketdata`/`senken-loader` — there is no second registration
/// step to re-run.
struct VenueShared {
    plugin: LoadedVenuePlugin,
    id: String,
    name: String,
    /// Probed once, right after loading — `senken_plugin::BarSource::supported`
    /// returns a plain `&[BarSpec]` with no error path, so there is nowhere
    /// to report a fresh guest call's own trap on every symbol-picker call.
    supported: Vec<BarSpec>,
    max_rows: usize,
    /// `true` unless a user has explicitly disabled this venue. Reading
    /// this is the *only* thing that changes when it flips — see this
    /// module's own [`DynamicVenueSource`] docs for why the venue itself
    /// stays registered either way.
    enabled: AtomicBool,
}

/// A component that has already proven it links against this host's
/// capability-zero surface, described its instruments, and answered its
/// own `supported-specs`/`max-rows` — everything [`DynamicVenueSource`]
/// needs to stand in for an ordinary compiled-in [`MarketDataSource`]/
/// [`BarSource`] pair.
enum DynamicVenueEntry {
    Loaded {
        shared: Arc<VenueShared>,
        origin: PluginOrigin,
    },
    /// The component names an unsupported `senken:plugin-api` version —
    /// mirrors [`DynamicIndicatorEntry::Incompatible`] exactly.
    Incompatible {
        origin: PluginOrigin,
        found_version: String,
        supported_version: String,
    },
    /// The component never loaded, or loaded but failed one of the probe
    /// calls (`supported-specs`, `max-rows`) this bridge makes once at
    /// registration time — mirrors [`DynamicIndicatorEntry::FailedToLoad`].
    FailedToLoad {
        origin: PluginOrigin,
        reason: String,
    },
}

/// One registered venue's full status, for the Plugins page — mirrors
/// [`DynamicIndicatorStatus`] exactly, reusing [`DynamicIndicatorState`]
/// itself: a dynamically loaded component's lifecycle (loaded and active,
/// user-disabled, incompatible, failed to load, or auto-disabled by its own
/// circuit breaker) is the identical five-state machine whether it is an
/// indicator or a venue.
#[derive(Debug, Clone, PartialEq)]
pub struct DynamicVenueStatus {
    /// This entry's identity: a real descriptor id once one has been read,
    /// otherwise a content hash of the bytes that failed.
    pub id: String,
    /// This venue's declared name — `None` for `Incompatible`/`FailedToLoad`,
    /// neither of which ever produced one.
    pub name: Option<String>,
    /// Where these bytes came from.
    pub origin: PluginOrigin,
    /// Which of the five states this entry is in right now.
    pub state: DynamicIndicatorState,
    /// This plugin's current runtime health — `None` for
    /// `Incompatible`/`FailedToLoad`.
    pub health: Option<PluginHealth>,
    /// This plugin's own ring log, oldest first.
    pub logs: Vec<PluginLogLine>,
}

/// Venue plugins loaded at runtime from an uploaded `.wasm` component,
/// presented as ordinary [`MarketDataSource`]/[`BarSource`] implementations
/// so the rest of the application never has to know a given source is
/// dynamic — see `DynamicVenueSource`.
///
/// **Disabling a venue never removes it from [`Self::marketdata_sources`]/
/// [`Self::bar_sources`].** This is the one place this bridge's behaviour
/// deliberately diverges from [`DynamicIndicators`]: an indicator that is
/// disabled disappears from its catalog outright (a chart falls back to a
/// host-drawn placeholder), but bars a venue already fetched must stay
/// readable with that venue turned off — storage is a user's own data, not
/// the plugin's, and disabling a plugin must never make previously-fetched
/// history unreachable. So a disabled venue's registration stays put; what
/// actually changes is that `DynamicVenueSource::instruments` reports an
/// empty catalog and `DynamicVenueSource::bars` refuses every fetch,
/// exactly the "reported with capabilities off, but not deregistered"
/// contract a compiled-in source's own disable already follows.
///
/// Cheap to clone: every clone shares the same [`PluginHost`], HTTP client
/// and registered-entry table.
#[derive(Clone)]
pub struct DynamicVenues {
    host: PluginHost,
    /// Shared by every registered venue's own [`VenueClient`] — one
    /// connection pool for the whole registry, the same way
    /// `senken_plugin::ActivationContext`'s own `shared_http_client`
    /// serves every compiled-in plugin from one pool.
    http_client: reqwest::Client,
    entries: Arc<RwLock<HashMap<String, DynamicVenueEntry>>>,
}

impl std::fmt::Debug for DynamicVenues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        f.debug_struct("DynamicVenues")
            .field("registered", &count)
            .finish_non_exhaustive()
    }
}

/// How long this registry's shared `reqwest::Client` waits for a whole
/// request/response, and to establish the connection — the same values
/// `senken_plugin::ActivationContext`'s own compiled-in-plugin client uses,
/// so a dynamic venue's own timeouts are not a second, differently-tuned
/// policy living beside the one every compiled-in adapter already gets.
const DYNAMIC_VENUE_HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const DYNAMIC_VENUE_HTTP_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl DynamicVenues {
    /// Builds an empty registry with its own capability-zero, memory-capped
    /// plugin host and one shared `reqwest::Client`.
    ///
    /// # Errors
    /// [`DynamicVenueError::Host`] if the underlying
    /// `senken_plugin_host::PluginHost` cannot be built;
    /// [`DynamicVenueError::HttpClientInit`] if the shared HTTP client
    /// cannot be built (a TLS backend failing to initialise, in practice).
    pub fn new() -> Result<Self, DynamicVenueError> {
        let host = PluginHost::new(PluginLimits::default())?;
        let http_client = reqwest::Client::builder()
            .timeout(DYNAMIC_VENUE_HTTP_REQUEST_TIMEOUT)
            .connect_timeout(DYNAMIC_VENUE_HTTP_CONNECT_TIMEOUT)
            .user_agent(concat!("senken/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| DynamicVenueError::HttpClientInit(error.to_string()))?;
        Ok(Self {
            host,
            http_client,
            entries: Arc::default(),
        })
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, DynamicVenueEntry>> {
        self.entries.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, DynamicVenueEntry>> {
        self.entries.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Loads `wasm` as a [`PluginOrigin::Uploaded`] registration — see
    /// [`Self::register_with_origin`] for the full contract.
    ///
    /// # Errors
    /// See [`Self::register_with_origin`].
    pub fn register(&self, wasm: &[u8]) -> Result<DynamicVenueInfo, DynamicVenueError> {
        self.register_with_origin(wasm, PluginOrigin::Uploaded)
    }

    /// Loads `wasm` exactly like [`Self::register_with_origin`], with
    /// `base_url_override` forwarded to
    /// [`PluginHost::load_venue`](senken_plugin_host::PluginHost::load_venue)
    /// unchanged: `Some` replaces the plugin's own declared origin, `None`
    /// uses it as-is. This is the same override a compiled-in adapter's own
    /// `with_url` builder already offers — a regional mirror, or a test
    /// double standing in for the real venue — applied to the dynamic path,
    /// and it is what [`Self::register_with_origin`] itself calls with
    /// `None`.
    ///
    /// # Errors
    /// See [`Self::register_with_origin`].
    pub fn register_with_origin_and_base_url(
        &self,
        wasm: &[u8],
        origin: PluginOrigin,
        base_url_override: Option<String>,
    ) -> Result<DynamicVenueInfo, DynamicVenueError> {
        let group = LimitGroup::new(&content_hash_id("venue", wasm));
        let client = VenueClient::new(self.http_client.clone(), group);
        let plugin = match self.host.load_venue(wasm, client, base_url_override) {
            Ok(plugin) => plugin,
            Err(host_error) => {
                self.record_load_failure(wasm, origin, &host_error);
                return Err(DynamicVenueError::Host(host_error));
            }
        };

        let id = plugin.descriptor().id.clone();
        let name = plugin.descriptor().name.clone();

        let supported = match plugin.supported_specs() {
            Ok(specs) => specs.into_iter().filter_map(bar_spec_from_wit).collect(),
            Err(call_error) => {
                self.record_probe_failure(wasm, origin, &call_error);
                return Err(DynamicVenueError::Call(call_error));
            }
        };
        let max_rows = match plugin.max_rows() {
            Ok(rows) => usize::try_from(rows).unwrap_or(usize::MAX),
            Err(call_error) => {
                self.record_probe_failure(wasm, origin, &call_error);
                return Err(DynamicVenueError::Call(call_error));
            }
        };

        let shared = Arc::new(VenueShared {
            plugin,
            id: id.clone(),
            name: name.clone(),
            supported,
            max_rows,
            enabled: AtomicBool::new(true),
        });
        self.write()
            .insert(id.clone(), DynamicVenueEntry::Loaded { shared, origin });
        Ok(DynamicVenueInfo { id, name })
    }

    /// Loads `wasm`, registering it under its own descriptor id, enabled by
    /// default, with a fresh [`LimitGroup`] scoped to this one plugin —
    /// keyed by a content hash of the bytes rather than the descriptor's
    /// own id, so the group exists (and can be inspected) even for the
    /// vanishingly unlikely case of two different uploads racing to
    /// register the same id. Uses the plugin's own declared origin as-is —
    /// see [`Self::register_with_origin_and_base_url`] for the override.
    ///
    /// Re-uploading the same id replaces the earlier registration outright,
    /// exactly like [`DynamicIndicators::register_with_origin`] — the old
    /// entry's own `Arc<VenueShared>` is simply dropped from the map, which
    /// drops its `LoadedVenuePlugin` and, with it, its `Store`.
    ///
    /// A component that fails to load, or loads but fails one of the two
    /// probe calls this bridge makes once (`supported-specs`, `max-rows`),
    /// is never simply dropped: it is recorded as `DynamicVenueEntry::Incompatible`
    /// or `DynamicVenueEntry::FailedToLoad` — see [`Self::all`] to read it
    /// back — even though this call still fails for its immediate caller.
    ///
    /// # Errors
    /// [`DynamicVenueError::Host`] if the component fails to load;
    /// [`DynamicVenueError::Call`] if it loads but fails a probe call.
    pub fn register_with_origin(
        &self,
        wasm: &[u8],
        origin: PluginOrigin,
    ) -> Result<DynamicVenueInfo, DynamicVenueError> {
        self.register_with_origin_and_base_url(wasm, origin, None)
    }

    fn record_load_failure(&self, wasm: &[u8], origin: PluginOrigin, error: &PluginHostError) {
        let id = content_hash_id("FailedVenue", wasm);
        let entry = if let PluginHostError::Incompatible { found, supported } = error {
            DynamicVenueEntry::Incompatible {
                origin,
                found_version: found.clone(),
                supported_version: supported.clone(),
            }
        } else {
            DynamicVenueEntry::FailedToLoad {
                origin,
                reason: error.to_string(),
            }
        };
        self.write().insert(id, entry);
    }

    fn record_probe_failure(&self, wasm: &[u8], origin: PluginOrigin, error: &VenueCallError) {
        let id = content_hash_id("FailedVenue", wasm);
        self.write().insert(
            id,
            DynamicVenueEntry::FailedToLoad {
                origin,
                reason: error.to_string(),
            },
        );
    }

    /// Every registered venue, presented as a [`MarketDataSource`] — a
    /// disabled one included, reporting an empty catalog rather than being
    /// absent from this list at all. See this type's own docs for why.
    #[must_use]
    pub fn marketdata_sources(&self) -> Vec<Arc<dyn MarketDataSource>> {
        self.read()
            .values()
            .filter_map(|entry| match entry {
                DynamicVenueEntry::Loaded { shared, .. } => Some(Arc::new(DynamicVenueSource {
                    shared: Arc::clone(shared),
                })
                    as Arc<dyn MarketDataSource>),
                DynamicVenueEntry::Incompatible { .. } | DynamicVenueEntry::FailedToLoad { .. } => {
                    None
                }
            })
            .collect()
    }

    /// Every registered venue, presented as a [`BarSource`] — see
    /// [`Self::marketdata_sources`], which this mirrors exactly.
    #[must_use]
    pub fn bar_sources(&self) -> Vec<Arc<dyn BarSource>> {
        self.read()
            .values()
            .filter_map(|entry| match entry {
                DynamicVenueEntry::Loaded { shared, .. } => Some(Arc::new(DynamicVenueSource {
                    shared: Arc::clone(shared),
                })
                    as Arc<dyn BarSource>),
                DynamicVenueEntry::Incompatible { .. } | DynamicVenueEntry::FailedToLoad { .. } => {
                    None
                }
            })
            .collect()
    }

    /// Every registered entry, regardless of state — see
    /// [`DynamicIndicators::all`], which this mirrors exactly.
    #[must_use]
    pub fn all(&self) -> Vec<DynamicVenueStatus> {
        self.read()
            .iter()
            .map(|(id, entry)| Self::status_for(id, entry))
            .collect()
    }

    fn status_for(id: &str, entry: &DynamicVenueEntry) -> DynamicVenueStatus {
        match entry {
            DynamicVenueEntry::Loaded { shared, origin } => {
                let health = shared.plugin.health();
                let state = if !shared.enabled.load(Ordering::Relaxed) {
                    DynamicIndicatorState::Disabled
                } else if let CircuitState::Open { reason } = &health.circuit {
                    DynamicIndicatorState::AutoDisabled {
                        reason: reason.clone(),
                    }
                } else {
                    DynamicIndicatorState::Active
                };
                DynamicVenueStatus {
                    id: id.to_owned(),
                    name: Some(shared.name.clone()),
                    origin: *origin,
                    state,
                    logs: shared.plugin.logs(),
                    health: Some(health),
                }
            }
            DynamicVenueEntry::Incompatible {
                origin,
                found_version,
                supported_version,
            } => DynamicVenueStatus {
                id: id.to_owned(),
                name: None,
                origin: *origin,
                state: DynamicIndicatorState::Incompatible {
                    found_version: found_version.clone(),
                    supported_version: supported_version.clone(),
                },
                health: None,
                logs: Vec::new(),
            },
            DynamicVenueEntry::FailedToLoad { origin, reason } => DynamicVenueStatus {
                id: id.to_owned(),
                name: None,
                origin: *origin,
                state: DynamicIndicatorState::FailedToLoad {
                    reason: reason.clone(),
                },
                health: None,
                logs: Vec::new(),
            },
        }
    }

    /// Flips `id`'s enabled flag — see this type's own docs for what
    /// "disabled" actually changes for a venue (never removal from
    /// [`Self::marketdata_sources`]/[`Self::bar_sources`]).
    ///
    /// Setting `enabled: true` also resets this plugin's circuit breaker,
    /// mirroring [`DynamicIndicators::set_enabled`] exactly and for the
    /// same reason.
    ///
    /// # Errors
    /// [`DynamicVenueError::UnknownPlugin`] if nothing is registered under
    /// `id`; [`DynamicVenueError::NotToggleable`] if it is registered but
    /// never finished loading.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), DynamicVenueError> {
        let entries = self.read();
        match entries
            .get(id)
            .ok_or_else(|| DynamicVenueError::UnknownPlugin(id.to_owned()))?
        {
            DynamicVenueEntry::Loaded { shared, .. } => {
                shared.enabled.store(enabled, Ordering::Relaxed);
                if enabled {
                    shared.plugin.reset_circuit_breaker();
                }
                Ok(())
            }
            DynamicVenueEntry::Incompatible { .. } | DynamicVenueEntry::FailedToLoad { .. } => {
                Err(DynamicVenueError::NotToggleable(id.to_owned()))
            }
        }
    }
}

/// One dynamically loaded venue, presented as an ordinary
/// [`MarketDataSource`]/[`BarSource`] pair — the type
/// [`DynamicVenues::marketdata_sources`]/[`DynamicVenues::bar_sources`]
/// hand back, so `senken-marketdata`/`senken-loader` (and everything built
/// on them) never has to know a given source came from a `.wasm` file
/// rather than compiled-in Rust.
struct DynamicVenueSource {
    shared: Arc<VenueShared>,
}

#[async_trait::async_trait]
impl MarketDataSource for DynamicVenueSource {
    fn id(&self) -> &str {
        &self.shared.id
    }

    fn name(&self) -> &str {
        &self.shared.name
    }

    async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
        if !self.shared.enabled.load(Ordering::Relaxed) {
            // Not an error: a disabled source's catalog is simply empty,
            // exactly the "capabilities off but still listed" contract a
            // compiled-in source already follows once disabled (see
            // `DynamicVenues`'s own docs for why this venue is not removed
            // from the registry at all).
            return Ok(Vec::new());
        }
        let shared = Arc::clone(&self.shared);
        let instruments = run_venue_call(move || shared.plugin.instruments())
            .await
            .map_err(source_error_from_venue_call_error)?;
        Ok(instruments.into_iter().map(instrument_from_wit).collect())
    }
}

#[async_trait::async_trait]
impl BarSource for DynamicVenueSource {
    fn source_id(&self) -> &str {
        &self.shared.id
    }

    fn supported(&self) -> &[BarSpec] {
        &self.shared.supported
    }

    fn max_rows(&self) -> usize {
        self.shared.max_rows
    }

    async fn bars(
        &self,
        symbol: &SourceSymbol,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, SourceError> {
        if !self.shared.enabled.load(Ordering::Relaxed) {
            // The server must keep refusing a dead source's own fetches —
            // hiding it in a client is not enforcement. See `DynamicVenues`'s
            // own docs for why this venue stays registered regardless.
            return Err(SourceError::rejected("this venue is currently disabled"));
        }
        let wit_spec =
            bar_spec_to_wit(spec).map_err(|error| SourceError::decode(error.to_string()))?;
        let source_symbol = symbol.as_str().to_owned();
        let range_start = range.start().as_nanos();
        let range_end = range.end().as_nanos();
        let shared = Arc::clone(&self.shared);
        let bars = run_venue_call(move || {
            shared
                .plugin
                .bars(&source_symbol, wit_spec, range_start, range_end)
        })
        .await
        .map_err(source_error_from_venue_call_error)?;
        Ok(bars.iter().map(bar_from_wit).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{BarSpec, BarUnit, DynamicIndicatorError, DynamicIndicators, Volume, bar_to_wit};
    use senken_core::UnixNanos;
    use senken_series::Bar;
    use std::num::NonZeroU32;

    fn bar(ts: i64, open: i64, high: i64, low: i64, close: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_nanos(ts),
            open,
            high,
            low,
            close,
            volume: Volume::Real(700),
            quote_volume: Some(1234),
            trade_count: Some(3),
            taker_buy_volume: Some(300),
        }
    }

    /// `senken_plugin_host::Scaled` (bindgen output) derives no
    /// `PartialEq`, so every assertion below compares fields directly
    /// rather than the whole struct.
    fn scaled_eq(actual: senken_plugin_host::Scaled, value: i64) -> bool {
        actual.scale == super::NOMINAL_SCALE && actual.value == value
    }

    /// The bridge must carry every raw integer across unchanged — this is
    /// what makes a dynamic indicator's arithmetic comparable to a
    /// built-in's at all (see this module's own doc comment).
    #[test]
    fn bar_to_wit_carries_every_raw_integer_unchanged() {
        let spec = BarSpec::new(15, BarUnit::Minute);
        let domain = bar(1_000, 100, 140, 90, 130);
        let wit = bar_to_wit(&domain, spec).unwrap();

        assert_eq!(wit.ts_open, 1_000);
        assert_eq!(wit.spec.step, 15);
        assert!(scaled_eq(wit.open, 100));
        assert!(scaled_eq(wit.high, 140));
        assert!(scaled_eq(wit.low, 90));
        assert!(scaled_eq(wit.close, 130));
        assert!(wit.quote_volume.is_some_and(|value| scaled_eq(value, 1234)));
        assert_eq!(wit.trade_count, Some(3));
        assert!(
            wit.taker_buy_volume
                .is_some_and(|value| scaled_eq(value, 300))
        );
        match wit.volume {
            senken_plugin_host::Volume::Real(value) => assert!(scaled_eq(value, 700)),
            other => panic!("expected Volume::Real, got {other:?}"),
        }
    }

    #[test]
    fn every_bar_unit_round_trips_through_the_bridge() {
        for (domain, wit) in [
            (BarUnit::Second, senken_plugin_host::BarUnit::Second),
            (BarUnit::Minute, senken_plugin_host::BarUnit::Minute),
            (BarUnit::Hour, senken_plugin_host::BarUnit::Hour),
            (BarUnit::Day, senken_plugin_host::BarUnit::Day),
            (BarUnit::Week, senken_plugin_host::BarUnit::Week),
            (BarUnit::Month, senken_plugin_host::BarUnit::Month),
        ] {
            assert_eq!(super::bar_unit_to_wit(domain).unwrap(), wit);
        }
    }

    #[test]
    fn a_zero_step_spec_still_bridges_since_nonzero_is_enforced_upstream() {
        let spec = BarSpec {
            step: NonZeroU32::new(1).unwrap(),
            unit: BarUnit::Day,
        };
        assert_eq!(super::bar_spec_to_wit(spec).unwrap().step, 1);
    }

    #[test]
    fn an_id_colliding_with_a_builtin_is_refused_at_registration() {
        // `register` never reaches the collision check without a byte
        // string wasmtime accepts as a component, so this proves the
        // check's own logic directly against a descriptor a real load
        // would have produced, rather than needing a compiled fixture just
        // to exercise a string comparison.
        assert!(senken_indicators::descriptor("Sma").is_some());
        let catalog = DynamicIndicators::new().unwrap();
        // No component was ever loaded, so nothing is registered — this
        // confirms the catalog starts empty rather than the collision path
        // itself, which the wasm-backed integration test in
        // `crates/runtime/tests/dynamic_indicators.rs` exercises for real.
        assert!(catalog.info("Sma").is_none());
        assert!(matches!(
            catalog.set_enabled("Sma", false).unwrap_err(),
            DynamicIndicatorError::UnknownPlugin(id) if id == "Sma"
        ));
    }
}

/// Proves [`DynamicVenues`] end to end against a genuine compiled
/// `venue-plugin` component — the same one `senken-plugin-host`'s own
/// `tests/venue.rs` proves the host-side loading path with — rather than
/// against a description of what a loaded venue would do.
#[cfg(test)]
mod dynamic_venues_tests {
    use super::{DynamicVenueError, DynamicVenues, PluginOrigin};
    use senken_core::TimeRange;
    use senken_core::UnixNanos;
    use senken_marketdata::SourceSymbol;
    use senken_series::{BarSpec, BarUnit};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use wiremock::matchers::{method, path as path_matcher};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    static BUILD_LOCK: Mutex<()> = Mutex::new(());

    /// Builds `senken-plugin-host`'s own `fixture-{name}` test fixture and
    /// returns the path to its compiled component.
    ///
    /// Duplicates (rather than depends on) `senken-plugin-host`'s own
    /// `tests/support::build_fixture`: that helper is private test code of
    /// a different crate, not a reusable library export, and this crate
    /// has no other reason to add a dependency on `senken-plugin-host`'s
    /// test-only surface. The fixture itself is not duplicated — this
    /// builds the exact same crate on disk, so both crates' tests prove
    /// their own layer against the identical compiled bytes.
    fn build_fixture(name: &str) -> PathBuf {
        let _guard = BUILD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../plugin-host/tests/fixtures")
            .join(name);
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "--target", "wasm32-wasip2"])
            .current_dir(&fixture_dir)
            .status()
            .expect("spawning `cargo build` for a test fixture must succeed");
        assert!(status.success(), "fixture `{name}` failed to build");
        let binary_name = format!("fixture_{}.wasm", name.replace('-', "_"));
        let wasm_path = fixture_dir
            .join("target/wasm32-wasip2/debug")
            .join(&binary_name);
        assert!(
            wasm_path.is_file(),
            "expected {} to exist after building fixture `{name}`",
            wasm_path.display()
        );
        wasm_path
    }

    /// OKX's own recorded responses — see `crates/plugin-host/tests/
    /// venue.rs` for the same bytes proving the layer directly below this
    /// one.
    const INSTRUMENTS: &[u8] =
        include_bytes!("../../../plugins/okx/tests/fixtures/instruments.json");

    fn mock_server_body() -> &'static [u8] {
        INSTRUMENTS
    }

    async fn mock_okx_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_matcher("/api/v5/public/instruments"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(mock_server_body(), "application/json"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_matcher("/api/v5/market/history-candles"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                include_bytes!("../../../plugins/okx/tests/fixtures/candles_1m.json").as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        server
    }

    #[test]
    fn a_venue_that_tries_a_socket_is_recorded_as_failed_rather_than_dropped() {
        let wasm = std::fs::read(build_fixture("venue-tries-socket")).unwrap();
        let venues = DynamicVenues::new().unwrap();

        let err = venues
            .register(&wasm)
            .expect_err("a component that can reach for a socket must never load");
        assert!(matches!(err, DynamicVenueError::Host(_)));

        let statuses = venues.all();
        assert_eq!(
            statuses.len(),
            1,
            "the failed attempt must still be recorded"
        );
        assert!(matches!(
            statuses[0].state,
            super::DynamicIndicatorState::FailedToLoad { .. }
        ));
    }

    #[tokio::test]
    async fn a_registered_venue_serves_instruments_and_bars_through_both_registries() {
        let server = mock_okx_server().await;
        let wasm = std::fs::read(build_fixture("venue-example")).unwrap();
        let venues = DynamicVenues::new().unwrap();

        let info = venues
            .register_with_origin_and_base_url(&wasm, PluginOrigin::Uploaded, Some(server.uri()))
            .expect("a well-behaved venue component must register");
        assert_eq!(info.id, "example-okx");

        let marketdata_sources = venues.marketdata_sources();
        let source = marketdata_sources
            .iter()
            .find(|source| source.id() == "example-okx")
            .expect("the registered venue must appear as a MarketDataSource");
        let instruments = source.instruments().await.unwrap();
        let btc = instruments
            .iter()
            .find(|instrument| instrument.symbol == "BTCUSDT")
            .expect("BTC-USDT must survive the fixture's own minimal parser");
        assert_eq!(btc.source_symbol, "BTC-USDT");

        let bar_sources = venues.bar_sources();
        let bar_source = bar_sources
            .iter()
            .find(|source| source.source_id() == "example-okx")
            .expect("the registered venue must appear as a BarSource");
        assert!(!bar_source.supported().is_empty());
        assert!(bar_source.max_rows() > 0);

        let range =
            TimeRange::new(UnixNanos::from_nanos(0), UnixNanos::from_nanos(i64::MAX)).unwrap();
        let bars = bar_source
            .bars(
                &SourceSymbol::assume("BTC-USDT"),
                BarSpec::new(1, BarUnit::Minute),
                range,
            )
            .await
            .unwrap();
        assert_eq!(bars.len(), 4, "the unconfirmed newest row must be dropped");
        assert_eq!(bars[0].open, 780_343);
    }

    #[tokio::test]
    async fn disabling_a_venue_empties_its_catalog_and_rejects_bars_but_keeps_it_registered() {
        let server = mock_okx_server().await;
        let wasm = std::fs::read(build_fixture("venue-example")).unwrap();
        let venues = DynamicVenues::new().unwrap();
        venues
            .register_with_origin_and_base_url(&wasm, PluginOrigin::Uploaded, Some(server.uri()))
            .unwrap();

        let source_before = venues
            .marketdata_sources()
            .into_iter()
            .find(|source| source.id() == "example-okx")
            .unwrap();
        assert!(!source_before.instruments().await.unwrap().is_empty());

        venues.set_enabled("example-okx", false).unwrap();

        // The registration itself must survive disabling — a venue's own
        // stored bars must stay addressable through it after the venue
        // goes quiet, not just before.
        assert_eq!(
            venues.all().len(),
            1,
            "disabling must not deregister the venue"
        );
        let marketdata_after = venues.marketdata_sources();
        let source_after = marketdata_after
            .iter()
            .find(|source| source.id() == "example-okx")
            .expect("a disabled venue must still be present in the registry");
        assert!(
            source_after.instruments().await.unwrap().is_empty(),
            "a disabled venue's instrument catalog must be empty"
        );

        let bar_source = venues
            .bar_sources()
            .into_iter()
            .find(|source| source.source_id() == "example-okx")
            .unwrap();
        let range =
            TimeRange::new(UnixNanos::from_nanos(0), UnixNanos::from_nanos(i64::MAX)).unwrap();
        let err = bar_source
            .bars(
                &SourceSymbol::assume("BTC-USDT"),
                BarSpec::new(1, BarUnit::Minute),
                range,
            )
            .await
            .expect_err("a disabled venue must refuse a bar fetch, not silently succeed");
        assert!(matches!(
            err,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));

        venues.set_enabled("example-okx", true).unwrap();
        let marketdata_reenabled = venues.marketdata_sources();
        let source_reenabled = marketdata_reenabled
            .iter()
            .find(|source| source.id() == "example-okx")
            .unwrap();
        assert!(
            !source_reenabled.instruments().await.unwrap().is_empty(),
            "re-enabling must restore the catalog without a fresh upload"
        );
    }

    #[tokio::test]
    async fn a_dynamic_venues_limit_group_budget_actually_holds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_matcher("/api/v5/public/instruments"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(INSTRUMENTS, "application/json")
                    .set_delay(std::time::Duration::from_millis(300)),
            )
            .mount(&server)
            .await;

        let wasm = std::fs::read(build_fixture("venue-example")).unwrap();
        let venues = DynamicVenues::new().unwrap();
        venues
            .register_with_origin_and_base_url(&wasm, PluginOrigin::Uploaded, Some(server.uri()))
            .unwrap();
        let source = std::sync::Arc::clone(
            venues
                .marketdata_sources()
                .first()
                .expect("the venue must have registered"),
        );

        let first = {
            let source = std::sync::Arc::clone(&source);
            tokio::spawn(async move { source.instruments().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let second = {
            let source = std::sync::Arc::clone(&source);
            tokio::spawn(async move { source.instruments().await })
        };

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), second)
                .await
                .is_err(),
            "a second call must wait behind the first for the venue's own \
             LimitGroup permit rather than running alongside it"
        );
        first.await.unwrap().unwrap();
    }
}
