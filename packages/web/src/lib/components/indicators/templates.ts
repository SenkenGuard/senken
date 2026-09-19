// Starting source for a new indicator. Both templates are Rust — `030`'s
// CEO decision reversed the in-app DSL (`indicator-lang`, removed) in favor
// of authoring directly against `senken-plugin-api`, the same SDK a venue
// plugin's `Guest`/`GuestInstance` implementation already goes through.
//
// The "SMA example" below is copied by hand from
// `crates/indicator-compile/tests/compile.rs`'s own `VALID_SOURCE` shape —
// itself a copy of `crates/plugin-host/tests/fixtures/deterministic/src/
// lib.rs` — swapped from that fixture's fixed busy-loop to a genuine
// incremental SMA built on the host's own `sma_update` builtin
// (`wit/senken.wit`'s `builtins` interface, backed by `senken_indicators::Sma`
// — never a second, in-guest reimplementation of it). Never write a
// template like this from memory: the compile service rejects `unsafe`,
// `std::fs`, `std::net` and a handful of other forms outright
// (`crates/indicator-compile/src/service.rs`), so an invented example can
// fail to compile before a new author has changed a single line.

/** A new indicator with nothing in it yet but a working skeleton — the
 * minimum `Guest`/`GuestInstance` implementation that compiles and plots a
 * flat line at the close price, so a first save has something to look at
 * before any real logic is written. */
export const BLANK_INDICATOR_TEMPLATE = `use senken_plugin_api::{
    Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue, PlotShape, PlotSpec,
    PlotValue,
};

struct MyIndicator;

impl Guest for MyIndicator {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "my-indicator".into(),
            title: "My indicator".into(),
            short_title: "MINE".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![PlotSpec {
                field: "value".into(),
                label: "Value".into(),
                shape: PlotShape::Line,
                color: "#4f8cff".into(),
            }],
        }
    }
}

struct Instance;

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        let close = bar.close.value as f64 / 10f64.powi(bar.close.scale as i32);
        OnBarResult {
            plots: vec![PlotValue {
                field: "value".into(),
                value: close,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(MyIndicator);
`;

/** A real, working simple moving average — one configurable `period`
 * parameter, plotted as an overlay line. `sma_update` is the same
 * incrementally-updated `Sma` every built-in indicator and every other
 * plugin author's SMA call goes through; this does not reimplement moving
 * averages, it just calls the host for one. */
export const SMA_INDICATOR_TEMPLATE = `use senken_plugin_api::{
    sma_update, Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamKind,
    ParamSpec, ParamValue, PlotShape, PlotSpec, PlotValue,
};

struct MySma;

impl Guest for MySma {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "my-sma".into(),
            title: "My SMA".into(),
            short_title: "SMA".into(),
            legend: "SMA({period})".into(),
            params: vec![ParamSpec {
                name: "period".into(),
                kind: ParamKind::Integer,
                default: ParamValue::Integer(14),
                min: Some(1.0),
            }],
            plots: vec![PlotSpec {
                field: "sma".into(),
                label: "SMA".into(),
                shape: PlotShape::Line,
                color: "#4f8cff".into(),
            }],
        }
    }
}

struct Instance {
    period: u32,
    // A unique slot per call site the host keys its own incremental Sma
    // state by (wit/senken.wit's builtins interface) - a second
    // instance of this same indicator, placed on another pane, gets its
    // own slot's state and never sees this one's running average.
    slot: u32,
}

impl GuestInstance for Instance {
    fn new(params: Vec<ParamValue>) -> Self {
        let period = match params.first() {
            Some(ParamValue::Integer(v)) => *v as u32,
            _ => 14,
        };
        Instance { period, slot: 0 }
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        let close = bar.close.value as f64 / 10f64.powi(bar.close.scale as i32);
        let value = sma_update(self.slot, close, self.period);
        OnBarResult {
            plots: vec![PlotValue {
                field: "sma".into(),
                value,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(MySma);
`;

export type IndicatorTemplateId = 'blank' | 'sma';

export const INDICATOR_TEMPLATES: { id: IndicatorTemplateId; label: string; source: string }[] = [
	{ id: 'blank', label: 'Blank', source: BLANK_INDICATOR_TEMPLATE },
	{ id: 'sma', label: 'SMA example', source: SMA_INDICATOR_TEMPLATE }
];
