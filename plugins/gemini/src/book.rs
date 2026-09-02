//! Gemini order-book depth — `GET /v1/book/{symbol}`.
//!
//! A fixed-depth snapshot fetched fresh on request, the same shape
//! `senken_subscription::BookSource` exists to serve for every venue in
//! this workspace — never a book maintained locally from deltas.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.gemini.com/v1/book/btcusd` returned `HTTP 200`:
//!
//! ```json
//! {"bids":[{"price":"77593.55","amount":"0.01780852","timestamp":"1788332493"},
//!           ...49 more...],
//!  "asks":[{"price":"77593.56","amount":"0.03382988","timestamp":"1788332493"},
//!           ...49 more...]}
//! ```
//!
//! Confirmed from this capture:
//! - **Levels are objects, not `[price, size]` pairs** — unlike every
//!   other book source in this workspace, `price`/`amount` are named
//!   fields, and each level carries its own `timestamp`.
//! - **Every level's `timestamp` was identical**: `"1788332493"`, on all
//!   50 bids and all 50 asks. This source reads the newest one across the
//!   whole response rather than assuming the first row is representative,
//!   in case that ever stops holding.
//! - `price` and `amount` are decimal strings, decoded the same way as
//!   every string-encoded field this project reads — unlike this venue's
//!   own `/v2/candles` endpoint in this same plugin, which sends bare
//!   numbers instead.
//! - `bids` are already descending (best, highest, price first) and
//!   `asks` already ascending (best, lowest, price first); this source
//!   re-sorts both explicitly regardless, the same discipline every other
//!   source in this workspace applies.
//! - No `len`, `limit_bids` or `limit_asks` parameter was sent, and the
//!   default answered with exactly 50 levels on each side.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **Whether a depth parameter exists and what it is named** was not
//!   tested; this source never sends one, so [`MAX_DEPTH`] is the
//!   observed default rather than a confirmed venue ceiling — a caller
//!   asking for more than 50 levels gets 50, not more.
//! - **The application-level error shape.** Gemini's REST API is
//!   documented, across the whole surface, to answer a failure with
//!   `{"result":"error","reason":"...","message":"..."}"` under `HTTP
//!   200`. That convention was not reproduced by this session's one
//!   request; `parse_book` recognises it defensively and reports
//!   [`SourceError::rejected`], but this is a cited, not an independently
//!   confirmed, fact.
//! - **An empty book** (an instrument with no resting orders on one or
//!   both sides) was not observed live, and would carry no level to read
//!   a timestamp from at all; [`GeminiBookSource::book_snapshot`] falls
//!   back to a [`Clock`] in that case.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_series::Clock;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const BOOK_BASE_URL: &str = "https://api.gemini.com/v1/book";

/// The depth this endpoint returns by default, with no depth parameter
/// sent — see the module docs on why nothing larger is requested.
const MAX_DEPTH: usize = 50;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other book source here.
const BOOK_FETCH_COST: u32 = 5;

/// One level: named fields, not a `[price, size]` pair — see the module
/// docs.
#[derive(Debug, Deserialize)]
struct RawLevel {
    price: String,
    amount: String,
    timestamp: String,
}

/// The whole document. `result`/`message` are present only on Gemini's
/// documented cross-endpoint error shape (see the module docs' final
/// bullet on why this is cited, not reproduced live) — `bids`/`asks` stay
/// `#[serde(default)]` so that shape still deserialises here instead of
/// failing before it can be recognised.
#[derive(Debug, Deserialize)]
struct BookResponse {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn parse_book(body: &[u8]) -> Result<BookResponse, SourceError> {
    let response: BookResponse = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.result.as_deref() == Some("error") {
        let message = response
            .message
            .unwrap_or_else(|| "gemini reported an error with no message".to_owned());
        return Err(SourceError::rejected(message));
    }
    Ok(response)
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
        .map(|row| {
            Ok(BookLevel {
                price: scaled(&row.price, price_scale)?,
                size: scaled(&row.amount, qty_scale)?,
            })
        })
        .collect()
}

/// Gemini order-book depth, fetched through a [`VenueClient`], stamped
/// from the newest level's own `timestamp` when the book is non-empty and
/// from a [`Clock`] otherwise — see the module docs.
#[derive(Clone)]
pub(crate) struct GeminiBookSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for GeminiBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl GeminiBookSource {
    /// Points this source at a different base URL — a local stand-in in
    /// tests. The `/{symbol}` path is appended at request time.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`GeminiBookSource`] against the real Gemini endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient, clock: Arc<dyn Clock>) -> GeminiBookSource {
    GeminiBookSource {
        url: BOOK_BASE_URL.to_owned(),
        client,
        clock,
    }
}

#[async_trait]
impl BookSource for GeminiBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        // Gemini's symbols are lower case in the URL, matching
        // `crate::bars`'s own treatment of the same trap.
        let url = format!("{}/{}", self.url, symbol.as_str().to_lowercase());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response = parse_book(&body)?;

        let price_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|row| row.price.as_str()),
        );
        let qty_scale = common_scale(
            response
                .bids
                .iter()
                .chain(&response.asks)
                .map(|row| row.amount.as_str()),
        );

        // The newest level's own timestamp across both sides — see the
        // module docs on why the first row is not assumed representative.
        let mut newest: Option<i64> = None;
        for row in response.bids.iter().chain(&response.asks) {
            let secs: i64 = row.timestamp.trim().parse().map_err(|_| {
                SourceError::decode(format!("{:?} is not a valid timestamp", row.timestamp))
            })?;
            newest = Some(newest.map_or(secs, |current| current.max(secs)));
        }
        // An empty book carries no level to read a timestamp from at all —
        // see the module docs.
        let ts = match newest {
            Some(secs) => UnixNanos::from_secs(secs)
                .ok_or_else(|| SourceError::decode(format!("book timestamp {secs}s overflowed")))?,
            None => self.clock.now(),
        };

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

    /// A real `GET /v1/book/btcusd` response, recorded 2026-09-02: 50
    /// bids and 50 asks, every level sharing the same timestamp.
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("BTCUSD")
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

    async fn mock_source() -> (MockServer, super::GeminiBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn levels_decode_from_named_fields_at_the_venues_own_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 50).await.unwrap();

        assert_eq!(snapshot.bids.len(), 50);
        assert_eq!(snapshot.asks.len(), 50);
        assert_eq!(snapshot.bids[0].price, 7_759_355, "best bid, 77593.55");
        assert_eq!(snapshot.asks[0].price, 7_759_356, "best ask, 77593.56");
        assert!(snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price));
        assert!(snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price));
        assert_eq!(snapshot.ts, UnixNanos::from_secs(1_788_332_493).unwrap());
    }

    #[tokio::test]
    async fn a_depth_above_the_observed_default_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_default_is_honoured_client_side() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&symbol(), 5).await.unwrap();
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
    }

    #[tokio::test]
    async fn an_empty_book_falls_back_to_the_clock_for_its_timestamp() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"bids":[],"asks":[]}"#))
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(1_788_332_500_000)))
            .with_url(server.uri());

        let snapshot = source.book_snapshot(&symbol(), 50).await.unwrap();
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
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":"error","reason":"InvalidSymbol","message":"symbol not found"}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let error = source.book_snapshot(&symbol(), 50).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
