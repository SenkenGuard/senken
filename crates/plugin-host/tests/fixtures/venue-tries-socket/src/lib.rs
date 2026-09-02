//! A venue plugin that reaches for a raw TCP socket instead of the host's
//! `fetch` — proves that `LimitGroup` cannot be bypassed by holding a
//! socket, because there is no `wasi:sockets` import to reach for in the
//! first place: this alone keeps the component from *loading* at all, the
//! same way `../tries-socket` proves it for the `indicator-plugin` world.

wit_bindgen::generate!({
    path: "../../../../../wit/senken.wit",
    world: "venue-plugin",
});

use exports::senken::plugin_api::venue::{Bar, Guest, Instrument, VenueDescriptor, VenueError};
use senken::plugin_api::types::BarSpec;

struct TriesSocket;

impl Guest for TriesSocket {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            id: "tries-socket".to_owned(),
            name: "Tries Socket".to_owned(),
            base_url: "https://example.invalid".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        // Never actually reached by this crate's own tests — the point is
        // that this code path existing at all is enough to fail loading,
        // before any call gets this far, exactly like the indicator
        // fixture this mirrors.
        let _ = std::net::TcpStream::connect("127.0.0.1:9");
        Ok(Vec::new())
    }

    fn supported_specs() -> Vec<BarSpec> {
        Vec::new()
    }

    fn max_rows() -> u32 {
        0
    }

    fn bars(
        _source_symbol: String,
        _spec: BarSpec,
        _range_start: i64,
        _range_end: i64,
    ) -> Result<Vec<Bar>, VenueError> {
        Ok(Vec::new())
    }
}

export!(TriesSocket);
