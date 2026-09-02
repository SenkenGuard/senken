//! A plugin that reaches for a TCP socket — proves that this alone keeps a
//! component from *loading* at all, since `wasi:sockets` is never linked:
//! the component's compiled imports carry the requirement unconditionally,
//! whether or not the host ever actually calls `handle-bar`.
use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};

struct TriesSocket;

impl Guest for TriesSocket {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "tries-socket".into(),
            title: "Tries Socket".into(),
            short_title: "SOCK".into(),
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
        let _ = std::net::TcpStream::connect("127.0.0.1:9");
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

senken_plugin_api::export!(TriesSocket);
