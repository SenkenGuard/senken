//! KuCoin order-book depth — `GET /api/v1/market/orderbook/level2_20`.
//!
//! A fresh HTTP fetch per call, never a maintained local book — the same
//! shape `senken-plugin-okx`'s own [`senken_subscription::BookSource`]
//! uses (see that crate's `book.rs` for the fuller rationale).
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.kucoin.com/api/v1/market/orderbook/level2_20?symbol=BTC-USDT`
//! returned `HTTP 200`:
//!
//! ```json
//! {"code":"200000","data":{"time":1788332424321,"sequence":"36463731205",
//! "bids":[["77603.1","0.54872723"],["77603","0.00004502"], … 20 levels],
//! "asks":[["77603.2","0.05435869"],["77603.3","0.00004502"], … 20 levels]}}
//! ```
//!
//! Confirmed from this capture:
//! - `code` is the string `"200000"` on success, matching every other
//!   KuCoin envelope this plugin already checks (`crate::OK`).
//! - Prices and sizes are strings, decoded the same scaled-integer way as
//!   every other KuCoin field this project reads.
//! - **This endpoint's depth is fixed at 20 levels a side by its own name
//!   (`level2_20`) — it takes no depth/limit query parameter at all.** A
//!   caller asking for fewer levels is honoured by truncating locally
//!   after the fetch, never by asking the venue for less; a caller asking
//!   for more than 20 simply gets what the venue has.
//! - Both sides arrive best-first already: `bids` descending from
//!   `77603.1` (the highest, best bid), `asks` ascending from `77603.2`
//!   (the lowest, best ask) — this source does not re-sort them.
//! - `time` is a bare integer of epoch milliseconds; `sequence` is a
//!   string and is read but not carried into the snapshot.
//!
//! # Quantities finer than a scaled `i64` can hold
//!
//! `plugins/kucoin/src/bars.rs` already found KuCoin reporting a spot
//! quantity with twenty decimal places on its `candles` endpoint — far
//! past what a scaled `i64` can represent. This capture's own sizes fit
//! comfortably (eight decimals at most), but nothing about this endpoint
//! rules out the same trap, so this source applies the identical
//! `quantity_scale` treatment: if any size in the batch does not fit an
//! `i64` at the batch's own common scale, the whole snapshot is refused
//! with a decode error — an honest absence of the book — rather than
//! rounding a quantity to force it to fit. Unlike a [`senken_series::Bar`],
//! a [`senken_subscription::BookLevel`] has no `Absent` variant to carry
//! a priced-but-sizeless level, so there is no partial answer available
//! here the way there is for bars.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for this
//! milestone, already spent on the capture above.
//! - **This endpoint's rate-limit weight** is not in any response header
//!   this capture returned. `BOOK_FETCH_COST` is this project's own
//!   conservative proactive budget, matching every other bar/book source in
//!   this workspace, not a venue-documented number.
//! - **An empty book** was not observed live; `data.bids`/`data.asks`
//!   defaulting to an empty `Vec` on a missing field is this source's own
//!   defensive default, not a confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale, exact_common_scale};
use serde::Deserialize;

use crate::OK;

const LEVEL2_20_URL: &str = "https://api.kucoin.com/api/v1/market/orderbook/level2_20";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling. Also the most this source can ever honour
/// upward: the venue itself never returns more than 20 a side (see module
/// docs), so this only ever narrows a request, never widens one.
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number (see module docs).
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size]`, both strings.
type RawLevel = (String, String);

#[derive(Debug, Deserialize)]
struct Envelope {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: BookData,
}

#[derive(Debug, Default, Deserialize)]
struct BookData {
    #[serde(default)]
    time: i64,
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
}

/// KuCoin order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct KucoinBookSource {
    source_id: String,
    url: String,
    client: VenueClient,
}

impl KucoinBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`KucoinBookSource`] against the real KuCoin endpoint.
#[must_use]
pub(crate) fn book_source(source_id: impl Into<String>, client: VenueClient) -> KucoinBookSource {
    KucoinBookSource {
        source_id: source_id.into(),
        url: LEVEL2_20_URL.to_owned(),
        client,
    }
}

/// Parses one side's raw levels at the venue's own shared price scale and a
/// quantity scale that only exists when every size in the batch actually
/// fits an `i64` at it — see the module docs on why there is no fallback.
fn parse_side(raw: &[RawLevel], price_scale: u8) -> Result<(Vec<BookLevel>, u8), SourceError> {
    let qty_scale = exact_common_scale(raw.iter().map(|level| level.1.as_str()))
        .ok_or_else(|| SourceError::decode("a level's size is finer than an i64 can hold"))?;
    let levels = raw
        .iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: scaled(price, price_scale)?,
                size: scaled(size, qty_scale)?,
            })
        })
        .collect::<Result<Vec<_>, SourceError>>()?;
    Ok((levels, qty_scale))
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for KucoinBookSource {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        // No depth parameter exists on this endpoint (see module docs); the
        // venue always answers with its own fixed 20 levels a side, and
        // `depth` is honoured by truncating after the fetch.
        let url = format!("{}?symbol={}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.code.is_empty() && envelope.code != OK {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        // Truncated before the scale is derived, not after: a level this
        // build cannot represent exactly (see the module docs) should
        // only fail the request when it is actually one of the levels
        // asked for, never because of a deeper level that was going to be
        // discarded anyway.
        let mut raw_bids = envelope.data.bids;
        let mut raw_asks = envelope.data.asks;
        raw_bids.truncate(depth);
        raw_asks.truncate(depth);

        let price_scale = common_scale(
            raw_bids
                .iter()
                .chain(&raw_asks)
                .map(|level| level.0.as_str()),
        );
        let (bids, bid_qty_scale) = parse_side(&raw_bids, price_scale)?;
        let (asks, ask_qty_scale) = parse_side(&raw_asks, price_scale)?;

        let ts = UnixNanos::from_millis(envelope.data.time).ok_or_else(|| {
            SourceError::decode(format!("book time {} overflowed", envelope.data.time))
        })?;

        // Both sides arrive best-first already (see module docs) — trusted
        // rather than re-sorted, the same as `senken-plugin-okx`'s book
        // source.
        BookSnapshot::new(
            ts,
            bids,
            price_scale,
            bid_qty_scale,
            asks,
            price_scale,
            ask_qty_scale,
        )
        .map_err(|source| SourceError::rejected(source.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::{LEVEL2_20_URL, book_source};
    use senken_marketdata::SourceSymbol;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTC-USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::KucoinBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source("kucoin-spot", test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source("kucoin-spot", test_client()).url,
            LEVEL2_20_URL,
            "must default to the real KuCoin endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 20).await.unwrap();

        assert_eq!(snapshot.bids.len(), 20, "the venue's own fixed depth");
        assert_eq!(snapshot.asks.len(), 20);
        assert_eq!(
            snapshot.price_scale, 1,
            "\"77603.1\" has one fractional digit"
        );
        assert_eq!(snapshot.bids[0].price, 776_031, "77603.1 at scale 1");
        assert_eq!(snapshot.asks[0].price, 776_032, "77603.2 at scale 1");
        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price > w[1].price),
            "bids must stay best-first descending, as the venue sent them"
        );
        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price < w[1].price),
            "asks must stay best-first ascending, as the venue sent them"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_424_321);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_venues_fixed_size_is_honoured_locally() {
        // This endpoint has no depth parameter at all (see module docs):
        // the venue always sends 20 a side, and this source must truncate
        // to `depth` itself rather than merely forwarding the request.
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_venues_fixed_size_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"code":"200000","data":{"time":0,"sequence":"0","bids":[],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source("kucoin-spot", test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 20).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_quantity_finer_than_an_i64_can_hold_is_refused_not_rounded() {
        // A synthetic level carrying the same shape of oversized decimal
        // `plugins/kucoin/src/bars.rs` found on the candles endpoint:
        // twenty fractional digits, far past an i64's reach at that scale.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"code":"200000","data":{"time":1788332424321,"sequence":"1",
                "bids":[["77603.1","89.56968223943530450117"]],"asks":[["77603.2","0.05"]]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source("kucoin-spot", test_client()).with_url(server.uri());

        let error = source
            .book_snapshot(&btc_usdt(), 20)
            .await
            .expect_err("a size finer than an i64 can hold must not be rounded into one");

        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Decode { .. }
        ));
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"code":"400100","msg":"Invalid parameter","data":{}}"#),
            )
            .mount(&server)
            .await;
        let source = book_source("kucoin-spot", test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 20).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
