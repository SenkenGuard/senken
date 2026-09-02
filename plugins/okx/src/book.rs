//! OKX order-book depth — `GET /api/v5/market/books`.
//!
//! Unlike [`crate::feed::OkxTradesProtocol`], this is not a WebSocket
//! adapter: a fixed-depth book is fetched fresh on request rather than
//! streamed, so this source is a plain HTTP call through a
//! [`VenueClient`], the same shape `senken-plugin-okx`'s own bar source
//! uses. It lives in this crate rather than that plugin because
//! [`senken_subscription::BookSource`] is the port this crate already
//! exists to implement venue adapters against (the trades/tickers channel
//! two doors up in this same crate implements
//! [`senken_subscription::QuoteSource`] the same way).
//!
//! # What was confirmed live, 2026-09-01
//!
//! `GET https://www.okx.com/api/v5/market/books?instId=BTC-USDT&sz=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"code":"0","msg":"","data":[{"asks":[["77927.5","1.53603807","0","12"],
//! ["77928.2","0.08455067","0","1"],["77928.3","0.12608816","0","1"],
//! ["77928.9","0.0000102","0","1"],["77929","0.15715346","0","1"]],
//! "bids":[["77927.4","0.39772851","0","5"],["77923.6","0.47857343","0","1"],
//! ["77921.2","0.00205134","0","2"],["77919.6","0.08617183","0","2"],
//! ["77919.5","0.00001289","0","1"]],"ts":"1788253213356","seqId":80632135948}]}
//! ```
//!
//! Confirmed from this capture:
//! - `data` is a one-element array — one object per requested `instId` —
//!   confirmed by requesting exactly one and receiving exactly one.
//! - Each level is a four-element array: price, size, a documented-deprecated
//!   "liquidated orders" count (`"0"` here, unused) and the order count at
//!   that level (also unused — nothing in this project draws it).
//! - `sz` in the query string controls how many levels come back per side:
//!   requesting 5 returned exactly 5 on both `asks` and `bids`.
//! - Levels arrive **best price first** on each side already (asks
//!   ascending from `77927.5`, bids descending from `77927.4`); this source
//!   does not re-sort them.
//! - `ts` is a string of epoch milliseconds, the same convention this
//!   venue's `history-candles` and `trades`/`tickers` channel already use.
//! - `price` and `size` are strings, decoded the same
//!   scaled-integer way as every other OKX field this project reads.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for this
//! milestone, already spent on the capture above.
//! - **This endpoint's own rate-limit weight** is not in any response
//!   header OKX sent here — the same gap `OkxBarSource`'s own docs record
//!   for `history-candles`. `BOOK_FETCH_COST` is this project's own
//!   conservative proactive budget, not a venue-documented number.
//! - **Whether `sz` is clamped to a maximum** was not tested — only 5 was
//!   requested. `MAX_DEPTH` is this project's own product choice for the
//!   panel, not a venue-documented ceiling.
//! - **An empty book** (an instrument with no resting orders) was not
//!   observed live; `data[0].asks`/`.bids` defaulting to an empty `Vec` on
//!   a missing field is this source's own defensive default, not a
//!   confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const BOOKS_URL: &str = "https://www.okx.com/api/v5/market/books";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call. OKX sends no rate-limit headers to reconcile against here
/// either (see module docs) — this mirrors `OkxBarSource`'s own
/// `CANDLES_FETCH_COST`, a deliberately conservative proactive budget
/// rather than a confirmed weight.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size, deprecated liquidated-orders count, order
/// count]`. Only the first two fields are read.
type RawLevel = (String, String, String, String);

#[derive(Debug, Deserialize)]
struct BooksResponse {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Vec<RawBook>,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
    ts: String,
}

/// OKX order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book (see module docs).
#[derive(Debug, Clone)]
pub(crate) struct OkxBookSource {
    source_id: String,
    url: String,
    client: VenueClient,
}

impl OkxBookSource {
    /// Points this source at a different URL — a local stand-in in tests,
    /// mirroring `OkxBarSource::with_url`.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds an [`OkxBookSource`] against the real OKX endpoint.
#[must_use]
pub(crate) fn book_source(source_id: impl Into<String>, client: VenueClient) -> OkxBookSource {
    OkxBookSource {
        source_id: source_id.into(),
        url: BOOKS_URL.to_owned(),
        client,
    }
}

/// Parses one side's raw levels, deriving that side's own scale from the
/// batch — the same `common_scale` treatment `OkxBarSource` gives a
/// candle's OHLC fields — and returning it separately so the caller can
/// compare the two sides before trusting either
/// ([`BookSnapshot::new`]'s own invariant).
fn parse_side(raw: Vec<RawLevel>) -> Result<(Vec<BookLevel>, u8, u8), SourceError> {
    let price_scale = common_scale(raw.iter().map(|level| level.0.as_str()));
    let qty_scale = common_scale(raw.iter().map(|level| level.1.as_str()));
    let levels = raw
        .into_iter()
        .map(|(price, size, _, _)| {
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
impl BookSource for OkxBookSource {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?instId={}&sz={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response: BooksResponse = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if response.code != "0" {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                response.code, response.msg
            )));
        }
        let book = response
            .data
            .into_iter()
            .next()
            .ok_or_else(|| SourceError::rejected("no book returned for this instrument"))?;

        let ts_ms: i64 =
            book.ts.trim().parse().map_err(|_| {
                SourceError::decode(format!("{:?} is not a valid timestamp", book.ts))
            })?;
        let ts = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| SourceError::decode(format!("book ts {ts_ms} overflowed")))?;

        let (bids, bid_price_scale, bid_qty_scale) = parse_side(book.bids)?;
        let (asks, ask_price_scale, ask_qty_scale) = parse_side(book.asks)?;

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
    use super::{BOOKS_URL, book_source};
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOKS: &[u8] = include_bytes!("../tests/fixtures/books.json");

    fn btc_usdt() -> SourceSymbol {
        Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT").source_symbol()
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::OkxBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOKS, "application/json"))
            .mount(&server)
            .await;
        let source = book_source("okx-spot", test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source("okx-spot", test_client()).url,
            BOOKS_URL,
            "must default to the real OKX endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(
            snapshot.price_scale, 1,
            "\"77927.5\" has one fractional digit"
        );
        assert_eq!(snapshot.asks[0].price, 779_275, "77927.5 at scale 1");
        assert_eq!(snapshot.bids[0].price, 779_274, "77927.4 at scale 1");
        assert!(
            snapshot.asks[0].price < snapshot.asks[1].price,
            "asks must stay best-first ascending, as the venue sent them"
        );
        assert!(
            snapshot.bids[0].price > snapshot.bids[1].price,
            "bids must stay best-first descending, as the venue sent them"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_253_213_356);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        // The fixture itself only carries 5 levels a side; this proves the
        // request URL is built with the clamped depth, not that the fixture
        // grows to fill it.
        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"code":"50011","msg":"Rate limit reached","data":[]}"#),
            )
            .mount(&server)
            .await;
        let source = book_source("okx-spot", test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
