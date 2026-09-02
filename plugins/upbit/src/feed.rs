//! Upbit's public `trade` stream.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://api.upbit.com/websocket/v1`, sent
//! `[{"ticket":"senken"},{"type":"trade","codes":["KRW-BTC"]},{"format":"DEFAULT"}]`
//! and received, **as a binary frame carrying plain UTF-8 JSON**:
//!
//! ```json
//! {"type":"trade","code":"KRW-BTC","timestamp":1788335180423,"trade_date":"2026-09-02","trade_time":"07:46:20","trade_timestamp":1788335180377,"trade_price":106691000.00000000,"trade_volume":0.00009372,"ask_bid":"ASK","prev_closing_price":107005000.00000000,"change":"FALL","change_price":314000.00000000,"sequential_id":17883351803770004,"best_ask_price":106705000,"best_ask_size":0.00906333,"best_bid_price":106691000,"best_bid_size":0.00702247,"stream_type":"SNAPSHOT"}
//! ```
//!
//! Three facts from that session, each of which a decoder gets wrong by
//! default:
//!
//! - **Nothing arrives as a text frame.** Upbit does not compress —
//!   the payload is ordinary UTF-8 JSON — it simply sends it with the
//!   binary opcode. A connection reading only text frames receives nothing
//!   at all, silently. [`VenueProtocol::decode_binary`] is where that is
//!   undone.
//! - **A subscribe *replaces* the connection's whole subscription list.**
//!   Confirmed by sending `codes":["KRW-BTC"]` and then
//!   `codes":["KRW-ETH"]`: BTC stopped arriving and ETH started. Upbit has
//!   no per-symbol unsubscribe at all. So this protocol keeps the set of
//!   codes it has been asked for and re-sends the whole set every time —
//!   see [`UpbitTradesProtocol::subscribe_frame`].
//! - **`PING` is answered** with `{"status":"UP"}`, itself a binary frame.
//!
//! Also read from the capture:
//! - **`trade_timestamp` is the trade's time; `timestamp` is the frame's.**
//!   They differ by 46ms here.
//! - `trade_price` and `trade_volume` are bare JSON numbers, read through
//!   [`RawValue`](serde_json::value::RawValue) so the venue's digits reach
//!   the scaled-integer parser rather than an `f64`.
//! - **The frame carries a genuine best bid and offer** — `best_bid_price`,
//!   `best_ask_price` and both sizes — on the trade stream itself, so this
//!   feed serves quotes without a second subscription.
//! - The venue symbol is `KRW-BTC`, exactly the catalog's `source_symbol`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use senken_marketdata::InstrumentId;
use senken_plugin::live::{quote, trade};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://api.upbit.com/websocket/v1` — confirmed live 2026-09-02.
pub(crate) const UPBIT_WS_URL: &str = "wss://api.upbit.com/websocket/v1";

/// Upbit joins quote and base with `-`, quote first (`KRW-BTC`).
const SEPARATOR: char = '-';

/// How often to send the confirmed `PING`. Our own conservative choice,
/// not a venue-published number.
const KEEPALIVE: Duration = Duration::from_secs(30);

/// Upbit's public trade stream.
pub(crate) struct UpbitTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
    /// Every code this connection is currently meant to be receiving.
    ///
    /// Upbit has no incremental subscribe: each request replaces the whole
    /// list, so the list has to be remembered rather than derived from the
    /// one instrument a call is about. Sorted so a frame for the same set
    /// is always the same string, which makes the tests below exact.
    subscribed: Mutex<BTreeSet<String>>,
}

impl UpbitTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
            subscribed: Mutex::new(BTreeSet::new()),
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Upbit native symbol known for {instrument}"))
        })
    }

    /// The request that makes the venue send exactly `codes` and nothing
    /// else.
    fn frame(codes: &BTreeSet<String>) -> String {
        let codes = codes
            .iter()
            .map(|code| format!("\"{code}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"[{{"ticket":"senken"}},{{"type":"trade","codes":[{codes}]}},{{"format":"DEFAULT"}}]"#
        )
    }
}

impl VenueProtocol for UpbitTradesProtocol {
    fn url(&self) -> &str {
        UPBIT_WS_URL
    }

    fn venue(&self) -> &'static str {
        "upbit"
    }

    /// Sends the **whole** set of codes this connection wants, not just
    /// `instrument`'s.
    ///
    /// Upbit's subscribe is a replacement, confirmed live: asking for
    /// `KRW-ETH` after `KRW-BTC` stops BTC. Emitting one code per call
    /// would therefore leave only the most recently leased instrument
    /// streaming, and every earlier lease silently receiving nothing.
    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let code = self.native_symbol(instrument)?;
        let mut subscribed = self
            .subscribed
            .lock()
            .map_err(|_| ConnectionError::new("Upbit's subscription set is poisoned"))?;
        subscribed.insert(code);
        Ok(Self::frame(&subscribed))
    }

    /// Also sends the whole remaining set: dropping one code means asking
    /// for the others again, since there is no unsubscribe to send.
    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let code = self.native_symbol(instrument)?;
        let mut subscribed = self
            .subscribed
            .lock()
            .map_err(|_| ConnectionError::new("Upbit's subscription set is poisoned"))?;
        subscribed.remove(&code);
        Ok(Self::frame(&subscribed))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        if frame.kind != "trade" {
            return Vec::new();
        }
        let Ok(instrument) =
            InstrumentId::new(&self.source_id, &normalise_symbol(frame.code, &[SEPARATOR]))
        else {
            return Vec::new();
        };
        let Some(ts) = senken_core::UnixNanos::from_millis(frame.trade_timestamp) else {
            return Vec::new();
        };
        let mut updates = Vec::new();
        if let (Some(price), Some(volume)) = (frame.trade_price, frame.trade_volume)
            && let Some(update) = trade(ts, price.get(), volume.get())
        {
            updates.push((instrument.clone(), LiveUpdate::Price(update)));
        }
        if let (Some(bid), Some(ask), Some(bid_size), Some(ask_size)) = (
            frame.best_bid_price,
            frame.best_ask_price,
            frame.best_bid_size,
            frame.best_ask_size,
        ) && let Some(update) = quote(ts, bid.get(), ask.get(), bid_size.get(), ask_size.get())
        {
            updates.push((instrument, LiveUpdate::Quote(update)));
        }
        updates
    }

    fn decode_binary(&self, bytes: &[u8]) -> Option<String> {
        // Not compressed — Upbit simply uses the binary opcode for plain
        // UTF-8 JSON.
        String::from_utf8(bytes.to_vec()).ok()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, "PING".to_owned()))
    }
}

/// One inbound frame. The `PING` answer (`{"status":"UP"}`) shares none of
/// these fields, so it decodes to a non-`trade` kind and is ignored.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default, rename = "type")]
    kind: &'a str,
    #[serde(borrow, default)]
    code: &'a str,
    /// The **trade's** epoch milliseconds — `timestamp` beside it is the
    /// frame's.
    #[serde(default)]
    trade_timestamp: i64,
    #[serde(borrow, default)]
    trade_price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    trade_volume: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_bid_price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_ask_price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_bid_size: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_ask_size: Option<&'a RawValue>,
}

/// Upbit's live-feed registration.
pub(crate) struct UpbitFeedSource {
    source_ids: Vec<String>,
}

impl UpbitFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for UpbitFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Confirmed live: the trade stream itself carries
        // `best_bid_price`, `best_ask_price` and both sizes.
        true
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(UpbitTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{UPBIT_WS_URL, UpbitTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;

    /// Upbit writes the quote asset first (`KRW-BTC`), so the normalised
    /// id is `KRWBTC`.
    struct DashedMap;
    impl SymbolMap for DashedMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            instrument
                .symbol()
                .strip_prefix("KRW")
                .map(|base| format!("KRW-{base}"))
        }
    }

    fn protocol() -> UpbitTradesProtocol {
        UpbitTradesProtocol::new(crate::SOURCE_ID, Arc::new(DashedMap))
    }

    fn instrument(symbol: &str) -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, symbol).unwrap()
    }

    /// The captured data frame, verbatim.
    const TRADE_FRAME: &str = r#"{"type":"trade","code":"KRW-BTC","timestamp":1788335180423,"trade_date":"2026-09-02","trade_time":"07:46:20","trade_timestamp":1788335180377,"trade_price":106691000.00000000,"trade_volume":0.00009372,"ask_bid":"ASK","prev_closing_price":107005000.00000000,"change":"FALL","change_price":314000.00000000,"sequential_id":17883351803770004,"best_ask_price":106705000,"best_ask_size":0.00906333,"best_bid_price":106691000,"best_bid_size":0.00702247,"stream_type":"SNAPSHOT"}"#;

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), UPBIT_WS_URL);
    }

    #[test]
    fn the_first_subscribe_frame_matches_the_confirmed_shape() {
        let text = protocol().subscribe_frame(&instrument("KRWBTC")).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame[0]["ticket"], "senken");
        assert_eq!(frame[1]["type"], "trade");
        assert_eq!(frame[1]["codes"][0], "KRW-BTC");
        assert_eq!(frame[2]["format"], "DEFAULT");
    }

    /// Confirmed live: a second subscribe *replaces* the first. Sending
    /// one code per call would leave only the most recently leased
    /// instrument streaming and every earlier one silently dead.
    #[test]
    fn a_second_subscribe_asks_for_both_codes_not_just_the_new_one() {
        let protocol = protocol();
        protocol.subscribe_frame(&instrument("KRWBTC")).unwrap();
        let text = protocol.subscribe_frame(&instrument("KRWETH")).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        let codes = frame[1]["codes"].as_array().unwrap();
        assert_eq!(codes.len(), 2);
        assert!(codes.iter().any(|c| c == "KRW-BTC"));
        assert!(codes.iter().any(|c| c == "KRW-ETH"));
    }

    /// There is no unsubscribe to send, so dropping one code means asking
    /// for the remaining ones again.
    #[test]
    fn an_unsubscribe_asks_for_everything_that_is_left() {
        let protocol = protocol();
        protocol.subscribe_frame(&instrument("KRWBTC")).unwrap();
        protocol.subscribe_frame(&instrument("KRWETH")).unwrap();
        let text = protocol.unsubscribe_frame(&instrument("KRWBTC")).unwrap();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame[1]["codes"], serde_json::json!(["KRW-ETH"]));
    }

    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let updates = protocol().parse_message(TRADE_FRAME);

        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(id, &instrument("KRWBTC"));
        // `106691000.00000000` — the trailing zeros carry no information.
        assert_eq!(update.price, 106_691_000);
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.qty, senken_series::Volume::Real(9_372));
        assert_eq!(update.qty_scale, 8);
        assert_eq!(
            update.ts.as_millis(),
            1_788_335_180_377,
            "`trade_timestamp`, not the frame's own `timestamp` (…423)"
        );
    }

    /// The same frame carries a best bid and offer, so one message yields
    /// both a trade and a quote.
    #[test]
    fn the_captured_trade_frame_also_decodes_to_a_quote() {
        let updates = protocol().parse_message(TRADE_FRAME);
        assert_eq!(updates.len(), 2);
        let (_, LiveUpdate::Quote(update)) = &updates[1] else {
            panic!("the second update must be the quote");
        };
        assert_eq!(update.bid, 106_691_000);
        assert_eq!(update.ask, 106_705_000);
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.bid_size, 702_247);
        assert_eq!(update.ask_size, 906_333);
        assert_eq!(update.qty_scale, 8);
    }

    /// Upbit sends nothing as text. A protocol that ignores binary frames
    /// receives no data at all from this venue, silently — and unlike HTX
    /// there is no compression to blame, only the opcode.
    #[test]
    fn a_binary_frame_is_read_as_the_plain_utf8_json_it_is() {
        let text = protocol().decode_binary(TRADE_FRAME.as_bytes()).unwrap();
        assert_eq!(text, TRADE_FRAME);
        assert_eq!(protocol().parse_message(&text).len(), 2);
    }

    #[test]
    fn the_confirmed_ping_answer_yields_nothing() {
        assert!(protocol().parse_message(r#"{"status":"UP"}"#).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn the_keepalive_is_the_confirmed_ping() {
        let (_, frame) = protocol().keepalive().unwrap();
        assert_eq!(frame, "PING");
    }
}
