//! Kraken spot bar fetching — `GET /0/public/OHLC`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `pair=XBTUSD&interval=60`, with no
//! `since` (the venue's own default window).
//!
//! 1. **Sort direction**: ascending by open time. Sorted again here anyway
//!    — a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a JSON number,
//!    the first element of each row.
//! 3. **Closed-candle detection**: no flag of any kind. The newest row is
//!    closed only once its open time plus the requested interval has
//!    passed — compared against [`Clock::now`], never the wall clock
//!    directly (see `clock.rs`).
//! 4. **Row cap (tested)**: exactly 721 rows came back for the recorded
//!    request, independently confirming the task brief's ~721-candle
//!    retention figure this session.
//! 5. **Pagination direction**: `since`, an inclusive epoch-second floor.
//!    **The trap**: this endpoint only retains roughly 721 candles: a
//!    `since` older than that silently answers with the newest window
//!    instead — HTTP 200, no error, the wrong data. This source never
//!    trusts that the rows it gets back actually start at `since`; see the
//!    guard below.
//!
//! # The legacy pair name is not the query symbol
//!
//! The response's `result` object is keyed by Kraken's legacy pair name
//! (`XXBTZUSD`), not the `pair` value the request was sent with
//! (`XBTUSD`) — this crate's own `spot_instrument` already prefers
//! `altname` for exactly this reason, so the [`SourceSymbol`] this source
//! receives is the query-friendly form, and the response key is read
//! generically (whichever key in `result` is not `"last"`) rather than
//! matched against the symbol at all.
//!
//! # What was verified, and what is a documented assumption
//!
//! - The requested window's row count (721) and its silent-truncation
//!   trap are as documented in the task brief this source was written
//!   against; this recording session's one-request-per-endpoint limit
//!   made a second call with an old `since` (to watch the silent jump
//!   happen directly) impossible, so the guard below is written from the
//!   documented behaviour, not from a second live observation.
//! - The fixture's last row is still forming as of the moment it was
//!   recorded (`ts_open` plus one hour is after the capture instant) —
//!   kept deliberately, so a test can assert the drop against a real row.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const OHLC_URL: &str = "https://api.kraken.com/0/public/OHLC";

/// The tested cap — see the module docs' point 4.
const MAX_ROWS: usize = 721;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, interval-in-minutes)` mapping — see the
/// module docs on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Hour, "60");

fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// Kraken's `interval` string (minutes) for `spec`, or `None` when `spec`
/// is not the one interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

/// One row: `[time, open, high, low, close, vwap, volume, count]`. `vwap`
/// is not read — nothing in [`senken_series::Bar`] carries it.
type RawRow = (i64, String, String, String, String, String, String, u32);

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    error: Vec<String>,
    #[serde(default)]
    result: serde_json::Map<String, serde_json::Value>,
}

/// Kraken spot bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (this endpoint sends no confirmation flag — see the module
/// docs).
#[derive(Clone)]
pub struct KrakenBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for KrakenBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KrakenBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl KrakenBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// `since` is sent as `range.start()`'s inclusive floor in epoch
    /// seconds — see the module docs' point 5 on why the response is never
    /// trusted to actually start there.
    fn ohlc_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let since = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        format!(
            "{}?pair={symbol}&interval={interval}&since={since}",
            self.url,
        )
    }
}

/// The Kraken spot bar source, registered under [`crate::SPOT_ID`].
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> KrakenBarSource {
    KrakenBarSource {
        url: OHLC_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for KrakenBarSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        MAX_ROWS
    }

    async fn bars(
        &self,
        symbol: &SourceSymbol,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, SourceError> {
        if range.start() >= range.end() {
            return Ok(Vec::new());
        }
        let interval = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;

        let url = self.ohlc_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if let Some(first) = envelope.error.first() {
            // Kraken reports argument errors inside an HTTP 200 body.
            return Err(SourceError::rejected(first.clone()));
        }
        // The pair data sits under Kraken's own legacy key, never the
        // symbol this request was sent with — see the module docs. `last`
        // is the only other key this object ever carries.
        let rows_value = envelope
            .result
            .iter()
            .find(|(key, _)| key.as_str() != "last")
            .map(|(_, value)| value.clone());
        let rows: Vec<RawRow> = match rows_value {
            Some(value) => serde_json::from_value(value).map_err(SourceError::decode)?,
            None => Vec::new(),
        };

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.6.as_str()));

        let now_nanos = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (ts_secs, open, high, low, close, _vwap, volume, count) in rows {
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;

            // No confirmation flag on this endpoint: a candle is closed
            // only once its own open time plus the bar's length has
            // passed.
            let close_nanos = ts_open.as_nanos().checked_add(bar_nanos).ok_or_else(|| {
                SourceError::decode(format!("close time for {ts_open} overflowed"))
            })?;
            if close_nanos > now_nanos {
                continue;
            }

            if !range.contains(ts_open) {
                outside += 1;
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: scaled(&open, price_scale)?,
                high: scaled(&high, price_scale)?,
                low: scaled(&low, price_scale)?,
                close: scaled(&close, price_scale)?,
                volume: Volume::Real(scaled(&volume, qty_scale)?),
                // Kraken's OHLC rows carry no quote-asset volume field.
                quote_volume: None,
                trade_count: Some(count),
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard, in this same workspace, for why an
        // answer made entirely of rows outside the requested range is
        // reported rather than swallowed — a `since` older than this
        // endpoint's roughly 721-candle retention is documented (module
        // docs, point 5) to silently answer with the newest window
        // instead, and a caller reaching past that boundary must not be
        // told "no data here".
        if bars.is_empty() && outside > 0 {
            return Err(SourceError::rejected(format!(
                "answered with {outside} closed bars, none inside the requested range — \
                 the range parameters were not honoured"
            )));
        }

        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses `raw` at `scale`, mapping an unparseable value — which should
/// never happen given `scale` was computed from this exact batch of
/// strings — to a decode error rather than panicking or guessing.
fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source_spot;

    /// A real `GET OHLC?pair=XBTUSD&interval=60` response, recorded
    /// 2026-09-02: 721 hourly rows keyed under `XXBTZUSD`, the last one
    /// still forming at capture time.
    const OHLC: &[u8] = include_bytes!("../tests/fixtures/ohlc_1h.json");

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

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("XBTUSD")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_785_736_800).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap()
    }

    async fn serving(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        server
    }

    /// Strictly before the fixture's last row closes
    /// (`1_788_328_800 + 3_600 = 1_788_332_400` seconds).
    fn clock_before_last_close() -> Arc<dyn Clock> {
        Arc::new(FixedClock(1_788_329_200_000))
    }

    fn clock_after_everything() -> Arc<dyn Clock> {
        Arc::new(FixedClock(4_102_444_800_000))
    }

    #[tokio::test]
    async fn the_still_forming_last_candle_is_never_returned() {
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_before_last_close())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 720, "721 rows, the last one still forming");
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_secs(1_788_328_800).unwrap())
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_seven_hundred_twenty_one_are_kept() {
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 721);
    }

    #[tokio::test]
    async fn rows_are_read_from_the_legacy_key_not_the_query_symbol() {
        // The fixture's object is keyed `XXBTZUSD`; the request was made
        // (and this source is asked) with `XBTUSD`.
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(
            !bars.is_empty(),
            "the legacy-keyed rows must still be found"
        );
    }

    #[tokio::test]
    async fn prices_and_volume_decode_at_the_batchs_common_scale() {
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // Row 0: open 62743.4, high 62782.6, low 62558.6, close 62558.6,
        // volume 42.77352262, count 1250 — at scale (1, 8).
        assert_eq!(first.open, 627_434);
        assert_eq!(first.high, 627_826);
        assert_eq!(first.low, 625_586);
        assert_eq!(first.close, 625_586);
        assert!(matches!(first.volume, Volume::Real(v) if v == 4_277_352_262));
        assert_eq!(first.trade_count, Some(1250));
        assert_eq!(first.quote_volume, None, "this endpoint reports none");
    }

    #[tokio::test]
    async fn timestamps_are_read_as_seconds_not_milliseconds() {
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_785_736_800).unwrap()
        );
        assert_eq!(
            bars[1].ts_open.as_nanos() - bars[0].ts_open.as_nanos(),
            3_600 * 1_000_000_000
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));
        // Only the fixture's second row (1_785_740_400) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_785_740_400).unwrap(),
            UnixNanos::from_secs(1_785_744_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_785_740_400).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        // This is the documented shape of Kraken's own trap: `since` older
        // than the ~721-candle retention silently answers with the newest
        // window. A caller whose requested range shares no overlap with
        // whatever actually came back must be told so, not handed an
        // empty result that reads as "no data here".
        let server = serving(OHLC).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));
        let elsewhere = TimeRange::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            UnixNanos::from_secs(1_700_003_600).unwrap(),
        )
        .unwrap();

        let error = source
            .bars(&symbol(), hour(), elsewhere)
            .await
            .expect_err("an answer entirely outside the range is a failure, not an absence");

        assert!(
            error.to_string().contains("not honoured"),
            "the error must say the range was ignored: {error}"
        );
    }

    #[tokio::test]
    async fn an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let body = br#"{"error":[],"result":{"XXBTZUSD":[],"last":1785736800}}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn an_error_array_is_a_rejection() {
        let body = br#"{"error":["EGeneral:Invalid arguments"],"result":{}}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("EGeneral"));
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Minute)).is_none());
        assert!(super::interval_of(BarSpec::new(4, BarUnit::Hour)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_spot(test_client(), clock_after_everything());
        for spec in source.supported() {
            assert!(super::interval_of(*spec).is_some());
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/ohlc", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_328_800).unwrap(),
            UnixNanos::from_secs(1_788_328_800).unwrap(),
        );

        if let Some(range) = inverted {
            assert!(
                source
                    .bars(&symbol(), hour(), range)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
