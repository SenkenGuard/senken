//! Bybit's public spot WebSocket — trades and tickers on one socket.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://stream.bybit.com/v5/public/spot`, sent
//! `{"op":"subscribe","args":["publicTrade.BTCUSDT","tickers.BTCUSDT"]}`,
//! and received, in order:
//!
//! ```json
//! {"success":true,"ret_msg":"subscribe","conn_id":"d9avrj8i70nu0j2a80n0-9cfc4","op":"subscribe"}
//! {"topic":"tickers.BTCUSDT","ts":1788335008126,"type":"snapshot","cs":113508069165,"data":{"symbol":"BTCUSDT","lastPrice":"77606.8",…}}
//! {"topic":"publicTrade.BTCUSDT","ts":1788335009483,"type":"snapshot","data":[{"i":"2290000001202469355","T":1788335009482,"p":"77606.8","v":"0.000712","S":"Buy","seq":113508069853,"s":"BTCUSDT","BT":false,"RPI":false}]}
//! ```
//!
//! In the same session, `{"op":"ping"}` was answered with
//! `{"success":true,"ret_msg":"pong","op":"ping"}` and
//! `{"op":"unsubscribe","args":["publicTrade.BTCUSDT"]}` with
//! `{"success":true,"ret_msg":"unsubscribe","op":"unsubscribe"}` — so both
//! frames this protocol sends beyond the subscribe are confirmed, not
//! assumed.
//!
//! Read from that capture:
//! - The venue symbol is `BTCUSDT`, with no separator — identical to the
//!   spot catalog's own `source_symbol`, so no translation is needed
//!   beyond looking it up.
//! - `p` and `v` are **strings**; `T` is the trade's own epoch
//!   milliseconds, distinct from the envelope's `ts` (the time Bybit sent
//!   the frame). The trade's time is the one that belongs on a tick.
//! - A `publicTrade` frame's `data` is an array — Bybit batches.
//! - The `tickers` channel on **spot** carries `lastPrice` and 24h
//!   statistics but **no `bid1Price`/`ask1Price`**. That is why
//!   [`BybitFeedSource::serves_quotes`] answers `false`: claiming a
//!   best-bid-and-offer this socket does not send would put bid/ask lines
//!   on a chart that never move.
//!
//! # Assumptions, flagged as such
//!
//! Bybit's own documented idle timeout was not measured here — holding a
//! socket open long enough to be dropped would take minutes per venue.
//! [`KEEPALIVE`] is a conservative interval well under any published one,
//! sending the ping frame that *was* confirmed live above.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://stream.bybit.com/v5/public/spot` — confirmed live 2026-09-02.
pub(crate) const BYBIT_SPOT_WS_URL: &str = "wss://stream.bybit.com/v5/public/spot";

/// Bybit's spot symbols carry no separator (`BTCUSDT`), but its option
/// symbols do (`BTC-26SEP25-…`), and this plugin's catalog strips `-` from
/// every market. Matching that rule here rather than assuming spot's shape
/// keeps the two from disagreeing if this feed ever serves more markets.
const SEPARATORS: [char; 1] = ['-'];

/// How often to send the confirmed `{"op":"ping"}`. Our own conservative
/// choice, not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(20);

/// Bybit's public spot trades channel.
pub(crate) struct BybitTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BybitTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Bybit native symbol known for {instrument}"))
        })
    }

    fn frame(op: &str, symbol: &str) -> String {
        format!(r#"{{"op":"{op}","args":["publicTrade.{symbol}"]}}"#)
    }
}

impl VenueProtocol for BybitTradesProtocol {
    fn url(&self) -> &str {
        BYBIT_SPOT_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bybit"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("subscribe", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("unsubscribe", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame>(text) else {
            return Vec::new();
        };
        if !frame.topic.starts_with("publicTrade.") {
            return Vec::new();
        }
        frame
            .data
            .iter()
            .filter_map(|entry| self.decode(entry))
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, r#"{"op":"ping"}"#.to_owned()))
    }
}

impl BybitTradesProtocol {
    fn decode(&self, entry: &Trade) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(&entry.symbol, &SEPARATORS),
        )
        .ok()?;
        let ts = senken_core::UnixNanos::from_millis(entry.time)?;
        Some((
            instrument,
            LiveUpdate::Price(trade(ts, &entry.price, &entry.size)?),
        ))
    }
}

/// One inbound frame. The subscribe acknowledgement carries no `topic`, so
/// `#[serde(default)]` decodes it to an empty one rather than an error.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    topic: String,
    #[serde(default)]
    data: Vec<Trade>,
}

/// One `publicTrade` entry. Bybit's field names are single letters; only
/// the four this crate reads are named.
#[derive(Debug, Deserialize)]
struct Trade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "v")]
    size: String,
    /// The trade's own epoch milliseconds, not the envelope's send time.
    #[serde(rename = "T")]
    time: i64,
}

/// Bybit's live-feed registration.
///
/// Spot only: the capture above is a spot socket, and Bybit's linear,
/// inverse and option markets each live behind a different URL that no
/// frame in this project has been seen from.
pub(crate) struct BybitFeedSource {
    source_ids: Vec<String>,
}

impl BybitFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for BybitFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // The spot `tickers` channel carries `lastPrice` and 24h stats but
        // no best bid or ask — see this module's docs.
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BybitTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BYBIT_SPOT_WS_URL, BybitTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    fn protocol() -> BybitTradesProtocol {
        BybitTradesProtocol::new(crate::SPOT_ID, Arc::new(IdentitySymbolMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BYBIT_SPOT_WS_URL);
    }

    #[test]
    fn the_subscribe_and_unsubscribe_frames_match_the_confirmed_shapes() {
        let subscribe: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(subscribe["op"], "subscribe");
        assert_eq!(subscribe["args"][0], "publicTrade.BTCUSDT");

        let unsubscribe: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(unsubscribe["op"], "unsubscribe");
    }

    /// Byte-for-byte a frame from this module's live capture.
    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"topic":"publicTrade.BTCUSDT","ts":1788335009483,"type":"snapshot","data":[{"i":"2290000001202469355","T":1788335009482,"p":"77606.8","v":"0.000712","S":"Buy","seq":113508069853,"s":"BTCUSDT","BT":false,"RPI":false}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a publicTrade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 776_068);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.qty, senken_series::Volume::Real(712));
        assert_eq!(update.qty_scale, 6);
        assert_eq!(
            update.ts.as_millis(),
            1_788_335_009_482,
            "the trade's own `T`, not the envelope's `ts` (1788335009483)"
        );
    }

    /// The tickers channel was subscribed in the same capture and its
    /// frames must not be mistaken for trades — it carries `lastPrice`,
    /// which is a *quote-looking* field with no size beside it.
    #[test]
    fn a_tickers_frame_yields_nothing() {
        let frame = r#"{"topic":"tickers.BTCUSDT","ts":1788335008126,"type":"snapshot","cs":113508069165,"data":{"symbol":"BTCUSDT","lastPrice":"77606.8","highPrice24h":"78685.3","lowPrice24h":"76412.6","prevPrice24h":"78605.2","volume24h":"7654.575275","turnover24h":"594624551.61837538","price24hPcnt":"-0.0127","usdIndexPrice":"77561.956977"}}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"success":true,"ret_msg":"subscribe","conn_id":"d9avrj8i70nu0j2a80n0-9cfc4","op":"subscribe"}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    /// The ping frame is the one confirmed live; a keep-alive that sent
    /// something else would be answered with an error, not a pong.
    #[test]
    fn the_keepalive_is_the_confirmed_ping_frame() {
        let (every, frame) = protocol().keepalive().unwrap();
        assert_eq!(frame, r#"{"op":"ping"}"#);
        assert!(every < std::time::Duration::from_secs(30));
    }
}
