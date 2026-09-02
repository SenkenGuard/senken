//! Crypto.com bar fetching — `GET /exchange/v1/public/get-candlestick`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `instrument_name=BTC_USDT`,
//! `timeframe=1h`.
//!
//! 1. **Sort direction**: ascending by open time (consecutive rows'
//!    timestamps increase). Sorted again here anyway — a venue's order is
//!    not a promise.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a JSON
//!    number, in field `t`.
//! 3. **Closed-candle detection**: no flag of any kind. The last row is
//!    closed only once `ts_open` plus the bar's own length has passed —
//!    compared against [`Clock::now`], never the wall clock directly (see
//!    `clock.rs`).
//! 4. **Row cap (tested)**: `count=300` is documented as this endpoint's
//!    maximum and was the value actually sent; the fixture's live answer
//!    (25 rows, the default history depth for a brand-new capture) does
//!    not by itself prove 300 rows come back rather than being truncated,
//!    but this task's one-request-per-endpoint limit ruled out a second
//!    call to confirm it directly, so `count=300` is used as the
//!    documented, requested value rather than a guess at something larger.
//! 5. **Pagination direction**: **`start_ts` is silently ignored.** Only
//!    `end_ts` anchors the window; the venue walks *backward* from it for
//!    up to `count` candles. Sending `start_ts` at all would claim a
//!    promise this endpoint does not keep, so this source never sends it.
//!
//! # Only one interval is offered
//!
//! Crypto.com's public documentation lists many `timeframe` values (`1m`,
//! `5m`, `15m`, `30m`, `1h`, `4h`, ...), but Gate's own candlestick endpoint
//! in this same workspace is proof that a venue here can answer HTTP 200
//! for an interval string it does not honour, and this recording session's
//! one-request-per-endpoint limit ruled out testing the others live.
//! Rather than guess, this source offers only `1h` — the one interval
//! actually requested and measured. Widening this table needs the same
//! live verification, not documentation.
//!
//! # What was verified, and what is a documented assumption
//!
//! - Every row's `t` was checked against its neighbours: consecutive rows
//!   are exactly 3 600 000 ms apart, confirming both the millisecond unit
//!   and that `timeframe=1h` was honoured rather than silently
//!   substituted.
//! - The fixture's last row is still forming as of the moment it was
//!   recorded (`ts_open` plus one hour is after the capture instant) —
//!   kept deliberately, the same way Gate's own fixture keeps one, so a
//!   test can assert the drop against a real row rather than a synthetic
//!   one.
//! - History depth beyond `count=300` was not probed; a caller stepping
//!   further back than that in one call is a `senken-loader` concern, not
//!   this source's.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const CANDLESTICK_URL: &str = "https://api.crypto.com/exchange/v1/public/get-candlestick";

/// The documented and requested cap for this endpoint. See the module
/// docs' point 4 on why this is not independently confirmed by the
/// recorded fixture.
const MAX_ROWS: usize = 300;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, timeframe)` mapping — see the module
/// docs on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Hour, "1h");

/// The specs this source can fetch — just [`INTERVAL`], see the module
/// docs.
fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// Crypto.com's `timeframe` string for `spec`, or `None` when `spec` is
/// not the one interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

/// The envelope every Crypto.com Exchange public endpoint answers with:
/// `code` is `0` on success, non-zero application errors arrive inside an
/// HTTP 200.
#[derive(Debug, Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    result: CandlestickResult,
}

#[derive(Debug, Default, Deserialize)]
struct CandlestickResult {
    #[serde(default)]
    data: Vec<RawCandle>,
}

/// One row of `get-candlestick`: an object, not a positional array.
#[derive(Debug, Deserialize)]
struct RawCandle {
    o: String,
    h: String,
    l: String,
    c: String,
    v: String,
    /// Epoch milliseconds.
    t: i64,
}

/// Crypto.com bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (this endpoint sends no confirmation flag — see the module
/// docs).
#[derive(Clone)]
pub struct CryptocomBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for CryptocomBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CryptocomBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl CryptocomBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// `start_ts` is never sent — see the module docs on why it would be a
    /// promise this endpoint does not keep. `end_ts` is the millisecond
    /// form of `range.end()`.
    fn candles_url(&self, symbol: &str, range: TimeRange) -> String {
        let end_ms = range.end().as_millis();
        format!(
            "{}?instrument_name={symbol}&timeframe={}&count={MAX_ROWS}&end_ts={end_ms}",
            self.url, INTERVAL.2,
        )
    }
}

/// The Crypto.com bar source, registered under [`crate::SOURCE_ID`] — the
/// one document this venue answers candlesticks for, covering spot,
/// perpetual and dated instruments alike by `instrument_name`.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> CryptocomBarSource {
    CryptocomBarSource {
        url: CANDLESTICK_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for CryptocomBarSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
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
        interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;

        let url = self.candles_url(symbol.as_str(), range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if envelope.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.message
            )));
        }
        let rows = envelope.result.data;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.o.as_str(),
                row.h.as_str(),
                row.l.as_str(),
                row.c.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.v.as_str()));

        let now_nanos = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for row in rows {
            let ts_open = UnixNanos::from_millis(row.t)
                .ok_or_else(|| SourceError::decode(format!("open time {} overflowed", row.t)))?;

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
                open: scaled(&row.o, price_scale)?,
                high: scaled(&row.h, price_scale)?,
                low: scaled(&row.l, price_scale)?,
                close: scaled(&row.c, price_scale)?,
                volume: Volume::Real(scaled(&row.v, qty_scale)?),
                // No quote-volume field on this endpoint.
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard, in this same workspace, for why an
        // answer made entirely of rows outside the requested range is
        // reported rather than swallowed — `start_ts` being silently
        // ignored on this very endpoint is exactly the shape of trap this
        // exists to catch: a caller asking for an unreachable window would
        // otherwise get back rows from wherever `end_ts` actually landed,
        // filtered down to nothing, and read that as "no data here".
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

    use super::bar_source;

    /// A real `GET get-candlestick?instrument_name=BTC_USDT&timeframe=1h`
    /// response, recorded 2026-09-02: 25 hourly rows, the last one still
    /// forming at capture time.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

    /// A `Clock` a test fully controls.
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
        SourceSymbol::assume("BTC_USDT")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    /// The whole window the fixture covers, and then some.
    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_788_242_400_000).unwrap(),
            UnixNanos::from_millis(1_788_400_000_000).unwrap(),
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

    /// A clock strictly before the fixture's last row closes
    /// (`1_788_328_800_000 + 3_600_000 = 1_788_332_400_000`).
    fn clock_before_last_close() -> Arc<dyn Clock> {
        Arc::new(FixedClock(1_788_329_200_000))
    }

    /// A clock well after every fixture row has closed.
    fn clock_after_everything() -> Arc<dyn Clock> {
        Arc::new(FixedClock(4_102_444_800_000))
    }

    #[tokio::test]
    async fn the_still_forming_last_candle_is_never_returned() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_before_last_close())
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars.len(),
            24,
            "the fixture holds 25 rows, one still forming"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_millis(1_788_328_800_000).unwrap()),
            "the still-forming row must not appear"
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_twenty_five_are_kept() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 25);
    }

    #[tokio::test]
    async fn rows_are_read_from_object_fields_not_a_positional_array() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // Row 0 of the fixture: o 79170.81, h 79170.81, l 78741.14,
        // c 78741.14, v 25.42127 — at the batch's common scale (2, 5).
        assert_eq!(first.open, 7_917_081);
        assert_eq!(first.high, 7_917_081);
        assert_eq!(first.low, 7_874_114);
        assert_eq!(first.close, 7_874_114);
        assert!(matches!(first.volume, Volume::Real(v) if v == 2_542_127));
        assert_eq!(first.quote_volume, None, "this endpoint reports none");
    }

    #[tokio::test]
    async fn timestamps_are_read_as_milliseconds_and_land_an_hour_apart() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_242_400_000).unwrap()
        );
        assert_eq!(
            bars[1].ts_open.as_nanos() - bars[0].ts_open.as_nanos(),
            3_600 * 1_000_000_000,
            "one hour apart, so the millisecond unit was read correctly"
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));
        // Only the fixture's third row (t = 1_788_249_600_000) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_249_600_000).unwrap(),
            UnixNanos::from_millis(1_788_250_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_249_600_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        // `start_ts` is silently ignored by this real endpoint — a caller
        // asking for a window nowhere near what `end_ts` actually answered
        // with must not read that as "no data here".
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));
        let elsewhere = TimeRange::new(
            UnixNanos::from_millis(1_700_000_000_000).unwrap(),
            UnixNanos::from_millis(1_700_003_600_000).unwrap(),
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
        let body = br#"{"id":-1,"method":"public/get-candlestick","code":0,"result":{"interval":"1h","data":[]}}"#;
        let server = serving(body).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn an_application_error_code_is_a_rejection() {
        let body = br#"{"id":-1,"method":"public/get-candlestick","code":10004,"message":"BAD_REQUEST","result":{}}"#;
        let server = serving(body).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("10004"));
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(interval_of_test(BarSpec::new(1, BarUnit::Minute)).is_none());
        assert!(interval_of_test(BarSpec::new(4, BarUnit::Hour)).is_none());
    }

    fn interval_of_test(spec: BarSpec) -> Option<&'static str> {
        super::interval_of(spec)
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source(test_client(), clock_after_everything());
        for spec in source.supported() {
            assert!(
                super::interval_of(*spec).is_some(),
                "{spec} is offered but has no interval mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/candles", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_328_800_000).unwrap(),
            UnixNanos::from_millis(1_788_328_800_000).unwrap(),
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
