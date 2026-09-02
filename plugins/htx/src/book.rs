//! HTX order-book depth — spot only, `GET /market/depth`.
//!
//! Spot only, for the same reason `bar_source_spot` alone covers bars in
//! this crate: HTX's three derivative markets live on `api.hbdm.com` with
//! their own paths, and this recording session's one-request-per-endpoint
//! limit made covering all four in one pass impossible (see `bars.rs`'s
//! own module docs for the identical scope decision there).
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.huobi.pro/market/depth?symbol=btcusdt&depth=5&type=step0`
//! returned `HTTP 200`:
//!
//! ```json
//! {"ch":"market.btcusdt.depth.step0","status":"ok","ts":1788332493740,
//! "tick":{"ts":1788332492716,"version":193149792213,
//! "bids":[[77590.36,0.096708],[77590.35,0.055072],[77580.35,3.22E-4],
//! [77578.94,5.16E-4],[77578.93,0.209952]],
//! "asks":[[77590.37,0.096708],[77600.42,0.128869],[77600.43,0.27673],
//! [77600.44,0.495188],[77601.52,0.495188]]}}
//! ```
//!
//! Confirmed from this capture:
//! - Depth is nested two levels down, under `tick.bids`/`tick.asks` — not
//!   at the envelope's top level, unlike the spot instrument list this
//!   crate's own `spot_instrument` reads.
//! - Each level is a **two-element array of bare JSON numbers**
//!   (`[price, size]`), never strings — the same trap this crate's own
//!   `bars.rs` documents for kline data, down to the same scientific
//!   notation (`3.22E-4`) on a small size. Decoded as [`RawValue`] and
//!   normalised with [`senken_core::plain_decimal`], the identical
//!   treatment `bars.rs` gives its own price and volume fields, so no
//!   `f64` appears anywhere in the path (see this crate's own top-level
//!   `AGENTS.md`).
//! - **Levels arrive best price first on each side already** (bids
//!   descending from `77590.36`, asks ascending from `77590.37`); this
//!   source does not re-sort them.
//! - **`depth=5` was honoured**: exactly 5 levels came back per side.
//! - There are two timestamps: the envelope's own `ts` (when HTX answered
//!   this request) and `tick.ts` (when the book itself was last updated).
//!   This source reports `tick.ts` — the book's own instant, the same
//!   choice `bars.rs` makes in preferring venue-reported times over a
//!   request-time stamp.
//! - `status` is `"ok"` on success; anything else is treated as a
//!   rejection, the same convention `crate::decode` already applies to
//!   the instrument-list endpoints in this crate.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **This endpoint's own error field names on failure** were not
//!   observed live — only a successful response was captured. HTX's
//!   documented convention elsewhere on this same host is hyphenated
//!   `err-code`/`err-msg` (unlike the underscored `err_msg` the
//!   instrument-list envelope this crate's `api.rs` already decodes
//!   uses), so both are accepted here defensively; either being absent on
//!   a genuine error would still leave `status != "ok"` as the rejection
//!   signal.
//! - **Whether `depth` is clamped to a maximum** was not tested — only 5
//!   was requested, and HTX's own documented depth values top out at 150
//!   for `type=step0`, which is not independently reconfirmed here.
//!   `MAX_DEPTH` is this project's own product choice for the panel,
//!   matching every other book source in this workspace, not a
//!   reconfirmed venue ceiling.
//! - **This endpoint's own rate-limit weight** is not in any response
//!   header HTX sent here; `BOOK_FETCH_COST` mirrors `CANDLES_FETCH_COST`
//!   in this crate's `bars.rs`, this project's own conservative proactive
//!   budget.
//! - **An empty book** was not observed live; `bids`/`asks` defaulting to
//!   an empty `Vec` on a missing field is this source's own defensive
//!   default, not a confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled, plain_decimal};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

const DEPTH_URL: &str = "https://api.huobi.pro/market/depth";

/// This project's own fixed panel depth — a product choice, not a
/// reconfirmed venue ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, matching
/// `CANDLES_FETCH_COST` in this crate's `bars.rs`.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size]`, both bare JSON numbers — see module docs.
type RawLevel = (Box<RawValue>, Box<RawValue>);

#[derive(Debug, Deserialize)]
struct DepthResponse {
    #[serde(default)]
    status: String,
    /// Not observed live on this endpoint — see module docs.
    #[serde(default, rename = "err-code")]
    err_code: String,
    /// Not observed live on this endpoint — see module docs.
    #[serde(default, rename = "err-msg")]
    err_msg: String,
    #[serde(default)]
    tick: Option<Tick>,
}

#[derive(Debug, Deserialize)]
struct Tick {
    /// Epoch milliseconds — the book's own last-update instant, not the
    /// envelope's request-time `ts` (see module docs).
    ts: i64,
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
}

/// HTX spot order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct HtxBookSource {
    url: String,
    client: VenueClient,
}

impl HtxBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds an [`HtxBookSource`] against the real endpoint, registered
/// under [`crate::SPOT_ID`].
#[must_use]
pub(crate) fn book_source_spot(client: VenueClient) -> HtxBookSource {
    HtxBookSource {
        url: DEPTH_URL.to_owned(),
        client,
    }
}

/// Normalises a raw JSON number's exact text (possibly scientific
/// notation) to plain decimal digits with no `f64` anywhere in the path —
/// the same treatment `bars.rs`'s own `normalize` gives kline fields.
fn normalize(raw: &str) -> Result<String, SourceError> {
    plain_decimal(raw)
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} is not a decimal number")))
}

/// Normalises a whole side's raw `(price, size)` pairs — see [`normalize`].
fn normalize_side(raw: Vec<RawLevel>) -> Result<Vec<(String, String)>, SourceError> {
    raw.into_iter()
        .map(|(price, size)| Ok((normalize(price.get())?, normalize(size.get())?)))
        .collect()
}

/// Parses one side's normalised `(price, size)` pairs into [`BookLevel`]s
/// at the book's own shared `price_scale`/`qty_scale`. The caller derives
/// both scales from **both sides together**: an empty side defaulting to
/// its own scale of 0 would spuriously disagree with a non-empty other
/// side — exactly the mismatch [`BookSnapshot::new`] exists to catch,
/// tripped here by how the scale was computed rather than by the venue.
///
/// A level whose size does not fit an `i64` at this scale — a real
/// possibility on any venue, per this workspace's `kucoin` bar source —
/// is left out rather than rounded: an honest absence, not a fabricated
/// number.
fn parse_side(
    normalized: Vec<(String, String)>,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
    let mut levels = Vec::with_capacity(normalized.len());
    for (price, size) in normalized {
        let Some(parsed_size) = parse_scaled(&size, qty_scale) else {
            tracing::warn!(
                price,
                size,
                "HTX reported a book level finer than a scaled i64 can hold; dropped, not rounded"
            );
            continue;
        };
        levels.push(BookLevel {
            price: scaled(&price, price_scale)?,
            size: parsed_size,
        });
    }
    Ok(levels)
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for HtxBookSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!(
            "{}?symbol={}&depth={depth}&type=step0",
            self.url,
            symbol.as_str()
        );
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response: DepthResponse = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !response.status.is_empty() && response.status != "ok" {
            return Err(SourceError::rejected(format!(
                "{} {}: {}",
                response.status, response.err_code, response.err_msg
            )));
        }
        let tick = response
            .tick
            .ok_or_else(|| SourceError::decode("response carried no tick"))?;

        let ts = UnixNanos::from_millis(tick.ts)
            .ok_or_else(|| SourceError::decode(format!("book ts {} overflowed", tick.ts)))?;

        let bids = normalize_side(tick.bids)?;
        let asks = normalize_side(tick.asks)?;
        // Derived from both sides together — see `parse_side`'s own docs
        // on why an empty side must never set its own, independent scale.
        let price_scale = common_scale(bids.iter().chain(&asks).map(|(price, _)| price.as_str()));
        let qty_scale = common_scale(bids.iter().chain(&asks).map(|(_, size)| size.as_str()));
        let bids = parse_side(bids, price_scale, qty_scale)?;
        let asks = parse_side(asks, price_scale, qty_scale)?;

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
    use super::{DEPTH_URL, book_source_spot};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book_spot.json");

    fn btcusdt() -> SourceSymbol {
        SourceSymbol::assume("btcusdt")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::HtxBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source_spot(test_client()).url,
            DEPTH_URL,
            "must default to the real HTX endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(
            snapshot.price_scale, 2,
            "77590.36 has two fractional digits"
        );
        assert_eq!(snapshot.bids[0].price, 7_759_036, "77590.36 at scale 2");
        assert_eq!(snapshot.asks[0].price, 7_759_037, "77590.37 at scale 2");
        assert!(
            snapshot.bids[0].price > snapshot.bids[1].price,
            "bids must stay best-first descending, as the venue sent them"
        );
        assert!(
            snapshot.asks[0].price < snapshot.asks[1].price,
            "asks must stay best-first ascending, as the venue sent them"
        );
        // The book's own last-update time (tick.ts), not the envelope's
        // request-time ts.
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_492_716);
    }

    #[tokio::test]
    async fn a_level_written_in_scientific_notation_decodes_without_going_through_f64() {
        // The fixture's third bid, "3.22E-4", is deliberately kept because
        // a hand-written fixture would not have thought to include one.
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusdt(), 5).await.unwrap();

        assert!(
            snapshot.bids[2].size > 0,
            "3.22E-4 must not decode to zero or fail"
        );
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btcusdt(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"ch":"market.btcusdt.depth.step0","status":"ok","ts":1788332493740,
                "tick":{"ts":1788332492716,"version":1,"bids":[],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btcusdt(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn an_error_status_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"status":"error","err-code":"invalid-parameter","err-msg":"invalid symbol"}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btcusdt(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn a_level_too_fine_for_a_scaled_i64_is_dropped_not_rounded() {
        // One level's size has 18 fractional digits, forcing the whole
        // side's shared scale to 18. Padded to that scale, the other
        // level's plain "100000" overflows an `i64` and must be dropped,
        // never rounded — the fine level that set the scale survives.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"status":"ok","ts":1788332493740,"tick":{"ts":1788332492716,"version":1,
                "bids":[[77590.36,0.000000000000000001],[77590.35,100000]],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btcusdt(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.len(),
            1,
            "the overflowing level is dropped, not the whole side"
        );
        assert_eq!(snapshot.bids[0].size, 1);
    }
}
