//! Bitget's public spot WebSocket — the `trade` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws.bitget.com/v2/ws/public`, sent
//! `{"op":"subscribe","args":[{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"}]}`
//! and received the acknowledgement
//! `{"event":"subscribe","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"}}`,
//! then a `"snapshot"` backfill and a stream of `"update"` frames:
//!
//! ```json
//! {"action":"update","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"},"data":[{"ts":"1788335044055","price":"77600.75","size":"0.001352","side":"buy","tradeId":"1478949856623706112"}],"ts":1788335044056}
//! ```
//!
//! In the same session the bare text frame `ping` was answered with the
//! bare text `pong`, and
//! `{"op":"unsubscribe","args":[{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"}]}`
//! was acknowledged with `{"event":"unsubscribe","arg":{…}}`.
//!
//! Read from that capture:
//! - The instrument appears **only on the envelope's `arg.instId`** — a
//!   `data` entry carries no symbol at all. A decoder that looked for one
//!   per entry would publish nothing.
//! - `ts`, `price` and `size` are all strings; `ts` is epoch milliseconds.
//! - The venue symbol is `BTCUSDT`, exactly the spot catalog's own
//!   `source_symbol`.
//! - The first frame after a subscribe has `action":"snapshot"` and
//!   replays recent history — up to 30 trades in this capture. It is
//!   decoded like any other: a leaseholder joining mid-stream gets the
//!   most recent trade last, which is the price it should show.
//!
//! # Assumptions, flagged as such
//!
//! Bitget's idle timeout was not measured. [`KEEPALIVE`] is a conservative
//! interval carrying the `ping` frame that *was* confirmed above.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://ws.bitget.com/v2/ws/public` — confirmed live 2026-09-02.
pub(crate) const BITGET_PUBLIC_WS_URL: &str = "wss://ws.bitget.com/v2/ws/public";

/// How often to send the confirmed bare-text `ping`. Our own conservative
/// choice, not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(20);

/// Bitget's public spot `trade` channel.
pub(crate) struct BitgetTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BitgetTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Bitget native symbol known for {instrument}"))
        })
    }

    fn frame(op: &str, symbol: &str) -> String {
        format!(
            r#"{{"op":"{op}","args":[{{"instType":"SPOT","channel":"trade","instId":"{symbol}"}}]}}"#
        )
    }
}

impl VenueProtocol for BitgetTradesProtocol {
    fn url(&self) -> &str {
        BITGET_PUBLIC_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bitget"
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
        let Some(arg) = frame.arg else {
            return Vec::new();
        };
        if arg.channel != "trade" {
            return Vec::new();
        }
        let Ok(instrument) =
            InstrumentId::new(&self.source_id, &normalise_symbol(&arg.inst_id, &[]))
        else {
            return Vec::new();
        };
        frame
            .data
            .iter()
            .filter_map(|entry| {
                let ms: i64 = entry.ts.trim().parse().ok()?;
                let ts = senken_core::UnixNanos::from_millis(ms)?;
                Some((
                    instrument.clone(),
                    LiveUpdate::Price(trade(ts, &entry.price, &entry.size)?),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, "ping".to_owned()))
    }
}

/// One inbound frame. The subscribe acknowledgement carries an `arg` but no
/// `data`; a data frame carries both.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    arg: Option<Arg>,
    #[serde(default)]
    data: Vec<Trade>,
}

#[derive(Debug, Deserialize)]
struct Arg {
    channel: String,
    #[serde(rename = "instId")]
    inst_id: String,
}

#[derive(Debug, Deserialize)]
struct Trade {
    /// Epoch milliseconds, as a string.
    ts: String,
    price: String,
    size: String,
}

/// Bitget's live-feed registration — spot only, the market the capture in
/// this module's docs came from. Bitget's three futures markets use the
/// same socket with a different `instType`, but no frame from any of them
/// has been seen in this project.
pub(crate) struct BitgetFeedSource {
    source_ids: Vec<String>,
}

impl BitgetFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for BitgetFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Only the `trade` channel is subscribed; Bitget's `ticker`
        // channel was never captured, so no bid/ask is claimed.
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BitgetTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BITGET_PUBLIC_WS_URL, BitgetTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    fn protocol() -> BitgetTradesProtocol {
        BitgetTradesProtocol::new(crate::SPOT_ID, Arc::new(IdentitySymbolMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BITGET_PUBLIC_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "subscribe");
        assert_eq!(frame["args"][0]["instType"], "SPOT");
        assert_eq!(frame["args"][0]["channel"], "trade");
        assert_eq!(frame["args"][0]["instId"], "BTCUSDT");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_op() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "unsubscribe");
        assert_eq!(frame["args"][0]["instId"], "BTCUSDT");
    }

    /// Byte-for-byte an `update` frame from this module's live capture.
    /// The instrument comes from the envelope's `arg.instId`: the entry
    /// itself has no symbol, so a decoder reading one per entry finds
    /// nothing and publishes nothing.
    #[test]
    fn the_captured_update_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"action":"update","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"},"data":[{"ts":"1788335044055","price":"77600.75","size":"0.001352","side":"buy","tradeId":"1478949856623706112"}],"ts":1788335044056}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_760_075);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty, senken_series::Volume::Real(1_352));
        assert_eq!(update.qty_scale, 6);
        assert_eq!(update.ts.as_millis(), 1_788_335_044_055);
    }

    #[test]
    fn a_frame_with_several_trades_decodes_to_one_update_each() {
        let frame = r#"{"action":"update","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"},"data":[{"ts":"1788335044368","price":"77600.75","size":"0.010592","side":"buy","tradeId":"1478949857936523268"},{"ts":"1788335044368","price":"77600.75","size":"0.055497","side":"buy","tradeId":"1478949857936523266"},{"ts":"1788335044368","price":"77600.75","size":"0.002863","side":"buy","tradeId":"1478949857936523264"}],"ts":1788335044369}"#;
        assert_eq!(protocol().parse_message(frame).len(), 3);
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"event":"subscribe","arg":{"instType":"SPOT","channel":"trade","instId":"BTCUSDT"}}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn a_channel_this_protocol_did_not_subscribe_is_ignored() {
        let frame = r#"{"action":"snapshot","arg":{"instType":"SPOT","channel":"books5","instId":"BTCUSDT"},"data":[{"ts":"1788335044055","price":"77600.75","size":"0.001352"}]}"#;
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
        assert_eq!(
            frame, "ping",
            "Bitget answers the bare text `ping` with `pong`"
        );
    }
}
