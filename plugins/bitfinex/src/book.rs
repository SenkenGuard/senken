//! Bitfinex spot order-book depth — `GET /v2/book/t{symbol}/P0`.
//!
//! Unlike [`crate::bars::BitfinexBarSource`], this is a fixed-depth
//! snapshot fetched fresh on request rather than streamed, the same shape
//! `senken_subscription::BookSource` exists to serve. Only the spot market
//! is registered here — mirroring [`crate::bar_source_spot`]'s own
//! spot-only scope — since the perpetual market's book has not been
//! audited.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api-pub.bitfinex.com/v2/book/tBTCUSD/P0` returned `HTTP
//! 200`:
//!
//! ```json
//! [[77694,3,0.07887288],[77689,1,0.03217933], ... ,[77650,1,0.25756344],
//!  [77704,2,-0.00032931],[77710,2,-0.03245298], ... ,[77760,1,-0.07426182]]
//! ```
//!
//! Confirmed from this capture:
//! - **The response is one flat array, not separate `bids`/`asks`
//!   arrays.** Each element is `[PRICE, COUNT, AMOUNT]`; there is no
//!   per-level field naming which side a row belongs to.
//! - **The sign of `AMOUNT` is the only side marker this endpoint sends**:
//!   every row in the first half of the capture carries a positive amount
//!   and a descending price (a bid book, best price — highest — first);
//!   every row in the second half carries a negative amount and an
//!   ascending price (an ask book, best price — lowest — first). This
//!   source splits on that sign rather than trusting the two halves to
//!   stay contiguous, and re-sorts each side explicitly regardless of the
//!   order the venue happened to send.
//! - **`PRICE` and `AMOUNT` arrive as bare JSON numbers**, not strings
//!   (`77694`, `-0.00032931`) — unlike every string-encoded source in this
//!   workspace. This project never routes a price through `f64`, not even
//!   transiently, so both fields are read as [`RawValue`], the workspace's
//!   `serde_json` `raw_value` feature, and handed to
//!   [`senken_core::parse_scaled`] as the venue's own exact digits.
//! - `COUNT` (field 1, the number of orders resting at that price) is
//!   read and discarded, the same as OKX's own unused order-count field.
//! - **The response carries no timestamp of any kind** — no `ts`, no
//!   `mts`, nothing. [`BitfinexBookSource::book_snapshot`] stamps the
//!   snapshot from a [`Clock`] instead, the same real-time source
//!   `BitfinexBarSource` closes candles against.
//! - The default request (no `len` parameter sent) returned exactly 25
//!   levels on each side.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! - **Whether a `len` query parameter changes the returned depth** was
//!   not tested — the access boundary allows exactly one short-lived live
//!   request for this milestone, already spent on the capture above. This
//!   source never sends `len` and [`MAX_DEPTH`] is the observed default
//!   rather than a confirmed venue ceiling: a caller asking for more than
//!   25 levels gets 25, not more.
//! - **An empty book** (an instrument with no resting orders on one or
//!   both sides) was not observed live; an empty array decoding to no
//!   levels on that side is this source's own defensive default.
//! - **The application-level error shape.** Bitfinex's v2 REST API is
//!   documented, across every endpoint, to answer an error with a
//!   three-element array — `["error", <code>, <message>]` — in place of
//!   the normal payload, still under `HTTP 200`. That general convention
//!   is not something this recording session's single request reproduced
//!   for this specific endpoint; `parse_book` recognises it defensively
//!   and reports [`SourceError::rejected`], but this is a cited, not an
//!   independently confirmed, fact.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::parse_scaled;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_series::Clock;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde_json::value::RawValue;

const BOOK_URL: &str = "https://api-pub.bitfinex.com/v2/book";

/// The depth this endpoint returns by default, with no `len` parameter
/// sent — see the module docs on why nothing larger is requested.
const MAX_DEPTH: usize = 25;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other book source here.
const BOOK_FETCH_COST: u32 = 5;

/// One row: `[PRICE, COUNT, AMOUNT]`, both `PRICE` and `AMOUNT` bare JSON
/// numbers — see the module docs. `COUNT` (field 1) is read and discarded.
type RawLevel = (Box<RawValue>, i64, Box<RawValue>);

/// Bitfinex's documented, cross-endpoint REST error envelope — see the
/// module docs' final bullet on why this is cited, not reproduced live.
type ErrorEnvelope = (String, i64, String);

/// Bitfinex spot order-book depth, fetched through a [`VenueClient`] and
/// stamped from a [`Clock`] — this endpoint sends no timestamp of its own
/// (see the module docs).
#[derive(Clone)]
pub(crate) struct BitfinexBookSource {
    source_id: &'static str,
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for BitfinexBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitfinexBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl BitfinexBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`BitfinexBookSource`] against the real Bitfinex endpoint.
#[must_use]
pub(crate) fn book_source(
    source_id: &'static str,
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> BitfinexBookSource {
    BitfinexBookSource {
        source_id,
        url: BOOK_URL.to_owned(),
        client,
        clock,
    }
}

/// Splits `raw`'s leading `-`, if any, from its magnitude — the sign
/// names which side a row belongs to (see the module docs); the magnitude
/// alone is what `scaled` needs.
fn split_sign(raw: &str) -> (bool, &str) {
    let trimmed = raw.trim();
    trimmed
        .strip_prefix('-')
        .map_or((false, trimmed), |magnitude| (true, magnitude))
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// Decodes the response body, recognising Bitfinex's cross-endpoint error
/// envelope (see the module docs) before falling back to a generic decode
/// error.
fn parse_book(body: &[u8]) -> Result<Vec<RawLevel>, SourceError> {
    match serde_json::from_slice::<Vec<RawLevel>>(body) {
        Ok(rows) => Ok(rows),
        Err(decode_err) => {
            if let Ok((tag, code, message)) = serde_json::from_slice::<ErrorEnvelope>(body)
                && tag == "error"
            {
                return Err(SourceError::rejected(format!("code {code}: {message}")));
            }
            Err(SourceError::decode(decode_err))
        }
    }
}

#[async_trait]
impl BookSource for BitfinexBookSource {
    fn source_id(&self) -> &str {
        self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        // The `t` prefix trap this venue's bars share: `source_symbol`
        // stores the configuration-list spelling, and the trading endpoint
        // needs it prefixed — see `crate::bars`' own module docs.
        let url = format!("{}/t{}/P0", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let rows = parse_book(&body)?;

        // One shared scale for both sides: they arrive interleaved in one
        // array with the same field format, so computing it once over the
        // whole batch is both simpler and guarantees `BookSnapshot::new`'s
        // matching-scale invariant can never fail here.
        let price_scale = common_scale(rows.iter().map(|(price, ..)| price.get()));
        let qty_scale = common_scale(rows.iter().map(|(_, _, amount)| split_sign(amount.get()).1));

        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for (price, _count, amount) in &rows {
            let (is_ask, magnitude) = split_sign(amount.get());
            let level = BookLevel {
                price: scaled(price.get(), price_scale)?,
                size: scaled(magnitude, qty_scale)?,
            };
            if is_ask {
                asks.push(level);
            } else {
                bids.push(level);
            }
        }

        // Best price first on both sides, regardless of the order the two
        // halves happened to arrive in — see the module docs.
        bids.sort_by_key(|level| std::cmp::Reverse(level.price));
        asks.sort_by_key(|level| level.price);
        bids.truncate(depth);
        asks.truncate(depth);

        BookSnapshot::new(
            self.clock.now(),
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
    use senken_marketdata::source::SourceError;
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_series::Clock;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::book_source;

    /// A real `GET /v2/book/tBTCUSD/P0` response, recorded 2026-09-02: 25
    /// positive-amount rows (bids, descending) followed by 25
    /// negative-amount rows (asks, ascending).
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn btcusd() -> SourceSymbol {
        Instrument::spot("BTCUSD", "BTCUSD", "BTC", "USD").source_symbol()
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

    async fn mock_source() -> (MockServer, super::BitfinexBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(
            crate::SPOT_ID,
            test_client(),
            Arc::new(FixedClock(1_788_332_500_000)),
        )
        .with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn levels_decode_from_bare_numbers_at_the_venues_own_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 25).await.unwrap();

        assert_eq!(snapshot.bids.len(), 25);
        assert_eq!(snapshot.asks.len(), 25);
        assert_eq!(
            snapshot.price_scale, 0,
            "every captured price is a whole number"
        );
        assert_eq!(snapshot.qty_scale, 8);
        assert_eq!(snapshot.bids[0].price, 77_694, "best bid, positive amount");
        assert_eq!(snapshot.bids[0].size, 7_887_288, "0.07887288 at scale 8");
        assert_eq!(snapshot.asks[0].price, 77_704, "best ask, negative amount");
        assert_eq!(
            snapshot.asks[0].size, 32_931,
            "0.00032931 at scale 8, sign dropped"
        );
    }

    #[tokio::test]
    async fn bids_and_asks_are_split_by_amount_sign_and_sorted_best_first() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 25).await.unwrap();

        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price),
            "bids must be descending, best price first"
        );
        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price),
            "asks must be ascending, best price first"
        );
        assert!(snapshot.bids.iter().all(|l| l.size > 0));
        assert!(
            snapshot.asks.iter().all(|l| l.size > 0),
            "the negative sign must not survive into a level's size"
        );
    }

    #[tokio::test]
    async fn a_depth_above_the_observed_default_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_full_book_is_honoured_client_side() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 3).await.unwrap();
        assert_eq!(snapshot.bids.len(), 3);
        assert_eq!(snapshot.asks.len(), 3);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(&b"[]"[..], "application/json"))
            .mount(&server)
            .await;
        let source = book_source(
            crate::SPOT_ID,
            test_client(),
            Arc::new(FixedClock(1_788_332_500_000)),
        )
        .with_url(server.uri());

        let snapshot = source.book_snapshot(&btcusd(), 25).await.unwrap();
        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn no_timestamp_on_the_wire_means_the_clock_stamps_the_snapshot() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusd(), 25).await.unwrap();
        assert_eq!(
            snapshot.ts,
            UnixNanos::from_millis(1_788_332_500_000).unwrap()
        );
    }

    #[tokio::test]
    async fn the_documented_error_envelope_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"["error",10020,"symbol: invalid"]"#),
            )
            .mount(&server)
            .await;
        let source = book_source(crate::SPOT_ID, test_client(), Arc::new(FixedClock(0)))
            .with_url(server.uri());

        let error = source.book_snapshot(&btcusd(), 25).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }
}
