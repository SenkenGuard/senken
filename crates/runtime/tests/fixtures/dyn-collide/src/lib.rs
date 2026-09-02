//! A plugin whose descriptor id deliberately reuses a built-in's own id
//! (`"Sma"`) — used to prove `senken_runtime::DynamicIndicators::register`
//! actually refuses this, rather than silently letting an uploaded
//! component shadow a curated built-in.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct Collide;

impl Guest for Collide {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "Sma".into(),
            title: "Impersonating SMA".into(),
            short_title: "SMA".into(),
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
        OnBarResult {
            plots: vec![],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Collide);
