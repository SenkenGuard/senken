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
//! # Why this module is not registered — the price scale is a guess
//!
//! Exactly the caveat `bars.rs` already carries, restated for this
//! endpoint: Phemex's own convention is that price scale is per-symbol,
//! and `plugins/phemex/src/api.rs` does not currently capture that
//! `priceScale` from the product catalogue at all. `PRICE_SCALE` below is
//! `4`, the same figure `bars.rs` inferred for `BTCUSD` specifically by
//! comparing its kline prices against BTC's simultaneously observed real
//! price on other venues — reused here because this capture's own best
//! bid (`775594000 / 10^4 = 77559.4`) lands in the same plausible BTC
//! range that same evidence established, which is corroborating, not new,
//! evidence. It is still **only a per-symbol observation**, not a general
//! solution: this source would silently misprice any other Phemex
//! contract whose `priceScale` differs, which is exactly why `lib.rs`
//! does not register it.
//!
//! `QTY_SCALE` (`0`) follows the same reasoning `bars.rs` applies to this
//! contract's own `volume` field: `BTCUSD` is a $1-per-contract inverse
//! instrument, so a resting size is a whole number of contracts with no
//! fractional part — corroborated here by every size in the capture being
//! an integer with no evident sub-unit meaning, e.g. `45623`, not
//! `45623.5`.
//!
//! # What was not verified, and is therefore a documented assumption
//!
//! The access boundary allows exactly one short-lived live request for this
//! milestone, already spent on the capture above (a second request
//! confirmed the trap endpoint's failure, not this one).
//! - **This endpoint's rate-limit weight** is not in any response header
//!   this capture returned. `BOOK_FETCH_COST` is this project's own
//!   conservative proactive budget, matching every other bar/book source
//!   in this workspace, not a venue-documented number.
//! - **An empty book** was not observed live; empty `asks`/`bids` arrays
//!   are handled, but that shape itself is this source's own inference,
//!   not a confirmed venue response.

use async_trait::async_trait;
use senken_core::UnixNanos;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_subscription::{BookLevel, BookSnapshot, BookSource};
use senken_venue::VenueClient;
use serde::Deserialize;

use crate::PERP_ID;

const ORDERBOOK_URL: &str = "https://api.phemex.com/md/orderbook";

/// **A single-symbol assumption, not a general solution — see the module
/// docs.** Correct for `BTCUSD` only; this is exactly why this source is
/// not registered.
const PRICE_SCALE: u8 = 4;

/// Whole contracts on this $1-per-contract inverse instrument — see the
/// module docs.
const QTY_SCALE: u8 = 0;

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
    book: RawBookLevels,
    timestamp: i64,
}

#[derive(Debug, Default, Deserialize)]
struct RawBookLevels {
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
}

/// Phemex order-book depth, fetched through a
/// [`senken_venue::VenueClient`] — a fresh request per call, never a
/// maintained local book. **Not registered** — see the module docs.
#[derive(Debug, Clone)]
pub struct PhemexBookSource {
    url: String,
    client: VenueClient,
}

impl PhemexBookSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `PhemexPerpBarSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a [`PhemexBookSource`] against the real Phemex endpoint.
/// Deliberately not called from `lib.rs`'s `activate_with_http` — see the
/// module docs. Left `pub`, mirroring `bar_source_perp`, so a caller that
/// has independently confirmed a symbol's `priceScale` can still use it
/// directly.
#[must_use]
pub fn book_source(client: VenueClient) -> PhemexBookSource {
    PhemexBookSource {
        url: ORDERBOOK_URL.to_owned(),
        client,
    }
}

fn to_levels(raw: Vec<RawLevel>) -> Vec<BookLevel> {
    raw.into_iter()
        .map(|(price, size)| BookLevel { price, size })
        .collect()
}

#[async_trait]
impl BookSource for PhemexBookSource {
    fn source_id(&self) -> &str {
        PERP_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}", self.url, symbol.as_str());
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

        let mut bids = to_levels(result.book.bids);
        let mut asks = to_levels(result.book.asks);
        bids.truncate(depth);
        asks.truncate(depth);

        let ts = UnixNanos::from_nanos(result.timestamp);

        // Both sides arrive best-first already (see module docs) — trusted
        // rather than re-sorted, the same as `senken-plugin-okx`'s book
        // source.
        BookSnapshot::new(
            ts,
            bids,
            PRICE_SCALE,
            QTY_SCALE,
            asks,
            PRICE_SCALE,
            QTY_SCALE,
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

    fn btc_usd() -> SourceSymbol {
        SourceSymbol::assume("BTCUSD")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn mock_source() -> (MockServer, super::PhemexBookSource) {
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
            ORDERBOOK_URL,
            "must default to the real Phemex endpoint, not require with_url"
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"error":null,"id":0,"result":{"book":{"asks":[],"bids":[]},
                "depth":30,"sequence":0,"symbol":"BTCUSD","timestamp":0,"type":"snapshot"}}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"error":{"code":6001,"message":"invalid argument"},"id":null,"result":null}"#,
            ))
            .mount(&server)
            .await;
        let source = book_source(test_client()).with_url(server.uri());

        let error = source.book_snapshot(&btc_usd(), 30).await.unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::source::SourceError::Rejected { .. }
        ));
    }
}
