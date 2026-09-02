//! A dynamic indicator that computes an exponential moving average of
//! `close` by calling straight into `senken_plugin_api::ema_update` — the
//! same compiled, already-tested `senken_indicators::Ema` the host's own
//! ten built-ins use. This fixture exists to prove `crates/runtime`'s own
//! dynamic-indicator bridge feeds a plugin the exact numbers a native
//! caller would, since a plugin computing through a *different* `Ema`
//! could never match one bar-for-bar.
use std::cell::Cell;

use senken_plugin_api::{
    Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamKind, ParamSpec, ParamValue,
    PlotShape, PlotSpec, PlotValue, ema_update,
};

struct DynEma;

impl Guest for DynEma {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            // Deliberately not "Ema" — a dynamic indicator must never
            // shadow a built-in's own id (see
            // `senken_runtime::DynamicIndicators::register`).
            id: "DynEma".into(),
            title: "Dynamic EMA".into(),
            short_title: "DYNEMA".into(),
            legend: "DynEma {period}".into(),
            params: vec![ParamSpec {
                name: "period".into(),
                kind: ParamKind::Integer,
                default: ParamValue::Integer(10),
                min: Some(1.0),
            }],
            plots: vec![PlotSpec {
                field: "value".into(),
                label: "VALUE".into(),
                shape: PlotShape::Line,
                color: "#f2f2ef".into(),
            }],
        }
    }
}

struct Instance {
    period: u32,
    bars_seen: Cell<u32>,
}

impl GuestInstance for Instance {
    fn new(params: Vec<ParamValue>) -> Self {
        let period = match params.first() {
            Some(ParamValue::Integer(value)) => u32::try_from(*value).unwrap_or(10),
            _ => 10,
        };
        Instance {
            period,
            bars_seen: Cell::new(0),
        }
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        self.bars_seen.set(self.bars_seen.get() + 1);
        // The same "already-extracted price" convention every built-in's
        // own `wit/senken.wit` doc comment describes: the host bridges a
        // real bar's raw scaled integer straight through as this `f64`,
        // never dividing by `scale` (see `senken_runtime::plugin_host`'s
        // own doc comment for why).
        let close = bar.close.value as f64;
        let value = ema_update(0, close, self.period);
        OnBarResult {
            plots: vec![PlotValue {
                field: "value".into(),
                value,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        self.bars_seen.get() >= self.period
    }

    fn reset(&self) {
        self.bars_seen.set(0);
    }
}

senken_plugin_api::export!(DynEma);
