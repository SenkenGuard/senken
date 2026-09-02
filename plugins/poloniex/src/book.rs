//! Poloniex order-book depth — `GET /markets/{symbol}/orderBook`.
//!
//! A fresh HTTP fetch per call, never a maintained local book — the same
//! shape `senken-plugin-okx`'s own [`senken_subscription::BookSource`]
//! uses (see that crate's `book.rs` for the fuller rationale).
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.poloniex.com/markets/BTC_USDT/orderBook?limit=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"bids":["77608.09","0.000039","77601.88","0.020039","77598.16","0.02",
//! "77597.38","0.016599","77593.42","0.020000"],
//! "asks":["77616.98","0.00018","77616.99","0.000039","77618.12","0.000039",
//! "77627.15","0.020000","77635.20","0.02"],
//! "scale":"0.01","time":1788332425666,"ts":1788332426138}
//! ```
//!
//! Confirmed from this capture:
//! - **`bids` and `asks` are flat arrays of alternating price, size
//!   strings — `[price, size, price, size, …]`, not an array of pairs**
//!   the way every other venue in this workspace shapes a book. This
//!   source chunks each into pairs before decoding.
//! - `limit` in the query string controls how many *pairs* come back per
//!   side: requesting 5 returned exactly 5 pairs (10 raw elements) on
//!   both `bids` and `asks`.
//! - Both sides arrive best-first already: `bids` descending from
//!   `77608.09` (the highest, best bid), `asks` ascending from `77616.98`
//!   (the lowest, best ask) — this source does not re-sort them.
//! - **Two timestamps, and they disagree**: `time` (1,788,332,425,666) and
//!   `ts` (1,788,332,426,138) differ by ~472 ms in this capture.
//!   `plugins/poloniex/src/bars.rs` already documents that this venue's
//!   `ts`-named field is a generic "message time" shared across
//!   endpoints, not the data's own instant — the same distinction is
//!   applied here: `time` is read as the book's own snapshot instant,
//!   `ts` is left unread.
//! - This endpoint carries **no in-band success/error code** at all,
//!   unlike BingX, Bitget or KuCoin's book endpoints — matching
//!   `plugins/poloniex/src/bars.rs`'s own candles endpoint, which is the
//!   same host family and also has no envelope. A rejection can only
//!   arrive as a non-success HTTP status, which `VenueClient` itself turns
//!   into a [`SourceError`] before this module's parsing ever runs.
//! - The `scale` field (`"0.01"`) was not used: it echoes the requested
//!   grouping precision, not a claim about the precision of the numbers
//!   actually returned, and every price here already carries its own
//!   decimal point read the same scaled-integer way as everywhere else in
//!   this project.
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
//!   was requested; Poloniex's own public API reference documents 150 as
//!   this endpoint's maximum, a claim this session did not itself verify
//!   live, so `MAX_DEPTH` stays this project's own conservative panel
//!   choice rather than that documented ceiling.
//! - **An empty book** was not observed live; empty `bids`/`asks` arrays
//!   are handled (an even-length array of zero pairs), but that shape
//!   itself is this source's own inference, not a confirmed venue
//!   response.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

/// The `/markets` prefix every order-book request is nested under:
/// `{MARKETS_URL}/{symbol}/orderBook`.
const MARKETS_URL: &str = "https://api.poloniex.com/markets";

/// This project's own fixed panel depth — a product choice, not an
/// independently confirmed venue ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number (see module docs).
const BOOK_FETCH_COST: u32 = 5;

#[derive(Debug, Default, Deserialize)]
struct RawBook {
    #[serde(default)]
    bids: Vec<String>,
    #[serde(default)]
    asks: Vec<String>,
    time: i64,
}

/// Poloniex order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct PoloniexBookSource {
    source_id: String,
    url: String,
    client: VenueClient,
}

impl PoloniexBookSource {
    /// Points this source at a different `/markets`-equivalent prefix — a
    /// local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`PoloniexBookSource`] against the real Poloniex endpoint.
#[must_use]
pub(crate) fn book_source(source_id: impl Into<String>, client: VenueClient) -> PoloniexBookSource {
    PoloniexBookSource {
        source_id: source_id.into(),
        url: MARKETS_URL.to_owned(),
        client,
    }
}

/// Groups a flat `[price, size, price, size, …]` array into pairs — see
/// the module docs on why this venue's shape is not already pairs.
///
/// A trailing unpaired element (which this endpoint has never been
/// observed to send) is dropped rather than treated as a malformed
/// response: the price half of an incomplete pair carries no usable size.
fn pairs(flat: &[String]) -> Vec<(&str, &str)> {
    flat.chunks_exact(2)
        .map(|pair| (pair[0].as_str(), pair[1].as_str()))
        .collect()
}

/// Parses one side's flat array, deriving that side's own scale from the
/// batch and returning it separately so the caller can compare the two
/// sides before trusting either ([`BookSnapshot::new`]'s own invariant).
fn parse_side(flat: &[String]) -> Result<(Vec<BookLevel>, u8, u8), SourceError> {
    let raw = pairs(flat);
    let price_scale = common_scale(raw.iter().map(|level| level.0));
    let qty_scale = common_scale(raw.iter().map(|level| level.1));
    let levels = raw
        .into_iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: scaled(price, price_scale)?,
                size: scaled(size, qty_scale)?,
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
impl BookSource for PoloniexBookSource {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}/{}/orderBook?limit={depth}", self.url, symbol.as_str());
        // No in-band code to check here (see module docs) — a rejection
        // already surfaced as an `Err` from `client.get` on a non-success
        // HTTP status, before this line returns.
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let raw: RawBook = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        // Both sides arrive best-first already (see module docs) — trusted
        // rather than re-sorted, the same as `senken-plugin-okx`'s book
        // source.
        let (bids, bid_price_scale, bid_qty_scale) = parse_side(&raw.bids)?;
        let (asks, ask_price_scale, ask_qty_scale) = parse_side(&raw.asks)?;

        let ts = UnixNanos::from_millis(raw.time)
            .ok_or_else(|| SourceError::decode(format!("book time {} overflowed", raw.time)))?;

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
    use super::{MARKETS_URL, book_source};
    use senken_marketdata::SourceSymbol;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTC_USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::PoloniexBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source("poloniex-spot", test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source("poloniex-spot", test_client()).url,
            MARKETS_URL,
            "must default to the real Poloniex endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn the_flat_alternating_array_decodes_into_paired_levels_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "\"77608.09\" has two fractional digits"
        );
        assert_eq!(snapshot.bids[0].price, 7_760_809, "77608.09 at scale 2");
        assert_eq!(snapshot.asks[0].price, 7_761_698, "77616.98 at scale 2");
    }

    #[tokio::test]
    async fn bids_and_asks_are_both_best_price_first() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price > w[1].price),
            "bids must stay best-first descending, as the venue sent them"
        );
        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price < w[1].price),
            "asks must stay best-first ascending, as the venue sent them"
        );
    }

    #[tokio::test]
    async fn the_snapshot_instant_is_read_from_time_not_ts() {
        // The fixture's `time` and `ts` fields disagree by ~472ms — using
        // the wrong one would still produce a plausible-looking timestamp,
        // just the wrong one.
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_425_666);
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
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"bids":[],"asks":[],"scale":"0.01","time":0,"ts":0}"#),
            )
            .mount(&server)
            .await;
        let source = book_source("poloniex-spot", test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_non_success_status_is_a_rejection() {
        // This endpoint carries no in-band error code (see module docs);
        // its only rejection signal is the HTTP status itself, which
        // `VenueClient` turns into an `Err` before this source ever parses
        // a body.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(
                    r#"{"code":"INVALID_ARGUMENT","message":"symbol not supported"}"#,
                ),
            )
            .mount(&server)
            .await;
        let source = book_source("poloniex-spot", test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Http { status: 400, .. }
        ));
    }
}
