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
/// `wss://openapi-ws-v2.bitmart.com/api?protocol=1.1` — the contract
/// market's own socket, confirmed live 2026-09-02.
pub(crate) const BITMART_FUTURES_WS_URL: &str = "wss://openapi-ws-v2.bitmart.com/api?protocol=1.1";

/// The contract market's trade channel.
const FUTURES_TRADE_TABLE: &str = "futures/trade";

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
    /// The socket and channel this market uses; the contract market
    /// shares neither with spot.
    market: Market,
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BitmartTradesProtocol {
    #[cfg(test)]
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self::for_market(source_id, Market::Spot, symbols)
    }

    /// A protocol for `market`.
    pub(crate) fn for_market(
        source_id: impl Into<Box<str>>,
        market: Market,
        symbols: Arc<dyn SymbolMap>,
    ) -> Self {
        Self {
            market,
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no BitMart native symbol known for {instrument}"))
        })
    }

    fn frame(&self, op: &str, symbol: &str) -> String {
        // Spot names the verb `op`; the contract market names it
        // `action`. Confirmed live 2026-09-02 — the wrong one is ignored
        // rather than refused, which is the worst way to be wrong.
        let (verb, table) = match self.market {
            Market::Spot => ("op", TRADE_TABLE),
            Market::Futures => ("action", FUTURES_TRADE_TABLE),
        };
        format!(r#"{{"{verb}":"{op}","args":["{table}:{symbol}"]}}"#)
    }
}

impl BitmartTradesProtocol {
    /// Decodes a contract-market frame — see [`FuturesFrame`] for why it
    /// cannot share spot's decoder.
    fn parse_futures(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<FuturesFrame>(text) else {
            return Vec::new();
        };
        if !frame.group.starts_with(FUTURES_TRADE_TABLE) {
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
                let ms = senken_venue::iso8601_ms(&entry.created_at)?;
                let ts = senken_core::UnixNanos::from_millis(ms)?;
                let (price, price_scale) = senken_plugin::live::scaled(&entry.deal_price)?;
                let _ = &entry.deal_vol;
                Some((
                    instrument,
                    LiveUpdate::Price(senken_subscription::PriceUpdate {
                        ts,
                        price,
                        price_scale,
                        // `deal_vol` counts contracts.
                        qty: senken_series::Volume::Absent,
                        qty_scale: 0,
                    }),
                ))
            })
            .collect()
    }
}

impl VenueProtocol for BitmartTradesProtocol {
    fn url(&self) -> &str {
        match self.market {
            Market::Spot => BITMART_WS_URL,
            Market::Futures => BITMART_FUTURES_WS_URL,
        }
    }

    fn venue(&self) -> &'static str {
        "bitmart"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(self.frame("subscribe", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(self.frame("unsubscribe", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        if self.market == Market::Futures {
            return self.parse_futures(text);
        }
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

/// Which BitMart market a protocol serves. The two share no host, no
/// subscribe verb, no channel name and no field names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Market {
    Spot,
    Futures,
}

/// One inbound contract frame.
///
/// Captured live 2026-09-02:
///
/// ```json
/// {"data":[{"trade_id":3000001725957437,"symbol":"BTCUSDT","deal_price":"76551.2","deal_vol":"3","way":6,"m":true,"created_at":"2026-09-02T11:18:07.530779933Z"}],"group":"futures/trade:BTCUSDT"}
/// ```
///
/// Nothing here matches spot's names: the channel is under `group` rather
/// than `table`, the price is `deal_price` rather than `price`, and the
/// instant is an RFC 3339 string rather than epoch milliseconds.
#[derive(Debug, Deserialize)]
struct FuturesFrame {
    #[serde(default)]
    group: String,
    #[serde(default)]
    data: Vec<FuturesTrade>,
}

#[derive(Debug, Deserialize)]
struct FuturesTrade {
    symbol: String,
    deal_price: String,
    /// A contract count, not a base amount — see this plugin's
    /// `futures` module for why no size is published.
    #[serde(default)]
    deal_vol: String,
    created_at: String,
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

/// `BitMart`'s live-feed registration. Its futures stream is a
/// different host that no capture in this project has reached.
pub(crate) struct BitmartFeedSource {
    source_ids: Vec<String>,
    market: Market,
}

impl BitmartFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
            market: Market::Spot,
        }
    }

    /// The contract market's stream: a different host, a different
    /// subscribe verb (`action` rather than `op`) and a different channel
    /// name. Confirmed live 2026-09-02.
    pub(crate) fn futures() -> Self {
        Self {
            source_ids: vec![crate::FUTURES_ID.to_owned()],
            market: Market::Futures,
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
        Arc::new(BitmartTradesProtocol::for_market(
            self.source_ids[0].as_str(),
            self.market,
            symbols,
        ))
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

    /// The contract market shares nothing with spot but the venue's name:
    /// a different host, `action` rather than `op`, `futures/trade`
    /// rather than `spot/trade`.
    #[test]
    fn the_futures_subscribe_uses_its_own_verb_and_channel() {
        let protocol = super::BitmartTradesProtocol::for_market(
            crate::FUTURES_ID,
            super::Market::Futures,
            Arc::new(UnderscoreMap),
        );
        assert_eq!(protocol.url(), super::BITMART_FUTURES_WS_URL);

        let text = protocol
            .subscribe_frame(&InstrumentId::new(crate::FUTURES_ID, "BTCUSDT").unwrap())
            .unwrap();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame["action"], "subscribe");
        assert!(frame.get("op").is_none(), "spot's verb is not this one");
        assert_eq!(frame["args"][0], "futures/trade:BTC_USDT");
    }

    /// Byte-for-byte a contract frame from the live capture. None of its
    /// field names match spot's, so spot's decoder reads nothing from it.
    #[test]
    fn the_captured_futures_frame_decodes_to_a_price() {
        let protocol = super::BitmartTradesProtocol::for_market(
            crate::FUTURES_ID,
            super::Market::Futures,
            Arc::new(UnderscoreMap),
        );
        let frame = r#"{"data":[{"trade_id":3000001725957437,"symbol":"BTCUSDT","deal_price":"76551.2","deal_vol":"3","way":6,"m":true,"created_at":"2026-09-02T11:18:07.530779933Z"}],"group":"futures/trade:BTCUSDT"}"#;

        let updates = protocol.parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a futures/trade frame must decode to a price update");
        };
        assert_eq!(
            id,
            &InstrumentId::new(crate::FUTURES_ID, "BTCUSDT").unwrap()
        );
        assert_eq!(update.price, 765_512);
        assert_eq!(update.price_scale, 1);
        assert_eq!(
            update.qty,
            senken_series::Volume::Absent,
            "`deal_vol` counts contracts, not base asset"
        );
        assert_eq!(update.ts.as_millis(), 1_788_347_887_530);
    }

    /// A spot frame must not be read by the contract decoder, or a market
    /// would be published under the wrong source.
    #[test]
    fn a_spot_frame_is_not_read_by_the_futures_decoder() {
        let protocol = super::BitmartTradesProtocol::for_market(
            crate::FUTURES_ID,
            super::Market::Futures,
            Arc::new(UnderscoreMap),
        );
        let spot = r#"{"data":[{"ms_t":1788335076327,"price":"77604.55","s_t":1788335076,"side":"sell","size":"0.01307","symbol":"BTC_USDT"}],"table":"spot/trade"}"#;
        assert!(protocol.parse_message(spot).is_empty());
    }
}
