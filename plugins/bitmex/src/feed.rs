//! `BitMEX`'s public `trade` and `quote` tables.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://ws.bitmex.com/realtime`, sent
//! `{"op":"subscribe","args":["trade:XBTUSD","quote:XBTUSD"]}` and
//! received a welcome frame, two `success` acknowledgements, a `partial`
//! snapshot per table, and then:
//!
//! ```json
//! {"table":"trade","action":"partial","keys":[],"types":{…},"filter":{"pool":"Aggregated","symbol":"XBTUSD"},"data":[{"timestamp":"2026-09-02T07:34:37.309Z","symbol":"XBTUSD","side":"Buy","size":8000,"price":77454.5,"tickDirection":"MinusTick","trdMatchID":"00000000-006d-1000-0000-00367d215623","grossValue":10328640,"homeNotional":0.1032864,"foreignNotional":8000.0}]}
//! {"table":"quote","action":"insert","data":[{"timestamp":"2026-09-02T07:48:59.167Z","symbol":"XBTUSD","bidSize":40600,"bidPrice":77465.5,"askPrice":77543.1,"askSize":314900,"pool":"Primary"}]}
//! ```
//!
//! Read from that capture:
//! - **Prices are bare JSON numbers.** `77454.5`, `77543.1`. They are read
//!   through [`RawValue`](serde_json::value::RawValue) so the venue's own
//!   digits reach the scaled-integer parser rather than an `f64`.
//! - **`size` is contracts, and that is the right choice here.** For
//!   `XBTUSD` a contract is one dollar, so `size` is USD — and the bars
//!   this project stores for BitMEX take their volume from the same
//!   contract-denominated field. `homeNotional` (the BTC figure) sits
//!   beside it; taking that instead would make a volume indicator step at
//!   the join between stored bars and live ticks.
//! - **`timestamp` is RFC 3339, not an epoch integer.**
//! - The venue symbol is `XBTUSD`, exactly the catalog's `source_symbol`.
//! - The `quote` table carries a genuine best bid and offer, so this feed
//!   serves quotes.
//! - A `quote` frame's `pool` is `"Primary"` or `"Secondary"`. Both are
//!   decoded: the pool a quote came from does not change what the numbers
//!   in it mean, and filtering to one would silently halve the update
//!   rate.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::{quote, trade};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://ws.bitmex.com/realtime` — confirmed live 2026-09-02.
pub(crate) const BITMEX_WS_URL: &str = "wss://ws.bitmex.com/realtime";

/// `BitMEX`'s public trade and quote tables.
pub(crate) struct BitmexTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl BitmexTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no BitMEX native symbol known for {instrument}"))
        })
    }

    fn frame(op: &str, symbol: &str) -> String {
        format!(r#"{{"op":"{op}","args":["trade:{symbol}","quote:{symbol}"]}}"#)
    }

    fn instrument(&self, symbol: &str) -> Option<InstrumentId> {
        InstrumentId::new(&self.source_id, &normalise_symbol(symbol, &['_'])).ok()
    }
}

impl VenueProtocol for BitmexTradesProtocol {
    fn url(&self) -> &str {
        BITMEX_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bitmex"
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
        match frame.table {
            "trade" => frame
                .data
                .iter()
                .filter_map(|row| self.decode_trade(row))
                .collect(),
            "quote" => frame
                .data
                .iter()
                .filter_map(|row| self.decode_quote(row))
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl BitmexTradesProtocol {
    fn decode_trade(&self, row: &Row<'_>) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = self.instrument(row.symbol)?;
        let ts = Self::timestamp(row.timestamp)?;
        Some((
            instrument,
            LiveUpdate::Price(trade(ts, row.price?.get(), row.size?.get())?),
        ))
    }

    fn decode_quote(&self, row: &Row<'_>) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = self.instrument(row.symbol)?;
        let ts = Self::timestamp(row.timestamp)?;
        Some((
            instrument,
            LiveUpdate::Quote(quote(
                ts,
                row.bid_price?.get(),
                row.ask_price?.get(),
                row.bid_size?.get(),
                row.ask_size?.get(),
            )?),
        ))
    }

    fn timestamp(raw: &str) -> Option<senken_core::UnixNanos> {
        senken_core::UnixNanos::from_millis(senken_venue::iso8601_ms(raw)?)
    }
}

/// One inbound frame. The welcome and `success` frames carry no `table`,
/// so `#[serde(default)]` decodes them to an empty one.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    table: &'a str,
    #[serde(borrow, default)]
    data: Vec<Row<'a>>,
}

/// One row of either table — the two share `symbol` and `timestamp` and
/// differ in which of the price fields they carry.
#[derive(Debug, Deserialize)]
struct Row<'a> {
    symbol: &'a str,
    /// RFC 3339, not an epoch integer.
    timestamp: &'a str,
    #[serde(borrow, default)]
    price: Option<&'a RawValue>,
    /// Contracts — for `XBTUSD`, dollars. See this module's docs.
    #[serde(borrow, default)]
    size: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "bidPrice")]
    bid_price: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "askPrice")]
    ask_price: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "bidSize")]
    bid_size: Option<&'a RawValue>,
    #[serde(borrow, default, rename = "askSize")]
    ask_size: Option<&'a RawValue>,
}

/// `BitMEX`'s live-feed registration.
pub(crate) struct BitmexFeedSource {
    source_ids: Vec<String>,
}

impl BitmexFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for BitmexFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Confirmed live: the `quote` table carries `bidPrice`, `askPrice`
        // and both sizes.
        true
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BitmexTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BITMEX_WS_URL, BitmexTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    fn protocol() -> BitmexTradesProtocol {
        BitmexTradesProtocol::new(crate::SOURCE_ID, Arc::new(IdentitySymbolMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, "XBTUSD").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BITMEX_WS_URL);
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "subscribe");
        assert_eq!(frame["args"][0], "trade:XBTUSD");
        assert_eq!(frame["args"][1], "quote:XBTUSD");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_op() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["op"], "unsubscribe");
    }

    /// Byte-for-byte a `trade` row from this module's live capture. The
    /// price is the bare number `77454.5`; reading it through an `f64`
    /// would be a float on the money path.
    #[test]
    fn the_captured_trade_row_decodes_to_the_exact_traded_price() {
        let frame = r#"{"table":"trade","action":"partial","keys":[],"filter":{"pool":"Aggregated","symbol":"XBTUSD"},"data":[{"timestamp":"2026-09-02T07:34:37.309Z","symbol":"XBTUSD","side":"Buy","size":8000,"price":77454.5,"tickDirection":"MinusTick","trdMatchID":"00000000-006d-1000-0000-00367d215623","grossValue":10328640,"homeNotional":0.1032864,"foreignNotional":8000.0}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade row must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 774_545);
        assert_eq!(update.price_scale, 1);
        assert_eq!(
            update.qty,
            senken_series::Volume::Real(8_000),
            "`size` (contracts), the same field the stored bars measure — not `homeNotional`"
        );
        assert_eq!(update.qty_scale, 0);
        assert_eq!(update.ts.as_millis(), 1_788_334_477_309);
    }

    /// Byte-for-byte a `quote` row from the same capture.
    #[test]
    fn the_captured_quote_row_decodes_to_a_quote() {
        let frame = r#"{"table":"quote","action":"insert","data":[{"timestamp":"2026-09-02T07:48:59.167Z","symbol":"XBTUSD","bidSize":40600,"bidPrice":77465.5,"askPrice":77543.1,"askSize":314900,"pool":"Primary"}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (_, LiveUpdate::Quote(update)) = &updates[0] else {
            panic!("a quote row must decode to a quote update");
        };
        assert_eq!(update.bid, 774_655);
        assert_eq!(update.ask, 775_431);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.bid_size, 40_600);
        assert_eq!(update.ask_size, 314_900);
    }

    /// Both liquidity pools carry real quotes; filtering to one would
    /// silently halve the update rate.
    #[test]
    fn a_secondary_pool_quote_is_decoded_too() {
        let frame = r#"{"table":"quote","action":"insert","data":[{"timestamp":"2026-09-02T07:48:59.256Z","symbol":"XBTUSD","bidSize":519500,"bidPrice":77470.1,"askPrice":77490.9,"askSize":519600,"pool":"Secondary"}]}"#;
        assert_eq!(protocol().parse_message(frame).len(), 1);
    }

    #[test]
    fn the_captured_welcome_and_acknowledgement_yield_nothing() {
        let welcome = r#"{"info":"Welcome to the BitMEX Realtime API.","version":"2.0.0","timestamp":"2026-09-02T07:44:44.033Z","docs":"https://www.bitmex.com/app/wsAPI","heartbeatEnabled":false,"appName":"ws-feedhandler"}"#;
        let ack = r#"{"success":true,"subscribe":"trade:XBTUSD","pool":"Aggregated","request":{"op":"subscribe","args":["trade:XBTUSD","quote:XBTUSD"]}}"#;
        assert!(protocol().parse_message(welcome).is_empty());
        assert!(protocol().parse_message(ack).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
