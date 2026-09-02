//! Phemex's public trade streams — both of them.
//!
//! # What was confirmed live, 2026-09-02
//!
//! One socket, `wss://ws.phemex.com`, two subscribe methods, and which one
//! a symbol answers to is decided by the same `priceScale` that decides
//! how its numbers are written (see [`crate::scales`]):
//!
//! ```json
//! {"id":1,"method":"trade.subscribe","params":["BTCUSD"]}
//! {"sequence":23611442091,"symbol":"BTCUSD","trades":[[1788346425275042617,"Buy",763550000,370], …]}
//!
//! {"id":3,"method":"trade_p.subscribe","params":["BTCUSDT"]}
//! {"dts":1788346442564928876,"mts":1788346442406331636,"sequence":67412126177,"symbol":"BTCUSDT","trades_p":[[1788346442396240233,"Buy","76426.7","0.091"], …]}
//! ```
//!
//! Asking the wrong one is not a quiet failure — `trade.subscribe` for
//! `BTCUSDT` came back `{"error":{"code":6001,"message":"invalid
//! argument"}}` — but it is a failure that only shows up per symbol, so
//! the choice is made from the catalogue rather than from the symbol's
//! spelling.
//!
//! Read from those captures:
//! - A row is `[timestamp, side, price, size]` and **the timestamp is
//!   epoch nanoseconds** — nineteen digits — where most venues here send
//!   milliseconds.
//! - On `trades` the price and size are pre-scaled integers at that
//!   symbol's own scales; on `trades_p` they are decimal text.
//! - Rows arrive newest first, and a subscribe replays recent history
//!   before the stream proper.
//!
//! # Why the catalogue is warmed on dial
//!
//! [`VenueProtocol::parse_message`] is synchronous, and a frame cannot be
//! read without knowing its symbol's scales. The one asynchronous moment
//! in a connection's life is [`VenueProtocol::endpoint`], which runs
//! before every dial including every reconnect — so the product list is
//! loaded there, and a frame that somehow arrives before it is dropped
//! rather than read at a guessed scale.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::VenueClient;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::scales::{ScaleCatalog, Scales};

/// `wss://ws.phemex.com` — confirmed live 2026-09-02.
pub(crate) const PHEMEX_WS_URL: &str = "wss://ws.phemex.com";

/// Phemex's public trade channels.
pub(crate) struct PhemexTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
    scales: ScaleCatalog,
}

impl PhemexTradesProtocol {
    pub(crate) fn new(
        source_id: impl Into<Box<str>>,
        symbols: Arc<dyn SymbolMap>,
        scales: ScaleCatalog,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
            scales,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Phemex native symbol known for {instrument}"))
        })
    }

    /// `trade` for a pre-scaled symbol, `trade_p` for a decimal one. The
    /// venue refuses the wrong one outright.
    fn method(&self, symbol: &str, verb: &str) -> Result<String, ConnectionError> {
        let scales = self.scales.cached(symbol).ok_or_else(|| {
            ConnectionError::new(format!(
                "Phemex's product list has not been loaded, so {symbol}'s channel is unknown"
            ))
        })?;
        let channel = if scales.is_decimal() {
            "trade_p"
        } else {
            "trade"
        };
        Ok(format!(
            r#"{{"id":1,"method":"{channel}.{verb}","params":["{symbol}"]}}"#
        ))
    }
}

#[async_trait::async_trait]
impl VenueProtocol for PhemexTradesProtocol {
    fn url(&self) -> &str {
        PHEMEX_WS_URL
    }

    /// Loads the product list before dialling — see the module docs.
    async fn endpoint(&self) -> Result<String, ConnectionError> {
        self.scales.warm().await.map_err(|source| {
            ConnectionError::new(format!(
                "Phemex's product list could not be loaded: {source}"
            ))
        })?;
        Ok(PHEMEX_WS_URL.to_owned())
    }

    fn venue(&self) -> &'static str {
        "phemex"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.method(&self.native_symbol(instrument)?, "subscribe")
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.method(&self.native_symbol(instrument)?, "unsubscribe")
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame<'_>>(text) else {
            return Vec::new();
        };
        let rows = frame.trades.or(frame.trades_p).unwrap_or_default();
        if rows.is_empty() {
            return Vec::new();
        }
        // Without the symbol's scales a row cannot be read at all, and
        // reading it at a guessed one is how a price lands four orders of
        // magnitude out.
        let Some(scales) = self.scales.cached(frame.symbol) else {
            return Vec::new();
        };
        // Phemex normalises nothing away: its own symbols carry no
        // separator, and spot's leading `s` is a market marker this
        // plugin's catalog already strips.
        let symbol = frame
            .symbol
            .strip_prefix(crate::SPOT_PREFIX)
            .unwrap_or(frame.symbol);
        let Ok(instrument) = InstrumentId::new(&self.source_id, &symbol.to_uppercase()) else {
            return Vec::new();
        };
        rows.iter()
            .filter_map(|row| {
                let update = decode(row, scales)?;
                Some((instrument.clone(), LiveUpdate::Price(update)))
            })
            .collect()
    }
}

/// One row: `[timestamp, side, price, size]`.
type Row<'a> = (i64, &'a str, &'a RawValue, &'a RawValue);

/// One inbound frame. A subscribe acknowledgement carries neither array.
#[derive(Debug, Deserialize)]
struct Frame<'a> {
    #[serde(borrow, default)]
    symbol: &'a str,
    /// Pre-scaled integers.
    #[serde(borrow, default)]
    trades: Option<Vec<Row<'a>>>,
    /// Decimal text.
    #[serde(borrow, default)]
    trades_p: Option<Vec<Row<'a>>>,
}

/// One row, read at `scales`.
fn decode(row: &Row<'_>, scales: Scales) -> Option<senken_subscription::PriceUpdate> {
    let &(nanos, _side, price, size) = row;
    let ts = UnixNanos::from_nanos(nanos);
    if scales.is_decimal() {
        // The digits arrive quoted; `RawValue::get` keeps the quotes.
        return trade(ts, unquote(price.get()), unquote(size.get()));
    }
    let price: i64 = price.get().trim().parse().ok()?;
    let size: i64 = size.get().trim().parse().ok()?;
    Some(senken_subscription::PriceUpdate {
        ts,
        price,
        price_scale: scales.price,
        qty: senken_series::Volume::Real(size),
        qty_scale: scales.quantity,
    })
}

/// Strips the quotes `RawValue` keeps around a JSON string.
fn unquote(raw: &str) -> &str {
    raw.trim().trim_matches('"')
}

/// Phemex's live-feed registration.
pub(crate) struct PhemexFeedSource {
    source_ids: Vec<String>,
    client: VenueClient,
    scales: ScaleCatalog,
}

impl PhemexFeedSource {
    pub(crate) fn new(source_id: &str, client: VenueClient, scales: ScaleCatalog) -> Self {
        Self {
            source_ids: vec![source_id.to_owned()],
            client,
            scales,
        }
    }
}

impl FeedSource for PhemexFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        let _ = &self.client;
        Arc::new(PhemexTradesProtocol::new(
            self.source_ids[0].as_str(),
            symbols,
            self.scales.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{PHEMEX_WS_URL, PhemexTradesProtocol};
    use crate::scales::ScaleCatalog;
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, SymbolMap, VenueProtocol};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const PRODUCTS: &[u8] = include_bytes!("../tests/fixtures/products.json");

    /// The captured frames, verbatim.
    const INVERSE: &str = r#"{"sequence":23611442091,"symbol":"BTCUSD","trades":[[1788346425275042617,"Buy",763550000,370],[1788346421335460204,"Sell",763399000,433]]}"#;
    const LINEAR: &str = r#"{"dts":1788346442564928876,"mts":1788346442406331636,"sequence":67412126177,"symbol":"BTCUSDT","trades_p":[[1788346442396240233,"Buy","76426.7","0.091"],[1788346442025639996,"Buy","76426.7","0.078"]]}"#;

    /// A map that hands back the normalised symbol unchanged — right for
    /// this venue's perpetuals, whose native form carries no separator.
    async fn protocol() -> (MockServer, PhemexTradesProtocol) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(PRODUCTS, "application/json"))
            .mount(&server)
            .await;
        let client = VenueClient::new(reqwest::Client::new(), LimitGroup::new("phemex-test"));
        let scales = ScaleCatalog::new(client).with_url(server.uri());
        let protocol = PhemexTradesProtocol::new(
            crate::PERP_ID,
            Arc::new(IdentitySymbolMap) as Arc<dyn SymbolMap>,
            scales,
        );
        protocol.endpoint().await.expect("the catalogue must warm");
        (server, protocol)
    }

    #[tokio::test]
    async fn the_confirmed_url_is_used() {
        let (_server, protocol) = protocol().await;
        assert_eq!(protocol.url(), PHEMEX_WS_URL);
        assert_eq!(protocol.endpoint().await.unwrap(), PHEMEX_WS_URL);
    }

    /// The venue refuses `trade.subscribe` for a V2 linear symbol with
    /// code 6001, so the channel has to come from the catalogue.
    #[tokio::test]
    async fn each_family_is_subscribed_on_the_channel_it_answers_to() {
        let (_server, protocol) = protocol().await;

        let inverse: serde_json::Value = serde_json::from_str(
            &protocol
                .subscribe_frame(&InstrumentId::new(crate::PERP_ID, "BTCUSD").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(inverse["method"], "trade.subscribe");

        let linear: serde_json::Value = serde_json::from_str(
            &protocol
                .subscribe_frame(&InstrumentId::new(crate::PERP_ID, "BTCUSDT").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(linear["method"], "trade_p.subscribe");
    }

    #[tokio::test]
    async fn an_unsubscribe_uses_the_same_channel() {
        let (_server, protocol) = protocol().await;
        let frame: serde_json::Value = serde_json::from_str(
            &protocol
                .unsubscribe_frame(&InstrumentId::new(crate::PERP_ID, "BTCUSD").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(frame["method"], "trade.unsubscribe");
    }

    /// Pre-scaled integers, read at `BTCUSD`'s own scale of 4.
    #[tokio::test]
    async fn an_inverse_frame_keeps_its_pre_scaled_digits() {
        let (_server, protocol) = protocol().await;

        let updates = protocol.parse_message(INVERSE);

        assert_eq!(updates.len(), 2);
        let (id, LiveUpdate::Price(first)) = &updates[0] else {
            panic!("a trades frame must decode to price updates");
        };
        assert_eq!(id, &InstrumentId::new(crate::PERP_ID, "BTCUSD").unwrap());
        assert_eq!(first.price, 763_550_000);
        assert_eq!(first.price_scale, 4);
        assert_eq!(first.qty, senken_series::Volume::Real(370));
        assert_eq!(first.qty_scale, 0, "inverse sizes are contract counts");
        assert_eq!(
            first.ts.as_nanos(),
            1_788_346_425_275_042_617,
            "nanoseconds, not milliseconds"
        );
    }

    /// The same socket, decimal text, on the other channel.
    #[tokio::test]
    async fn a_linear_frame_is_read_as_decimal_text() {
        let (_server, protocol) = protocol().await;

        let updates = protocol.parse_message(LINEAR);

        assert_eq!(updates.len(), 2);
        let (id, LiveUpdate::Price(first)) = &updates[0] else {
            panic!("a trades_p frame must decode to price updates");
        };
        assert_eq!(id, &InstrumentId::new(crate::PERP_ID, "BTCUSDT").unwrap());
        assert_eq!(first.price, 764_267);
        assert_eq!(first.price_scale, 1);
        assert_eq!(first.qty, senken_series::Volume::Real(91));
        assert_eq!(first.qty_scale, 3);
    }

    /// A frame whose symbol the catalogue does not describe is dropped,
    /// not read at whatever scale the last one used.
    #[tokio::test]
    async fn a_frame_for_an_unknown_symbol_is_dropped() {
        let (_server, protocol) = protocol().await;
        let frame =
            r#"{"sequence":1,"symbol":"NOTLISTED","trades":[[1788346425275042617,"Buy",1,1]]}"#;
        assert!(protocol.parse_message(frame).is_empty());
    }

    #[tokio::test]
    async fn garbage_input_yields_no_updates_rather_than_a_panic() {
        let (_server, protocol) = protocol().await;
        assert!(protocol.parse_message("not json").is_empty());
        assert!(protocol.parse_message("{}").is_empty());
        assert!(
            protocol
                .parse_message(r#"{"id":1,"result":{"status":"success"},"error":null}"#)
                .is_empty()
        );
    }
}
