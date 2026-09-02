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
    /// Which socket this market streams from — the three markets are
    /// three hosts.
    url: String,
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BingxTradesProtocol {
    #[cfg(test)]
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self::for_market(source_id, BINGX_WS_URL, symbols)
    }

    /// A protocol streaming from `url`.
    pub(crate) fn for_market(
        source_id: impl Into<Box<str>>,
        url: impl Into<String>,
        symbols: Arc<dyn SymbolMap>,
    ) -> Self {
        Self {
            url: url.into(),
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
        &self.url
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
        let Some(payload) = frame.data else {
            return Vec::new();
        };
        if !frame.data_type.ends_with(TRADE_STREAM) {
            return Vec::new();
        }
        payload
            .trades()
            .iter()
            .filter_map(|entry| {
                let instrument = InstrumentId::new(
                    &self.source_id,
                    &normalise_symbol(&entry.symbol, &[SEPARATOR]),
                )
                .ok()?;
                let ts = senken_core::UnixNanos::from_millis(entry.executed_at)?;
                Some((
                    instrument,
                    LiveUpdate::Price(trade(ts, &entry.price, &entry.qty)?),
                ))
            })
            .collect()
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
    /// The stream this frame belongs to, `BTC-USDT@trade`. It is the only
    /// thing both markets agree on: the perpetual streams carry no `e`
    /// field on a trade at all, so a decoder keying on that reads nothing
    /// from them — and, since an unrecognised frame is not an error,
    /// reports a connected and permanently silent market.
    #[serde(default, rename = "dataType")]
    data_type: String,
    #[serde(default)]
    data: Option<Payload>,
}

/// The suffix the trade stream's `dataType` ends with.
const TRADE_STREAM: &str = "@trade";

/// The spot stream sends one trade as an object; the two perpetual
/// streams send an array of them. Confirmed live 2026-09-02 — and the
/// difference is silent, because a decoder expecting the wrong one reads
/// no trades at all and reports no error.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Payload {
    One(Box<Trade>),
    Many(Vec<Trade>),
}

impl Payload {
    fn trades(&self) -> &[Trade] {
        match self {
            Self::One(trade) => std::slice::from_ref(trade),
            Self::Many(trades) => trades,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Trade {
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
    url: String,
}

impl BingxFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
            url: BINGX_WS_URL.to_owned(),
        }
    }

    /// A feed for one of `BingX`'s perpetual markets — each on its own
    /// host, both gzip-compressed like spot. Confirmed live 2026-09-02.
    pub(crate) fn for_market(source_id: &str, url: impl Into<String>) -> Self {
        Self {
            source_ids: vec![source_id.to_owned()],
            url: url.into(),
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
        Arc::new(BingxTradesProtocol::for_market(
            self.source_ids[0].as_str(),
            self.url.clone(),
            symbols,
        ))
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

    /// The perpetual streams send `data` as an **array** where spot sends
    /// a single object. A decoder built for one reads nothing from the
    /// other, and reports no error while doing it — which is how a market
    /// ends up registered, connected and permanently silent. Captured
    /// live 2026-09-02 from `open-api-swap.bingx.com`.
    #[test]
    fn a_perpetual_frame_sends_its_trades_as_an_array() {
        let protocol = BingxTradesProtocol::for_market(
            crate::LINEAR_ID,
            "wss://open-api-swap.bingx.com/swap-market",
            Arc::new(DashedMap),
        );
        // Verbatim: note there is no `"e"` field at all, unlike spot's.
        let frame = r#"{"code":0,"dataType":"BTC-USDT@trade","data":[{"q":"0.0009","p":"76398.3","T":1788348255196,"m":false,"s":"BTC-USDT"}]}"#;

        let updates = protocol.parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a perpetual trade frame must decode to a price update");
        };
        assert_eq!(id, &InstrumentId::new(crate::LINEAR_ID, "BTCUSDT").unwrap());
        assert_eq!(update.price, 763_983);
    }
}
