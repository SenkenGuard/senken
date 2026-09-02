//! Bitget order-book depth — `GET /api/v2/spot/market/orderbook`.
//!
//! A fresh HTTP fetch per call, never a maintained local book — the same
//! shape `senken-plugin-okx`'s own [`senken_subscription::BookSource`]
//! uses (see that crate's `book.rs` for the fuller rationale).
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.bitget.com/api/v2/spot/market/orderbook?symbol=BTCUSDT&limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"code":"00000","msg":"success","requestTime":1788332421622,
//! "data":{"asks":[["77615","0.02"],["77619.44","0.005003"],
//! ["77619.47","0.012883"],["77620","0.02"],["77622.3","0.051649"]],
//! "bids":[["77614.99","2.128451"],["77614.91","0.000019"],
//! ["77614.09","0.335773"],["77614.08","1.084208"],["77611.88","0.002"]],
//! "ts":"1788332421495"}}
//! ```
//!
//! Confirmed from this capture:
//! - `code` is the *string* `"00000"` on success, matching every other
//!   Bitget envelope this plugin already checks (`crate::OK`).
//! - Prices and sizes are strings, decoded the same scaled-integer way as
//!   every other Bitget field this project reads.
//! - `limit` in the query string controls how many levels come back per
//!   side: requesting 5 returned exactly 5 on both `asks` and `bids`.
//! - **Both sides arrive best-first already**: `asks` ascending from
//!   `77615` (the lowest, best ask), `bids` descending from `77614.99`
//!   (the highest, best bid) — this source does not re-sort them, unlike
//!   `senken-plugin-bingx`'s book source next to it, which has to.
//! - `ts` is a string of epoch milliseconds.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for this
//! milestone, already spent on the capture above.
//! - **This endpoint's rate-limit weight** is not in any response header
//!   this capture returned. `BOOK_FETCH_COST` is this project's own
//!   conservative proactive budget, matching every other bar/book source in
//!   this workspace, not a venue-documented number.
//! - **Whether `limit` is clamped to a maximum** was not tested — only 5
//!   was requested. `MAX_DEPTH` is this project's own product choice for
//!   the panel, not a venue-documented ceiling.
//! - **An empty book** was not observed live; `data.bids`/`data.asks`
//!   defaulting to an empty `Vec` on a missing field is this source's own
//!   defensive default, not a confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

use crate::OK;

const ORDERBOOK_URL: &str = "https://api.bitget.com/api/v2/spot/market/orderbook";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
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
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    ts: String,
}

/// Bitget order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct BitgetBookSource {
    source_id: String,
    url: String,
    client: VenueClient,
}

impl BitgetBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BitgetBookSource`] against the real Bitget endpoint.
#[must_use]
pub(crate) fn book_source(source_id: impl Into<String>, client: VenueClient) -> BitgetBookSource {
    BitgetBookSource {
        source_id: source_id.into(),
        url: ORDERBOOK_URL.to_owned(),
        client,
    }
}

/// Parses one side's raw levels, deriving that side's own scale from the
/// batch and returning it separately so the caller can compare the two
/// sides before trusting either ([`BookSnapshot::new`]'s own invariant).
fn parse_side(raw: Vec<RawLevel>) -> Result<(Vec<BookLevel>, u8, u8), SourceError> {
    let price_scale = common_scale(raw.iter().map(|level| level.0.as_str()));
    let qty_scale = common_scale(raw.iter().map(|level| level.1.as_str()));
    let levels = raw
        .into_iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: scaled(&price, price_scale)?,
                size: scaled(&size, qty_scale)?,
            })
        })
        .collect::<Result<Vec<_>, SourceError>>()?;
    Ok((levels, price_scale, qty_scale))
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for BitgetBookSource {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}&limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.code.is_empty() && envelope.code != OK {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        let ts_ms: i64 = envelope.data.ts.trim().parse().map_err(|_| {
            SourceError::decode(format!("{:?} is not a valid timestamp", envelope.data.ts))
        })?;
        let ts = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| SourceError::decode(format!("book ts {ts_ms} overflowed")))?;

        // Both sides arrive best-first already (see module docs) — trusted
        // rather than re-sorted, the same as `senken-plugin-okx`'s book
        // source.
        let (bids, bid_price_scale, bid_qty_scale) = parse_side(envelope.data.bids)?;
        let (asks, ask_price_scale, ask_qty_scale) = parse_side(envelope.data.asks)?;

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
    use super::{ORDERBOOK_URL, book_source};
    use senken_marketdata::SourceSymbol;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTCUSDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::BitgetBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source("bitget-spot", test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source("bitget-spot", test_client()).url,
            ORDERBOOK_URL,
            "must default to the real Bitget endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77619.44\" has two fractional digits"
        );
        assert_eq!(snapshot.asks[0].price, 7_761_500, "77615 at scale 2");
        assert_eq!(snapshot.bids[0].price, 7_761_499, "77614.99 at scale 2");
        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price < w[1].price),
            "asks must stay best-first ascending, as the venue sent them"
        );
        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price > w[1].price),
            "bids must stay best-first descending, as the venue sent them"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_421_495);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"code":"00000","msg":"success","requestTime":0,"data":{"asks":[],"bids":[],"ts":"0"}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source("bitget-spot", test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(snapshot.asks.is_empty());
        assert!(snapshot.bids.is_empty());
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(
                    r#"{"code":"40034","msg":"Parameter does not exist","data":{}}"#,
                ),
            )
            .mount(&server)
            .await;
        let source = book_source("bitget-spot", test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
