//! BitMart order-book depth — `GET /spot/quotation/v3/books`.
//!
//! Like [`crate::bars`], this needed a live replacement: the legacy
//! `/spot/v1/symbols/book` endpoint is still reachable but answers an
//! empty book for a real, actively-traded market
//! (`{"code":1000,"message":"OK","data":{"timestamp":...,"buys":[],
//! "sells":[]}}` for `BTC_USDT`), which is indistinguishable from "no
//! resting orders" unless you already know better. The v3 quotation
//! endpoint this source uses instead answered a real ladder.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api-cloud.bitmart.com/spot/quotation/v3/books?symbol=BTC_USDT&limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"data":{"ts":"1788332458019","asks":[["77655.15","1.82257"],
//! ["77655.16","0.15868"],["77655.17","0.21189"],["77655.18","0.08905"],
//! ["77655.19","0.23384"]],"bids":[["77655.12","0.40842"],
//! ["77655.11","0.14826"],["77655.10","0.14850"],["77655.09","0.16263"],
//! ["77655.08","0.16122"]],"symbol":"BTC_USDT"},
//! "trace":"8bdab713-9750-47ec-bf7b-ce2e8fc0ef68","code":1000,
//! "message":"success","success":false}
//! ```
//!
//! Confirmed from this capture:
//! - `code` is `1000` on success, the same convention `bars` and this
//!   crate's own instrument catalog already use — **not** the top-level
//!   `success` field, which reads `false` here despite the request having
//!   plainly succeeded.
//! - Each level is a two-element array: price then size, both decimal
//!   strings.
//! - `limit` in the query string controls how many levels come back per
//!   side: requesting 5 returned exactly 5 on both `asks` and `bids`.
//! - Levels arrived **best price first** on each side already (asks
//!   ascending from `77655.15`, bids descending from `77655.12`), but this
//!   source sorts explicitly anyway rather than trust a single capture —
//!   see [`sorted_side`].
//! - `ts` is a string of epoch **milliseconds** — unlike `bars`' own `ts`
//!   field on this same host, which is epoch seconds.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! - **This endpoint's own maximum `limit`** was not probed above 5.
//!   [`MAX_DEPTH`] is this project's own panel choice, the same as every
//!   other book source in this workspace, not a venue-documented ceiling.
//! - **An empty book** was not observed live for a real market (the dead
//!   v1 endpoint's empty `buys`/`sells` above is not evidence about this
//!   endpoint); `asks`/`bids` defaulting to an empty `Vec` on a missing
//!   field is this source's own defensive default.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, exact_common_scale};
use serde::Deserialize;

const BOOKS_URL: &str = "https://api-cloud.bitmart.com/spot/quotation/v3/books";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, matching
/// every other book source in this workspace.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size]`, both decimal strings.
type RawLevel = (String, String);

#[derive(Debug, Deserialize)]
struct BooksEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<RawBook>,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
    ts: String,
}

/// BitMart spot order-book depth, fetched through a [`VenueClient`] — a
/// fresh request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct BitmartBookSource {
    url: String,
    client: VenueClient,
}

impl BitmartBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BitmartBookSource`] against the real BitMart endpoint.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> BitmartBookSource {
    BitmartBookSource {
        url: BOOKS_URL.to_owned(),
        client,
    }
}

/// Parses one side's raw levels at the scale its own batch of decimal
/// strings implies, then sorts it best-price-first and caps it at `depth`.
///
/// The sort is unconditional rather than trusted from the venue: this
/// source's only live capture happened to already arrive in the right
/// order, and one capture is not a guarantee that holds under every market
/// condition.
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

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for BitmartBookSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}&limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: BooksEnvelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        // `code` is what signals success, not the top-level `success`
        // field — see the module docs.
        if envelope.code != 1000 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.message
            )));
        }
        let book = envelope
            .data
            .ok_or_else(|| SourceError::rejected("no book returned for this instrument"))?;

        let ts_ms: i64 =
            book.ts.trim().parse().map_err(|_| {
                SourceError::decode(format!("{:?} is not a valid timestamp", book.ts))
            })?;
        let ts = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| SourceError::decode(format!("book ts {ts_ms} overflowed")))?;

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

    async fn mock_source(body: &'static [u8]) -> (MockServer, super::BitmartBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(book_source(test_client()).url, super::BOOKS_URL);
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_venues_own_scale() {
        let (_server, source) = mock_source(BOOK).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77655.15\" has two fractional digits"
        );
        assert_eq!(snapshot.asks[0].price, 7_765_515, "77655.15 at scale 2");
        assert_eq!(snapshot.bids[0].price, 7_765_512, "77655.12 at scale 2");
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_458_019);
    }

    #[tokio::test]
    async fn bids_and_asks_are_sorted_best_first_even_if_the_venue_was_not() {
        // A hand-scrambled order the venue must never actually be trusted
        // to avoid, since one live capture cannot prove it always sorts.
        // `.05` rather than a whole number: `decimal_places` trims trailing
        // zeros, so a batch of e.g. `"101.00"` would imply scale **0**, not
        // 2 — these carry a genuine fractional digit so the scale this
        // batch implies is unambiguous.
        let scrambled = br#"{"code":1000,"message":"OK","data":{"ts":"1000",
            "asks":[["101.05","1"],["100.05","1"],["102.05","1"]],
            "bids":[["98.05","1"],["99.05","1"],["97.05","1"]],
            "symbol":"BTC_USDT"}}"#;
        let (_server, source) = mock_source(scrambled).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(
            snapshot.asks.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![10_005, 10_105, 10_205],
            "asks must come back ascending regardless of request order"
        );
        assert_eq!(
            snapshot.bids.iter().map(|l| l.price).collect::<Vec<_>>(),
            vec![9_905, 9_805, 9_705],
            "bids must come back descending regardless of request order"
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
        let empty = br#"{"code":1000,"message":"OK","data":{"ts":"1000","asks":[],"bids":[],
            "symbol":"BTC_USDT"}}"#;
        let (_server, source) = mock_source(empty).await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();
        assert!(snapshot.asks.is_empty());
        assert!(snapshot.bids.is_empty());
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let rejected = br#"{"code":50000,"message":"symbol not found","data":null}"#;
        let (_server, source) = mock_source(rejected).await;

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn the_success_field_is_ignored_since_the_venue_sends_false_on_a_real_success() {
        // Confirmed live: a genuinely successful response carries
        // `"success":false` — see the module docs. Only `code` may gate
        // acceptance.
        let (_server, source) = mock_source(BOOK).await;
        assert!(source.book_snapshot(&btc_usdt(), 5).await.is_ok());
    }
}
