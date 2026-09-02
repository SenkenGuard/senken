//! `WhiteBIT`'s public `trades` subscription.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://api.whitebit.com/ws`, sent
//! `{"id":1,"method":"trades_subscribe","params":["BTC_USDT"]}`, received
//! `{"error": null, "result": {"status": "success"}, "id": 1}` and then:
//!
//! ```json
//! {"method": "trades_update", "params": ["BTC_USDT", [{"id": 23416219526, "time": 1788335142.5668371, "price": "77572.99", "amount": "0.012017", "type": "buy", "rpi": false}]], "id": null}
//! ```
//!
//! In the same session `{"id":1,"method":"ping","params":[]}` was answered
//! with `{"error": null, "result": "pong", "id": 1}`, and
//! `trades_unsubscribe` with `{"status": "success"}`.
//!
//! Read from that capture:
//! - **`params` is a positional array**: the symbol first, then the list of
//!   trades. There is no symbol on an individual trade, so the envelope is
//!   the only place to read it.
//! - **`time` is fractional epoch seconds as a bare JSON number** —
//!   `1788335142.5668371`, seven fractional digits. Truncating to whole
//!   seconds would put every trade in a busy second on one instant; read
//!   at nanosecond scale the digits survive exactly.
//! - `price` and `amount` are strings, and the venue symbol is `BTC_USDT`
//!   — the catalog's own `source_symbol`.
//! - **`trades_unsubscribe` takes no parameters**: it drops every trade
//!   subscription on the connection, not one symbol. This protocol's
//!   unsubscribe therefore cannot be per-instrument; see
//!   [`WhitebitTradesProtocol::unsubscribe_frame`].

use std::sync::Arc;
use std::time::Duration;

use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://api.whitebit.com/ws` — confirmed live 2026-09-02.
pub(crate) const WHITEBIT_WS_URL: &str = "wss://api.whitebit.com/ws";

/// `WhiteBIT` joins base and quote with `_`.
const SEPARATOR: char = '_';

/// Fractional digits to read `time` at. Nine of them turn fractional
/// seconds into a nanosecond count, which is what [`UnixNanos`] holds.
const TIME_FRACTION: u8 = 9;

/// How often to send the confirmed `ping`. Our own conservative choice,
/// not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(20);

/// `WhiteBIT`'s public trades subscription.
pub(crate) struct WhitebitTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl WhitebitTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }
}

impl VenueProtocol for WhitebitTradesProtocol {
    fn url(&self) -> &str {
        WHITEBIT_WS_URL
    }

    fn venue(&self) -> &'static str {
        "whitebit"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no WhiteBIT native symbol known for {instrument}"))
        })?;
        Ok(format!(
            r#"{{"id":1,"method":"trades_subscribe","params":["{symbol}"]}}"#
        ))
    }

    /// Drops **every** trade subscription on this connection, not just
    /// `instrument`'s: `trades_unsubscribe` takes no parameters, which was
    /// confirmed live by sending it with an empty `params` and getting a
    /// success.
    ///
    /// That is safe here because the pool only unsubscribes when the last
    /// lease on a connection is released — and the reconnect path replays
    /// exactly what is leased, so a connection that still carried other
    /// instruments would get them back on its next redial rather than
    /// losing them for good.
    fn unsubscribe_frame(&self, _instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(r#"{"id":1,"method":"trades_unsubscribe","params":[]}"#.to_owned())
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        if frame.method.as_deref() != Some("trades_update") {
            return Vec::new();
        }
        let Some(params) = frame.params else {
            return Vec::new();
        };
        let Ok((symbol, trades)) = serde_json::from_str::<(String, Vec<Trade<'_>>)>(params.get())
        else {
            return Vec::new();
        };
        let Ok(instrument) =
            InstrumentId::new(&self.source_id, &normalise_symbol(&symbol, &[SEPARATOR]))
        else {
            return Vec::new();
        };
        trades
            .iter()
            .filter_map(|entry| {
                // Fractional seconds at nanosecond scale is a nanosecond
                // count; whole seconds would collapse a busy second.
                let nanos = senken_core::parse_scaled(entry.time.get(), TIME_FRACTION)?;
                Some((
                    instrument.clone(),
                    LiveUpdate::Price(trade(
                        UnixNanos::from_nanos(nanos),
                        &entry.price,
                        &entry.amount,
                    )?),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((
            KEEPALIVE,
            r#"{"id":1,"method":"ping","params":[]}"#.to_owned(),
        ))
    }
}

/// One inbound frame. A response to a request has `result` and a null
/// `method`; a push has both `method` and `params`.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(default)]
    method: Option<String>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
}

#[derive(Debug, Deserialize)]
struct Trade<'a> {
    /// Fractional epoch seconds, a bare JSON number.
    #[serde(borrow)]
    time: &'a RawValue,
    price: String,
    amount: String,
}

/// `WhiteBIT`'s live-feed registration.
pub(crate) struct WhitebitFeedSource {
    source_ids: Vec<String>,
}

impl WhitebitFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for WhitebitFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(WhitebitTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{WHITEBIT_WS_URL, WhitebitTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;

    struct UnderscoreMap;
    impl SymbolMap for UnderscoreMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            instrument
                .symbol()
                .strip_suffix("USDT")
                .map(|base| format!("{base}_USDT"))
        }
    }

    fn protocol() -> WhitebitTradesProtocol {
        WhitebitTradesProtocol::new(crate::SOURCE_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), WHITEBIT_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "trades_subscribe");
        assert_eq!(frame["params"][0], "BTC_USDT");
    }

    /// Confirmed live: `trades_unsubscribe` takes no parameters at all.
    /// Sending a symbol in `params` would be inventing an argument the
    /// venue never acknowledged.
    #[test]
    fn the_unsubscribe_frame_carries_no_parameters() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "trades_unsubscribe");
        assert_eq!(frame["params"], serde_json::json!([]));
    }

    /// Byte-for-byte a frame from this module's live capture, spacing
    /// included — WhiteBIT writes `", "` between members and a decoder
    /// must not care.
    #[test]
    fn the_captured_update_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"method": "trades_update", "params": ["BTC_USDT", [{"id": 23416219526, "time": 1788335142.5668371, "price": "77572.99", "amount": "0.012017", "type": "buy", "rpi": false}]], "id": null}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades_update must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_757_299);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(12_017));
        assert_eq!(update.qty_scale, 6);
        assert_eq!(
            update.ts.as_nanos(),
            1_788_335_142_566_837_100,
            "the fractional seconds survive to the nanosecond"
        );
    }

    /// Two trades a fraction of a second apart must not land on the same
    /// instant — which is what reading `time` as whole seconds would do.
    #[test]
    fn two_trades_inside_one_second_keep_distinct_timestamps() {
        let frame = r#"{"method": "trades_update", "params": ["BTC_USDT", [{"id": 1, "time": 1788335142.5668371, "price": "77572.99", "amount": "0.012017", "type": "buy"}, {"id": 2, "time": 1788335142.5670190, "price": "77572.99", "amount": "0.001525", "type": "buy"}]], "id": null}"#;
        let updates = protocol().parse_message(frame);
        assert_eq!(updates.len(), 2);
        let (LiveUpdate::Price(a), LiveUpdate::Price(b)) = (updates[0].1, updates[1].1) else {
            panic!("both entries must decode to price updates");
        };
        assert!(a.ts < b.ts);
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        assert!(
            protocol()
                .parse_message(r#"{"error": null, "result": {"status": "success"}, "id": 1}"#)
                .is_empty()
        );
        assert!(
            protocol()
                .parse_message(r#"{"error": null, "result": "pong", "id": 1}"#)
                .is_empty()
        );
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn the_keepalive_is_the_confirmed_ping_request() {
        let (_, frame) = protocol().keepalive().unwrap();
        let frame: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame["method"], "ping");
    }
}
