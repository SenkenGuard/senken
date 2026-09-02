//! `BingX`'s public spot `@trade` stream.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://open-api-ws.bingx.com/market`, sent
//! `{"id":"senken-1","reqType":"sub","dataType":"BTC-USDT@trade"}` and
//! received — **every frame gzip-compressed, none of them text**:
//!
//! ```json
//! {"code":0,"id":"senken-1","msg":"SUCCESS","timestamp":1788336365712}
//! {"code":0,"data":{"E":1788336366302,"T":1788336366288,"e":"trade","m":false,"p":"77495.49","q":"0.0074439581595275245","s":"BTC-USDT","t":"233847470"},"dataType":"BTC-USDT@trade","success":true,"timestamp":1788336366302}
//! ```
//!
//! Read from that capture:
//! - **Every frame is gzip.** A connection reading only text frames gets
//!   nothing at all from BingX, without an error to say so.
//!   [`VenueProtocol::decode_binary`] is where that is undone.
//! - **`q` carries nineteen fractional digits.** That still fits an `i64`
//!   here, but it is close enough to the limit that a larger trade would
//!   not — which is why the shared decoder publishes the price with an
//!   absent size rather than dropping the tick.
//! - **`T` is the trade's time; `E` is the event's.** They differ by 14ms
//!   in this frame.
//! - `p` and `q` are strings, and the venue symbol is `BTC-USDT` — the
//!   catalog's own `source_symbol`.
//!
//! # A conservative measure, not an observation
//!
//! BingX is documented to send a bare `Ping` that the client answers with
//! `Pong`. **No such frame arrived in 25 seconds of this capture**, so the
//! shape below is not something this project has seen. It is answered
//! anyway because the cost of doing so is one frame and the cost of not
//! doing so is a socket dropped for silence; a `Ping` that never comes
//! means the branch never runs.

use std::io::Read;
use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://open-api-ws.bingx.com/market` — confirmed live 2026-09-02.
pub(crate) const BINGX_WS_URL: &str = "wss://open-api-ws.bingx.com/market";

/// `BingX` joins base and quote with `-`.
const SEPARATOR: char = '-';

/// `BingX`'s public spot trade stream.
pub(crate) struct BingxTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BingxTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn data_type(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no BingX native symbol known for {instrument}"))
        })?;
        Ok(format!("{symbol}@trade"))
    }
}

impl VenueProtocol for BingxTradesProtocol {
    fn url(&self) -> &str {
        BINGX_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bingx"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let data_type = self.data_type(instrument)?;
        Ok(format!(
            r#"{{"id":"{data_type}","reqType":"sub","dataType":"{data_type}"}}"#
        ))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let data_type = self.data_type(instrument)?;
        Ok(format!(
            r#"{{"id":"{data_type}","reqType":"unsub","dataType":"{data_type}"}}"#
        ))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame>(text) else {
            return Vec::new();
        };
        let Some(data) = frame.data else {
            return Vec::new();
        };
        if data.event != "trade" {
            return Vec::new();
        }
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(&data.symbol, &[SEPARATOR]),
        ) else {
            return Vec::new();
        };
        let Some(ts) = senken_core::UnixNanos::from_millis(data.executed_at) else {
            return Vec::new();
        };
        trade(ts, &data.price, &data.qty)
            .map(|update| vec![(instrument, LiveUpdate::Price(update))])
            .unwrap_or_default()
    }

    fn decode_binary(&self, bytes: &[u8]) -> Option<String> {
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn reply_to(&self, text: &str) -> Option<String> {
        // Never observed live — see this module's docs.
        (text.trim() == "Ping").then(|| "Pong".to_owned())
    }
}

/// One inbound frame. The subscribe acknowledgement carries `msg` and no
/// `data`, so it decodes to `None`.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    data: Option<Trade>,
}

#[derive(Debug, Deserialize)]
struct Trade {
    #[serde(rename = "e")]
    event: String,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    /// The trade's own epoch milliseconds — `E` beside it is the event's.
    #[serde(rename = "T")]
    executed_at: i64,
}

/// `BingX`'s live-feed registration — spot only. Its perpetual markets
/// stream from a different host that no capture here has reached.
pub(crate) struct BingxFeedSource {
    source_ids: Vec<String>,
}

impl BingxFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for BingxFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BingxTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BINGX_WS_URL, BingxTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::io::Write;
    use std::sync::Arc;

    struct DashedMap;
    impl SymbolMap for DashedMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            instrument
                .symbol()
                .strip_suffix("USDT")
                .map(|base| format!("{base}-USDT"))
        }
    }

    fn protocol() -> BingxTradesProtocol {
        BingxTradesProtocol::new(crate::SPOT_ID, Arc::new(DashedMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    /// The captured data frame, verbatim.
    const TRADE_FRAME: &str = r#"{"code":0,"data":{"E":1788336366302,"T":1788336366288,"e":"trade","m":false,"p":"77495.49","q":"0.0074439581595275245","s":"BTC-USDT","t":"233847470"},"dataType":"BTC-USDT@trade","success":true,"timestamp":1788336366302}"#;

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BINGX_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["reqType"], "sub");
        assert_eq!(frame["dataType"], "BTC-USDT@trade");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_request_type() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["reqType"], "unsub");
        assert_eq!(frame["dataType"], "BTC-USDT@trade");
    }

    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let updates = protocol().parse_message(TRADE_FRAME);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_749_549);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty_scale, 19);
        assert_eq!(
            update.qty,
            senken_series::Volume::Real(74_439_581_595_275_245)
        );
        assert_eq!(
            update.ts.as_millis(),
            1_788_336_366_288,
            "`T`, the trade's own time, not `E` (…302)"
        );
    }

    /// BingX sends nothing as text. A protocol that cannot inflate a gzip
    /// frame receives no data at all from this venue, silently.
    #[test]
    fn a_gzip_frame_is_inflated_to_the_text_the_parser_reads() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(TRADE_FRAME.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let text = protocol().decode_binary(&compressed).unwrap();

        assert_eq!(text, TRADE_FRAME);
        assert_eq!(protocol().parse_message(&text).len(), 1);
    }

    #[test]
    fn a_binary_frame_that_is_not_gzip_yields_nothing_rather_than_a_panic() {
        assert!(protocol().decode_binary(b"not gzip at all").is_none());
    }

    #[test]
    fn the_documented_ping_is_answered_and_nothing_else_is() {
        assert_eq!(protocol().reply_to("Ping").as_deref(), Some("Pong"));
        assert!(protocol().reply_to(TRADE_FRAME).is_none());
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"code":0,"id":"senken-1","msg":"SUCCESS","timestamp":1788336365712}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
