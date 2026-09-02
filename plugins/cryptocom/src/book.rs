//! Crypto.com Exchange order-book depth — `GET
//! /exchange/v1/public/get-book`.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.crypto.com/exchange/v1/public/get-book?instrument_name=BTC_USDT&depth=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"id":-1,"method":"public/get-book","code":0,
//! "result":{"depth":5,"data":[{
//! "bids":[["77626.21","0.00139","2"],["77624.62","0.06437","2"],
//! ["77624.61","0.11000","2"],["77624.58","0.01933","1"],
//! ["77622.48","0.03284","1"]],
//! "asks":[["77626.22","0.02085","3"],["77628.34","0.00168","1"],
//! ["77628.69","0.00644","1"],["77631.90","0.00005","1"],
//! ["77631.91","0.03211","1"]],
//! "t":1788332469151}],"instrument_name":"BTC_USDT"}}
//! ```
//!
//! Confirmed from this capture:
//! - `code` is `0` on success, the same envelope [`crate::bars`] already
//!   reads for this venue; `result.data` is a one-element array, the same
//!   "one document per request" shape `bars`' own `get-candlestick`
//!   response uses.
//! - Each level is a **three**-element decimal-string array: price, size,
//!   and the number of orders resting at that price. Only the first two
//!   are read.
//! - `depth` in the query string controls how many levels come back per
//!   side, and is also echoed back in `result.depth`: requesting 5
//!   returned exactly 5 on both `asks` and `bids`.
//! - Levels arrived best-first already (bids descending from `77626.21`,
//!   asks ascending from `77626.22`), but this source sorts explicitly
//!   anyway — one capture is not a guarantee.
//! - `t` is a bare JSON number of epoch milliseconds, the per-snapshot
//!   report time.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! This endpoint's own maximum `depth` was not probed above 5.
//! [`MAX_DEPTH`] is this project's own panel choice, not a
//! venue-documented ceiling. An empty book was not observed live;
//! `bids`/`asks` defaulting to an empty `Vec` on a missing field is this
//! source's own defensive default.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, exact_common_scale};
use serde::Deserialize;

const BOOK_URL: &str = "https://api.crypto.com/exchange/v1/public/get-book";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call, matching every other book source in this workspace.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size, order_count]`, every field a decimal string.
/// Only `price` and `size` are read.
type RawLevel = (String, String, String);

#[derive(Debug, Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    result: BookResult,
}

#[derive(Debug, Default, Deserialize)]
struct BookResult {
    #[serde(default)]
    data: Vec<RawBook>,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
    t: i64,
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

fn sorted_side(
    raw: Vec<RawLevel>,
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
        .map(|(price, size, _order_count)| {
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

/// Crypto.com order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book. Registered under
/// [`crate::SOURCE_ID`], the one document this venue answers depth for,
/// covering spot, perpetual and dated instruments alike by
/// `instrument_name` — the same reach `bars` documents for candlesticks.
#[derive(Debug, Clone)]
pub(crate) struct CryptocomBookSource {
    url: String,
    client: VenueClient,
}

impl CryptocomBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`CryptocomBookSource`] against the real Crypto.com endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> CryptocomBookSource {
    CryptocomBookSource {
        url: BOOK_URL.to_owned(),
        client,
    }
}

#[async_trait]
impl BookSource for CryptocomBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!(
            "{}?instrument_name={}&depth={depth}",
            self.url,
            symbol.as_str()
        );
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if envelope.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.message
            )));
        }
        let book = envelope
            .result
            .data
            .into_iter()
            .next()
            .ok_or_else(|| SourceError::rejected("no book returned for this instrument"))?;

        let ts = UnixNanos::from_millis(book.t)
            .ok_or_else(|| SourceError::decode(format!("book t {} overflowed", book.t)))?;

        let (bids, bid_price_scale, bid_qty_scale) =
            sorted_side(book.bids, depth, |a, b| b.price.cmp(&a.price))?;
        let (asks, ask_price_scale, ask_qty_scale) =
            sorted_side(book.asks, depth, |a, b| a.price.cmp(&b.price))?;

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

    use super::book_source;

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTC_USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source(body: &'static [u8]) -> (MockServer, super::CryptocomBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_venues_own_scale() {
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77626.21\" has two fractional digits"
        );
        assert_eq!(snapshot.bids[0].price, 7_762_621, "best bid 77626.21");
        assert_eq!(snapshot.asks[0].price, 7_762_622, "best ask 77626.22");
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_469_151);
    }

    #[tokio::test]
    async fn bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        // `.05` rather than a whole number: `decimal_places` trims trailing
        // zeros, so a batch of e.g. `"98.00"` would imply scale **0**, not
        // 2 — these carry a genuine fractional digit so the scale this
        // batch implies is unambiguous.
        let scrambled = br#"{"code":0,"result":{"depth":5,"data":[{
            "bids":[["98.05","1","1"],["99.05","1","1"],["97.05","1","1"]],
            "asks":[["101.05","1","1"],["100.05","1","1"],["102.05","1","1"]],
            "t":1000}],"instrument_name":"BTC_USDT"}}"#;
        let (_server, source) = mock_source(scrambled).await;
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
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let empty = br#"{"code":0,"result":{"depth":5,"data":[{"bids":[],"asks":[],
            "t":1000}],"instrument_name":"BTC_USDT"}}"#;
        let (_server, source) = mock_source(empty).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();
        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_nonzero_code_is_a_rejection() {
        let rejected = br#"{"code":10004,"message":"BAD_REQUEST","result":{"depth":0,"data":[]}}"#;
        let (_server, source) = mock_source(rejected).await;

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { reason } if reason.contains("10004")));
    }
}
