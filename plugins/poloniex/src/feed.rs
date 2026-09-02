//! Poloniex's public `trades` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws.poloniex.com/ws/public`, sent
//! `{"event":"subscribe","channel":["trades"],"symbols":["BTC_USDT"]}`,
//! received `{"event":"subscribe","channel":"trades","symbols":["BTC_USDT"]}`
//! and then:
//!
//! ```json
//! {"channel":"trades","data":[{"symbol":"BTC_USDT","amount":"1419.8458164","quantity":"0.018308","takerSide":"sell","createTime":1788335123813,"price":"77553.3","id":"240066176","ts":1788335123827}]}
//! ```
//!
//! In the same session `{"event":"ping"}` was answered with
//! `{"event":"pong"}`, and
//! `{"event":"unsubscribe","channel":["trades"],"symbols":["BTC_USDT"]}`
//! was acknowledged.
//!
//! Read from that capture:
//! - **`quantity` is the base-asset size; `amount` is the quote-asset
//!   notional.** `0.018308` BTC at `77553.3` is `1419.85` USDT, and the
//!   frame carries both. A bar's volume is the base quantity, so reading
//!   `amount` would inflate every volume by roughly the price.
//! - **`createTime` is when the trade happened; `ts` is when Poloniex sent
//!   the frame.** They differ by 14ms here.
//! - `price` and `quantity` are strings; both times are bare epoch
//!   milliseconds.
//! - The venue symbol is `BTC_USDT`, exactly the catalog's `source_symbol`.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://ws.poloniex.com/ws/public` — confirmed live 2026-09-02.
pub(crate) const POLONIEX_PUBLIC_WS_URL: &str = "wss://ws.poloniex.com/ws/public";

/// Poloniex joins base and quote with `_`.
const SEPARATOR: char = '_';

/// How often to send the confirmed `{"event":"ping"}`. Our own conservative
/// choice, not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(20);

/// Poloniex's public `trades` channel.
pub(crate) struct PoloniexTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl PoloniexTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Poloniex native symbol known for {instrument}"))
        })
    }

    fn frame(event: &str, symbol: &str) -> String {
        format!(r#"{{"event":"{event}","channel":["trades"],"symbols":["{symbol}"]}}"#)
    }
}

impl VenueProtocol for PoloniexTradesProtocol {
    fn url(&self) -> &str {
        POLONIEX_PUBLIC_WS_URL
    }

    fn venue(&self) -> &'static str {
        "poloniex"
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
        if frame.channel != "trades" {
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
                let ts = senken_core::UnixNanos::from_millis(entry.create_time)?;
                Some((
                    instrument,
                    LiveUpdate::Price(trade(ts, &entry.price, &entry.quantity)?),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, r#"{"event":"ping"}"#.to_owned()))
    }
}

/// One inbound frame. The subscribe acknowledgement has no `data`, so
/// `#[serde(default)]` decodes it to an empty one.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    data: Vec<Trade>,
}

#[derive(Debug, Deserialize)]
struct Trade {
    symbol: String,
    price: String,
    /// The **base**-asset size. `amount` beside it is the quote-asset
    /// notional and is deliberately not read.
    quantity: String,
    /// When the trade happened, in epoch milliseconds — not the `ts`
    /// beside it, which is when Poloniex sent the frame.
    #[serde(rename = "createTime")]
    create_time: i64,
}

/// Poloniex's live-feed registration — spot only. The perpetual market is
/// a separate socket that no capture in this project has reached.
pub(crate) struct PoloniexFeedSource {
    source_ids: Vec<String>,
}

impl PoloniexFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for PoloniexFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(PoloniexTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{POLONIEX_PUBLIC_WS_URL, PoloniexTradesProtocol};
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

    fn protocol() -> PoloniexTradesProtocol {
        PoloniexTradesProtocol::new(crate::SPOT_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), POLONIEX_PUBLIC_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "subscribe");
        assert_eq!(frame["channel"][0], "trades");
        assert_eq!(frame["symbols"][0], "BTC_USDT");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_event() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "unsubscribe");
    }

    /// Byte-for-byte a frame from this module's live capture. The volume
    /// asserted here is `quantity` (0.018308 BTC), not `amount`
    /// (1419.8458164 USDT) — reading the wrong one inflates every bar's
    /// volume by roughly the price and still looks plausible on a chart.
    #[test]
    fn the_captured_trade_frame_uses_the_base_quantity_not_the_quote_amount() {
        let frame = r#"{"channel":"trades","data":[{"symbol":"BTC_USDT","amount":"1419.8458164","quantity":"0.018308","takerSide":"sell","createTime":1788335123813,"price":"77553.3","id":"240066176","ts":1788335123827}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 775_533);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.qty, senken_series::Volume::Real(18_308));
        assert_eq!(update.qty_scale, 6);
        assert_eq!(
            update.ts.as_millis(),
            1_788_335_123_813,
            "`createTime`, not the frame's own `ts` (…827)"
        );
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"event":"subscribe","channel":"trades","symbols":["BTC_USDT"]}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn the_confirmed_pong_yields_nothing() {
        assert!(protocol().parse_message(r#"{"event":"pong"}"#).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn the_keepalive_is_the_confirmed_ping_frame() {
        let (_, frame) = protocol().keepalive().unwrap();
        assert_eq!(frame, r#"{"event":"ping"}"#);
    }
}
