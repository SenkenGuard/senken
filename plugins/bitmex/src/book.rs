//! BitMEX order-book depth — `GET /api/v1/orderBook/L2`.
//!
//! A fixed-depth snapshot fetched fresh on request, the same shape
//! `senken_subscription::BookSource` exists to serve for every venue in
//! this workspace — never a book maintained locally from deltas.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://www.bitmex.com/api/v1/orderBook/L2?symbol=XBTUSD&depth=5`
//! returned `HTTP 200`:
//!
//! ```json
//! [{"symbol":"XBTUSD","id":234027002072,"side":"Sell","size":97300,
//!   "price":77611.0,"pool":"Aggregated","timestamp":"2026-09-02T07:00:52.301Z",
//!   "transactTime":"2026-09-02T07:00:52.230Z"},
//!  ... four more "Sell" rows ...,
//!  {"symbol":"XBTUSD","id":234027001780,"side":"Buy","size":519600,
//!   "price":77556.2, ...},
//!  ... four more "Buy" rows ...]
//! ```
//!
//! Confirmed from this capture:
//! - **The response is one flat array of rows, not separate `bids`/`asks`
//!   arrays.** Each row carries its own `side`, `"Buy"` or `"Sell"` — this
//!   source splits on that field rather than assuming the two sides stay
//!   contiguous in the array (they did here, five `Sell` rows followed by
//!   five `Buy` rows, but nothing about the response says that is a
//!   promise).
//! - **`depth=5` was honoured exactly**: five `Sell` rows and five `Buy`
//!   rows came back, not the whole book — unlike Bitstamp, Coinbase and
//!   Gemini's book endpoints in this same batch, all three of which
//!   ignore their own closest thing to a depth parameter.
//! - **Neither side arrived best-price-first.** The `Sell` rows came back
//!   `77611.0, 77587.9, 77585.6, 77584.1, 77576.3` — descending, i.e. the
//!   *worst* ask first and the best (lowest) ask last. The `Buy` rows
//!   happened to already be descending (`77556.2` down to `77523.7`),
//!   which is best-first for a bid side, but that is this capture's own
//!   coincidence, not something to trust: both sides are re-sorted here
//!   regardless.
//! - **`price` arrives as a bare JSON number**, `77611.0` — not a string.
//!   This project never routes a price through `f64`, not even
//!   transiently, so it is read as [`RawValue`] and handed to
//!   [`senken_core::parse_scaled`] as the venue's own exact digits.
//!   `size` is also read as [`RawValue`] for the same reason, though every
//!   value observed here was a whole number of contracts.
//! - **The response carries no top-level timestamp**, but every row's own
//!   `timestamp` field agreed exactly (`2026-09-02T07:00:52.301Z` on all
//!   ten). This source stamps the snapshot from the newest row's
//!   `timestamp` rather than assuming they always agree.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **Whether `depth` is clamped to a maximum** was not tested — only 5
//!   was requested. [`MAX_DEPTH`] is this project's own conservative
//!   product choice, not a venue-documented ceiling.
//! - **An empty book** (an instrument with no resting orders) was not
//!   observed live, and would carry no row to read a timestamp from at
//!   all; [`BitmexBookSource::book_snapshot`] falls back to a [`Clock`]
//!   in that case.
//! - **The application-level error shape.** BitMEX's REST API is
//!   documented, across the whole surface, to answer a failure with
//!   `{"error":{"message":"...","name":"..."}}` under `HTTP 200` on some
//!   endpoints. That convention was not reproduced by this session's one
//!   request; `parse_book` recognises it defensively and reports
//!   [`SourceError::rejected`], but this is a cited, not an independently
//!   confirmed, fact.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_series::Clock;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale, iso8601_ms};
use serde::Deserialize;
use serde_json::value::RawValue;

const ORDER_BOOK_URL: &str = "https://www.bitmex.com/api/v1/orderBook/L2";

/// This project's own conservative product choice — `depth=5` was tested
/// and honoured exactly, but nothing larger was tried (see the module
/// docs).
const MAX_DEPTH: usize = 25;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other book source here.
const BOOK_FETCH_COST: u32 = 5;

/// One row: a `side`-tagged level, not a `[price, size]` pair — see the
/// module docs.
#[derive(Debug, Deserialize)]
struct RawRow {
    side: String,
    size: Box<RawValue>,
    price: Box<RawValue>,
    timestamp: String,
}

/// BitMEX's documented error shape on some endpoints — see the module
/// docs' final bullet on why this is cited, not reproduced live.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    message: String,
}

fn parse_book(body: &[u8]) -> Result<Vec<RawRow>, SourceError> {
    match serde_json::from_slice::<Vec<RawRow>>(body) {
        Ok(rows) => Ok(rows),
        Err(decode_err) => {
            if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(body) {
                return Err(SourceError::rejected(envelope.error.message));
            }
            Err(SourceError::decode(decode_err))
        }
    }
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// BitMEX order-book depth, fetched through a [`VenueClient`], stamped
/// from the newest row's own `timestamp` when the book is non-empty and
/// from a [`Clock`] otherwise — see the module docs.
#[derive(Clone)]
pub(crate) struct BitmexBookSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for BitmexBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmexBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl BitmexBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BitmexBookSource`] against the real BitMEX endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient, clock: Arc<dyn Clock>) -> BitmexBookSource {
    BitmexBookSource {
        url: ORDER_BOOK_URL.to_owned(),
        client,
        clock,
    }
}

#[async_trait]
impl BookSource for BitmexBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}&depth={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let rows = parse_book(&body)?;

        let price_scale = common_scale(rows.iter().map(|row| row.price.get()));
        let qty_scale = common_scale(rows.iter().map(|row| row.size.get()));

        let mut newest: Option<UnixNanos> = None;
        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for row in &rows {
            let Some(ms) = iso8601_ms(&row.timestamp) else {
                return Err(SourceError::decode(format!(
                    "{:?} is not a valid timestamp",
                    row.timestamp
                )));
            };
            let ts = UnixNanos::from_millis(ms)
                .ok_or_else(|| SourceError::decode(format!("row timestamp {ms}ms overflowed")))?;
            newest = Some(newest.map_or(ts, |current| current.max(ts)));

            let level = BookLevel {
                price: scaled(row.price.get(), price_scale)?,
                size: scaled(row.size.get(), qty_scale)?,
            };
            match row.side.as_str() {
                "Buy" => bids.push(level),
                "Sell" => asks.push(level),
                other => {
                    return Err(SourceError::decode(format!("unknown book side {other:?}")));
                }
            }
        }

        // Best price first on both sides — see the module docs on why
        // neither side's own order is trusted.
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        asks.sort_by_key(|level| level.price);
        bids.truncate(depth);
        asks.truncate(depth);

        // An empty book carries no row to read a timestamp from at all —
        // see the module docs.
        let ts = newest.unwrap_or_else(|| self.clock.now());

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
    use std::sync::Arc;

    use senken_core::UnixNanos;
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::source::SourceError;
    use senken_series::Clock;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::book_source;

    /// A real `GET /api/v1/orderBook/L2?symbol=XBTUSD&depth=5` response,
    /// recorded 2026-09-02T07:00:52Z: five `Sell` rows then five `Buy`
    /// rows, neither side sorted best-first (see the module docs).
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("XBTUSD")
    }

    #[derive(Debug)]
    struct FixedClock(i64);

    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_millis(self.0).unwrap()
        }

        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    async fn mock_source() -> (MockServer, super::BitmexBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn rows_are_split_by_the_side_field_at_the_venues_own_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.price_scale, 1, "77611.0 has one decimal");
        assert_eq!(
            snapshot.qty_scale, 0,
            "every observed size is a whole number"
        );
    }

    #[tokio::test]
    async fn neither_side_is_trusted_to_arrive_best_first() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 5).await.unwrap();

        // Best ask is the lowest Sell price, 77576.3 — last in the venue's
        // own descending order, first here.
        assert_eq!(snapshot.asks[0].price, 775_763);
        assert!(snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price));
        // Best bid is the highest Buy price, 77556.2.
        assert_eq!(snapshot.bids[0].price, 775_562);
        assert!(snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price));
    }

    #[tokio::test]
    async fn the_snapshot_is_stamped_from_the_rows_own_timestamp() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 5).await.unwrap();
        assert_eq!(
            snapshot.ts,
            UnixNanos::from_millis(senken_venue::iso8601_ms("2026-09-02T07:00:52.301Z").unwrap())
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_falls_back_to_the_clock_for_its_timestamp() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(&b"[]"[..], "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(1_788_332_500_000)))
            .with_url(server.uri());

        let snapshot = source.book_snapshot(&symbol(), 5).await.unwrap();
        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
        assert_eq!(
            snapshot.ts,
            UnixNanos::from_millis(1_788_332_500_000).unwrap()
        );
    }

    #[tokio::test]
    async fn the_documented_error_shape_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"error":{"message":"Invalid symbol","name":"HTTPError"}}"#,
                ),
            )
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let error = source.book_snapshot(&symbol(), 5).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
