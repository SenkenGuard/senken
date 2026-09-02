//! Gate's public `spot.trades` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://api.gateio.ws/ws/v4/`, sent
//! `{"channel":"spot.trades","event":"subscribe","payload":["BTC_USDT"]}`
//! — deliberately **without** the `time` field Gate's examples show, and
//! it was accepted:
//!
//! ```json
//! {"time":1788336080,"time_ms":1788336080750,"conn_id":"b2c2b0629e0c34a3","channel":"spot.trades","event":"subscribe","payload":["BTC_USDT"],"result":{"status":"success"}}
//! {"time":1788336082,"time_ms":1788336082402,"channel":"spot.trades","event":"update","result":{"id":217831625,"id_market":217831625,"create_time":1788336082,"create_time_ms":"1788336082402.208000","side":"sell","currency_pair":"BTC_USDT","amount":"0.000021","price":"77474","range":"217831625-217831625","stock":"BTC","money":"USDT","trade_mode":0}}
//! ```
//!
//! That the subscribe works without `time` matters: a protocol that had to
//! stamp its own frames would need a clock, and a frame built from the
//! wrong one is rejected in a way that looks like a network fault.
//!
//! Read from that capture:
//! - **`create_time_ms` is a string of milliseconds with six fractional
//!   digits** — `"1788336082402.208000"`. Six fractional digits of a
//!   millisecond *is* a nanosecond count, which is exactly what
//!   [`senken_core::UnixNanos`] holds, so no precision is thrown away.
//!   Truncating to the whole millisecond would collapse three consecutive
//!   trades in this very capture (`…402.208`, `…402.233`, `…402.254`) onto
//!   one instant.
//! - `amount` is the **base**-asset size (`stock":"BTC"` names it) and
//!   `price` is quoted in `money":"USDT"`. Both are strings.
//! - The venue symbol is `BTC_USDT`, exactly the catalog's `source_symbol`.
//! - An `update` frame carries exactly one trade in `result`, not an array.
//!
//! # Not verified
//!
//! The unsubscribe (`event":"unsubscribe"`, otherwise identical) was sent
//! but the connection was closed before its acknowledgement was read.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;

/// `wss://api.gateio.ws/ws/v4/` — confirmed live 2026-09-02.
pub(crate) const GATE_WS_URL: &str = "wss://api.gateio.ws/ws/v4/";

/// Gate joins base and quote with `_`.
const SEPARATOR: char = '_';

/// Fractional digits Gate writes after the millisecond in
/// `create_time_ms`. Six of them make the whole string a nanosecond count.
const CREATE_TIME_FRACTION: u8 = 6;

/// Gate's public spot `trades` channel.
pub(crate) struct GateTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl GateTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Gate native symbol known for {instrument}"))
        })
    }

    fn frame(event: &str, symbol: &str) -> String {
        format!(r#"{{"channel":"spot.trades","event":"{event}","payload":["{symbol}"]}}"#)
    }
}

impl VenueProtocol for GateTradesProtocol {
    fn url(&self) -> &str {
        GATE_WS_URL
    }

    fn venue(&self) -> &'static str {
        "gate"
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
        if frame.channel != "spot.trades" || frame.event != "update" {
            return Vec::new();
        }
        let Some(result) = frame.result else {
            return Vec::new();
        };
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(&result.currency_pair, &[SEPARATOR]),
        ) else {
            return Vec::new();
        };
        // Milliseconds with six fractional digits is a nanosecond count.
        let Some(nanos) = senken_core::parse_scaled(&result.create_time_ms, CREATE_TIME_FRACTION)
        else {
            return Vec::new();
        };
        trade(UnixNanos::from_nanos(nanos), &result.price, &result.amount)
            .map(|update| vec![(instrument, LiveUpdate::Price(update))])
            .unwrap_or_default()
    }
}

/// One inbound frame. The acknowledgement's `result` is a status object
/// rather than a trade, so it is read as `Option` and decodes to `None`.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    result: Option<Trade>,
}

#[derive(Debug, Deserialize)]
struct Trade {
    currency_pair: String,
    price: String,
    /// The **base**-asset size — Gate names the asset in `stock`.
    amount: String,
    /// Milliseconds with six fractional digits, as a string.
    create_time_ms: String,
}

/// Gate's live-feed registration — spot only. Gate's futures streams live
/// on different paths split by settlement currency
/// (`/v4/ws/usdt`, `/v4/ws/btc`, `/v4/ws/delivery/usdt`), none of which
/// this project has captured a frame from.
pub(crate) struct GateFeedSource {
    source_ids: Vec<String>,
}

impl GateFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for GateFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(GateTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{GATE_WS_URL, GateTradesProtocol};
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

    fn protocol() -> GateTradesProtocol {
        GateTradesProtocol::new(crate::SPOT_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), GATE_WS_URL);
    }

    /// The confirmed subscribe carries no `time`, and adding one would
    /// force this protocol to read a clock it has no business reading.
    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape_and_stamps_no_time() {
        let text = protocol().subscribe_frame(&instrument()).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame["channel"], "spot.trades");
        assert_eq!(frame["event"], "subscribe");
        assert_eq!(frame["payload"][0], "BTC_USDT");
        assert!(frame.get("time").is_none());
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_event() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "unsubscribe");
    }

    /// Byte-for-byte an `update` frame from this module's live capture.
    #[test]
    fn the_captured_update_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"time":1788336082,"time_ms":1788336082402,"channel":"spot.trades","event":"update","result":{"id":217831625,"id_market":217831625,"create_time":1788336082,"create_time_ms":"1788336082402.208000","side":"sell","currency_pair":"BTC_USDT","amount":"0.000021","price":"77474","range":"217831625-217831625","stock":"BTC","money":"USDT","trade_mode":0}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a spot.trades update must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 77_474);
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.qty, senken_series::Volume::Real(21));
        assert_eq!(update.qty_scale, 6);
        assert_eq!(update.ts.as_nanos(), 1_788_336_082_402_208_000);
    }

    /// Three trades in the capture share one millisecond and differ only
    /// in `create_time_ms`'s fractional part. Truncating to milliseconds
    /// puts all three on one instant, which is how a tick stream loses its
    /// ordering.
    #[test]
    fn two_trades_in_the_same_millisecond_keep_distinct_timestamps() {
        let first = r#"{"channel":"spot.trades","event":"update","result":{"create_time_ms":"1788336082402.208000","currency_pair":"BTC_USDT","amount":"0.000021","price":"77474"}}"#;
        let second = r#"{"channel":"spot.trades","event":"update","result":{"create_time_ms":"1788336082402.233000","currency_pair":"BTC_USDT","amount":"0.000021","price":"77474"}}"#;
        let (_, LiveUpdate::Price(a)) = protocol().parse_message(first)[0] else {
            panic!("expected a price update");
        };
        let (_, LiveUpdate::Price(b)) = protocol().parse_message(second)[0] else {
            panic!("expected a price update");
        };
        assert_eq!(a.ts.as_millis(), b.ts.as_millis());
        assert!(a.ts < b.ts, "the sub-millisecond digits must still order");
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"time":1788336080,"time_ms":1788336080750,"conn_id":"b2c2b0629e0c34a3","trace_id":"e3b8","channel":"spot.trades","event":"subscribe","payload":["BTC_USDT"],"result":{"status":"success"},"requestId":"e3b8"}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
