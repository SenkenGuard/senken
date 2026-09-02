//! Gate.io order-book depth — spot and USDT perpetual, two different
//! response shapes behind the same [`BookSource`] port.
//!
//! Only the two markets this module's docs record live are served here.
//! Gate's BTC-settled perpetuals and USDT delivery futures — each already
//! registered as a [`senken_marketdata::MarketDataSource`] for
//! instruments — have no book source of their own for the same reason
//! `bar_source_spot` alone covers bars: an unverified endpoint is not
//! offered rather than guessed at.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.gateio.ws/api/v4/spot/order_book?currency_pair=BTC_USDT&limit=5`:
//!
//! ```json
//! {"current":1788332490408,"update":1788332490406,
//! "asks":[["77617","0.013003"],["77618","0.039934"],["77619","0.03388"],
//! ["77622.8","0.057988"],["77622.9","0.154891"]],
//! "bids":[["77616.9","0.057125"],["77614.8","0.045"],["77614.4","0.003221"],
//! ["77614","0.0002"],["77612.2","0.04"]]}
//! ```
//!
//! `GET https://api.gateio.ws/api/v4/futures/usdt/order_book?contract=BTC_USDT&limit=5`:
//!
//! ```json
//! {"current":1788332492.975,"update":1788332492.969,
//! "asks":[{"s":6972,"p":"77590.5"},{"s":1,"p":"77593.3"},{"s":192,"p":"77593.4"},
//! {"s":644,"p":"77593.5"},{"s":644,"p":"77594.5"}],
//! "bids":[{"s":27528,"p":"77590.4"},{"s":331,"p":"77590.3"},{"s":28,"p":"77588.3"},
//! {"s":1256,"p":"77587.2"},{"s":2579,"p":"77587.1"}]}
//! ```
//!
//! Confirmed from these two captures:
//! - **Spot levels are `[price, size]` string pairs**, decoded the same
//!   scaled-integer way as every other Gate field this project reads.
//! - **Futures levels are objects**, `{"s": size, "p": "price"}` — `p` is
//!   still a string, but `s` is a **bare integer** naming a whole number
//!   of contracts, never fractional on this market. Reading it as the
//!   spot shape would fail to deserialize outright rather than silently
//!   misparse, which is why this module keeps the two shapes as separate
//!   types instead of forcing one onto both markets.
//! - **`limit` is honoured on both markets**: 5 requested, 5 returned per
//!   side, on both endpoints.
//! - **Both sides arrive best price first already** on both markets
//!   (asks ascending, bids descending); this source does not re-sort.
//! - **`current`'s unit differs by market and neither venue document says
//!   so**: spot's `current` is a bare integer of epoch **milliseconds**
//!   (`1788332490408`, 13 digits); futures' `current` is a bare
//!   **fractional-second** float (`1788332492.975`). Reading the futures
//!   value as milliseconds would misplace the snapshot by three orders of
//!   magnitude; reading it through `f64` at all is avoided by parsing its
//!   exact text instead (see [`futures_timestamp`]) — this project never
//!   routes a number through a float, and a timestamp is no exception
//!   just because it isn't money.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request per
//! endpoint for this milestone, both already spent on the captures above.
//! - **Whether `limit` is clamped to a maximum** was not tested on either
//!   market — only 5 was requested. `MAX_DEPTH` is this project's own
//!   product choice for the panel, matching every other book source in
//!   this workspace, not a venue-documented ceiling.
//! - **Neither endpoint's rate-limit weight** is in any response header
//!   Gate sent here; `BOOK_FETCH_COST` mirrors `CANDLES_FETCH_COST` in
//!   this crate's `bars.rs`, this project's own conservative proactive
//!   budget.
//! - **An empty book** was not observed live on either market; `bids`/
//!   `asks` defaulting to an empty `Vec` on a missing field is this
//!   source's own defensive default, not a confirmed venue shape.

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

const BASE_URL: &str = "https://api.gateio.ws/api/v4";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs).
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, matching
/// `CANDLES_FETCH_COST` in this crate's `bars.rs`.
const BOOK_FETCH_COST: u32 = 5;

/// Which Gate market this source instance serves — the two response
/// shapes documented above, and the query-parameter name each expects
/// (`currency_pair` on spot, `contract` on futures).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Spot,
    UsdtPerp,
}

impl Market {
    const fn symbol_param(self) -> &'static str {
        match self {
            Self::Spot => "currency_pair",
            Self::UsdtPerp => "contract",
        }
    }
}

#[derive(Debug, Deserialize)]
struct SpotBook {
    /// Bare integer epoch milliseconds — see module docs.
    current: i64,
    #[serde(default)]
    bids: Vec<(String, String)>,
    #[serde(default)]
    asks: Vec<(String, String)>,
}

/// One futures level: a whole number of contracts and a string price —
/// see module docs on why this cannot share [`SpotBook`]'s shape.
#[derive(Debug, Deserialize)]
struct FuturesLevel {
    s: i64,
    p: String,
}

#[derive(Debug, Deserialize)]
struct FuturesBook {
    /// Bare fractional-second float, as exact text — see
    /// [`futures_timestamp`] and the module docs.
    current: Box<RawValue>,
    #[serde(default)]
    bids: Vec<FuturesLevel>,
    #[serde(default)]
    asks: Vec<FuturesLevel>,
}

/// Gate order-book depth, fetched through a [`VenueClient`] — a fresh
/// request per call, never a maintained local book.
#[derive(Debug, Clone)]
pub(crate) struct GateBookSource {
    source_id: &'static str,
    market: Market,
    url: String,
    client: VenueClient,
}

impl GateBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the spot book source, registered under [`crate::SPOT_ID`].
#[must_use]
pub(crate) fn book_source_spot(client: VenueClient) -> GateBookSource {
    GateBookSource {
        source_id: crate::SPOT_ID,
        market: Market::Spot,
        url: format!("{BASE_URL}/spot/order_book"),
        client,
    }
}

/// Builds the USDT perpetual book source, registered under
/// [`crate::USDT_PERP_ID`].
#[must_use]
pub(crate) fn book_source_usdt_perp(client: VenueClient) -> GateBookSource {
    GateBookSource {
        source_id: crate::USDT_PERP_ID,
        market: Market::UsdtPerp,
        url: format!("{BASE_URL}/futures/usdt/order_book"),
        client,
    }
}

/// Parses `[price, size]` string pairs into [`BookLevel`]s at the book's
/// own shared `price_scale`/`qty_scale` — spot's own shape. The caller
/// derives both scales from **both sides together**: an empty side
/// defaulting to its own scale of 0 would spuriously disagree with a
/// non-empty other side — exactly the mismatch [`BookSnapshot::new`]
/// exists to catch, tripped here by how the scale was computed rather
/// than by the venue.
///
/// A level whose size does not fit an `i64` at this scale — a real
/// possibility on any venue, per this workspace's `kucoin` bar source —
/// is left out rather than rounded: an honest absence, not a fabricated
/// number.
fn spot_side(
    raw: Vec<(String, String)>,
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
    let mut levels = Vec::with_capacity(raw.len());
    for (price, size) in raw {
        let Some(parsed_size) = parse_scaled(&size, qty_scale) else {
            tracing::warn!(
                price,
                size,
                "Gate spot reported a book level finer than a scaled i64 can hold; \
                 dropped, not rounded"
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

/// Parses futures levels into [`BookLevel`]s at the book's own shared
/// `price_scale` (see `spot_side`'s own docs on why it is derived from
/// both sides together). `s` is always a whole contract count already
/// decoded as `i64` by `serde` — never a fraction on this market (see
/// module docs) — so, unlike `spot_side`, there is no string-to-scaled-
/// integer step or overflow to guard on the quantity side.
fn futures_side(raw: Vec<FuturesLevel>, price_scale: u8) -> Result<Vec<BookLevel>, SourceError> {
    raw.into_iter()
        .map(|level| {
            Ok(BookLevel {
                price: scaled(&level.p, price_scale)?,
                size: level.s,
            })
        })
        .collect()
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// Parses `raw`'s exact text as `<seconds>.<fractional>`, never through
/// `f64` — see the module docs on why futures' `current` cannot be read
/// the way spot's is.
fn futures_timestamp(raw: &str) -> Option<UnixNanos> {
    let raw = raw.trim();
    let (secs_part, frac_part) = raw.split_once('.').unwrap_or((raw, ""));
    let secs: i64 = secs_part.parse().ok()?;
    let mut digits = frac_part.chars().chain(std::iter::repeat('0'));
    let ms: i64 = digits.by_ref().take(3).collect::<String>().parse().ok()?;
    let total_ms = secs.checked_mul(1000)?.checked_add(ms)?;
    UnixNanos::from_millis(total_ms)
}

#[async_trait]
impl BookSource for GateBookSource {
    fn source_id(&self) -> &str {
        self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!(
            "{}?{}={}&limit={depth}",
            self.url,
            self.market.symbol_param(),
            symbol.as_str()
        );
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;

        // Both sides' scales are derived jointly — see `spot_side`'s own
        // docs on why an empty side must never set its own, independent
        // scale.
        let (ts, bids, asks, price_scale, qty_scale) = match self.market {
            Market::Spot => {
                let book: SpotBook = serde_json::from_slice(&body).map_err(SourceError::decode)?;
                let ts = UnixNanos::from_millis(book.current).ok_or_else(|| {
                    SourceError::decode(format!("book current {} overflowed", book.current))
                })?;
                let price_scale = common_scale(
                    book.bids
                        .iter()
                        .chain(&book.asks)
                        .map(|(price, _)| price.as_str()),
                );
                let qty_scale = common_scale(
                    book.bids
                        .iter()
                        .chain(&book.asks)
                        .map(|(_, size)| size.as_str()),
                );
                let bids = spot_side(book.bids, price_scale, qty_scale)?;
                let asks = spot_side(book.asks, price_scale, qty_scale)?;
                (ts, bids, asks, price_scale, qty_scale)
            }
            Market::UsdtPerp => {
                let book: FuturesBook =
                    serde_json::from_slice(&body).map_err(SourceError::decode)?;
                let ts = futures_timestamp(book.current.get()).ok_or_else(|| {
                    SourceError::decode(format!(
                        "book current {:?} is not a valid timestamp",
                        book.current.get()
                    ))
                })?;
                let price_scale = common_scale(
                    book.bids
                        .iter()
                        .chain(&book.asks)
                        .map(|level| level.p.as_str()),
                );
                let bids = futures_side(book.bids, price_scale)?;
                let asks = futures_side(book.asks, price_scale)?;
                // Contracts are always whole — see `futures_side`'s own docs.
                (ts, bids, asks, price_scale, 0)
            }
        };

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
    use super::{book_source_spot, book_source_usdt_perp};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SPOT_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_spot.json");
    const PERP_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_usdt_perp.json");

    fn btc_usdt() -> SourceSymbol {
        SourceSymbol::assume("BTC_USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn serving(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn spot_levels_decode_best_price_first_at_the_correct_scale() {
        let server = serving(SPOT_BOOK).await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.asks[0].price, 776_170, "77617 at scale 1");
        assert_eq!(snapshot.bids[0].price, 776_169, "77616.9 at scale 1");
        assert!(
            snapshot.asks[0].price < snapshot.asks[1].price,
            "asks must stay best-first ascending"
        );
        assert!(
            snapshot.bids[0].price > snapshot.bids[1].price,
            "bids must stay best-first descending"
        );
        assert_eq!(snapshot.ts.as_millis(), 1_788_332_490_408);
    }

    #[tokio::test]
    async fn futures_levels_carry_a_whole_contract_count_not_a_fraction() {
        let server = serving(PERP_BOOK).await;
        let source = book_source_usdt_perp(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        assert_eq!(snapshot.qty_scale, 0, "contracts are always whole");
        assert_eq!(snapshot.asks[0].size, 6972);
        assert_eq!(snapshot.bids[0].size, 27_528);
    }

    #[tokio::test]
    async fn futures_current_is_fractional_seconds_not_milliseconds() {
        // Reading 1788332492.975 as milliseconds would land in 1970.
        let server = serving(PERP_BOOK).await;
        let source = book_source_usdt_perp(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(snapshot.ts.as_millis(), 1_788_332_492_975);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let server = serving(SPOT_BOOK).await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 500).await.unwrap();
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"current":1788332490408,"update":1788332490406,"asks":[],"bids":[]}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_spot_level_too_fine_for_a_scaled_i64_is_dropped_not_rounded() {
        // One level's size has 18 fractional digits, forcing the whole
        // side's shared scale to 18. Padded to that scale, the other
        // level's plain "100000" overflows an `i64` and must be dropped,
        // never rounded — the fine level that set the scale survives.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"current":1788332490408,"update":1788332490406,"asks":[],
                "bids":[["77616.9","0.000000000000000001"],["77614.8","100000"]]}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_spot(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&btc_usdt(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.len(),
            1,
            "the overflowing level is dropped, not the whole side"
        );
        assert_eq!(snapshot.bids[0].size, 1);
    }
}
