//! A plugin whose `handle-bar` always panics — used by
//! `crates/runtime/tests/dynamic_indicators.rs` to prove that
//! `senken_runtime::DynamicIndicators` reports a plugin as auto-disabled,
//! with the circuit breaker's own reason, once its trap streak trips the
//! breaker — never silently, and never conflated with a user's own
//! deliberate disable.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct DynPanics;

impl Guest for DynPanics {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "DynPanics".into(),
            title: "Dyn Panics".into(),
            short_title: "DPANIC".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance;

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance
    }

    fn handle_bar(&self, _bar: Bar) -> OnBarResult {
        panic!("fixture-dyn-panics: this plugin always panics");
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(DynPanics);
