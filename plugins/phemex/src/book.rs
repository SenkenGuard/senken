//! Phemex order-book depth — `GET /md/orderbook`.
//!
//! **Not registered.** See the long comment in `lib.rs` where
//! `register_book_source` would go, and read on: this module has the same
//! per-symbol price-scale gap `plugins/phemex/src/bars.rs` already
//! documents for candles, and it is not safe to guess at for a live panel.
//!
//! # The obvious endpoint is the trap
//!
//! `GET /md/v2/orderbook?symbol=BTCUSD` returns **HTTP 500** on this venue,
//! confirmed live 2026-09-02:
//!
//! ```json
//! {"error":{"code":6001,"message":"invalid argument"},"id":null,"result":null}
//! ```
//!
//! The working path is the older, undocumented-sounding
//! `GET /md/orderbook`, used throughout this module.
//!
//! # What was confirmed live, 2026-09-02
//!
//! `GET https://api.phemex.com/md/orderbook?symbol=BTCUSD` returned
//! `HTTP 200`:
//!
//! ```json
//! {"error":null,"id":0,"result":{"book":{
//! "asks":[[775595000,45623],[775597000,62236], … 30 levels],
//! "bids":[[775594000,14436],[775556000,200000], … 30 levels]},
//! "depth":30,"sequence":23610201470,"symbol":"BTCUSD",
//! "timestamp":1788332427954609378,"type":"snapshot"}}
//! ```
//!
//! Confirmed from this capture:
//! - The envelope is `{error, id, result}` — the same JSON-RPC-shaped
//!   wrapper the trap endpoint's error response uses (`error` non-null
//!   there, null here), so `error` is checked the same way on this
//!   endpoint even though this one capture never exercised it non-null.
//! - Each level is a **two-element array of bare JSON integers**, `[price,
//!   size]` — never strings, and (unlike every decimal-string venue in
//!   this workspace) never run through [`senken_core::parse_scaled`]: a
//!   bare JSON integer has no fractional digits to lose, so it is read
//!   directly as `i64`, the identical treatment `bars.rs` gives Phemex's
//!   kline fields.
//! - **This endpoint returns a fixed 30 levels a side — the `depth` field
//!   in the envelope confirms it — and takes no depth/limit query
//!   parameter at all** in the one request this session made (only
//!   `symbol` was sent). A caller asking for fewer levels would have to
//!   be honoured by truncating locally after the fetch, the same as
//!   `senken-plugin-kucoin`'s `level2_20` book source next to this one.
//! - **`timestamp` is already nanoseconds** — nineteen digits,
//!   `1788332427954609378`, which is
//!   [`senken_core::UnixNanos::from_nanos`] directly, needing no unit
//!   conversion at all. This is a third timestamp unit for Phemex in this
//!   workspace: `bars.rs`'s `kline/list` uses whole seconds, and this
//!   endpoint uses nanoseconds — a fact worth restating precisely because
//!   the two look nothing alike and a copy-pasted conversion from one
//!   source to the other would silently misplace every timestamp by nine
//!   orders of magnitude.
//! - Both sides arrive best-first already: `asks` ascending from
//!   `775595000` (the lowest, best ask), `bids` descending from
//!   `775594000` (the highest, best bid) — this source does not re-sort
//!   them.
//!
//! # Two endpoints, chosen by the symbol's own scale
//!
//! Phemex serves depth in the same two shapes it serves klines in, and
//! [`crate::scales`] is what says which a symbol uses:
//!
//! ```text
//! priceScale > 0  ->  /md/orderbook      result.book        [[764355000, 27873], ...]
//! priceScale == 0 ->  /md/v2/orderbook   result.orderbook_p [["76482.4", "0.013"], ...]
//! ```
//!
//! Both recorded live 2026-09-02. The first pair are integers at that
//! symbol's own price and quantity scales; the second are ordinary
//! decimal text. Asking the integer endpoint about a V2 linear symbol
//! answers, but with a book that is not the one requested — so the choice
//! is made from the catalogue, never from the symbol's spelling.

use async_trait::async_trait;
use senken_core::UnixNanos;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::VenueClient;
use serde::Deserialize;

/// Depth for a symbol whose numbers are pre-scaled integers.
const ORDERBOOK_URL: &str = "https://api.phemex.com/md/orderbook";

/// Depth for a symbol whose numbers are decimal text — the V2 linear
/// perpetuals.
const ORDERBOOK_V2_URL: &str = "https://api.phemex.com/md/v2/orderbook";

/// Scale a decimal-family book is stored at. Its `tickSize` is `0.1` and
/// its `qtyStepSize` `0.001`, so these are finer than the venue quotes and
/// leave no rounding to do; a level finer than this is refused, never
/// rounded.
const V2_PRICE_SCALE: u8 = 4;
/// See [`V2_PRICE_SCALE`].
const V2_QTY_SCALE: u8 = 8;

/// This project's own fixed panel depth — a product choice. The venue
/// itself never returns more than 30 a side in this capture (see module
/// docs), so this only ever narrows a request, never widens one.
const MAX_DEPTH: usize = 30;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number (see module docs).
const BOOK_FETCH_COST: u32 = 5;

/// One level: `[price, size]`, already-scaled bare integers — never
/// decimal strings. See the module docs.
type RawLevel = (i64, i64);

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    error: Option<RawError>,
    #[serde(default)]
    result: Option<RawResult>,
}

#[derive(Debug, Deserialize)]
struct RawError {
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct RawResult {
    /// Present on `/md/orderbook`: pre-scaled integers.
    #[serde(default)]
    book: Option<RawBookLevels>,
    /// Present on `/md/v2/orderbook`: decimal text.
    #[serde(default)]
    orderbook_p: Option<RawDecimalLevels>,
    /// Nanoseconds on the integer endpoint; the V2 one names it `dts`.
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    dts: i64,
}

#[derive(Debug, Default, Deserialize)]
struct RawBookLevels {
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
}

/// One V2 level: `[price, size]` as decimal strings.
type RawDecimalLevel = (String, String);

#[derive(Debug, Default, Deserialize)]
struct RawDecimalLevels {
    #[serde(default)]
    asks: Vec<RawDecimalLevel>,
    #[serde(default)]
    bids: Vec<RawDecimalLevel>,
}

/// Phemex order-book depth, fetched through a
/// [`senken_venue::VenueClient`] — a fresh request per call, never a
/// maintained local book. **Not registered** — see the module docs.
#[derive(Debug, Clone)]
pub struct PhemexBookSource {
    source_id: &'static str,
    url: String,
    v2_url: String,
    client: VenueClient,
    scales: crate::scales::ScaleCatalog,
}

impl PhemexBookSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `PhemexPerpBarSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Points the decimal-family endpoint at a different URL.
    #[must_use]
    pub fn with_v2_url(mut self, url: impl Into<String>) -> Self {
        self.v2_url = url.into();
        self
    }
}

/// Builds a [`PhemexBookSource`] for `source_id` against the real Phemex
/// endpoints.
#[must_use]
pub fn book_source(
    source_id: &'static str,
    client: VenueClient,
    scales: crate::scales::ScaleCatalog,
) -> PhemexBookSource {
    PhemexBookSource {
        source_id,
        url: ORDERBOOK_URL.to_owned(),
        v2_url: ORDERBOOK_V2_URL.to_owned(),
        client,
        scales,
    }
}

fn to_levels(raw: Vec<RawLevel>) -> Vec<BookLevel> {
    raw.into_iter()
        .map(|(price, size)| BookLevel { price, size })
        .collect()
}

/// Decimal-text levels, read at the V2 family's fixed scales.
fn decimal_levels(raw: Vec<RawDecimalLevel>) -> Result<Vec<BookLevel>, SourceError> {
    raw.into_iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: at(&price, V2_PRICE_SCALE)?,
                size: at(&size, V2_QTY_SCALE)?,
            })
        })
        .collect()
}

fn at(raw: &str, scale: u8) -> Result<i64, SourceError> {
    senken_core::parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[async_trait]
impl BookSource for PhemexBookSource {
    fn source_id(&self) -> &str {
        self.source_id
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let scales = self.scales.get(symbol.as_str()).await?;
        let base = if scales.is_decimal() {
            &self.v2_url
        } else {
            &self.url
        };
        let url = format!("{base}?symbol={}", symbol.as_str());
        let body = self.client.get(&url, BOOK_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if let Some(error) = envelope.error {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                error.code, error.message
            )));
        }
        let result = envelope
            .result
            .ok_or_else(|| SourceError::rejected("no result and no error in the response"))?;

        let (mut bids, mut asks, price_scale, qty_scale) = if scales.is_decimal() {
            let levels = result
                .orderbook_p
                .ok_or_else(|| SourceError::rejected("no orderbook_p in a V2 response"))?;
            (
                decimal_levels(levels.bids)?,
                decimal_levels(levels.asks)?,
                V2_PRICE_SCALE,
                V2_QTY_SCALE,
            )
        } else {
            let levels = result
                .book
                .ok_or_else(|| SourceError::rejected("no book in the response"))?;
            (
                to_levels(levels.bids),
                to_levels(levels.asks),
                scales.price,
                scales.quantity,
            )
        };
        bids.truncate(depth);
        asks.truncate(depth);

        // The integer endpoint names its instant `timestamp`; the V2 one
        // names it `dts`. Both are nanoseconds.
        let ts = UnixNanos::from_nanos(if result.timestamp != 0 {
            result.timestamp
        } else {
            result.dts
        });

        // Both sides arrive best-first already (see module docs) — trusted
        // rather than re-sorted, the same as `senken-plugin-okx`'s book
        // source.
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
    use super::{ORDERBOOK_URL, book_source};
    use senken_marketdata::SourceSymbol;
    use senken_subscription::BookSource;
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book.json");

    fn btc_usd() -> SourceSymbol {
        SourceSymbol::assume("BTCUSD")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    /// A real `GET /public/products` response — the document that says
    /// how each symbol's numbers are written.
    const PRODUCTS: &[u8] = include_bytes!("../tests/fixtures/products.json");
    /// A real `GET /md/v2/orderbook?symbol=BTCUSDT` response, recorded
    /// 2026-09-02: decimal text, under `orderbook_p`.
    const BOOK_V2: &[u8] = include_bytes!("../tests/fixtures/book_linear.json");

    async fn serving(integer_book: &'static [u8], v2_book: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/public/products"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(PRODUCTS, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/md/v2/orderbook"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(v2_book, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/md/orderbook"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(integer_book, "application/json"))
            .mount(&server)
            .await;
        server
    }

    fn source_against(server: &MockServer) -> super::PhemexBookSource {
        let scales = crate::scales::ScaleCatalog::new(test_client())
            .with_url(format!("{}/public/products", server.uri()));
        book_source(crate::PERP_ID, test_client(), scales)
            .with_url(format!("{}/md/orderbook", server.uri()))
            .with_v2_url(format!("{}/md/v2/orderbook", server.uri()))
    }

    async fn mock_source() -> (MockServer, super::PhemexBookSource) {
        let server = serving(BOOK, BOOK_V2).await;
        let source = source_against(&server);
        (server, source)
    }

    #[test]
    fn the_real_urls_are_used_by_default() {
        let scales = crate::scales::ScaleCatalog::new(test_client());
        let source = book_source(crate::PERP_ID, test_client(), scales);
        assert_eq!(
            source.url, ORDERBOOK_URL,
            "must default to the real Phemex endpoint, not require with_url"
        );
        assert_eq!(source.v2_url, super::ORDERBOOK_V2_URL);
    }

    /// The V2 linear family answers a different endpoint, in decimal
    /// text, under a differently named key. Asking the integer endpoint
    /// about one of these symbols answers — with a book that is not the
    /// one requested — so the catalogue decides, not the spelling.
    #[tokio::test]
    async fn a_decimal_family_symbol_is_read_from_the_v2_endpoint() {
        let server = serving(BOOK, BOOK_V2).await;
        let source = source_against(&server);

        let snapshot = source
            .book_snapshot(&SourceSymbol::assume("BTCUSDT"), 5)
            .await
            .unwrap();

        // "76482.4" at four digits, "0.013" at eight.
        assert_eq!(snapshot.asks[0].price, 764_824_000);
        assert_eq!(snapshot.price_scale, 4);
        assert_eq!(snapshot.qty_scale, 8);
        assert!(snapshot.ts.as_nanos() > 0, "the V2 endpoint names it `dts`");
    }

    /// The scale carried on the snapshot has to be the symbol's own, not
    /// a constant: `BTCUSD` is 4 and `sBTCUSDT` is 8 on the same venue.
    #[tokio::test]
    async fn the_snapshot_carries_the_symbols_own_price_scale() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 5).await.unwrap();
        assert_eq!(snapshot.price_scale, 4);
        assert_eq!(snapshot.qty_scale, 0, "inverse sizes are contract counts");
    }

    /// Refusing beats guessing: a scale invented for an unlisted symbol
    /// is how a book ends up four orders of magnitude out.
    #[tokio::test]
    async fn a_symbol_absent_from_the_product_list_is_refused() {
        let (_server, source) = mock_source().await;
        assert!(
            source
                .book_snapshot(&SourceSymbol::assume("NOTLISTED"), 5)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fixture_levels_decode_as_bare_pre_scaled_integers() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 30).await.unwrap();

        assert_eq!(snapshot.bids.len(), 30, "the venue's own fixed depth");
        assert_eq!(snapshot.asks.len(), 30);
        // Row 0: bid price 775594000, ask price 775595000, sizes 14436 and
        // 45623, used verbatim, never run through `parse_scaled`.
        assert_eq!(snapshot.bids[0].price, 775_594_000);
        assert_eq!(snapshot.asks[0].price, 775_595_000);
        assert_eq!(snapshot.bids[0].size, 14_436);
        assert_eq!(snapshot.asks[0].size, 45_623);
    }

    #[tokio::test]
    async fn bids_and_asks_are_both_best_price_first() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 30).await.unwrap();

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
    async fn the_timestamp_is_read_as_nanoseconds_not_seconds() {
        // Nineteen digits — reading it as seconds or milliseconds the way
        // `bars.rs`'s kline endpoint does would misplace this by many
        // orders of magnitude.
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 30).await.unwrap();
        assert_eq!(snapshot.ts.as_nanos(), 1_788_332_427_954_609_378);
    }

    #[tokio::test]
    async fn a_requested_depth_below_the_venues_fixed_size_is_honoured_locally() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 5).await.unwrap();

        assert_eq!(snapshot.bids.len(), 5);
        assert_eq!(snapshot.asks.len(), 5);
    }

    #[tokio::test]
    async fn a_requested_depth_above_the_venues_fixed_size_is_clamped_not_rejected() {
        let (_server, source) = mock_source().await;
        let snapshot = source.book_snapshot(&btc_usd(), 500).await.unwrap();
        assert!(snapshot.bids.len() <= super::MAX_DEPTH);
    }

    #[tokio::test]
    async fn an_empty_book_is_an_absence_not_an_error() {
        let server = serving(
            br#"{"error":null,"id":0,"result":{"book":{"asks":[],"bids":[]},
                "depth":30,"sequence":0,"symbol":"BTCUSD","timestamp":0,"type":"snapshot"}}"#,
            BOOK_V2,
        )
        .await;
        let source = source_against(&server);

        let snapshot = source.book_snapshot(&btc_usd(), 30).await.unwrap();

        assert!(snapshot.bids.is_empty());
        assert!(snapshot.asks.is_empty());
    }

    #[tokio::test]
    async fn an_application_error_inside_http_200_is_a_rejection() {
        // The exact envelope shape observed live on the trap endpoint
        // (`/md/v2/orderbook`, HTTP 500) — reused here because both
        // endpoints share the same `{error, id, result}` wrapper, and this
        // one has never itself been observed to fail.
        let server = serving(
            br#"{"error":{"code":6001,"message":"invalid argument"},"id":null,"result":null}"#,
            BOOK_V2,
        )
        .await;
        let source = source_against(&server);

        let error = source.book_snapshot(&btc_usd(), 30).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
