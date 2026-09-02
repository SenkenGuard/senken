//! Upbit order-book depth — `GET /v1/orderbook`.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.upbit.com/v1/orderbook?markets=KRW-BTC` returned
//! `HTTP 200`:
//!
//! ```json
//! [{"market":"KRW-BTC","timestamp":1788332467626,
//! "total_ask_size":2.40192334,"total_bid_size":5.88234340,
//! "orderbook_units":[{"bid_price":106747000,"bid_size":0.01071562,
//! "ask_price":106749000,"ask_size":0.02507658}, … 30 entries …],
//! "level":0}]
//! ```
//!
//! Confirmed from this capture:
//! - The top level is an array, one object per requested `markets` — the
//!   same shape `bars`' own docs record for candles, generalised to one
//!   request supporting several markets at once (this source only ever
//!   asks for one).
//! - **Both sides live in one array.** `orderbook_units` carries
//!   `bid_price`/`bid_size`/`ask_price`/`ask_size` side by side in a
//!   single row, not two separate arrays the way every other venue in this
//!   workspace reports a book — this source splits them.
//! - Every price and size field is a **bare JSON number**, exactly like
//!   `bars`' own candle fields, and read the same way: as a
//!   [`Box<RawValue>`] so [`parse_scaled`] sees the venue's exact digits,
//!   never an `f64`.
//! - **There is no depth request parameter at all.** No `count`, `limit`,
//!   or `level` in the query string changed anything — the endpoint
//!   answered all 30 rows regardless (`level` in the response is a
//!   price-bucket grouping control, not a row count, and was left at its
//!   default here). This source therefore always fetches the full ladder
//!   and caps it to the requested depth **client-side**, unlike every
//!   other venue in this workspace where the venue itself enforces the
//!   cap.
//! - Rows arrived already best-price-first on each side (bid descending
//!   from `106747000`, ask ascending from `106749000`), but this source
//!   sorts explicitly anyway — one capture is not a guarantee.
//! - `timestamp` is a bare JSON number of epoch milliseconds — the
//!   snapshot's own report time, not to be confused with `bars`' own
//!   `candle_date_time_utc`, which this endpoint does not have.
//!
//! # `market` is already venue-native
//!
//! Exactly like `bars`, Upbit's own `KRW-BTC` identifier is what this
//! crate's instrument catalog stores as
//! [`Instrument::source_symbol`](senken_marketdata::Instrument::source_symbol),
//! so it is passed straight through with no reversal at this layer — see
//! this crate's own module docs on why the pair is written backwards.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! An empty book was not observed live (`BTC` on Upbit is never quiet);
//! `orderbook_units` defaulting to an empty `Vec` on a missing field is
//! this source's own defensive default. [`MAX_DEPTH`] is this project's
//! own panel choice, not a venue-documented ceiling — the live capture
//! answered fewer than that (30 rows split means 30 levels are simply what
//! the venue always sends).

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, exact_common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

const ORDERBOOK_URL: &str = "https://api.upbit.com/v1/orderbook";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs). Applied client-side: this
/// endpoint takes no depth parameter at all.
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call, matching every other book source in this workspace.
const BOOK_FETCH_COST: u32 = 5;

/// One row of `orderbook_units`: both sides side by side, not two arrays —
/// see the module docs.
#[derive(Debug, Deserialize)]
struct RawUnit {
    bid_price: Box<RawValue>,
    bid_size: Box<RawValue>,
    ask_price: Box<RawValue>,
    ask_size: Box<RawValue>,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    timestamp: i64,
    #[serde(default)]
    orderbook_units: Vec<RawUnit>,
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// Upbit order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct UpbitBookSource {
    url: String,
    client: VenueClient,
}

impl UpbitBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds an [`UpbitBookSource`] against the real Upbit endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> UpbitBookSource {
    UpbitBookSource {
        url: ORDERBOOK_URL.to_owned(),
        client,
    }
}

#[async_trait]
impl BookSource for UpbitBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        // No depth parameter exists on this endpoint — see the module
        // docs — so the full ladder is always requested and capped below.
        let url = format!("{}?markets={}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let books: Vec<RawBook> = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        let book = books
            .into_iter()
            .next()
            .ok_or_else(|| SourceError::rejected("no book returned for this instrument"))?;

        let ts = UnixNanos::from_millis(book.timestamp).ok_or_else(|| {
            SourceError::decode(format!("book timestamp {} overflowed", book.timestamp))
        })?;

        let price_scale = exact_common_scale(
            book.orderbook_units
                .iter()
                .flat_map(|unit| [unit.bid_price.get(), unit.ask_price.get()]),
        )
        .ok_or_else(|| {
            SourceError::decode("book prices reported finer than a scaled i64 can hold")
        })?;
        let qty_scale = exact_common_scale(
            book.orderbook_units
                .iter()
                .flat_map(|unit| [unit.bid_size.get(), unit.ask_size.get()]),
        )
        .ok_or_else(|| {
            SourceError::decode("book sizes reported finer than a scaled i64 can hold")
        })?;

        let mut bids = Vec::with_capacity(book.orderbook_units.len());
        let mut asks = Vec::with_capacity(book.orderbook_units.len());
        for unit in &book.orderbook_units {
            bids.push(BookLevel {
                price: scaled(unit.bid_price.get(), price_scale)?,
                size: scaled(unit.bid_size.get(), qty_scale)?,
            });
            asks.push(BookLevel {
                price: scaled(unit.ask_price.get(), price_scale)?,
                size: scaled(unit.ask_size.get(), qty_scale)?,
            });
        }

        // Sorted explicitly rather than trusted from the venue — one
        // capture happening to already be ordered is not a guarantee — and
        // capped to `depth` only after sorting, so the levels kept are
        // always the best ones, whatever order the venue actually sent.
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        bids.truncate(depth);
        asks.sort_by_key(|level| level.price);
        asks.truncate(depth);

        BookSnapshot::new(
            ts,
            bids,
            price_scale,
            qty_scale,
            asks,
            price_scale,
            qty_scale,
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

    fn krw_btc() -> SourceSymbol {
        SourceSymbol::assume("KRW-BTC")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source(body: &'static [u8]) -> (MockServer, super::UpbitBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_units_split_into_two_sides_and_decode_at_the_venues_own_scale() {
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&krw_btc(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5, "clamped to the requested depth");
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(
            snapshot.price_scale, 0,
            "won prices carry no fractional digits"
        );
        assert_eq!(
            snapshot.bids[0].price, 106_747_000,
            "best bid from bid_price"
        );
        assert_eq!(
            snapshot.asks[0].price, 106_749_000,
            "best ask from ask_price"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_467_626);
    }

    #[tokio::test]
    async fn bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        let scrambled = br#"[{"market":"KRW-BTC","timestamp":1000,
            "total_ask_size":3,"total_bid_size":3,"orderbook_units":[
            {"bid_price":98,"bid_size":1,"ask_price":101,"ask_size":1},
            {"bid_price":99,"bid_size":1,"ask_price":100,"ask_size":1},
            {"bid_price":97,"bid_size":1,"ask_price":102,"ask_size":1}
            ],"level":0}]"#;
        let (_server, source) = mock_source(scrambled).await;
        let snapshot = source.book_snapshot(&krw_btc(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![99, 98, 97],
            "bids must come back descending regardless of row order"
        );
        assert_eq!(
            snapshot.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![100, 101, 102],
            "asks must come back ascending regardless of row order"
        );
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_client_side() {
        // The fixture itself carries 30 rows a side, more than MAX_DEPTH —
        // this endpoint has no request parameter to ask for fewer, so the
        // clamp must happen after the fact.
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&krw_btc(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_full_ladder_keeps_only_the_best_levels() {
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&krw_btc(), 3).await.unwrap();
        assert_eq!(snapshot.bids.len(), 3);
        assert_eq!(snapshot.asks.len(), 3);
        assert_eq!(snapshot.bids[2].price, 106_738_000, "third-best bid kept");
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let empty = br#"[{"market":"KRW-BTC","timestamp":1000,
            "total_ask_size":0,"total_bid_size":0,"orderbook_units":[],"level":0}]"#;
        let (_server, source) = mock_source(empty).await;
        let snapshot = source.book_snapshot(&krw_btc(), 5).await.unwrap();
        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn no_market_returned_is_a_rejection() {
        let (_server, source) = mock_source(b"[]").await;
        let error = source.book_snapshot(&krw_btc(), 5).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
