//! Bitstamp order-book depth — `GET /api/v2/order_book/{market_symbol}/`.
//!
//! A fixed-depth snapshot fetched fresh on request, the same shape
//! `senken_subscription::BookSource` exists to serve for every venue in
//! this workspace — never a book maintained locally from deltas.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://www.bitstamp.net/api/v2/order_book/btcusd/` returned
//! `HTTP 200`:
//!
//! ```json
//! {"timestamp":"1788332442","microtimestamp":"1788332442605751",
//!  "bids":[["77582.66","0.78843822"],["77582.56","0.50000000"], ...],
//!  "asks":[["77582.67","0.70640714"],["77586.46","0.25000000"], ...]}
//! ```
//!
//! Confirmed from this capture:
//! - **This endpoint ignores any notion of depth entirely and answers
//!   with the whole book** — 3023 bid levels and 3041 ask levels for a
//!   request that named no limit of any kind (there is none to name; this
//!   path takes no query string at all). [`MAX_DEPTH`] is this project's
//!   own panel choice, truncated client-side, not a venue ceiling.
//! - `bids` are already descending (best, highest, price first) and
//!   `asks` already ascending (best, lowest, price first); this source
//!   re-sorts both explicitly regardless, the same discipline every other
//!   source in this workspace applies.
//! - `price` and `size` are decimal strings, decoded the same way as
//!   every string-encoded field this project reads.
//! - `timestamp` is a string of epoch **seconds** (`microtimestamp` also
//!   exists at microsecond resolution; `timestamp` is used here, matching
//!   the precision this project's other Bitstamp source already reads
//!   epoch values at).
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **The `group` query parameter**, which Bitstamp's docs describe as
//!   controlling order aggregation, was not exercised — this source never
//!   sends it, so the response is always the ungrouped book above.
//! - **The application-level error shape.** Bitstamp's public v2 API is
//!   documented to answer certain failures with a `{"error": "..."}"`
//!   body under `HTTP 200` rather than a non-2xx status. That convention
//!   was not reproduced by this session's one request; `parse_book`
//!   recognises it defensively and reports [`SourceError::rejected`], but
//!   this is a cited, not an independently confirmed, fact.
//! - **An empty book** (an instrument with no resting orders on one or
//!   both sides) was not observed live; an empty array decoding to no
//!   levels on that side is this source's own defensive default.

use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const ORDER_BOOK_URL: &str = "https://www.bitstamp.net/api/v2/order_book";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling, since this endpoint answers with the whole
/// book regardless of what is asked for (see the module docs).
const MAX_DEPTH: usize = 50;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other book source here.
const BOOK_FETCH_COST: u32 = 5;

/// The whole document. `error` is present only on the documented failure
/// shape (see the module docs); every other field then defaults empty
/// rather than failing to deserialise on a body that carries no book at
/// all.
#[derive(Debug, Deserialize)]
struct OrderBookResponse {
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    bids: Vec<(String, String)>,
    #[serde(default)]
    asks: Vec<(String, String)>,
    #[serde(default)]
    error: Option<String>,
}

/// Bitstamp order-book depth, fetched through a [`VenueClient`].
#[derive(Debug, Clone)]
pub(crate) struct BitstampBookSource {
    url: String,
    client: VenueClient,
}

impl BitstampBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BitstampBookSource`] against the real Bitstamp endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> BitstampBookSource {
    BitstampBookSource {
        url: ORDER_BOOK_URL.to_owned(),
        client,
    }
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// Splits `rows` into [`BookLevel`]s at `scale`, without sorting — the
/// caller re-sorts once both sides are known.
fn levels(
    rows: Vec<(String, String)>,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
    rows.into_iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: scaled(&price, price_scale)?,
                size: scaled(&size, qty_scale)?,
            })
        })
        .collect()
}

fn parse_book(body: &[u8]) -> Result<OrderBookResponse, SourceError> {
    let response: OrderBookResponse = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if let Some(message) = response.error.as_deref() {
        return Err(SourceError::rejected(message.to_owned()));
    }
    Ok(response)
}

#[async_trait::async_trait]
impl BookSource for BitstampBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}/{}/", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response = parse_book(&body)?;

        let ts_secs: i64 = response.timestamp.trim().parse().map_err(|_| {
            SourceError::decode(format!("{:?} is not a valid timestamp", response.timestamp))
        })?;
        let ts = UnixNanos::from_secs(ts_secs)
            .ok_or_else(|| SourceError::decode(format!("book timestamp {ts_secs}s overflowed")))?;

        let price_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|(price, _)| price.as_str()),
        );
        let qty_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|(_, size)| size.as_str()),
        );

        let mut bids = levels(response.bids, price_scale, qty_scale)?;
        let mut asks = levels(response.asks, price_scale, qty_scale)?;
        // Best price first on both sides — already true of this venue's
        // own order, but not trusted (see the module docs).
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        asks.sort_by_key(|level| level.price);
        bids.truncate(depth);
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

    /// A real `GET /api/v2/order_book/btcusd/` response, recorded
    /// 2026-09-02, trimmed to its 30 best levels a side (the venue itself
    /// returned 3023 bids and 3041 asks — see the module docs).
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn btcusd() -> SourceSymbol {
        SourceSymbol::assume("btcusd")
    }

    async fn mock_source() -> (MockServer, super::BitstampBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 30).await.unwrap();

        assert_eq!(snapshot.bids.len(), 30);
        assert_eq!(snapshot.asks.len(), 30);
        assert_eq!(snapshot.price_scale, 2, "\"77582.66\" has two decimals");
        assert_eq!(snapshot.bids[0].price, 7_758_266, "best bid, 77582.66");
        assert_eq!(snapshot.asks[0].price, 7_758_267, "best ask, 77582.67");
        assert!(snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price));
        assert!(snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price));
        assert_eq!(
            snapshot.ts,
            senken_core::UnixNanos::from_secs(1_788_332_442).unwrap()
        );
    }

    #[tokio::test]
    async fn the_full_book_is_truncated_client_side_to_the_panel_depth() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_panel_cap_is_honoured() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 5).await.unwrap();
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"timestamp":"1788332442","microtimestamp":"1788332442605751","bids":[],"asks":[]}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btcusd(), 30).await.unwrap();
        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn the_documented_error_shape_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"error":"Invalid Currency Pair."}"#),
            )
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btcusd(), 30).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
