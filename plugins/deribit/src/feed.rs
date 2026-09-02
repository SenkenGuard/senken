//! Deribit's public `trades` and `ticker` channels.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://www.deribit.com/ws/api/v2`, sent
//! `{"jsonrpc":"2.0","id":1,"method":"public/subscribe","params":{"channels":["trades.BTC-PERPETUAL.100ms","ticker.BTC-PERPETUAL.100ms"]}}`,
//! received the JSON-RPC result echoing both channel names, and then:
//!
//! ```json
//! {"jsonrpc":"2.0","method":"subscription","params":{"channel":"trades.BTC-PERPETUAL.100ms","data":[{"timestamp":1788335114435,"price":77529.5,"direction":"buy","index_price":77504.36,"instrument_name":"BTC-PERPETUAL","trade_seq":298099955,"amount":170.0,"mark_price":77529.76,"tick_direction":3,"trade_id":"443253707","contracts":17.0}]}}
//! {"jsonrpc":"2.0","method":"subscription","params":{"channel":"ticker.BTC-PERPETUAL.100ms","data":{"timestamp":1788336092899,…,"instrument_name":"BTC-PERPETUAL","best_ask_price":77436,"best_bid_price":77435.5,"best_ask_amount":56300,"best_bid_amount":10}}}
//! ```
//!
//! Read from that capture:
//! - **Every number is a bare JSON number, not a string.** `77529.5`,
//!   `170.0`, `77435.5`. They are read through
//!   [`RawValue`](serde_json::value::RawValue) so the venue's own digits
//!   reach the scaled-integer parser untouched — deserialising a price as
//!   `f64` and formatting it back is a float on the money path.
//! - **The `.100ms` suffix is the public interval.** Deribit's `.raw`
//!   variant of the same channels requires authentication; `.100ms` does
//!   not, which is why this protocol asks for it.
//! - The instrument is on each entry as `instrument_name`
//!   (`BTC-PERPETUAL`), the catalog's own `source_symbol`.
//! - `timestamp` is epoch milliseconds.
//! - The `ticker` channel carries `best_bid_price`/`best_ask_price` and
//!   both amounts, so this feed genuinely serves quotes.
//!
//! # Why a Deribit tick carries no volume
//!
//! A trade on this channel reports `amount` and `contracts`, and for
//! `BTC-PERPETUAL` both are USD-denominated: `170.0` for `17.0` contracts
//! at $10 each. The bars this project stores for Deribit take their
//! `volume` from the venue's own **base-currency** field, with the USD
//! figure kept separately as the quote volume. Publishing `amount` as a
//! tick's volume would therefore hand a volume indicator two different
//! units either side of the join between stored bars and live ticks — a
//! step change that looks like a real move in traded size.
//!
//! Deriving the base amount as `amount / price` is not the answer either:
//! that is a division on the money path whose result is not a quantity the
//! venue ever reported.
//!
//! So the tick carries [`Volume::Absent`](senken_series::Volume::Absent) —
//! "the venue did not report this", which is a fact — and the price, which
//! is what a live feed is for. Restoring volume means finding a
//! base-currency size on a Deribit trade frame, which this capture does
//! not contain.

use std::sync::Arc;

use senken_marketdata::InstrumentId;
use senken_plugin::live::{quote, trade_without_size};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://www.deribit.com/ws/api/v2` — confirmed live 2026-09-02.
pub(crate) const DERIBIT_WS_URL: &str = "wss://www.deribit.com/ws/api/v2";

/// The public interval suffix both channels are subscribed at. The `.raw`
/// alternative needs authentication.
const INTERVAL: &str = "100ms";

/// Deribit joins the parts of an instrument name with `-` for its
/// derivatives (`BTC-PERPETUAL`) and `_` for spot (`BTC_USDT`). Both have
/// to be stripped, and by the same rule this plugin's catalog uses — a
/// decoder that strips only the dash produces `BTC_USDT` where the catalog
/// holds `BTCUSDT`, so every spot trade is attributed to an instrument
/// that does not exist and silently dropped.
const SEPARATORS: [char; 2] = ['-', '_'];

/// Deribit's public trades and ticker channels.
pub(crate) struct DeribitTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl DeribitTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Deribit native symbol known for {instrument}"))
        })
    }

    fn frame(method: &str, symbol: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"public/{method}","params":{{"channels":["trades.{symbol}.{INTERVAL}","ticker.{symbol}.{INTERVAL}"]}}}}"#
        )
    }

    fn instrument(&self, name: &str) -> Option<InstrumentId> {
        InstrumentId::new(&self.source_id, &normalise_symbol(name, &SEPARATORS)).ok()
    }
}

impl VenueProtocol for DeribitTradesProtocol {
    fn url(&self) -> &str {
        DERIBIT_WS_URL
    }

    fn venue(&self) -> &'static str {
        "deribit"
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
        let Some(params) = frame.params else {
            return Vec::new();
        };
        if params.channel.starts_with("trades.") {
            let Ok(trades) = serde_json::from_str::<Vec<Trade<'_>>>(params.data.get()) else {
                return Vec::new();
            };
            return trades
                .iter()
                .filter_map(|entry| self.decode_trade(entry))
                .collect();
        }
        if params.channel.starts_with("ticker.") {
            let Ok(ticker) = serde_json::from_str::<Ticker<'_>>(params.data.get()) else {
                return Vec::new();
            };
            return self.decode_quote(&ticker).into_iter().collect();
        }
        Vec::new()
    }
}

impl DeribitTradesProtocol {
    fn decode_trade(&self, entry: &Trade<'_>) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = self.instrument(entry.instrument_name)?;
        let ts = senken_core::UnixNanos::from_millis(entry.timestamp)?;
        Some((
            instrument,
            LiveUpdate::Price(trade_without_size(ts, entry.price.get())?),
        ))
    }

    fn decode_quote(&self, ticker: &Ticker<'_>) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = self.instrument(ticker.instrument_name)?;
        let ts = senken_core::UnixNanos::from_millis(ticker.timestamp)?;
        Some((
            instrument,
            LiveUpdate::Quote(quote(
                ts,
                ticker.best_bid_price?.get(),
                ticker.best_ask_price?.get(),
                ticker.best_bid_amount?.get(),
                ticker.best_ask_amount?.get(),
            )?),
        ))
    }
}

/// One inbound frame. A JSON-RPC *result* carries no `params`, so it
/// decodes to `None` rather than an error.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    params: Option<Params<'a>>,
}

/// A subscription frame's payload. `data` is an array for `trades` and an
/// object for `ticker`, so it stays raw until the channel says which.
#[derive(Debug, Deserialize)]
struct Params<'a> {
    channel: &'a str,
    #[serde(borrow)]
    data: &'a RawValue,
}

#[derive(Debug, Deserialize)]
struct Trade<'a> {
    instrument_name: &'a str,
    timestamp: i64,
    #[serde(borrow)]
    price: &'a RawValue,
}

#[derive(Debug, Deserialize)]
struct Ticker<'a> {
    instrument_name: &'a str,
    timestamp: i64,
    #[serde(borrow, default)]
    best_bid_price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_ask_price: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_bid_amount: Option<&'a RawValue>,
    #[serde(borrow, default)]
    best_ask_amount: Option<&'a RawValue>,
}

/// Deribit's live-feed registration.
pub(crate) struct DeribitFeedSource {
    source_ids: Vec<String>,
}

impl DeribitFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SOURCE_ID.to_owned()],
        }
    }
}

impl FeedSource for DeribitFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // Confirmed live: `ticker.<instrument>.100ms` carries
        // `best_bid_price`, `best_ask_price` and both amounts.
        true
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(DeribitTradesProtocol::new(crate::SOURCE_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{DERIBIT_WS_URL, DeribitTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use std::sync::Arc;

    /// The catalog holds `BTC-PERPETUAL` while the normalised id strips
    /// the dash.
    struct DashedMap;
    impl SymbolMap for DashedMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            instrument
                .symbol()
                .strip_suffix("PERPETUAL")
                .map(|base| format!("{base}-PERPETUAL"))
        }
    }

    fn protocol() -> DeribitTradesProtocol {
        DeribitTradesProtocol::new(crate::SOURCE_ID, Arc::new(DashedMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SOURCE_ID, "BTCPERPETUAL").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), DERIBIT_WS_URL);
    }

    /// `.100ms` is public; `.raw` needs authentication. Asking for the
    /// wrong one fails at subscribe time with an error frame this
    /// protocol would decode to silence.
    #[test]
    fn the_subscribe_frame_asks_for_the_public_hundred_millisecond_channels() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "public/subscribe");
        assert_eq!(frame["params"]["channels"][0], "trades.BTC-PERPETUAL.100ms");
        assert_eq!(frame["params"]["channels"][1], "ticker.BTC-PERPETUAL.100ms");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_method() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["method"], "public/unsubscribe");
    }

    /// Byte-for-byte a trades frame from this module's live capture. The
    /// price arrives as the bare number `77529.5`; reading it through an
    /// `f64` would be a float on the money path.
    #[test]
    fn the_captured_trade_frame_decodes_to_the_exact_traded_price() {
        let frame = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"trades.BTC-PERPETUAL.100ms","data":[{"timestamp":1788335114435,"price":77529.5,"direction":"buy","index_price":77504.36,"instrument_name":"BTC-PERPETUAL","trade_seq":298099955,"amount":170.0,"mark_price":77529.76,"tick_direction":3,"trade_id":"443253707","contracts":17.0}]}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 775_295);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.ts.as_millis(), 1_788_335_114_435);
        // `amount":170.0` is USD notional, not the base-currency figure
        // the stored bars measure volume in — see this module's docs.
        assert_eq!(update.qty, senken_series::Volume::Absent);
    }

    /// Byte-for-byte a ticker frame from the same capture, with the two
    /// sides at *different* digit counts (`77436` and `77435.5`) — the
    /// case a per-side scaling would drop.
    #[test]
    fn the_captured_ticker_frame_decodes_to_a_quote_despite_uneven_sides() {
        let frame = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"ticker.BTC-PERPETUAL.100ms","data":{"timestamp":1788336092899,"state":"open","index_price":77421.35,"instrument_name":"BTC-PERPETUAL","last_price":77440,"best_ask_price":77436,"best_bid_price":77435.5,"best_ask_amount":56300,"best_bid_amount":10}}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (_, LiveUpdate::Quote(update)) = &updates[0] else {
            panic!("a ticker frame must decode to a quote update");
        };
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.bid, 774_355);
        assert_eq!(update.ask, 774_360);
        assert_eq!(update.qty_scale, 0);
        assert_eq!(update.bid_size, 10);
        assert_eq!(update.ask_size, 56_300);
    }

    /// Deribit writes spot with an underscore (`BTC_USDT`) and its
    /// derivatives with a dash (`BTC-PERPETUAL`), and this plugin's
    /// catalog strips both. A decoder that strips only the dash builds
    /// `BTC_USDT`, which is not an instrument this catalog holds, so every
    /// spot frame is dropped without an error to say so. Captured live
    /// 2026-09-02.
    #[test]
    fn a_spot_frame_whose_symbol_uses_an_underscore_is_attributed() {
        struct UnderscoreMap;
        impl SymbolMap for UnderscoreMap {
            fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
                instrument
                    .symbol()
                    .strip_suffix("USDT")
                    .map(|base| format!("{base}_USDT"))
            }
        }
        let protocol = DeribitTradesProtocol::new(crate::SOURCE_ID, Arc::new(UnderscoreMap));
        let frame = r#"{"jsonrpc":"2.0","method":"subscription","params":{"channel":"ticker.BTC_USDT.100ms","data":{"timestamp":1788339167138,"state":"open","stats":{"high":78356.0,"low":76520.0,"price_change":-1.3525,"volume":2.607,"volume_usd":202234.52,"volume_notional":202313.1595},"index_price":76756.0963,"instrument_name":"BTC_USDT","last_price":76729.0,"min_price":75220.0,"max_price":78292.0,"mark_price":76756.0963,"best_ask_price":76754.0,"best_bid_price":76714.0,"best_ask_amount":0.0013,"best_bid_amount":0.0003}}}"#;

        let updates = protocol.parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Quote(update)) = &updates[0] else {
            panic!("a spot ticker frame must decode to a quote update");
        };
        assert_eq!(
            id,
            &InstrumentId::new(crate::SOURCE_ID, "BTCUSDT").unwrap(),
            "the underscore has to be stripped, exactly as the catalog strips it"
        );
        assert_eq!(update.bid, 76_714);
        assert_eq!(update.ask, 76_754);
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.bid_size, 3);
        assert_eq!(update.ask_size, 13);
        assert_eq!(update.qty_scale, 4);

        // The subscribe has to name the venue's own form, underscore and
        // all, or the venue never sends this frame at all.
        let subscribe: serde_json::Value = serde_json::from_str(
            &protocol
                .subscribe_frame(&InstrumentId::new(crate::SOURCE_ID, "BTCUSDT").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(subscribe["params"]["channels"][0], "trades.BTC_USDT.100ms");
    }

    #[test]
    fn the_captured_subscribe_result_yields_nothing() {
        let frame = r#"{"jsonrpc":"2.0","id":1,"result":["trades.BTC-PERPETUAL.100ms","ticker.BTC-PERPETUAL.100ms"],"usIn":1788335113491884,"usOut":1788335113492056,"usDiff":172,"testnet":false}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
