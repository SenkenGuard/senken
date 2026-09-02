//! `BitMart`'s public `spot/trade` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws-manager-compress.bitmart.com/api?protocol=1.1`,
//! sent `{"op":"subscribe","args":["spot/trade:BTC_USDT"]}`, received
//! `{"topic":"spot/trade:BTC_USDT","event":"subscribe"}` and then:
//!
//! ```json
//! {"data":[{"ms_t":1788335076327,"price":"77604.55","s_t":1788335076,"side":"sell","size":"0.01307","symbol":"BTC_USDT"}],"table":"spot/trade"}
//! ```
//!
//! In the same session the bare text `ping` was answered with `pong`.
//!
//! Read from that capture:
//! - Despite `ws-manager-**compress**` in the host name, every frame
//!   arrived as **plain text**. No gzip decoding is needed here (unlike
//!   HTX and BingX, which genuinely compress).
//! - `ms_t` is the trade's epoch milliseconds; `s_t` beside it is the same
//!   instant in whole seconds and would lose the sub-second ordering.
//! - `price` and `size` are strings, and the symbol is on each entry.
//! - The venue symbol is `BTC_USDT`, exactly the catalog's `source_symbol`.
//!
//! # Not verified
//!
//! An unsubscribe was sent on a connection that had never subscribed and
//! came back `{"errorCode":"90009","errorMessage":"Invalid unsubscription"}`
//! — which tells us the frame *shape* is understood and that BitMart
//! rejects unsubscribing something it is not sending, but not that a real
//! unsubscribe succeeds. The pool's own bookkeeping does not depend on it:
//! a rejected unsubscribe wastes bandwidth until the next reconnect
//! replays exactly what is leased.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://ws-manager-compress.bitmart.com/api?protocol=1.1` — confirmed
/// live 2026-09-02.
pub(crate) const BITMART_WS_URL: &str = "wss://ws-manager-compress.bitmart.com/api?protocol=1.1";

/// The channel this protocol subscribes, before the `:SYMBOL` suffix.
const TRADE_TABLE: &str = "spot/trade";

/// `BitMart` joins base and quote with `_`.
const SEPARATOR: char = '_';

/// How often to send the confirmed bare-text `ping`. Our own conservative
/// choice, not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// `BitMart`'s public spot trade channel.
pub(crate) struct BitmartTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BitmartTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no BitMart native symbol known for {instrument}"))
        })
    }

    fn frame(op: &str, symbol: &str) -> String {
        format!(r#"{{"op":"{op}","args":["{TRADE_TABLE}:{symbol}"]}}"#)
    }
}

impl VenueProtocol for BitmartTradesProtocol {
    fn url(&self) -> &str {
        BITMART_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bitmart"
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
        if frame.table != TRADE_TABLE {
            return Vec::new();
        }
        frame
            .data
            .iter()
            .filter_map(|entry| {
                let instrument = InstrumentId::new(
                    &self.source_id,
                    &normalise_symbol(&entry.symbol, &[SEPARATOR]),
                )
                .ok()?;
                let ts = senken_core::UnixNanos::from_millis(entry.ms_t)?;
                Some((
                    instrument,
                    LiveUpdate::Price(trade(ts, &entry.price, &entry.size)?),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, "ping".to_owned()))
    }
}

/// One inbound frame. The subscribe acknowledgement has `topic`/`event`
/// and no `table`, so it decodes to an empty table rather than an error.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    table: String,
    #[serde(default)]
    data: Vec<Trade>,
}

#[derive(Debug, Deserialize)]
struct Trade {
    symbol: String,
    price: String,
    size: String,
    /// Epoch milliseconds. `s_t` beside it is whole seconds.
    ms_t: i64,
}

/// `BitMart`'s live-feed registration — spot only. Its futures stream is a
/// different host that no capture in this project has reached.
pub(crate) struct BitmartFeedSource {
    source_ids: Vec<String>,
}

impl BitmartFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for BitmartFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BitmartTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BITMART_WS_URL, BitmartTradesProtocol};
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

    fn protocol() -> BitmartTradesProtocol {
        BitmartTradesProtocol::new(crate::SPOT_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BITMART_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "subscribe");
        assert_eq!(frame["args"][0], "spot/trade:BTC_USDT");
    }

    #[test]
    fn an_unsubscribe_frame_names_the_same_channel() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "unsubscribe");
        assert_eq!(frame["args"][0], "spot/trade:BTC_USDT");
    }

    /// Byte-for-byte a frame from this module's live capture.
    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"data":[{"ms_t":1788335076327,"price":"77604.55","s_t":1788335076,"side":"sell","size":"0.01307","symbol":"BTC_USDT"}],"table":"spot/trade"}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a spot/trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_760_455);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(1_307));
        assert_eq!(update.qty_scale, 5);
        assert_eq!(
            update.ts.as_millis(),
            1_788_335_076_327,
            "`ms_t`, not the whole-second `s_t` beside it"
        );
    }

    /// BitMart pads its prices (`"77576.00"`). Trailing zeros carry no
    /// information, and this project's convention — `decimal_places` —
    /// drops them, so the same price arrives at a smaller scale than a
    /// neighbouring unpadded one. What must not change is the number: the
    /// scale and the integer have to stay consistent with each other.
    #[test]
    fn a_padded_price_decodes_to_the_same_number_at_its_own_scale() {
        let frame = r#"{"data":[{"ms_t":1788335082732,"price":"77576.00","s_t":1788335082,"side":"sell","size":"0.01871","symbol":"BTC_USDT"}],"table":"spot/trade"}"#;
        let updates = protocol().parse_message(frame);
        let (_, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a spot/trade frame must decode to a price update");
        };
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.price, 77_576);
        assert_eq!(update.qty_scale, 5);
        assert_eq!(update.qty, senken_series::Volume::Real(1_871));
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"topic":"spot/trade:BTC_USDT","event":"subscribe"}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("pong").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn the_keepalive_is_the_confirmed_bare_text_ping() {
        let (_, frame) = protocol().keepalive().unwrap();
        assert_eq!(frame, "ping");
    }
}
