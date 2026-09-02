//! Crypto.com's public `trade` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://stream.crypto.com/exchange/v1/market`, sent
//! `{"id":1,"method":"subscribe","params":{"channels":["trade.BTC_USDT"]},"nonce":1788335000000}`
//! and received:
//!
//! ```json
//! {"id":-1,"method":"subscribe","code":0,"result":{"instrument_name":"BTC_USDT","subscription":"trade.BTC_USDT","channel":"trade","data":[{"d":"1788335095880381802","t":1788335095880,"p":"77558.92","q":"0.00004","s":"SELL","i":"BTC_USDT","m":"4611686018690432925"}]}}
//! {"id":1788335101113,"method":"public/heartbeat","code":0}
//! ```
//!
//! Two shapes in that capture each matter:
//!
//! - **The acknowledgement and the data frames are the same shape.** Both
//!   are `method":"subscribe"` with a `result`; the first carries a
//!   backfill of recent trades and the rest carry one or two new ones. So
//!   there is nothing to filter out — every `result.data` entry is a real
//!   trade, and the first frame gives a chart a price immediately.
//! - **Crypto.com pings the client** with `public/heartbeat` and expects
//!   `public/respond-heartbeat` carrying the *same* `id`. A connection
//!   that does not answer is dropped. [`VenueProtocol::reply_to`] is where
//!   that is answered.
//!
//! Also read from the capture: `p` and `q` are strings, `t` is the trade's
//! epoch milliseconds, `q` is the base-asset quantity (the same measure
//! this project's Crypto.com bars store), and the venue symbol is
//! `BTC_USDT` — the catalog's own `source_symbol`.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://stream.crypto.com/exchange/v1/market` — confirmed live
/// 2026-09-02.
pub(crate) const CRYPTOCOM_WS_URL: &str = "wss://stream.crypto.com/exchange/v1/market";

/// Crypto.com joins base and quote with `_` on spot (`BTC_USDT`) and
/// marks a perpetual with `-` (`BTCUSD-PERP`). Both are stripped, and by
/// the same rule this plugin's catalog uses — a decoder stripping only one
/// of them builds a symbol the catalog does not hold, and every frame for
/// that market is dropped with nothing to say so.
const SEPARATORS: [char; 2] = ['_', '-'];

/// Crypto.com's public `trade` channel.
pub(crate) struct CryptocomTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl CryptocomTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!(
                "no Crypto.com native symbol known for {instrument}"
            ))
        })
    }

    fn frame(method: &str, symbol: &str) -> String {
        format!(r#"{{"id":1,"method":"{method}","params":{{"channels":["trade.{symbol}"]}}}}"#)
    }
}

impl VenueProtocol for CryptocomTradesProtocol {
    fn url(&self) -> &str {
        CRYPTOCOM_WS_URL
    }

    fn venue(&self) -> &'static str {
        "cryptocom"
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
        let Some(result) = frame.result else {
            return Vec::new();
        };
        if result.channel != "trade" {
            return Vec::new();
        }
        result
            .data
            .iter()
            .filter_map(|entry| {
                let instrument = InstrumentId::new(
                    &self.source_id,
                    &normalise_symbol(&entry.instrument, &SEPARATORS),
                )
                .ok()?;
                let ts = senken_core::UnixNanos::from_millis(entry.time)?;
                Some((
                    instrument,
                    LiveUpdate::Price(trade(ts, &entry.price, &entry.quantity)?),
                ))
            })
            .collect()
    }

    fn reply_to(&self, text: &str) -> Option<String> {
        // The answer must echo the heartbeat's own id; Crypto.com drops a
        // connection that stops answering.
        let frame = serde_json::from_str::<Heartbeat>(text).ok()?;
        (frame.method == "public/heartbeat").then(|| {
            format!(
                r#"{{"id":{},"method":"public/respond-heartbeat"}}"#,
                frame.id
            )
        })
    }
}

/// One inbound frame. A heartbeat carries no `result`, so it decodes to
/// `None` here and is handled by [`CryptocomTradesProtocol::reply_to`].
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    result: Option<SubscriptionResult>,
}

#[derive(Debug, Deserialize)]
struct SubscriptionResult {
    channel: String,
    #[serde(default)]
    data: Vec<Trade>,
}

#[derive(Debug, Deserialize)]
struct Trade {
    #[serde(rename = "i")]
    instrument: String,
    #[serde(rename = "p")]
    price: String,
    /// Base-asset quantity.
    #[serde(rename = "q")]
    quantity: String,
    /// Epoch milliseconds.
    #[serde(rename = "t")]
    time: i64,
}

/// A venue-initiated keep-alive.
#[derive(Debug, Deserialize)]
struct Heartbeat {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    method: String,
}

/// Crypto.com's live-feed registration.
pub(crate) struct CryptocomFeedSource {
    source_ids: Vec<String>,
}

impl CryptocomFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for CryptocomFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(CryptocomTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{CRYPTOCOM_WS_URL, CryptocomTradesProtocol};
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

    fn protocol() -> CryptocomTradesProtocol {
        CryptocomTradesProtocol::new(crate::SOURCE_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), CRYPTOCOM_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "subscribe");
        assert_eq!(frame["params"]["channels"][0], "trade.BTC_USDT");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_method() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "unsubscribe");
    }

    /// Byte-for-byte a data frame from this module's live capture.
    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"id":-1,"method":"subscribe","code":0,"result":{"instrument_name":"BTC_USDT","subscription":"trade.BTC_USDT","channel":"trade","data":[{"d":"1788335095880381802","t":1788335095880,"p":"77558.92","q":"0.00004","s":"SELL","i":"BTC_USDT","m":"4611686018690432925"},{"d":"1788335095880381801","t":1788335095880,"p":"77558.92","q":"0.00064","s":"SELL","i":"BTC_USDT","m":"4611686018690432925"}]}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 2);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_755_892);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(4));
        assert_eq!(update.qty_scale, 5);
        assert_eq!(update.ts.as_millis(), 1_788_335_095_880);
    }

    /// The answer must echo the heartbeat's own id — Crypto.com drops a
    /// connection that stops answering, and an id it did not send is not
    /// an answer.
    #[test]
    fn the_captured_heartbeat_is_answered_with_the_same_id() {
        let reply = protocol()
            .reply_to(r#"{"id":1788335101113,"method":"public/heartbeat","code":0}"#)
            .unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["method"], "public/respond-heartbeat");
        assert_eq!(reply["id"], 1_788_335_101_113_i64);
    }

    #[test]
    fn a_data_frame_needs_no_reply() {
        assert!(
            protocol()
                .reply_to(r#"{"id":-1,"method":"subscribe","code":0,"result":{"channel":"trade","data":[]}}"#)
                .is_none()
        );
        assert!(protocol().reply_to("not json").is_none());
    }

    /// Crypto.com's spot and perpetual markets are one source here, and
    /// the two write their symbols differently: `BTC_USDT` against
    /// `BTCUSD-PERP`. The catalog strips both separators, so a decoder
    /// that strips only the underscore attributes every perpetual trade to
    /// an instrument that does not exist — and drops it with nothing to
    /// say so. Captured live 2026-09-02.
    #[test]
    fn a_perpetual_frame_whose_symbol_uses_a_dash_is_attributed() {
        struct PerpMap;
        impl SymbolMap for PerpMap {
            fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
                instrument
                    .symbol()
                    .strip_suffix("PERP")
                    .map(|base| format!("{base}-PERP"))
            }
        }
        let protocol = CryptocomTradesProtocol::new(crate::SOURCE_ID, Arc::new(PerpMap));
        let frame = r#"{"id":3,"method":"subscribe","code":0,"result":{"instrument_name":"BTCUSD-PERP","subscription":"trade.BTCUSD-PERP","channel":"trade","data":[{"d":"1788339242977354950","t":1788339242977,"p":"76774.5","q":"0.0065","s":"SELL","i":"BTCUSD-PERP","m":"4611686018789914730"}]}}"#;

        let updates = protocol.parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a perpetual trade frame must decode to a price update");
        };
        assert_eq!(
            id,
            &InstrumentId::new(crate::SOURCE_ID, "BTCUSDPERP").unwrap(),
            "the dash has to be stripped, exactly as the catalog strips it"
        );
        assert_eq!(update.price, 767_745);
        assert_eq!(update.price_scale, 1);
    }

    #[test]
    fn a_channel_this_protocol_did_not_subscribe_is_ignored() {
        let frame = r#"{"id":-1,"method":"subscribe","code":0,"result":{"instrument_name":"BTC_USDT","subscription":"book.BTC_USDT","channel":"book","data":[{"i":"BTC_USDT","p":"1","q":"1","t":1}]}}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
