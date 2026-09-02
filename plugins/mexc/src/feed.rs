//! MEXC's futures `push.deal` channel.
//!
//! # Why futures and not spot
//!
//! MEXC's spot stream refuses the public trades channel outright. Sent to
//! `wss://wbs.mexc.com/ws` on 2026-09-02:
//!
//! ```json
//! {"method":"SUBSCRIPTION","params":["spot@public.deals.v3.api@BTCUSDT"]}
//! {"id":0,"code":0,"msg":"Not Subscribed successfully! [spot@public.deals.v3.api@BTCUSDT].  Reason： Blocked! "}
//! ```
//!
//! That is the venue declining, not a transport fault, so this plugin
//! registers a live feed for its **futures** source only. `mexc-spot`
//! keeps its instruments, bars and depth; it simply has no stream.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://contract.mexc.com/edge`, sent
//! `{"method":"sub.deal","param":{"symbol":"BTC_USDT"}}`, received
//! `{"channel":"rs.sub.deal","data":"success","ts":1788335242021}` and then:
//!
//! ```json
//! {"symbol":"BTC_USDT","data":[{"p":77525.9,"v":3,"T":2,"O":3,"M":1,"t":1788335242156,"i":"16005949540","cts":"1788335242156"}],"channel":"push.deal","ts":1788335242156}
//! ```
//!
//! In the same session `{"method":"ping"}` was answered with
//! `{"channel":"pong","data":1788336475469,"ts":1788336475469}`.
//!
//! Read from that capture:
//! - **`p` and `v` are bare JSON numbers**, read through
//!   [`RawValue`](serde_json::value::RawValue) so the venue's digits reach
//!   the scaled-integer parser rather than an `f64`.
//! - **`v` is a contract count, not a base-asset quantity.** `v":3` at
//!   $77,525 is not 3 BTC. MEXC's contract multiplier for `BTC_USDT` is
//!   0.0001 BTC, and this project's MEXC futures *bars* already report
//!   [`Volume::Absent`](senken_series::Volume::Absent) for exactly this
//!   reason rather than publish a figure that is wrong by that factor. A
//!   live tick does the same, so the two agree.
//! - **The symbol is on the envelope, not on an entry.**
//! - `t` is the trade's epoch milliseconds.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade_without_size;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://contract.mexc.com/edge` — confirmed live 2026-09-02.
pub(crate) const MEXC_FUTURES_WS_URL: &str = "wss://contract.mexc.com/edge";

/// MEXC joins base and quote with `_`.
const SEPARATOR: char = '_';

/// How often to send the confirmed `{"method":"ping"}`. Our own
/// conservative choice, not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// MEXC's futures trade channel.
pub(crate) struct MexcTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl MexcTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no MEXC native symbol known for {instrument}"))
        })
    }

    fn frame(method: &str, symbol: &str) -> String {
        format!(r#"{{"method":"{method}","param":{{"symbol":"{symbol}"}}}}"#)
    }
}

impl VenueProtocol for MexcTradesProtocol {
    fn url(&self) -> &str {
        MEXC_FUTURES_WS_URL
    }

    fn venue(&self) -> &'static str {
        "mexc"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("sub.deal", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("unsub.deal", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        if frame.channel != "push.deal" {
            return Vec::new();
        }
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(frame.symbol, &[SEPARATOR]),
        ) else {
            return Vec::new();
        };
        frame
            .data
            .iter()
            .filter_map(|entry| {
                let ts = senken_core::UnixNanos::from_millis(entry.time)?;
                Some((
                    instrument.clone(),
                    LiveUpdate::Price(trade_without_size(ts, entry.price.get())?),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, r#"{"method":"ping"}"#.to_owned()))
    }
}

/// One inbound frame. The acknowledgement's `data` is the string
/// `"success"` rather than an array, so it is only read once `channel`
/// says this is a push.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    channel: &'a str,
    #[serde(borrow, default)]
    symbol: &'a str,
    #[serde(borrow, default)]
    data: Vec<Deal<'a>>,
}

#[derive(Debug, Deserialize)]
struct Deal<'a> {
    #[serde(borrow, rename = "p")]
    price: &'a RawValue,
    /// Epoch milliseconds.
    #[serde(rename = "t")]
    time: i64,
}

/// MEXC's live-feed registration — futures only, because spot's trades
/// channel answers "Blocked!" (see this module's docs).
pub(crate) struct MexcFeedSource {
    source_ids: Vec<String>,
}

impl MexcFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::FUTURES_ID.to_owned()],
        }
    }
}

impl FeedSource for MexcFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(MexcTradesProtocol::new(crate::FUTURES_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{MEXC_FUTURES_WS_URL, MexcTradesProtocol};
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

    fn protocol() -> MexcTradesProtocol {
        MexcTradesProtocol::new(crate::FUTURES_ID, Arc::new(UnderscoreMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::FUTURES_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), MEXC_FUTURES_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "sub.deal");
        assert_eq!(frame["param"]["symbol"], "BTC_USDT");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_method() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "unsub.deal");
        assert_eq!(frame["param"]["symbol"], "BTC_USDT");
    }

    /// Byte-for-byte a frame from this module's live capture. `v":3` is a
    /// contract count, and publishing it as a base-asset volume would be
    /// wrong by MEXC's contract multiplier — the same reason this
    /// plugin's futures bars report no base volume either.
    #[test]
    fn the_captured_deal_frame_carries_a_price_and_no_contract_count_as_volume() {
        let frame = r#"{"symbol":"BTC_USDT","data":[{"p":77525.9,"v":3,"T":2,"O":3,"M":1,"t":1788335242156,"i":"16005949540","cts":"1788335242156"}],"channel":"push.deal","ts":1788335242156}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a push.deal frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 775_259);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.qty, senken_series::Volume::Absent);
        assert_eq!(update.ts.as_millis(), 1_788_335_242_156);
    }

    #[test]
    fn a_frame_with_two_deals_decodes_to_two_updates() {
        let frame = r#"{"symbol":"BTC_USDT","data":[{"p":77526,"v":208,"T":1,"O":3,"M":2,"t":1788335243261,"i":"16005949690","cts":"1788335243261"},{"p":77525.9,"v":5,"T":2,"O":3,"M":1,"t":1788335243151,"i":"16005949659","cts":"1788335243151"}],"channel":"push.deal","ts":1788335243261}"#;
        assert_eq!(protocol().parse_message(frame).len(), 2);
    }

    #[test]
    fn the_captured_acknowledgement_and_pong_yield_nothing() {
        assert!(
            protocol()
                .parse_message(r#"{"channel":"rs.sub.deal","data":"success","ts":1788335242021}"#)
                .is_empty()
        );
        assert!(
            protocol()
                .parse_message(r#"{"channel":"pong","data":1788336475469,"ts":1788336475469}"#)
                .is_empty()
        );
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn the_keepalive_is_the_confirmed_ping_frame() {
        let (_, frame) = protocol().keepalive().unwrap();
        assert_eq!(frame, r#"{"method":"ping"}"#);
    }
}
