//! MEXC order-book depth: `GET /api/v3/depth` for spot, `GET
//! /api/v1/contract/depth/{symbol}` on the dedicated `contract.mexc.com`
//! host for futures — the same host split [`crate::bars`] already
//! documents for klines, and the same reason: `contract.mexc.com` answers
//! market data unauthenticated even though its instrument-list endpoint
//! 403s.
//!
//! # Spot: what was confirmed live, 2026-09-02
//!
//! `GET https://api.mexc.com/api/v3/depth?symbol=BTCUSDT&limit=5` returned
//! `HTTP 200`:
//!
//! ```json
//! {"lastUpdateId":80395323931,"bids":[["77616.42","0.02555831"],
//! ["77616.38","0.00032200"],["77614.44","0.29501146"],
//! ["77612.11","0.29501146"],["77611.25","0.05130653"]],
//! "asks":[["77616.99","0.00245559"],["77617.83","0.01084902"],
//! ["77617.84","0.05130653"],["77618.47","0.05130653"],
//! ["77624.12","0.05130653"]],"timestamp":1788332467067}
//! ```
//!
//! - No envelope, no error field at all — a rejected request is signalled
//!   only by HTTP status, which [`VenueClient`] already turns into a
//!   [`SourceError`] before this source ever sees a body.
//! - `bids`/`asks` are two-element decimal-string arrays: price, size.
//! - `limit` controls depth per side: 5 requested, 5 returned each side.
//! - Levels arrived best-first already (bids descending from `77616.42`,
//!   asks ascending from `77616.99`), but this source sorts explicitly
//!   anyway — one capture is not a guarantee.
//! - `timestamp` is a bare JSON number of epoch milliseconds.
//!
//! # Futures: what was confirmed live, 2026-09-02
//!
//! `GET https://contract.mexc.com/api/v1/contract/depth/BTC_USDT?limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"success":true,"code":0,"data":{"cts":null,
//! "asks":[[77594.2,451965,7],[77594.3,6024,1],[77594.4,5202,1],
//! [77594.5,45673,1],[77594.6,106215,1]],
//! "bids":[[77594.1,48738,5],[77594,6462,2],[77593.9,5177,1],
//! [77593.8,5805,2],[77593.7,5159,1]],
//! "version":41421153720,"timestamp":1788332467297}}
//! ```
//!
//! - `success`/`code` gate acceptance, the same envelope [`crate::bars`]
//!   already reads for this host.
//! - Every field is a **bare JSON number**, not a decimal string — unlike
//!   this same call's spot sibling. Each level is read as a
//!   `(Box<RawValue>, Box<RawValue>, i64)`: price, size, order count. Size
//!   here is contracts, not base-asset quantity — the same caveat `bars`
//!   documents for `vol` on this host — but a book panel shows resting
//!   depth in the venue's own unit regardless, exactly as `size` on every
//!   other book source in this workspace is whatever unit the venue quotes
//!   it in.
//! - `limit` controls depth per side here too: 5 requested, 5 returned.
//! - Levels arrived best-first already (asks ascending from `77594.2`,
//!   bids descending from `77594.1`); sorted explicitly regardless.
//! - `timestamp` is a bare JSON number of epoch milliseconds.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! Neither endpoint's own maximum `limit` was probed above 5.
//! [`MAX_DEPTH`] is this project's own panel choice on both markets, not a
//! venue-documented ceiling. An empty book was not observed live on
//! either endpoint; an empty `Vec` on a missing `bids`/`asks` field is this
//! source's own defensive default.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, exact_common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::{FUTURES_ID, SPOT_ID};

const SPOT_DEPTH_URL: &str = "https://api.mexc.com/api/v3/depth";
const FUTURES_DEPTH_URL: &str = "https://contract.mexc.com/api/v1/contract/depth";

/// This project's own fixed panel depth on both markets — a product
/// choice, not a venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call, matching every other book source in this workspace.
const BOOK_FETCH_COST: u32 = 5;

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

fn best_first_asks(a: &BookLevel, b: &BookLevel) -> std::cmp::Ordering {
    a.price.cmp(&b.price)
}

fn best_first_bids(a: &BookLevel, b: &BookLevel) -> std::cmp::Ordering {
    b.price.cmp(&a.price)
}

// ---------------------------------------------------------------------
// Spot
// ---------------------------------------------------------------------

/// One spot level: `[price, size]`, both decimal strings.
type RawSpotLevel = (String, String);

#[derive(Debug, Deserialize)]
struct SpotDepth {
    #[serde(default)]
    bids: Vec<RawSpotLevel>,
    #[serde(default)]
    asks: Vec<RawSpotLevel>,
    timestamp: i64,
}

fn sorted_spot_side(
    raw: Vec<RawSpotLevel>,
    depth: usize,
    best_first: impl Fn(&BookLevel, &BookLevel) -> std::cmp::Ordering,
) -> Result<(Vec<BookLevel>, u8, u8), SourceError> {
    let price_scale =
        exact_common_scale(raw.iter().map(|level| level.0.as_str())).ok_or_else(|| {
            SourceError::decode("book prices reported finer than a scaled i64 can hold")
        })?;
    let qty_scale =
        exact_common_scale(raw.iter().map(|level| level.1.as_str())).ok_or_else(|| {
            SourceError::decode("book sizes reported finer than a scaled i64 can hold")
        })?;
    let mut levels = raw
        .into_iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: scaled(&price, price_scale)?,
                size: scaled(&size, qty_scale)?,
            })
        })
        .collect::<Result<Vec<_>, SourceError>>()?;
    levels.sort_by(best_first);
    levels.truncate(depth);
    Ok((levels, price_scale, qty_scale))
}

/// MEXC spot order-book depth, fetched fresh through a [`VenueClient`] on
/// every call.
#[derive(Debug, Clone)]
pub(crate) struct MexcSpotBookSource {
    url: String,
    client: VenueClient,
}

impl MexcSpotBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`MexcSpotBookSource`] against the real MEXC spot endpoint.
#[must_use]
pub(crate) fn spot_book_source(client: VenueClient) -> MexcSpotBookSource {
    MexcSpotBookSource {
        url: SPOT_DEPTH_URL.to_owned(),
        client,
    }
}

#[async_trait]
impl BookSource for MexcSpotBookSource {
    fn source_id(&self) -> &str {
        SPOT_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}&limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let raw: SpotDepth = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let ts = UnixNanos::from_millis(raw.timestamp).ok_or_else(|| {
            SourceError::decode(format!("book timestamp {} overflowed", raw.timestamp))
        })?;

        let (bids, bid_price_scale, bid_qty_scale) =
            sorted_spot_side(raw.bids, depth, best_first_bids)?;
        let (asks, ask_price_scale, ask_qty_scale) =
            sorted_spot_side(raw.asks, depth, best_first_asks)?;

        BookSnapshot::new(
            ts,
            bids,
            bid_price_scale,
            bid_qty_scale,
            asks,
            ask_price_scale,
            ask_qty_scale,
        )
        .map_err(|source| SourceError::rejected(source.to_string()))
    }
}

// ---------------------------------------------------------------------
// Futures
// ---------------------------------------------------------------------

/// One futures level: `[price, size, order_count]`, every field a bare
/// JSON number — see the module docs for why price and size are read as
/// [`Box<RawValue>`] rather than a typed number.
type RawFuturesLevel = (Box<RawValue>, Box<RawValue>, i64);

#[derive(Debug, Deserialize)]
struct FuturesDepthData {
    #[serde(default)]
    bids: Vec<RawFuturesLevel>,
    #[serde(default)]
    asks: Vec<RawFuturesLevel>,
    timestamp: i64,
}

#[derive(Debug, Deserialize)]
struct FuturesDepthEnvelope {
    success: bool,
    code: i64,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<FuturesDepthData>,
}

fn sorted_futures_side(
    raw: Vec<RawFuturesLevel>,
    depth: usize,
    best_first: impl Fn(&BookLevel, &BookLevel) -> std::cmp::Ordering,
) -> Result<(Vec<BookLevel>, u8, u8), SourceError> {
    let price_scale =
        exact_common_scale(raw.iter().map(|level| level.0.get())).ok_or_else(|| {
            SourceError::decode("book prices reported finer than a scaled i64 can hold")
        })?;
    let qty_scale = exact_common_scale(raw.iter().map(|level| level.1.get())).ok_or_else(|| {
        SourceError::decode("book sizes reported finer than a scaled i64 can hold")
    })?;
    let mut levels = raw
        .into_iter()
        .map(|(price, size, _order_count)| {
            Ok(BookLevel {
                price: scaled(price.get(), price_scale)?,
                size: scaled(size.get(), qty_scale)?,
            })
        })
        .collect::<Result<Vec<_>, SourceError>>()?;
    levels.sort_by(best_first);
    levels.truncate(depth);
    Ok((levels, price_scale, qty_scale))
}

/// MEXC futures order-book depth, fetched fresh through a [`VenueClient`]
/// on every call.
#[derive(Debug, Clone)]
pub(crate) struct MexcFuturesBookSource {
    url: String,
    client: VenueClient,
}

impl MexcFuturesBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    /// The symbol is appended as a path segment, so `url` is the endpoint
    /// *without* the trailing `/{symbol}`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`MexcFuturesBookSource`] against the real MEXC futures
/// endpoint.
#[must_use]
pub(crate) fn futures_book_source(client: VenueClient) -> MexcFuturesBookSource {
    MexcFuturesBookSource {
        url: FUTURES_DEPTH_URL.to_owned(),
        client,
    }
}

#[async_trait]
impl BookSource for MexcFuturesBookSource {
    fn source_id(&self) -> &str {
        FUTURES_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}/{}?limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: FuturesDepthEnvelope =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.success {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code,
                envelope.message.as_deref().unwrap_or("no message")
            )));
        }
        let data = envelope
            .data
            .ok_or_else(|| SourceError::rejected("no book returned for this instrument"))?;

        let ts = UnixNanos::from_millis(data.timestamp).ok_or_else(|| {
            SourceError::decode(format!("book timestamp {} overflowed", data.timestamp))
        })?;

        let (bids, bid_price_scale, bid_qty_scale) =
            sorted_futures_side(data.bids, depth, best_first_bids)?;
        let (asks, ask_price_scale, ask_qty_scale) =
            sorted_futures_side(data.asks, depth, best_first_asks)?;

        BookSnapshot::new(
            ts,
            bids,
            bid_price_scale,
            bid_qty_scale,
            asks,
            ask_price_scale,
            ask_qty_scale,
        )
        .map_err(|source| SourceError::rejected(source.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::source::SourceError;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{futures_book_source, spot_book_source};

    const SPOT_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_spot.json");
    const FUTURES_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_futures.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTCUSDT")
    }

    fn btc_usdt_underscore() -> SourceSymbol {
        SourceSymbol::assume("BTC_USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_spot(body: &'static [u8]) -> (MockServer, super::MexcSpotBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source = spot_book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    async fn mock_futures(body: &'static [u8]) -> (MockServer, super::MexcFuturesBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source = futures_book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn spot_fixture_levels_decode_best_price_first_at_the_venues_own_scale() {
        let (_server, source) = mock_spot(SPOT_BOOK).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77616.42\" has two fractional digits"
        );
        assert_eq!(snapshot.bids[0].price, 7_761_642, "best bid 77616.42");
        assert_eq!(snapshot.asks[0].price, 7_761_699, "best ask 77616.99");
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_467_067);
    }

    #[tokio::test]
    async fn spot_bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        // `.05` rather than a whole number: `decimal_places` trims trailing
        // zeros, so a batch of e.g. `"98.00"` would imply scale **0**, not
        // 2 — these carry a genuine fractional digit so the scale this
        // batch implies is unambiguous.
        let scrambled = br#"{"bids":[["98.05","1"],["99.05","1"],["97.05","1"]],
            "asks":[["101.05","1"],["100.05","1"],["102.05","1"]],"timestamp":1000}"#;
        let (_server, source) = mock_spot(scrambled).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![9_905, 9_805, 9_705]
        );
        assert_eq!(
            snapshot.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![10_005, 10_105, 10_205]
        );
    }

    #[tokio::test]
    async fn spot_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_spot(SPOT_BOOK).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn spot_empty_book_is_an_absence_not_an_error() {
        let empty = br#"{"bids":[],"asks":[],"timestamp":1000}"#;
        let (_server, source) = mock_spot(empty).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();
        assert!(snapshot.asks.is_empty());
        assert!(snapshot.bids.is_empty());
    }

    #[tokio::test]
    async fn futures_fixture_levels_decode_from_bare_numbers_not_strings() {
        let (_server, source) = mock_futures(FUTURES_BOOK).await;
        let snapshot = source
            .book_snapshot(&btc_usdt_underscore(), 5)
            .await
            .unwrap();

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 1,
            "\"77594.2\" has one fractional digit"
        );
        assert_eq!(snapshot.asks[0].price, 775_942, "best ask 77594.2");
        assert_eq!(snapshot.bids[0].price, 775_941, "best bid 77594.1");
        assert_eq!(
            snapshot.asks[0].size, 451_965,
            "bare integer contract count"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_467_297);
    }

    #[tokio::test]
    async fn futures_bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        let scrambled = br#"{"success":true,"code":0,"data":{"cts":null,
            "bids":[[98.5,1,1],[99.5,1,1],[97.5,1,1]],
            "asks":[[101.5,1,1],[100.5,1,1],[102.5,1,1]],
            "version":1,"timestamp":1000}}"#;
        let (_server, source) = mock_futures(scrambled).await;
        let snapshot = source
            .book_snapshot(&btc_usdt_underscore(), 5)
            .await
            .unwrap();

        assert_eq!(
            snapshot.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![995, 985, 975],
            "bids must come back descending regardless of row order"
        );
        assert_eq!(
            snapshot.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![1005, 1015, 1025],
            "asks must come back ascending regardless of row order"
        );
    }

    #[tokio::test]
    async fn futures_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_futures(FUTURES_BOOK).await;
        let snapshot = source
            .book_snapshot(&btc_usdt_underscore(), 500)
            .await
            .unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn futures_empty_book_is_an_absence_not_an_error() {
        let empty = br#"{"success":true,"code":0,"data":{"cts":null,"bids":[],"asks":[],
            "version":1,"timestamp":1000}}"#;
        let (_server, source) = mock_futures(empty).await;
        let snapshot = source
            .book_snapshot(&btc_usdt_underscore(), 5)
            .await
            .unwrap();
        assert!(snapshot.asks.is_empty());
        assert!(snapshot.bids.is_empty());
    }

    #[tokio::test]
    async fn futures_an_unsuccessful_document_is_a_rejection() {
        let rejected = br#"{"success":false,"code":600,"message":"Parameter error"}"#;
        let (_server, source) = mock_futures(rejected).await;

        let error = source
            .book_snapshot(&btc_usdt_underscore(), 5)
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
