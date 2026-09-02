//! Kraken order-book depth — spot `GET /0/public/Depth` and Kraken Futures
//! `GET /derivatives/api/v3/orderbook`. Two different endpoints, two
//! different traps, so two source types rather than one shared shape.
//!
//! # Spot: confirmed live, 2026-09-02
//!
//! `GET https://api.kraken.com/0/public/Depth?pair=XBTUSD&count=5`:
//!
//! ```json
//! {"error":[],"result":{"XXBTZUSD":{
//! "asks":[["77586.40000","0.001",1788332500],["77588.70000","0.001",1788332487],
//! ["77591.30000","0.072",1788332499],["77592.50000","0.001",1788332484],
//! ["77594.70000","0.203",1788332500]],
//! "bids":[["77586.30000","0.864",1788332500],["77585.00000","0.084",1788332500],
//! ["77584.90000","0.001",1788332492],["77584.70000","0.593",1788332492],
//! ["77581.00000","0.001",1788332492]]}}}
//! ```
//!
//! Confirmed from this capture:
//! - **The legacy pair name, not the query symbol, again**: `result` is
//!   keyed `XXBTZUSD`, exactly the trap `bars.rs`'s own module docs
//!   record for `OHLC` — reused here the same way, reading whichever key
//!   `result` carries rather than matching it against the request symbol.
//!   Unlike `OHLC`'s `result`, this endpoint's has no sibling `last` key
//!   to skip past — there is exactly one entry, whatever it is named.
//! - **No snapshot-level timestamp at all.** Each level carries its own
//!   trailing integer (epoch seconds, last update), but nothing at the
//!   response's top level names "when this book was true". This source
//!   reports the most recent of those per-level times when any level
//!   exists, and falls back to [`Clock::now`] only when both sides are
//!   empty and no level timestamp exists to read — see
//!   [`KrakenSpotBookSource`]'s own docs.
//! - `count=5` was honoured: exactly 5 levels came back per side.
//! - Levels arrive best price first on each side already (asks ascending
//!   from `77586.40000`, bids descending from `77586.30000`); sorted again
//!   here anyway, the same defensive stance every bar source in this
//!   workspace takes — a venue's order is not a promise.
//! - Price and size are both strings, decoded the usual scaled-integer
//!   way.
//! - Kraken reports argument errors inside an HTTP 200 body's `error`
//!   array, the same convention `bars.rs` already handles for `OHLC`.
//!
//! # Futures: confirmed live, 2026-09-02
//!
//! `GET https://futures.kraken.com/derivatives/api/v3/orderbook?symbol=PI_XBTUSD`
//! returned `HTTP 200`, `orderBook.bids` holding 72 levels and
//! `orderBook.asks` holding 27 — no `count`/`depth` parameter exists on
//! this endpoint at all, so none was sent; the full resting book came
//! back regardless (see the recorded fixture, kept whole rather than
//! trimmed — a truncated recording would not be a recording).
//!
//! Confirmed from this capture:
//! - **Prices and sizes are bare JSON numbers**, not strings — unlike
//!   spot. Decoded as [`RawValue`] and read through
//!   [`senken_core::parse_scaled`] directly, no `f64` anywhere in the
//!   path (see this crate's own top-level `AGENTS.md`).
//! - **Asks arrive ascending (best price first)**, matching every other
//!   venue in this workspace: `[77614.5,28]` first, `[999999,4]` last.
//! - **Bids arrive ascending too** — `[31126.5,10]` first, `[77529,27]`
//!   last — which is **worst price first**, the opposite of every other
//!   book source in this workspace. Trusting this order would hand a
//!   caller the least competitive bids as "the top of book"; this source
//!   sorts bids descending itself before truncating, rather than assuming
//!   the venue's own order means anything.
//! - **`serverTime` is an ISO-8601 string** (`"2026-09-02T07:01:42.049Z"`),
//!   read with [`senken_venue::iso8601_ms`], the same helper this crate's
//!   own `futures_instrument` already uses for `lastTradingTime`.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request per
//! endpoint for this milestone, both already spent on the captures above.
//! - **The futures orderbook endpoint's own error shape** was not
//!   observed live — only a successful `"result":"success"` response was
//!   captured. `DepthResponse`'s `result`/`error` fields mirror
//!   `InstrumentsResponse` in this crate's own `api.rs`, the sibling
//!   endpoint on the same `/derivatives/api/v3/` host, rather than being
//!   independently reconfirmed here.
//! - **Whether spot's `count` is clamped to a maximum**, and **whatever
//!   panel depth is applied to futures' unbounded response**, are both
//!   this project's own product choice (`MAX_DEPTH`), not a venue-
//!   documented ceiling.
//! - **Neither endpoint's rate-limit weight** is in any response header
//!   sent here; `BOOK_FETCH_COST` mirrors `CANDLES_FETCH_COST` in this
//!   crate's `bars.rs`, this project's own conservative proactive budget.
//! - **An empty book** was not observed live on either market; empty
//!   `bids`/`asks` are this source's own defensive default on a missing
//!   field, not a confirmed venue shape.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_series::Clock;
use senken_venue::{VenueClient, common_scale, iso8601_ms};
use serde::Deserialize;
use serde_json::value::RawValue;

const SPOT_DEPTH_URL: &str = "https://api.kraken.com/0/public/Depth";
const FUTURES_ORDERBOOK_URL: &str = "https://futures.kraken.com/derivatives/api/v3/orderbook";

/// This project's own fixed panel depth — a product choice, not a
/// venue-documented ceiling (see module docs). On futures this is also
/// the truncation point applied client-side, since the venue returns its
/// whole book regardless of anything requested.
const MAX_DEPTH: usize = 20;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, matching
/// `CANDLES_FETCH_COST` in this crate's `bars.rs`.
const BOOK_FETCH_COST: u32 = 5;

/// One spot level: `[price, size, last-update epoch seconds]`.
type SpotRawLevel = (String, String, i64);

#[derive(Debug, Deserialize)]
struct SpotDepthResponse {
    #[serde(default)]
    error: Vec<String>,
    #[serde(default)]
    result: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SpotRawBook {
    #[serde(default)]
    bids: Vec<SpotRawLevel>,
    #[serde(default)]
    asks: Vec<SpotRawLevel>,
}

/// Kraken spot order-book depth. Needs a [`Clock`], unlike every other
/// book source in this workspace, because this endpoint sends no
/// snapshot-level timestamp of its own — see the module docs.
#[derive(Clone)]
pub(crate) struct KrakenSpotBookSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for KrakenSpotBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KrakenSpotBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl KrakenSpotBookSource {
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
pub(crate) fn book_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> KrakenSpotBookSource {
    KrakenSpotBookSource {
        url: SPOT_DEPTH_URL.to_owned(),
        client,
        clock,
    }
}

/// Parses one spot side's raw levels at the book's own shared
/// `price_scale`/`qty_scale` — derived from **both** sides together in
/// [`BookSource::book_snapshot`] below, never one side alone (an empty
/// side defaulting to its own scale of 0 would spuriously disagree with a
/// non-empty other side — exactly the mismatch [`BookSnapshot::new`]
/// exists to catch, tripped here by how the scale was computed rather
/// than by the venue). Sorted into `ascending` order (`false` for bids —
/// best, highest price first; `true` for asks — best, lowest price
/// first) regardless of the order the venue answered in, then truncated
/// to `depth`. Also returns this side's own most-recent per-level update
/// time.
///
/// A level whose size does not fit an `i64` at this scale — a real
/// possibility on any venue, per this workspace's `kucoin` bar source —
/// is left out rather than rounded: an honest absence, not a fabricated
/// number.
fn parse_spot_side(
    raw: Vec<SpotRawLevel>,
    price_scale: u8,
    qty_scale: u8,
    ascending: bool,
    depth: usize,
) -> Result<(Vec<BookLevel>, Option<i64>), SourceError> {
    let mut levels = Vec::with_capacity(raw.len());
    let mut latest_ts: Option<i64> = None;
    for (price, size, ts) in raw {
        latest_ts = Some(latest_ts.map_or(ts, |current| current.max(ts)));
        let Some(parsed_size) = parse_scaled(&size, qty_scale) else {
            tracing::warn!(
                price,
                size,
                "Kraken reported a spot book level finer than a scaled i64 can hold; \
                 dropped, not rounded"
            );
            continue;
        };
        levels.push(BookLevel {
            price: scaled(&price, price_scale)?,
            size: parsed_size,
        });
    }
    if ascending {
        levels.sort_by(|a, b| a.price.cmp(&b.price).then(a.size.cmp(&b.size)));
    } else {
        levels.sort_by(|a, b| b.price.cmp(&a.price).then(a.size.cmp(&b.size)));
    }
    levels.truncate(depth);
    Ok((levels, latest_ts))
}

fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for KrakenSpotBookSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?pair={}&count={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response: SpotDepthResponse =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if let Some(first) = response.error.first() {
            // Kraken reports argument errors inside an HTTP 200 body.
            return Err(SourceError::rejected(first.clone()));
        }
        // Whichever key `result` carries — see the module docs on why this
        // never matches against the request symbol.
        let book = match response.result.into_iter().next() {
            Some((_, value)) => {
                serde_json::from_value::<SpotRawBook>(value).map_err(SourceError::decode)?
            }
            None => SpotRawBook {
                bids: Vec::new(),
                asks: Vec::new(),
            },
        };

        // Derived from both sides together — see `parse_spot_side`'s own
        // docs on why an empty side must never set its own, independent
        // scale.
        let price_scale = common_scale(
            book.bids
                .iter()
                .chain(&book.asks)
                .map(|(p, _, _)| p.as_str()),
        );
        let qty_scale = common_scale(
            book.bids
                .iter()
                .chain(&book.asks)
                .map(|(_, s, _)| s.as_str()),
        );
        let (bids, bids_ts) = parse_spot_side(book.bids, price_scale, qty_scale, false, depth)?;
        let (asks, asks_ts) = parse_spot_side(book.asks, price_scale, qty_scale, true, depth)?;

        let ts = match bids_ts.into_iter().chain(asks_ts).max() {
            Some(secs) => UnixNanos::from_secs(secs)
                .ok_or_else(|| SourceError::decode(format!("level ts {secs}s overflowed")))?,
            None => self.clock.now(),
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

/// One futures level: `[price, size]`, both bare JSON numbers — unlike
/// spot (see module docs).
type FuturesRawLevel = (Box<RawValue>, Box<RawValue>);

#[derive(Debug, Deserialize)]
struct FuturesDepthResponse {
    /// `"success"` on success — see module docs on why this mirrors
    /// `InstrumentsResponse` rather than being independently reconfirmed.
    #[serde(default)]
    result: String,
    #[serde(default)]
    error: String,
    #[serde(default, rename = "serverTime")]
    server_time: String,
    #[serde(default, rename = "orderBook")]
    order_book: FuturesRawBook,
}

#[derive(Debug, Default, Deserialize)]
struct FuturesRawBook {
    #[serde(default)]
    bids: Vec<FuturesRawLevel>,
    #[serde(default)]
    asks: Vec<FuturesRawLevel>,
}

/// Kraken Futures order-book depth. This endpoint has no `count`/`depth`
/// parameter and returns its entire resting book regardless — depth is
/// enforced entirely client-side, after sorting (see module docs).
#[derive(Debug, Clone)]
pub(crate) struct KrakenFuturesBookSource {
    url: String,
    client: VenueClient,
}

impl KrakenFuturesBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the futures book source, registered under [`crate::FUTURES_ID`].
#[must_use]
pub(crate) fn book_source_futures(client: VenueClient) -> KrakenFuturesBookSource {
    KrakenFuturesBookSource {
        url: FUTURES_ORDERBOOK_URL.to_owned(),
        client,
    }
}

/// Parses one futures side's raw levels at the book's own shared
/// `price_scale`/`qty_scale` — derived from **both** sides together in
/// [`BookSource::book_snapshot`] below, never one side alone (see
/// `parse_spot_side`'s own docs on why). Sorted into `ascending` order
/// (`true` for asks, `false` for bids) regardless of the order the venue
/// answered in, then truncated to `depth`. See the module docs on why
/// bids specifically cannot be trusted to arrive sorted.
///
/// A level whose size does not fit an `i64` at this scale — a real
/// possibility on any venue, per this workspace's `kucoin` bar source —
/// is left out rather than rounded: an honest absence, not a fabricated
/// number.
fn parse_futures_side(
    raw: Vec<FuturesRawLevel>,
    price_scale: u8,
    qty_scale: u8,
    ascending: bool,
    depth: usize,
) -> Result<Vec<BookLevel>, SourceError> {
    let mut levels = Vec::with_capacity(raw.len());
    for (price, size) in raw {
        let Some(parsed_size) = parse_scaled(size.get(), qty_scale) else {
            tracing::warn!(
                price = price.get(),
                size = size.get(),
                "Kraken Futures reported a book level finer than a scaled i64 can hold; \
                 dropped, not rounded"
            );
            continue;
        };
        levels.push(BookLevel {
            price: scaled(price.get(), price_scale)?,
            size: parsed_size,
        });
    }
    if ascending {
        levels.sort_by(|a, b| a.price.cmp(&b.price).then(a.size.cmp(&b.size)));
    } else {
        levels.sort_by(|a, b| b.price.cmp(&a.price).then(a.size.cmp(&b.size)));
    }
    levels.truncate(depth);
    Ok(levels)
}

#[async_trait]
impl BookSource for KrakenFuturesBookSource {
    fn source_id(&self) -> &str {
        crate::FUTURES_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        // No count/depth parameter exists on this endpoint — see module
        // docs. Sending one would silently do nothing.
        let url = format!("{}?symbol={}", self.url, symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let response: FuturesDepthResponse =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !response.result.is_empty() && response.result != "success" {
            return Err(SourceError::rejected(format!(
                "{}: {}",
                response.result, response.error
            )));
        }

        let ts_ms = iso8601_ms(&response.server_time).ok_or_else(|| {
            SourceError::decode(format!(
                "{:?} is not a valid ISO-8601 timestamp",
                response.server_time
            ))
        })?;
        let ts = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| SourceError::decode(format!("serverTime {ts_ms}ms overflowed")))?;

        // Derived from both sides together — see `parse_spot_side`'s own
        // docs (shared reasoning) on why an empty side must never set its
        // own, independent scale.
        let price_scale = common_scale(
            response
                .order_book
                .bids
                .iter()
                .chain(&response.order_book.asks)
                .map(|(price, _)| price.get()),
        );
        let qty_scale = common_scale(
            response
                .order_book
                .bids
                .iter()
                .chain(&response.order_book.asks)
                .map(|(_, size)| size.get()),
        );
        let bids = parse_futures_side(
            response.order_book.bids,
            price_scale,
            qty_scale,
            false,
            depth,
        )?;
        let asks = parse_futures_side(
            response.order_book.asks,
            price_scale,
            qty_scale,
            true,
            depth,
        )?;

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
    use std::sync::Arc;

    use senken_core::UnixNanos;
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_series::Clock;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{book_source_futures, book_source_spot};

    const SPOT_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_spot.json");
    const FUTURES_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_futures.json");

    #[derive(Debug)]
    struct FixedClock(i64);

    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_millis(self.0).unwrap()
        }

        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn xbtusd() -> SourceSymbol {
        SourceSymbol::assume("XBTUSD")
    }

    fn pi_xbtusd() -> SourceSymbol {
        SourceSymbol::assume("PI_XBTUSD")
    }

    async fn serving(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        server
    }

    // --- spot ---

    #[tokio::test]
    async fn spot_levels_decode_best_price_first_at_the_correct_scale() {
        let server = serving(SPOT_BOOK).await;
        let source =
            book_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let snapshot = source.book_snapshot(&xbtusd(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
        // Trailing zeros never count toward scale (`decimal_places`
        // trims them), so "77586.40000" only contributes one significant
        // fractional digit, not five.
        assert_eq!(snapshot.price_scale, 1);
        assert_eq!(snapshot.asks[0].price, 775_864, "77586.40000 at scale 1");
        assert_eq!(snapshot.bids[0].price, 775_863, "77586.30000 at scale 1");
        assert!(
            snapshot.asks[0].price < snapshot.asks[1].price,
            "asks must sort best-first ascending"
        );
        assert!(
            snapshot.bids[0].price > snapshot.bids[1].price,
            "bids must sort best-first descending"
        );
    }

    #[tokio::test]
    async fn the_rows_are_read_from_the_legacy_key_not_the_query_symbol() {
        // The fixture's object is keyed `XXBTZUSD`; the request was made
        // (and this source is asked) with `XBTUSD`.
        let server = serving(SPOT_BOOK).await;
        let source =
            book_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let snapshot = source.book_snapshot(&xbtusd(), 5).await.unwrap();

        assert!(!snapshot.bids.is_empty() && !snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn the_snapshot_timestamp_is_the_most_recent_level_update() {
        // No snapshot-level timestamp exists on this endpoint at all — see
        // the module docs. The fixture's most recent per-level time is
        // 1788332500.
        let server = serving(SPOT_BOOK).await;
        let source =
            book_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let snapshot = source.book_snapshot(&xbtusd(), 5).await.unwrap();

        assert_eq!(snapshot.ts, UnixNanos::from_secs(1_788_332_500).unwrap());
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error_and_falls_back_to_the_clock() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"error":[],"result":{"XXBTZUSD":{"asks":[],"bids":[]}}}"#),
            )
            .mount(&server)
            .await;
        let clock_ts = UnixNanos::from_millis(1_788_332_500_000).unwrap();
        let source = book_source_spot(test_client(), Arc::new(FixedClock(1_788_332_500_000)))
            .with_url(server.uri());

        let snapshot = source.book_snapshot(&xbtusd(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
        assert_eq!(
            snapshot.ts, clock_ts,
            "no level exists to read a timestamp from, so the clock is used"
        );
    }

    #[tokio::test]
    async fn an_error_array_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"error":["EGeneral:Invalid arguments"],"result":{}}"#),
            )
            .mount(&server)
            .await;
        let source =
            book_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let error = source.book_snapshot(&xbtusd(), 5).await.unwrap_err();
        assert!(error.to_string().contains("EGeneral"));
    }

    #[tokio::test]
    async fn a_spot_level_too_fine_for_a_scaled_i64_is_dropped_not_rounded() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"error":[],"result":{"XXBTZUSD":{"asks":[],
                "bids":[["77586.30000","0.000000000000000001",1788332500],
                ["77585.00000","100000",1788332500]]}}}"#,
            ))
            .mount(&server)
            .await;
        let source =
            book_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());

        let snapshot = source.book_snapshot(&xbtusd(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.len(),
            1,
            "the overflowing level is dropped, not the whole side"
        );
        assert_eq!(snapshot.bids[0].size, 1);
    }

    // --- futures ---

    #[tokio::test]
    async fn futures_bids_are_re_sorted_best_first_since_the_venue_sends_them_worst_first() {
        let server = serving(FUTURES_BOOK).await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        // The fixture's raw bids array starts [31126.5,...], its worst
        // price. The best bid in the fixture is 77529.
        assert_eq!(snapshot.price_scale, 1);
        assert_eq!(snapshot.bids[0].price, 775_290, "77529 at scale 1");
        assert!(
            snapshot.bids.windows(2).all(|w| w[0].price >= w[1].price),
            "bids must come out descending regardless of the venue's own order"
        );
    }

    #[tokio::test]
    async fn futures_asks_stay_best_first_ascending() {
        let server = serving(FUTURES_BOOK).await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        assert!(
            snapshot.asks.windows(2).all(|w| w[0].price <= w[1].price),
            "asks must come out ascending"
        );
        assert_eq!(snapshot.asks[0].price, 776_145, "77614.5 at scale 1");
    }

    #[tokio::test]
    async fn the_requested_depth_is_respected_even_though_the_venue_ignores_it() {
        // The fixture carries 72 bids and 27 asks; the venue took no
        // count/depth parameter and sent all of them regardless. This is
        // Kraken Futures' whole point: depth has to be enforced here.
        let server = serving(FUTURES_BOOK).await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_panel_cap_is_clamped_not_rejected() {
        let server = serving(FUTURES_BOOK).await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
        assert!(snapshot.asks.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn the_server_time_is_read_from_iso_8601_not_epoch_seconds() {
        let server = serving(FUTURES_BOOK).await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        assert_eq!(
            snapshot.ts,
            UnixNanos::from_millis(1_788_332_502_049).unwrap()
        );
    }

    #[tokio::test]
    async fn an_empty_futures_book_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":"success","serverTime":"2026-09-02T07:01:42.049Z",
                "orderBook":{"bids":[],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn a_non_success_result_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":"error","error":"invalid_symbol","serverTime":"2026-09-02T07:01:42.049Z",
                "orderBook":{"bids":[],"asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn a_futures_level_too_fine_for_a_scaled_i64_is_dropped_not_rounded() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":"success","serverTime":"2026-09-02T07:01:42.049Z",
                "orderBook":{"bids":[[77586.4,0.000000000000000001],[77585.0,100000]],
                "asks":[]}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source_futures(test_client()).with_url(server.uri());

        let snapshot = source.book_snapshot(&pi_xbtusd(), 5).await.unwrap();

        assert_eq!(
            snapshot.bids.len(),
            1,
            "the overflowing level is dropped, not the whole side"
        );
        assert_eq!(snapshot.bids[0].size, 1);
    }
}
