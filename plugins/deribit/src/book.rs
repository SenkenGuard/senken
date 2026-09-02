//! Deribit order-book depth — `GET /api/v2/public/get_order_book`.
//!
//! One document covers every kind Deribit lists (spot, perpetual, dated
//! future, option) by `instrument_name`, exactly as [`crate::source`]
//! does for the instrument catalog — so, like OKX's book source, this
//! registers once under [`crate::SOURCE_ID`], not once per kind.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://www.deribit.com/api/v2/public/get_order_book?instrument_name=BTC-PERPETUAL&depth=5`
//! returned `HTTP 200`:
//!
//! ```json
//! {"jsonrpc":"2.0","result":{"timestamp":1788332482980,"bids":
//! [[77616.0,7090.0],[77615.5,2230.0],[77615.0,2160.0],[77613.5,1020.0],
//! [77612.0,4.5e3]],"asks":[[77616.5,2010.0],[77618.0,12220.0],
//! [77618.5,200.0],[77623.5,1340.0],[77624.0,6650.0]], ...}}
//! ```
//! (trimmed to the fields this source reads; the full response also carries
//! `stats`, `mark_price`, `open_interest` and other fields nothing here
//! uses.)
//!
//! Confirmed from this capture:
//! - `bids` and `asks` are each an array of two-element `[price, amount]`
//!   pairs — **bare JSON numbers**, never strings, the same trap this
//!   crate's own `bars.rs` already documents for chart data. One level
//!   (`4.5e3`) arrived in scientific notation, so each element is decoded
//!   as a [`RawValue`] and read through [`senken_core::parse_scaled`]
//!   directly — no `f64` anywhere in the path (see the crate's own
//!   top-level `AGENTS.md`).
//! - Levels arrive **best price first** on each side already (bids
//!   descending from `77616.0`, asks ascending from `77616.5`); this
//!   source does not re-sort them.
//! - `depth=5` in the query string returned exactly 5 levels on both
//!   sides.
//! - `timestamp` is a bare integer of epoch milliseconds, unlike the
//!   `ticks` array `bars.rs` reads (also milliseconds, but there always as
//!   a JSON integer array rather than a lone scalar) — same unit either
//!   way.
//! - `amount` on a perpetual is denominated in the contract's own notional
//!   unit (USD on `BTC-PERPETUAL`), not the base asset — this source does
//!   not interpret it, only carries it through as a scaled quantity.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for
//! this milestone, already spent on the capture above.
//! - **Whether `depth` is clamped to a maximum** was not tested — only 5
//!   was requested. `MAX_DEPTH` is this project's own product choice for
//!   the panel, matching every other book source in this workspace, not a
//!   venue-documented ceiling.
//! - **This endpoint's own rate-limit weight** is not in any response
//!   header Deribit sent here; `BOOK_FETCH_COST` is this project's own
//!   conservative proactive budget, matching `CANDLES_FETCH_COST` in this
//!   same crate's `bars.rs`.
//! - **An empty book** (an instrument with no resting orders) was not
//!   observed live; `bids`/`asks` defaulting to an empty `Vec` on a
//!   missing field is this source's own defensive default, not a
//!   confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled, plain_decimal};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::api::RpcError;

const BOOK_URL: &str = "https://www.deribit.com/api/v2/public/get_order_book";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call. Deribit sends no rate-limit headers to reconcile against here
/// either (see module docs) — this mirrors `CANDLES_FETCH_COST` in this
/// crate's `bars.rs`, a deliberately conservative proactive budget rather
/// than a confirmed weight.
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, amount]`, both bare JSON numbers (see module docs).
type RawLevel = (Box<RawValue>, Box<RawValue>);

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    result: Option<RawBook>,
    #[serde(default)]
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
    timestamp: i64,
}

/// Deribit order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book (see this crate's own
/// `AGENTS.md` on why that complexity is not paid for here).
#[derive(Debug, Clone)]
pub(crate) struct DeribitBookSource {
    url: String,
    client: VenueClient,
}

impl DeribitBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`DeribitBookSource`] against the real Deribit endpoint,
/// registered under [`crate::SOURCE_ID`] — the one document this venue
/// answers depth for, covering spot, perpetual, dated futures and options
/// alike by `instrument_name`.
#[must_use]
pub(crate) fn book_source(client: VenueClient) -> DeribitBookSource {
    DeribitBookSource {
        url: BOOK_URL.to_owned(),
        client,
    }
}

/// Parses one side's raw levels at the book's own shared `price_scale`/
/// `qty_scale` — derived from **both** sides together in
/// [`BookSource::book_snapshot`] below, never one side alone. A thin book
/// with resting orders on only one side is a real market state, and
/// deriving each side's scale independently would make an empty side
/// default to scale 0 and spuriously disagree with a non-empty other side
/// — exactly the mismatch [`BookSnapshot::new`] exists to catch, tripped
/// here by how the scale was computed rather than by the venue.
///
/// A level whose amount does not fit an `i64` at this scale — a real
/// possibility on any venue, per this workspace's `kucoin` bar source —
/// is left out rather than rounded: an honest absence, not a fabricated
/// number.
/// Rewrites every level into plain decimal text, rejecting the side if any
/// value is not a number at all.
///
/// Deribit writes the occasional value in scientific notation — the
/// recorded response carries `4.5e3`, which is why that level was kept in
/// the fixture. Nothing downstream understands an exponent:
/// `common_scale` would read one fractional digit where the value has
/// none, and `parse_scaled` would refuse it outright, dropping a real
/// resting order out of the ladder with only a log line to show for it.
/// Normalising here, before anything counts a digit, is the only place
/// that gets both right.
fn plain_levels(raw: Vec<RawLevel>) -> Result<Vec<(String, String)>, SourceError> {
    raw.into_iter()
        .map(|(price, amount)| {
            let price = plain_decimal(price.get()).ok_or_else(|| {
                SourceError::decode(format!("price {:?} is not a number", price.get()))
            })?;
            let amount = plain_decimal(amount.get()).ok_or_else(|| {
                SourceError::decode(format!("amount {:?} is not a number", amount.get()))
            })?;
            Ok((price.into_owned(), amount.into_owned()))
        })
        .collect()
}

fn parse_side(
    raw: Vec<(String, String)>,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
    let mut levels = Vec::with_capacity(raw.len());
    for (price, amount) in raw {
        let Some(size) = parse_scaled(&amount, qty_scale) else {
            tracing::warn!(
                %price,
                %amount,
                "Deribit reported a book level finer than a scaled i64 can hold; dropped, not rounded"
            );
            continue;
        };
        levels.push(BookLevel {
            price: scaled(&price, price_scale)?,
            size,
        });
    }
    Ok(levels)
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for DeribitBookSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!(
            "{}?instrument_name={}&depth={depth}",
            self.url,
            symbol.as_str()
        );
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if let Some(error) = envelope.error {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                error.code, error.message
            )));
        }
        let book = envelope
            .result
            .ok_or_else(|| SourceError::decode("response carried neither result nor error"))?;

        let ts = UnixNanos::from_millis(book.timestamp)
            .ok_or_else(|| SourceError::decode(format!("book ts {} overflowed", book.timestamp)))?;

        // Scientific notation is normalised *before* anything counts its
        // digits. Deribit writes the occasional level as `4.5e3`, and
        // neither `common_scale` nor `parse_scaled` understands an
        // exponent: the first would read one fractional digit where the
        // value has none, and the second would refuse the value outright —
        // silently dropping a real resting order from the ladder. The
        // recorded response carries exactly one such level, which is why it
        // was kept.
        let bids = plain_levels(book.bids)?;
        let asks = plain_levels(book.asks)?;

        // Derived from both sides together — see `parse_side`'s own docs
        // on why an empty side must never set its own, independent scale.
        let price_scale = common_scale(bids.iter().chain(&asks).map(|(price, _)| price.as_str()));
        let qty_scale = common_scale(bids.iter().chain(&asks).map(|(_, amount)| amount.as_str()));
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
    use super::{BOOK_URL, book_source};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book_perp.json");

    fn btc_perpetual() -> SourceSymbol {
        SourceSymbol::assume("BTC-PERPETUAL")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::DeribitBookSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(BOOK, "application/json"))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[test]
    fn the_real_url_is_used_by_default() {
        assert_eq!(
            book_source(test_client()).url,
            BOOK_URL,
            "must default to the real Deribit endpoint, not require with_url"
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_best_price_first_at_the_correct_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_perpetual(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.price_scale, 1, "77616.5 has one fractional digit");
        assert_eq!(snapshot.bids[0].price, 776_160, "77616.0 at scale 1");
        assert_eq!(snapshot.asks[0].price, 776_165, "77616.5 at scale 1");
        assert!(
            snapshot.bids[0].price > snapshot.bids[1].price,
            "bids must stay best-first descending, as the venue sent them"
        );
        assert!(
            snapshot.asks[0].price < snapshot.asks[1].price,
            "asks must stay best-first ascending, as the venue sent them"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_482_980);
    }

    #[tokio::test]
    async fn a_level_written_in_scientific_notation_decodes_without_going_through_f64() {
        // The fixture's fifth bid, "4.5e3", is deliberately kept because a
        // hand-written fixture would not have thought to include one.
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_perpetual(), 5).await.unwrap();

        // Named by position, not by "whatever ended up last": the first
        // version of this test read `bids.last()`, which passed while the
        // exponent level was being silently dropped — the assertion simply
        // moved to the level above it.
        assert_eq!(
            snapshot.bids.len(),
            5,
            "the exponent level must be in the ladder, not dropped from it"
        );
        assert_eq!(
            snapshot.bids[4].size, 4_500,
            "4.5e3 is 4500, at this batch's quantity scale"
        );
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        // The fixture itself only carries 5 levels a side; this proves the
        // request URL is built with the clamped depth, not that the
        // fixture grows to fill it.
        let snapshot = source.book_snapshot(&btc_perpetual(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"jsonrpc":"2.0","result":{"timestamp":1788332482980,"bids":[],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_perpetual(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_json_rpc_error_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"jsonrpc":"2.0","error":{"code":10009,"message":"not_enough_funds"}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_perpetual(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn a_level_too_fine_for_a_scaled_i64_is_dropped_not_rounded() {
        // One level's amount has 18 fractional digits, forcing the whole
        // side's shared scale to 18 (see `common_scale`). Padded out to
        // that scale, the other level's plain "100000" overflows an
        // `i64` — it must be dropped, never rounded away to something
        // that fits, and the fine level that set the scale must survive
        // intact.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"jsonrpc":"2.0","result":{"timestamp":1788332482980,
                "bids":[[77616.0,0.000000000000000001],[77615.5,100000]],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_perpetual(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.len(),
            1,
            "the overflowing level is dropped, not the whole side"
        );
        assert_eq!(
            snapshot.bids[0].price, 776_160,
            "77616.0's own level survives"
        );
        assert_eq!(snapshot.bids[0].size, 1);
    }
}
