//! WhiteBIT order-book depth — `GET /api/v4/public/orderbook/{market}`.
//!
//! Unlike `bars` (whose klines are only live on the legacy v1 host), depth
//! is on v4 — the same generation as this crate's own instrument catalog.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://whitebit.com/api/v4/public/orderbook/BTC_USDT?limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"ticker_id":"BTC_USDT","timestamp":1788332468,
//! "asks":[["77631.31","0.013532"],["77640.02","0.004989"],
//! ["77641.26","0.011136"],["77641.83","0.033957"],
//! ["77643.64","0.056737"]],
//! "bids":[["77631.3","0.013532"],["77623.7","0.0083"],
//! ["77622.59","0.012321"],["77621.89","0.03533"],
//! ["77620.07","0.068997"]]}
//! ```
//!
//! Confirmed from this capture:
//! - No envelope and no error field at all on a successful response — a
//!   rejected request is signalled only by HTTP status, which
//!   [`VenueClient`] already turns into a [`SourceError`] before this
//!   source sees a body.
//! - Each level is a two-element decimal-string array: price, size — the
//!   same shape `bars` reads on this venue's legacy host.
//! - `limit` controls depth per side: 5 requested, 5 returned on both
//!   `asks` and `bids`.
//! - Levels arrived best-first already (asks ascending from `77631.31`,
//!   bids descending from `77631.3`), but this source sorts explicitly
//!   anyway — one capture is not a guarantee.
//! - `timestamp` is a bare JSON number of epoch **seconds** — unlike
//!   every other book source in this workspace, which reports
//!   milliseconds.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! This endpoint's own maximum `limit` was not probed above 5.
//! [`MAX_DEPTH`] is this project's own panel choice, not a
//! venue-documented ceiling. An empty book was not observed live;
//! `asks`/`bids` defaulting to an empty `Vec` on a missing field is this
//! source's own defensive default.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, exact_common_scale};
use serde::Deserialize;

const ORDERBOOK_URL: &str = "https://whitebit.com/api/v4/public/orderbook";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call, matching every other book source in this workspace.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size]`, both decimal strings.
type RawLevel = (String, String);

#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
    timestamp: i64,
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

/// WhiteBIT order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct WhitebitBookSource {
    url: String,
    client: VenueClient,
}

impl WhitebitBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    /// The market is appended as a path segment, so `url` is the endpoint
    /// *without* the trailing `/{market}`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`WhitebitBookSource`] against the real WhiteBIT endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> WhitebitBookSource {
    WhitebitBookSource {
        url: ORDERBOOK_URL.to_owned(),
        client,
    }
}

#[async_trait]
impl BookSource for WhitebitBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}/{}?limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let book: RawBook = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let ts = UnixNanos::from_secs(book.timestamp).ok_or_else(|| {
            SourceError::decode(format!("book timestamp {}s overflowed", book.timestamp))
        })?;

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

    async fn mock_source(body: &'static [u8]) -> (MockServer, super::WhitebitBookSource) {
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

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77631.31\" has two fractional digits"
        );
        assert_eq!(snapshot.asks[0].price, 7_763_131, "best ask 77631.31");
        assert_eq!(snapshot.bids[0].price, 7_763_130, "best bid 77631.3");
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_468_000);
    }

    #[tokio::test]
    async fn bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        // `.05` rather than a whole number: `decimal_places` trims trailing
        // zeros, so a batch of e.g. `"101.00"` would imply scale **0**, not
        // 2 — these carry a genuine fractional digit so the scale this
        // batch implies is unambiguous.
        let scrambled = br#"{"ticker_id":"BTC_USDT","timestamp":1000,
            "asks":[["101.05","1"],["100.05","1"],["102.05","1"]],
            "bids":[["98.05","1"],["99.05","1"],["97.05","1"]]}"#;
        let (_server, source) = mock_source(scrambled).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(
            snapshot.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![10_005, 10_105, 10_205]
        );
        assert_eq!(
            snapshot.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![9_905, 9_805, 9_705]
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
        let empty = br#"{"ticker_id":"BTC_USDT","timestamp":1000,"asks":[],"bids":[]}"#;
        let (_server, source) = mock_source(empty).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();
        assert!(snapshot.asks.is_empty());
        assert!(snapshot.bids.is_empty());
    }

    #[tokio::test]
    async fn an_http_error_status_is_reported_since_this_endpoint_has_no_in_body_error_shape() {
        // This endpoint's only observed rejection signal is HTTP status —
        // see the module docs — so this is the venue-error test for this
        // source, in place of an in-200-body error code no live capture
        // ever showed this endpoint sending.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(404).set_body_string("market not found"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(error, SourceError::Http { status: 404, .. }));
    }
}
