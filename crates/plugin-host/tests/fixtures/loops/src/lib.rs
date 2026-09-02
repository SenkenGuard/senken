//! A plugin whose `handle-bar` never returns — proves a live call bounded
//! by an epoch deadline is cut off instead of freezing the host.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct Loops;

impl Guest for Loops {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "loops".into(),
            title: "Loops".into(),
            short_title: "LOOP".into(),
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
        // A side-effecting counter, so the compiler cannot prove this loop
        // is dead code and remove the back-edge the epoch check rides on.
        let mut spins: u64 = 0;
        loop {
            spins = spins.wrapping_add(1);
            std::hint::black_box(spins);
        }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Loops);
