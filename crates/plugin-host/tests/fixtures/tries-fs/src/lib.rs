//! A plugin that reaches for the filesystem — proves that this alone keeps
//! a component from *loading* at all, since `wasi:filesystem` is never
//! linked: the component's compiled imports carry the requirement
//! unconditionally, whether or not the host ever actually calls
//! `handle-bar`.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct TriesFs;

impl Guest for TriesFs {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "tries-fs".into(),
            title: "Tries Filesystem".into(),
            short_title: "FS".into(),
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
        // Never actually reached by this crate's own tests — the point is
        // that this code path existing at all is enough to fail loading,
        // before any call gets this far.
        let _ = std::fs::File::open("/etc/passwd");
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

senken_plugin_api::export!(TriesFs);
