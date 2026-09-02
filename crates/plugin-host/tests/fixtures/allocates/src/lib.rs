//! A plugin whose `handle-bar` allocates without bound — proves the memory
//! ceiling denies the growth rather than the process being killed.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct Allocates;

impl Guest for Allocates {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "allocates".into(),
            title: "Allocates".into(),
            short_title: "ALLOC".into(),
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
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        loop {
            chunks.push(vec![0_u8; 1024 * 1024]);
            std::hint::black_box(&chunks);
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Allocates);
