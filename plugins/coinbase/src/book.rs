//! Coinbase Exchange spot order-book depth —
//! `GET /products/{id}/book?level=2`.
//!
//! A fixed-depth snapshot fetched fresh on request, the same shape
//! `senken_subscription::BookSource` exists to serve for every venue in
//! this workspace — never a book maintained locally from deltas. Only the
//! Exchange spot market is registered here, mirroring
//! [`crate::bar_source_spot`]'s own spot-only scope.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.exchange.coinbase.com/products/BTC-USD/book?level=2`
//! returned `HTTP 200`:
//!
//! ```json
//! {"bids":[["77588.84","0.02001939",3],["77586.35","0.0001",1], ...],
//!  "asks":[["77588.85","0.04411574",3],["77589.4","0.00141306",1], ...],
//!  "sequence":135436302345,"auction_mode":false,"auction":null,
//!  "time":"2026-09-02T07:01:32.019204671Z"}
//! ```
//!
//! Confirmed from this capture:
//! - **`level=2` does not mean "top 50 aggregated"**: this call answered
//!   with 21572 bid levels and 22528 ask levels, the whole aggregated
//!   book, not a capped page of it. [`MAX_DEPTH`] is this project's own
//!   panel choice, truncated client-side, not a venue ceiling — and there
//!   is no separate depth parameter to send instead.
//! - Each level is `[price, size, num_orders]`; the third field (the
//!   count of individual orders aggregated into that price) is read and
//!   discarded, the same as OKX's own unused order-count field.
//! - `bids` are already descending (best, highest, price first) and
//!   `asks` already ascending (best, lowest, price first); this source
//!   re-sorts both explicitly regardless, the same discipline every other
//!   source in this workspace applies.
//! - `price` and `size` are decimal strings, decoded the same way as
//!   every string-encoded field this project reads.
//! - `time` is ISO 8601 with sub-millisecond fractional seconds; read
//!   through [`senken_venue::iso8601_ms`] the same way every other ISO
//!   8601 field in this workspace is, at millisecond precision.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **`level=1` and `level=3`** were not requested; this source always
//!   sends `level=2`.
//! - **The application-level error shape.** Coinbase's REST APIs are
//!   documented, across the whole surface, to answer a failure with
//!   `{"message": "..."}"` under `HTTP 200` on some endpoints. That
//!   convention was not reproduced by this session's one request;
//!   `parse_book` recognises it defensively and reports
//!   [`SourceError::rejected`], but this is a cited, not an independently
//!   confirmed, fact.
//! - **An empty book** (an instrument with no resting orders on one or
//!   both sides) was not observed live; an empty array decoding to no
//!   levels on that side is this source's own defensive default.

use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale, iso8601_ms};
use serde::Deserialize;

const PRODUCTS_URL: &str = "https://api.exchange.coinbase.com/products";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling, since `level=2` answers with the whole
/// aggregated book regardless of what is asked for (see the module docs).
const MAX_DEPTH: usize = 50;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other book source here.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size, num_orders]` — the third field is read and
/// discarded, see the module docs.
type RawLevel = (String, String, u32);

/// The success shape. `message` is present only on the documented failure
/// shape (see the module docs); every other field then defaults empty
/// rather than failing to deserialise on a body that carries no book at
/// all.
#[derive(Debug, Deserialize)]
struct BookResponse {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    time: String,
    #[serde(default)]
    message: Option<String>,
}

/// Coinbase Exchange spot order-book depth, fetched through a
/// [`VenueClient`].
#[derive(Debug, Clone)]
pub(crate) struct CoinbaseBookSource {
    url: String,
    client: VenueClient,
}

impl CoinbaseBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`CoinbaseBookSource`] against the real Coinbase Exchange
/// endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> CoinbaseBookSource {
    CoinbaseBookSource {
        url: PRODUCTS_URL.to_owned(),
        client,
    }
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

fn levels(
    rows: Vec<RawLevel>,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
    rows.into_iter()
        .map(|(price, size, _num_orders)| {
            Ok(BookLevel {
                price: scaled(&price, price_scale)?,
                size: scaled(&size, qty_scale)?,
            })
        })
        .collect()
}

fn parse_book(body: &[u8]) -> Result<BookResponse, SourceError> {
    let response: BookResponse = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if let Some(message) = response.message.as_deref() {
        return Err(SourceError::rejected(message.to_owned()));
    }
    Ok(response)
}

#[async_trait::async_trait]
impl BookSource for CoinbaseBookSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}/{}/book?level=2", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response = parse_book(&body)?;

        let ms = iso8601_ms(&response.time).ok_or_else(|| {
            SourceError::decode(format!("{:?} is not a valid timestamp", response.time))
        })?;
        let ts = UnixNanos::from_millis(ms)
            .ok_or_else(|| SourceError::decode(format!("book time {ms}ms overflowed")))?;

        let price_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|(price, ..)| price.as_str()),
        );
        let qty_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|(_, size, _)| size.as_str()),
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
    use senken_marketdata::source::SourceError;
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::book_source;

    /// A real `GET /products/BTC-USD/book?level=2` response, recorded
    /// 2026-09-02, trimmed to its 30 best levels a side (the venue itself
    /// returned 21572 bids and 22528 asks — see the module docs).
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn btcusd() -> SourceSymbol {
        Instrument::spot("BTCUSD", "BTC-USD", "BTC", "USD").source_symbol()
    }

    async fn mock_source() -> (MockServer, super::CoinbaseBookSource) {
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
        assert_eq!(snapshot.bids[0].price, 7_758_884, "best bid, 77588.84");
        assert_eq!(snapshot.asks[0].price, 7_758_885, "best ask, 77588.85");
        assert!(snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price));
        assert!(snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price));
    }

    #[tokio::test]
    async fn the_whole_aggregated_book_is_truncated_client_side_to_the_panel_depth() {
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
                r#"{"bids":[],"asks":[],"sequence":1,"auction_mode":false,"auction":null,"time":"2026-09-02T07:01:32.019204671Z"}"#,
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
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"message":"NotFound"}"#))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btcusd(), 30).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
