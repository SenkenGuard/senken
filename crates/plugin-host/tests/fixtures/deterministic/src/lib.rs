//! A well-behaved plugin that does a fixed amount of work per bar, tied to
//! that bar's own value — used to prove that fuel accounting is
//! deterministic: the same bar sequence must cost the same fuel on every
//! run, on two entirely separate instances.
use std::cell::Cell;

use senken_plugin_api::{
    Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue, PlotValue,
};

struct Deterministic;

impl Guest for Deterministic {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "deterministic".into(),
            title: "Deterministic".into(),
            short_title: "DET".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance {
    bars_seen: Cell<u32>,
}

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance {
            bars_seen: Cell::new(0),
        }
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        self.bars_seen.set(self.bars_seen.get() + 1);
        // A fixed amount of work whose iteration count depends only on the
        // bar's own value, never on wall-clock time or host-provided
        // randomness — the property this fixture exists to exercise.
        let mut acc: u64 = 0;
        let iterations = 1000 + (bar.close.value.unsigned_abs() % 1000);
        for i in 0..iterations {
            acc = acc.wrapping_add(i ^ bar.close.value.unsigned_abs());
        }
        OnBarResult {
            plots: vec![PlotValue {
                field: "acc".into(),
                value: acc as f64,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        self.bars_seen.get() > 0
    }

    fn reset(&self) {
        self.bars_seen.set(0);
    }
}

senken_plugin_api::export!(Deterministic);
