//! BingX order-book depth — `GET /openApi/spot/v1/market/depth`.
//!
//! A fresh HTTP fetch per call, never a maintained local book — the same
//! shape `senken-plugin-okx`'s own [`senken_subscription::BookSource`]
//! uses (see that crate's `book.rs` for the fuller rationale).
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://open-api.bingx.com/openApi/spot/v1/market/depth?symbol=BTC-USDT&limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"code":0,"timestamp":1788332412826,"data":{"bids":[["77607.64","0.005522"],
//! ["77604.73","0.133077"],["77604.54","0.000552"],["77604.49","0.000119"],
//! ["77604.46","0.032076"]],"asks":[["77610.98","0.000052"],["77610.60","0.000225"],
//! ["77610.57","0.000212"],["77610.56","4.153901"],["77607.66","0.004423"]]},
//! "ts":1788332412826,"lastUpdateId":16318041565}
//! ```
//!
//! Confirmed from this capture:
//! - `code` is `0` on success — a bare JSON integer, matching every other
//!   BingX envelope this plugin already checks in `lib.rs`.
//! - Prices and sizes arrive as **strings** here, unlike the bare numbers
//!   `bars.rs` has to guard against on the kline endpoint — no
//!   [`serde_json::value::RawValue`] trick is needed for this one.
//! - `limit` in the query string controls how many levels come back per
//!   side: requesting 5 returned exactly 5 on both `bids` and `asks`.
//! - **`bids` arrive best-first (descending from 77607.64), but `asks`
//!   arrive worst-first — descending from 77610.98 down to 77607.66, the
//!   *opposite* of best-first ascending.** This is the trap this module
//!   exists to record: trusting the venue's own order the way
//!   `senken-plugin-okx`'s book source does would silently invert the ask
//!   side. Both sides are re-sorted explicitly below rather than trusted.
//! - `ts` (top level, alongside the near-identical `timestamp`) is a bare
//!   integer of epoch milliseconds.
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

const DEPTH_URL: &str = "https://open-api.bingx.com/openApi/spot/v1/market/depth";

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
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: BookData,
    /// The response instant, in epoch milliseconds.
    ///
    /// Read from the envelope's `timestamp`, not from the `ts` that sits
    /// *inside* `data`: the recording carries both, and they are not the
    /// same clock. `timestamp` is when the venue answered; `data.ts` is
    /// carried alongside `lastUpdateId` as part of the book's own
    /// bookkeeping. A snapshot is stamped with when it was reported.
    timestamp: i64,
}

#[derive(Debug, Default, Deserialize)]
struct BookData {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
}

/// BingX order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct BingxBookSource {
    source_id: String,
    url: String,
    client: VenueClient,
}

impl BingxBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BingxBookSource`] against the real BingX endpoint.
#[must_use]
pub(crate) fn book_source(source_id: impl Into<String>, client: VenueClient) -> BingxBookSource {
    BingxBookSource {
        source_id: source_id.into(),
        url: DEPTH_URL.to_owned(),
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
impl BookSource for BingxBookSource {
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
        if envelope.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        let (mut bids, bid_price_scale, bid_qty_scale) = parse_side(envelope.data.bids)?;
        let (mut asks, ask_price_scale, ask_qty_scale) = parse_side(envelope.data.asks)?;

        // Observed live (see module docs): bids arrive best-first but asks
        // arrive worst-first. Both sides are re-sorted unconditionally
        // rather than trusting whichever order the venue happened to send,
        // since that order has already been seen to differ per side.
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        asks.sort_by_key(|level| level.price);

        let ts = UnixNanos::from_millis(envelope.timestamp).ok_or_else(|| {
            SourceError::decode(format!("book ts {} overflowed", envelope.timestamp))
        })?;

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
    use super::{DEPTH_URL, book_source};
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

    async fn mock_source() -> (MockServer, super::BingxBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source("bingx-spot", test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source("bingx-spot", test_client()).url,
            DEPTH_URL,
            "must default to the real BingX endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77607.64\" has two fractional digits"
        );
        assert_eq!(snapshot.bids[0].price, 7_760_764, "77607.64 at scale 2");
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_412_826);
    }

    #[tokio::test]
    async fn bids_and_asks_are_both_returned_best_first_even_though_the_venue_only_sorts_bids() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price > w[1].price),
            "bids must be descending, best (highest) first"
        );
        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price < w[1].price),
            "asks must be ascending, best (lowest) first — the venue itself \
             sends them worst-first, so this only holds if the source \
             re-sorts them"
        );
        assert_eq!(
            snapshot.asks[0].price, 7_760_766,
            "77607.66 is the lowest (best) ask, even though it is the last \
             entry the venue sent"
        );
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
                r#"{"code":0,"timestamp":0,"data":{"bids":[],"asks":[],"ts":0,"lastUpdateId":0}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source("bingx-spot", test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"code":100202,"msg":"Insufficient balance","timestamp":0,"data":{"bids":[],"asks":[],"ts":0}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source("bingx-spot", test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
