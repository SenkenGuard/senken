//! Bridges Senken's own domain types to the WIT wire shapes
//! `senken-plugin-host` speaks, and the catalog of indicators loaded from
//! uploaded `.wasm` components on top of it.
//!
//! This lives here, not in `senken-plugin-host` or `senken-plugin-api`,
//! because it needs a domain crate (`senken-series` for [`senken_series::Bar`],
//! and `senken-indicators` for the exact vocabulary a built-in's own
//! descriptor already uses) and the plugin runtime at the same time.
//! `senken-plugin-api` must never depend on a domain crate — a published
//! SDK must not ship a domain crate's implementation alongside it — and
//! `senken-plugin-host`'s own domain dependency exists for one purpose only
//! (backing the `builtins` WIT import with real indicator state machines).
//! Neither crate is the right place for a *second*, unrelated use of both
//! at once, so it lives at the one layer that already depends on
//! everything: the runtime.
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
use std::sync::{Arc, PoisonError, RwLock};

use senken_indicators::{
    Drawable, Extend, LabelAnchor, ParamDefault, ParamKind, PlotShape, Point, PriceCoord,
    SeriesShape,
};
use senken_plugin_host::{
    Bar as WitBar, BarSpec as WitBarSpec, BarUnit as WitBarUnit, CircuitState,
    CompiledIndicatorInstance, Drawable as WitDrawable, Extend as WitExtend,
    IndicatorDescriptor as WitIndicatorDescriptor, LabelAnchor as WitLabelAnchor,
    LoadedCompiledIndicator, LoadedPlugin, ParamKind as WitParamKind, ParamValue as WitParamValue,
    PlotShape as WitPlotShape, PluginHealth, PluginHost, PluginHostError, PluginInstance,
    PluginLimits, PluginLogLine, PriceCoord as WitPriceCoord, Scaled as WitScaled,
    SeriesShape as WitSeriesShape, Volume as WitVolume,
};
use senken_series::{Bar, BarSpec, BarUnit, Volume};
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
