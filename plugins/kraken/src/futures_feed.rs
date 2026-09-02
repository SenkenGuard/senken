//! Kraken Futures' public `trade` feed.
//!
//! # Why this market has a stream and spot does not
//!
//! Kraken's *spot* stream is unreachable from this catalog: it wants
//! `BTC/USD` while every spot REST call wants `XBTUSD`, and an instrument
//! carries one symbol (see this crate's own module docs). Kraken Futures
//! has no such split — the product id is `PF_XBTUSD` on both its REST API
//! and its socket, which is exactly what this plugin already stores.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://futures.kraken.com/ws/v1`, sent
//! `{"event":"subscribe","feed":"trade","product_ids":["PF_XBTUSD"]}` and
//! received:
//!
//! ```json
//! {"event":"subscribed","feed":"trade","product_ids":["PF_XBTUSD"]}
//! {"feed":"trade_snapshot","product_id":"PF_XBTUSD","trades":[{"product_id":"PF_XBTUSD","feed":"trade","uid":"207b30c3-…","side":"buy","type":"fill","time":1788348473868,"qty":0.0013,"price":76432.0,"seq":883384}]}
//! ```
//!
//! Read from that capture:
//! - **Two feed names carry trades**: `trade_snapshot` is the backfill
//!   sent on subscribing, `trade` each new one. Both decode, so a chart
//!   shows a price immediately rather than after the next fill.
//! - A snapshot's trades are an array under `trades`; a live one is the
//!   frame itself. Both shapes are read.
//! - `price` and `qty` are **bare JSON numbers**, read through
//!   [`RawValue`](serde_json::value::RawValue) so the venue's own digits
//!   reach the scaled-integer parser rather than an `f64`.
//! - `time` is epoch milliseconds.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://futures.kraken.com/ws/v1` — confirmed live 2026-09-02.
pub(crate) const KRAKEN_FUTURES_WS_URL: &str = "wss://futures.kraken.com/ws/v1";

/// Kraken Futures' public trade feed.
pub(crate) struct KrakenFuturesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl KrakenFuturesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!(
                "no Kraken Futures product id known for {instrument}"
            ))
        })
    }

    fn frame(event: &str, product: &str) -> String {
        format!(r#"{{"event":"{event}","feed":"trade","product_ids":["{product}"]}}"#)
    }

    fn decode(&self, row: &RawTrade<'_>) -> Option<(InstrumentId, LiveUpdate)> {
        // The product id carries no separator this catalog strips, so the
        // stored form and the wire form are the same string.
        let instrument = InstrumentId::new(&self.source_id, row.product_id).ok()?;
        let ts = senken_core::UnixNanos::from_millis(row.time)?;
        Some((
            instrument,
            LiveUpdate::Price(trade(ts, row.price.get(), row.qty.get())?),
        ))
    }
}

impl VenueProtocol for KrakenFuturesProtocol {
    fn url(&self) -> &str {
        KRAKEN_FUTURES_WS_URL
    }

    fn venue(&self) -> &'static str {
        "kraken-futures"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("subscribe", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("unsubscribe", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        match frame.feed {
            // The backfill sent on subscribing: an array under `trades`.
            "trade_snapshot" => frame
                .trades
                .iter()
                .filter_map(|row| self.decode(row))
                .collect(),
            // A live fill: the frame *is* the trade.
            "trade" => {
                let (Some(price), Some(qty)) = (frame.price, frame.qty) else {
                    return Vec::new();
                };
                self.decode(&RawTrade {
                    product_id: frame.product_id,
                    time: frame.time,
                    price,
                    qty,
                })
                .into_iter()
                .collect()
            }
            _ => Vec::new(),
        }
    }
}

/// One inbound frame. The `subscribed` acknowledgement carries an `event`
/// and no trade fields, so every field below defaults.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    feed: &'a str,
    #[serde(borrow, default)]
    product_id: &'a str,
    #[serde(default)]
    time: i64,
    #[serde(borrow, default)]
    price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    qty: Option<&'a RawValue>,
    #[serde(borrow, default)]
    trades: Vec<RawTrade<'a>>,
}

#[derive(Debug, Deserialize)]
struct RawTrade<'a> {
    #[serde(borrow)]
    product_id: &'a str,
    /// Epoch milliseconds.
    time: i64,
    #[serde(borrow)]
    price: &'a RawValue,
    #[serde(borrow)]
    qty: &'a RawValue,
}

/// Kraken Futures' live-feed registration.
pub(crate) struct KrakenFuturesFeedSource {
    source_ids: Vec<String>,
}

impl KrakenFuturesFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::FUTURES_ID.to_owned()],
        }
    }
}

impl FeedSource for KrakenFuturesFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(KrakenFuturesProtocol::new(crate::FUTURES_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{KRAKEN_FUTURES_WS_URL, KrakenFuturesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    fn protocol() -> KrakenFuturesProtocol {
        KrakenFuturesProtocol::new(crate::FUTURES_ID, Arc::new(IdentitySymbolMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::FUTURES_ID, "PF_XBTUSD").unwrap()
    }

    #[test]
    fn the_confirmed_url_and_subscribe_shape_are_used() {
        assert_eq!(protocol().url(), KRAKEN_FUTURES_WS_URL);
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "subscribe");
        assert_eq!(frame["feed"], "trade");
        assert_eq!(frame["product_ids"][0], "PF_XBTUSD");
    }

    #[test]
    fn an_unsubscribe_has_the_symmetric_event() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "unsubscribe");
    }

    /// Byte-for-byte the snapshot from this module's live capture. It is
    /// the backfill sent on subscribing; discarding it leaves a chart
    /// blank until the next fill.
    #[test]
    fn the_captured_snapshot_decodes_to_prices() {
        let frame = r#"{"feed":"trade_snapshot","product_id":"PF_XBTUSD","trades":[{"product_id":"PF_XBTUSD","feed":"trade","uid":"207b30c3-85f0-41d0-a362-8e79701cb8c6","side":"buy","type":"fill","time":1788348473868,"qty":0.0013,"price":76432.0,"seq":883384}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade_snapshot must decode to price updates");
        };
        assert_eq!(id, &instrument());
        // `76432.0` — a trailing zero carries no information.
        assert_eq!(update.price, 76_432);
        assert_eq!(update.qty, senken_series::Volume::Real(13));
        assert_eq!(update.qty_scale, 4);
        assert_eq!(update.ts.as_millis(), 1_788_348_473_868);
    }

    /// A live fill is the frame itself, not an entry in an array — the
    /// two shapes share a feed name prefix and nothing else.
    #[test]
    fn a_live_fill_is_read_from_the_frame_itself() {
        let frame = r#"{"feed":"trade","product_id":"PF_XBTUSD","uid":"1257ccfb","side":"sell","type":"fill","seq":883385,"time":1788348474000,"qty":0.5,"price":76430.5}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (_, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade frame must decode to a price update");
        };
        assert_eq!(update.price, 764_305);
        assert_eq!(update.price_scale, 1);
    }

    #[test]
    fn the_captured_acknowledgement_yields_nothing() {
        let frame = r#"{"event":"subscribed","feed":"trade","product_ids":["PF_XBTUSD"]}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
