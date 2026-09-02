//! Coinbase Exchange's public `matches` and `ticker` channels.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws-feed.exchange.coinbase.com`, sent
//! `{"type":"subscribe","product_ids":["BTC-USD"],"channels":["matches","ticker"]}`,
//! received the `subscriptions` acknowledgement, and then:
//!
//! ```json
//! {"type":"match","trade_id":1087540758,"maker_order_id":"5b268d37-…","taker_order_id":"b281704b-…","side":"buy","size":"0.00000009","price":"77572.87","product_id":"BTC-USD","sequence":135437581854,"time":"2026-09-02T07:44:10.104422Z"}
//! {"type":"ticker","sequence":135437581854,"product_id":"BTC-USD","price":"77572.87","open_24h":"78609.92","volume_24h":"7336.73471395","low_24h":"76366.12","high_24h":"78635.01","volume_30d":"209162.21626191","best_bid":"77572.87","best_bid_size":"0.00009686","best_ask":"77575.85","best_ask_size":"0.04350000","side":"sell","time":"2026-09-02T07:44:10.104422Z","trade_id":1087540758,"last_size":"0.00000009"}
//! ```
//!
//! Read from that capture:
//! - **The `ticker` channel really does carry a best bid and offer** —
//!   `best_bid`, `best_ask` and both sizes — which is why this is one of
//!   the few feeds that answers `true` to
//!   [`serves_quotes`](FeedSource::serves_quotes).
//! - `price` and `size` are strings; `time` is RFC 3339 with microsecond
//!   precision, not an epoch integer.
//! - The first `matches` frame after subscribing has `type":"last_match"`
//!   rather than `"match"` — same shape, the most recent trade before the
//!   subscription. Both are decoded, so a chart shows a price immediately
//!   instead of waiting for the next trade.
//! - The venue symbol is `BTC-USD`, exactly the catalog's `source_symbol`.
//!
//! # Not verified
//!
//! No unsubscribe was sent. `{"type":"unsubscribe",…}` is the mirror of the
//! confirmed subscribe and is what this protocol emits. No application-level
//! keep-alive was seen; the socket answered WebSocket control pings, which
//! the transport handles.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::{quote, trade};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://ws-feed.exchange.coinbase.com` — confirmed live 2026-09-02.
pub(crate) const COINBASE_WS_URL: &str = "wss://ws-feed.exchange.coinbase.com";

/// Coinbase joins base and quote with `-`.
const SEPARATOR: char = '-';

/// Coinbase Exchange's public trade and quote channels.
pub(crate) struct CoinbaseTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl CoinbaseTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Coinbase native symbol known for {instrument}"))
        })
    }

    fn frame(kind: &str, symbol: &str) -> String {
        format!(r#"{{"type":"{kind}","product_ids":["{symbol}"],"channels":["matches","ticker"]}}"#)
    }

    fn instrument(&self, product_id: &str) -> Option<InstrumentId> {
        InstrumentId::new(&self.source_id, &normalise_symbol(product_id, &[SEPARATOR])).ok()
    }
}

impl VenueProtocol for CoinbaseTradesProtocol {
    fn url(&self) -> &str {
        COINBASE_WS_URL
    }

    fn venue(&self) -> &'static str {
        "coinbase"
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
        let Some(ms) = senken_venue::iso8601_ms(&frame.time) else {
            return Vec::new();
        };
        let Some(ts) = senken_core::UnixNanos::from_millis(ms) else {
            return Vec::new();
        };
        let Some(instrument) = self.instrument(&frame.product_id) else {
            return Vec::new();
        };
        match frame.kind.as_str() {
            // `last_match` is the backfill the venue sends on subscribing:
            // the same shape, and the price a chart should show at once
            // rather than after the next trade.
            "match" | "last_match" => trade(ts, &frame.price, &frame.size)
                .map(|update| vec![(instrument, LiveUpdate::Price(update))])
                .unwrap_or_default(),
            "ticker" => {
                let (Some(bid), Some(ask), Some(bid_size), Some(ask_size)) = (
                    frame.best_bid.as_deref(),
                    frame.best_ask.as_deref(),
                    frame.best_bid_size.as_deref(),
                    frame.best_ask_size.as_deref(),
                ) else {
                    return Vec::new();
                };
                quote(ts, bid, ask, bid_size, ask_size)
                    .map(|update| vec![(instrument, LiveUpdate::Quote(update))])
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }
}

/// One inbound frame. The `subscriptions` acknowledgement shares none of
/// the fields below, so `#[serde(default)]` decodes it to empties rather
/// than an error.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    product_id: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    price: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    best_bid: Option<String>,
    #[serde(default)]
    best_ask: Option<String>,
    #[serde(default)]
    best_bid_size: Option<String>,
    #[serde(default)]
    best_ask_size: Option<String>,
}

/// Coinbase's live-feed registration — the spot exchange only. The
/// International (perpetual) venue is a different host that no capture in
/// this project has reached.
pub(crate) struct CoinbaseFeedSource {
    source_ids: Vec<String>,
}

impl CoinbaseFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for CoinbaseFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Confirmed live: the `ticker` channel carries `best_bid`,
        // `best_ask` and both sizes — see this module's docs.
        true
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(CoinbaseTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{COINBASE_WS_URL, CoinbaseTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;

    /// The catalog holds `BTC-USD` while the normalised id is `BTCUSD`.
    struct DashedMap;
    impl SymbolMap for DashedMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            let symbol = instrument.symbol();
            symbol.strip_suffix("USD").map(|base| format!("{base}-USD"))
        }
    }

    fn protocol() -> CoinbaseTradesProtocol {
        CoinbaseTradesProtocol::new(crate::SPOT_ID, Arc::new(DashedMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSD").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), COINBASE_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["type"], "subscribe");
        assert_eq!(frame["product_ids"][0], "BTC-USD");
        assert_eq!(frame["channels"][0], "matches");
        assert_eq!(frame["channels"][1], "ticker");
    }

    #[test]
    fn an_unsubscribe_frame_names_the_same_product() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["type"], "unsubscribe");
        assert_eq!(frame["product_ids"][0], "BTC-USD");
    }

    /// Byte-for-byte a `match` frame from this module's live capture.
    #[test]
    fn the_captured_match_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"type":"match","trade_id":1087540758,"maker_order_id":"5b268d37-59d7-4b16-935c-3854d4cf956d","taker_order_id":"b281704b-3d2e-4631-bc49-75ca291b445b","side":"buy","size":"0.00000009","price":"77572.87","product_id":"BTC-USD","sequence":135437581854,"time":"2026-09-02T07:44:10.104422Z"}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a match frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_757_287);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(9));
        assert_eq!(update.qty_scale, 8);
        assert_eq!(update.ts.as_millis(), 1_788_335_050_104);
    }

    /// The backfill frame the venue sends on subscribing is a trade too;
    /// discarding it leaves a chart with no price until the next one.
    #[test]
    fn the_captured_last_match_backfill_is_decoded_as_a_trade() {
        let frame = r#"{"type":"last_match","trade_id":1087540757,"maker_order_id":"a043a07c-eced-4623-8ac6-b2803f82ee63","taker_order_id":"883ef465-8d3c-4d7d-87a9-7d7e895874b2","side":"buy","size":"0.00000035","price":"77572.86","product_id":"BTC-USD","sequence":135437581270,"time":"2026-09-02T07:44:09.449472Z"}"#;
        let updates = protocol().parse_message(frame);
        assert_eq!(updates.len(), 1);
        assert!(matches!(updates[0].1, LiveUpdate::Price(_)));
    }

    /// Byte-for-byte a `ticker` frame from the same capture. Its bid and
    /// ask have the same number of digits here; the shared-scale rule that
    /// makes an uneven pair survive is proved in `senken_plugin::live`.
    #[test]
    fn the_captured_ticker_frame_decodes_to_a_quote() {
        let frame = r#"{"type":"ticker","sequence":135437581854,"product_id":"BTC-USD","price":"77572.87","open_24h":"78609.92","volume_24h":"7336.73471395","low_24h":"76366.12","high_24h":"78635.01","volume_30d":"209162.21626191","best_bid":"77572.87","best_bid_size":"0.00009686","best_ask":"77575.85","best_ask_size":"0.04350000","side":"sell","time":"2026-09-02T07:44:10.104422Z","trade_id":1087540758,"last_size":"0.00000009"}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (_, LiveUpdate::Quote(update)) = &updates[0] else {
            panic!("a ticker frame must decode to a quote update");
        };
        assert_eq!(update.bid, 7_757_287);
        assert_eq!(update.ask, 7_757_585);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.bid_size, 9_686);
        assert_eq!(update.ask_size, 4_350_000);
        assert_eq!(update.qty_scale, 8);
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"type":"subscriptions","channels":[{"name":"matches","product_ids":["BTC-USD"],"account_ids":null},{"name":"ticker","product_ids":["BTC-USD"],"account_ids":null}]}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
