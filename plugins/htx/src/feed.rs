//! HTX's public `trade.detail` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://api.huobi.pro/ws`, sent
//! `{"sub":"market.btcusdt.trade.detail","id":"1"}` and received — **every
//! frame gzip-compressed, none of them text**:
//!
//! ```json
//! {"id":"1","status":"ok","subbed":"market.btcusdt.trade.detail","ts":1788335115914}
//! {"ch":"market.btcusdt.trade.detail","ts":1788335110454,"tick":{"id":193150647715,"ts":1788335110452,"data":[{"id":1931506477151671992419320033,"ts":1788335110452,"tradeId":103627913547,"amount":1.8E-4,"price":77523.25,"direction":"sell","isRpiTrade":false}]}}
//! {"ping":1788335119660}
//! ```
//!
//! Three facts from that capture each break a decoder that misses them:
//!
//! - **Every frame is gzip.** A connection that only reads text frames
//!   receives nothing at all from HTX, and sees no error while doing it.
//!   [`VenueProtocol::decode_binary`] is where that is undone.
//! - **HTX pings the client, and drops a connection that does not answer.**
//!   `{"ping":<ts>}` must be answered with `{"pong":<ts>}` carrying the
//!   *same* number. [`VenueProtocol::reply_to`] is where that is answered.
//! - **Sizes arrive in scientific notation.** `1.8E-4` is 0.00018 BTC.
//!   Counting the fractional digits of that literal gives 1, so a decoder
//!   that does not normalise first stores 1.8 — four orders of magnitude
//!   out, and entirely plausible on a chart.
//!
//! Prices and sizes are bare JSON numbers, read through
//! [`RawValue`](serde_json::value::RawValue) so the venue's own digits
//! reach the scaled-integer parser untouched.
//!
//! The channel embeds the market in **lower case** —
//! `market.btcusdt.trade.detail` — which is exactly the form HTX's catalog
//! stores as `source_symbol`.

use std::io::Read;
use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://api.huobi.pro/ws` — confirmed live 2026-09-02.
pub(crate) const HTX_SPOT_WS_URL: &str = "wss://api.huobi.pro/ws";

/// HTX's public spot trade channel.
pub(crate) struct HtxTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl HtxTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn channel(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no HTX native symbol known for {instrument}"))
        })?;
        Ok(format!("market.{symbol}.trade.detail"))
    }
}

impl VenueProtocol for HtxTradesProtocol {
    fn url(&self) -> &str {
        HTX_SPOT_WS_URL
    }

    fn venue(&self) -> &'static str {
        "htx"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let channel = self.channel(instrument)?;
        Ok(format!(r#"{{"sub":"{channel}","id":"senken"}}"#))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let channel = self.channel(instrument)?;
        Ok(format!(r#"{{"unsub":"{channel}","id":"senken"}}"#))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        let (Some(channel), Some(tick)) = (frame.ch, frame.tick) else {
            return Vec::new();
        };
        let Some(symbol) = channel
            .strip_prefix("market.")
            .and_then(|rest| rest.strip_suffix(".trade.detail"))
        else {
            return Vec::new();
        };
        // Normalised by the same rule the catalog uses, rather than by
        // hand: HTX's symbols are lower case, and the two forms must not
        // be able to drift apart.
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &senken_venue::normalise_symbol(symbol, &['-']),
        ) else {
            return Vec::new();
        };
        tick.data
            .iter()
            .filter_map(|entry| {
                let ts = senken_core::UnixNanos::from_millis(entry.ts)?;
                Some((
                    instrument.clone(),
                    LiveUpdate::Price(trade(ts, entry.price.get(), entry.amount.get())?),
                ))
            })
            .collect()
    }

    fn decode_binary(&self, bytes: &[u8]) -> Option<String> {
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn reply_to(&self, text: &str) -> Option<String> {
        // The pong must echo the ping's own number; HTX drops a connection
        // that answers with anything else, or not at all.
        let ping = serde_json::from_str::<Ping>(text).ok()?.ping?;
        Some(format!(r#"{{"pong":{ping}}}"#))
    }
}

/// One inbound frame. The subscribe acknowledgement has `status`/`subbed`
/// and no `ch`, so both fields below decode to `None`.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    ch: Option<&'a str>,
    #[serde(borrow, default)]
    tick: Option<Tick<'a>>,
}

#[derive(Debug, Deserialize)]
struct Tick<'a> {
    #[serde(borrow)]
    data: Vec<Trade<'a>>,
}

#[derive(Debug, Deserialize)]
struct Trade<'a> {
    /// Epoch milliseconds.
    ts: i64,
    #[serde(borrow)]
    price: &'a RawValue,
    /// Base-asset size, often in scientific notation (`1.8E-4`).
    #[serde(borrow)]
    amount: &'a RawValue,
}

/// A venue-initiated keep-alive. Every other frame decodes to `ping: None`.
#[derive(Debug, Deserialize)]
struct Ping {
    #[serde(default)]
    ping: Option<i64>,
}

/// HTX's live-feed registration — spot only. HTX's three derivative
/// markets each stream from a different host, none captured here.
pub(crate) struct HtxFeedSource {
    source_ids: Vec<String>,
}

impl HtxFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for HtxFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(HtxTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{HTX_SPOT_WS_URL, HtxTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::io::Write;
    use std::sync::Arc;

    /// HTX's native symbols are lower case while the normalised id is not.
    struct LowercaseMap;
    impl SymbolMap for LowercaseMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            Some(instrument.symbol().to_lowercase())
        }
    }

    fn protocol() -> HtxTradesProtocol {
        HtxTradesProtocol::new(crate::SPOT_ID, Arc::new(LowercaseMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    /// The captured data frame, verbatim.
    const TRADE_FRAME: &str = r#"{"ch":"market.btcusdt.trade.detail","ts":1788335110454,"tick":{"id":193150647715,"ts":1788335110452,"data":[{"id":1931506477151671992419320033,"ts":1788335110452,"tradeId":103627913547,"amount":1.8E-4,"price":77523.25,"direction":"sell","isRpiTrade":false}]}}"#;

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), HTX_SPOT_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_names_the_lower_case_channel() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["sub"], "market.btcusdt.trade.detail");
    }

    #[test]
    fn an_unsubscribe_frame_names_the_same_channel() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["unsub"], "market.btcusdt.trade.detail");
    }

    /// `1.8E-4` is 0.00018 BTC. A decoder that counts the fractional
    /// digits of that literal without normalising first reads 1.8 — ten
    /// thousand times too large, and entirely plausible on a chart.
    #[test]
    fn the_captured_trade_frame_reads_a_scientific_notation_size_correctly() {
        let updates = protocol().parse_message(TRADE_FRAME);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade.detail frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 7_752_325);
        assert_eq!(update.price_scale, 2);
        assert_eq!(update.qty_scale, 5);
        assert_eq!(
            update.qty,
            senken_series::Volume::Real(18),
            "1.8E-4 is 18 at scale 5, not 18 at scale 1"
        );
        assert_eq!(update.ts.as_millis(), 1_788_335_110_452);
    }

    /// HTX sends nothing as text. A protocol that cannot inflate a gzip
    /// frame receives no data at all from this venue, silently.
    #[test]
    fn a_gzip_frame_is_inflated_to_the_text_the_parser_reads() {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(TRADE_FRAME.as_bytes()).unwrap();
        let compressed = encoder.finish().unwrap();

        let text = protocol().decode_binary(&compressed).unwrap();

        assert_eq!(text, TRADE_FRAME);
        assert_eq!(protocol().parse_message(&text).len(), 1);
    }

    #[test]
    fn a_binary_frame_that_is_not_gzip_yields_nothing_rather_than_a_panic() {
        assert!(protocol().decode_binary(b"not gzip at all").is_none());
    }

    /// The pong must carry the ping's own number. HTX drops a connection
    /// that answers with anything else.
    #[test]
    fn the_captured_ping_is_answered_with_the_same_number() {
        let reply = protocol().reply_to(r#"{"ping":1788335119660}"#).unwrap();
        assert_eq!(reply, r#"{"pong":1788335119660}"#);
    }

    #[test]
    fn a_data_frame_needs_no_reply() {
        assert!(protocol().reply_to(TRADE_FRAME).is_none());
        assert!(protocol().reply_to("not json").is_none());
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame =
            r#"{"id":"1","status":"ok","subbed":"market.btcusdt.trade.detail","ts":1788335115914}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
