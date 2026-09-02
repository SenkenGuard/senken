//! Bitstamp's public `live_trades` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws.bitstamp.net`, sent
//! `{"event":"bts:subscribe","data":{"channel":"live_trades_btcusd"}}`,
//! received `{"event":"bts:subscription_succeeded","channel":"live_trades_btcusd","data":{}}`
//! and then:
//!
//! ```json
//! {"data":{"id":628186942,"timestamp":"1788335012","amount":0.00006514,"amount_str":"0.00006514","price":77558.07,"price_str":"77558.07","type":1,"microtimestamp":"1788335012854000","buy_order_id":2045952085757952,"sell_order_id":2045952156454912},"channel":"live_trades_btcusd","event":"trade"}
//! ```
//!
//! Read from that capture, and each of these is why a field was chosen:
//! - **`price_str` and `amount_str`, never `price` and `amount`.** Bitstamp
//!   sends both, and the unsuffixed pair are bare JSON numbers — reading
//!   those puts a trade price through an `f64`, which this project does not
//!   do for money. The `_str` pair carry the venue's own digits.
//! - **`microtimestamp`, not `timestamp`.** The latter is whole seconds;
//!   the former is microseconds and is what distinguishes two trades in
//!   the same second (this capture has two).
//! - The channel name embeds the market in **lower case** —
//!   `live_trades_btcusd` — which is exactly the form Bitstamp's catalog
//!   stores as `source_symbol` and its REST order book takes in its path.
//!   No case conversion is needed in either direction.
//!
//! # Not verified
//!
//! No unsubscribe was sent in this capture. `bts:unsubscribe` is the
//! documented mirror of the confirmed `bts:subscribe` and is what this
//! protocol emits; if Bitstamp were to ignore it, the pool's own
//! bookkeeping is still correct and a stale venue subscription costs
//! bandwidth until the next reconnect replays what is actually leased.
//! No application-level keep-alive was observed — Bitstamp's socket
//! answered WebSocket control pings, which the transport handles itself.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use serde::Deserialize;

/// `wss://ws.bitstamp.net` — confirmed live 2026-09-02.
pub(crate) const BITSTAMP_WS_URL: &str = "wss://ws.bitstamp.net";

/// The channel prefix the market symbol is appended to.
const TRADES_CHANNEL: &str = "live_trades_";

/// Bitstamp's public `live_trades` channel.
pub(crate) struct BitstampTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BitstampTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn channel(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Bitstamp native symbol known for {instrument}"))
        })?;
        Ok(format!("{TRADES_CHANNEL}{symbol}"))
    }

    fn frame(event: &str, channel: &str) -> String {
        format!(r#"{{"event":"{event}","data":{{"channel":"{channel}"}}}}"#)
    }
}

impl VenueProtocol for BitstampTradesProtocol {
    fn url(&self) -> &str {
        BITSTAMP_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bitstamp"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("bts:subscribe", &self.channel(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("bts:unsubscribe", &self.channel(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame>(text) else {
            return Vec::new();
        };
        if frame.event != "trade" {
            return Vec::new();
        }
        let Some(symbol) = frame.channel.strip_prefix(TRADES_CHANNEL) else {
            return Vec::new();
        };
        // Normalised by the same rule the catalog uses, rather than by
        // hand: Bitstamp's symbols are lower case, and the two forms must
        // not be able to drift apart.
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &senken_venue::normalise_symbol(symbol, &['-']),
        ) else {
            return Vec::new();
        };
        let Some(update) = frame
            .data
            .micro
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(senken_core::UnixNanos::from_micros)
            .and_then(|ts| trade(ts, &frame.data.price, &frame.data.amount))
        else {
            return Vec::new();
        };
        vec![(instrument, LiveUpdate::Price(update))]
    }
}

/// One inbound frame. Only `trade` events carry a `data` this protocol can
/// read, so the acknowledgement's empty `data: {}` decodes to empty
/// strings rather than failing.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    event: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    data: Trade,
}

#[derive(Debug, Default, Deserialize)]
struct Trade {
    /// The venue's own price digits. `price` beside it is a bare JSON
    /// number and is deliberately not read.
    #[serde(default, rename = "price_str")]
    price: String,
    #[serde(default, rename = "amount_str")]
    amount: String,
    /// Epoch **microseconds**, as a string.
    #[serde(default, rename = "microtimestamp")]
    micro: String,
}

/// Bitstamp's live-feed registration.
pub(crate) struct BitstampFeedSource {
    source_ids: Vec<String>,
}

impl BitstampFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for BitstampFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Only `live_trades` is subscribed. Bitstamp's `order_book`
        // channel would carry a top of book, but no frame from it was
        // captured, and depth already has its own source.
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BitstampTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BITSTAMP_WS_URL, BitstampTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;

    /// Bitstamp's native symbols are lower case, so the identity map would
    /// be the wrong shape to test against — the catalog holds `btcusd`
    /// while the normalised id is `BTCUSD`.
    struct LowercaseMap;
    impl SymbolMap for LowercaseMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            Some(instrument.symbol().to_lowercase())
        }
    }

    fn protocol() -> BitstampTradesProtocol {
        BitstampTradesProtocol::new(crate::SOURCE_ID, Arc::new(LowercaseMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, "BTCUSD").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BITSTAMP_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "bts:subscribe");
        assert_eq!(frame["data"]["channel"], "live_trades_btcusd");
    }

    #[test]
    fn an_unsubscribe_frame_names_the_same_channel() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "bts:unsubscribe");
        assert_eq!(frame["data"]["channel"], "live_trades_btcusd");
    }

    /// Byte-for-byte the first trade from this module's live capture.
    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"data":{"id":628186942,"timestamp":"1788335012","amount":0.00006514,"amount_str":"0.00006514","price":77558.07,"price_str":"77558.07","type":1,"microtimestamp":"1788335012854000","buy_order_id":2045952085757952,"sell_order_id":2045952156454912},"channel":"live_trades_btcusd","event":"trade"}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_755_807);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(6_514));
        assert_eq!(update.qty_scale, 8);
    }

    /// `timestamp` is whole seconds and `microtimestamp` is microseconds;
    /// the capture holds two trades sharing one second, which only the
    /// latter tells apart.
    #[test]
    fn the_microsecond_timestamp_is_the_one_read() {
        let frame = r#"{"data":{"id":628186945,"timestamp":"1788335015","amount":0.0000079,"amount_str":"0.00000790","price":77558.08,"price_str":"77558.08","type":0,"microtimestamp":"1788335015407000","buy_order_id":1,"sell_order_id":2},"channel":"live_trades_btcusd","event":"trade"}"#;
        let updates = protocol().parse_message(frame);
        let (_, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(update.ts.as_micros(), 1_788_335_015_407_000);
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame =
            r#"{"event":"bts:subscription_succeeded","channel":"live_trades_btcusd","data":{}}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn a_frame_from_another_channel_yields_nothing() {
        let frame = r#"{"data":{"price_str":"1","amount_str":"1","microtimestamp":"1788335015407000"},"channel":"order_book_btcusd","event":"data"}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
