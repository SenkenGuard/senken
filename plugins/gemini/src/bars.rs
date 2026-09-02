//! Gemini bar fetching — `GET /v2/candles/{symbol}/{time_frame}`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `btcusd/1m`.
//!
//! 1. **Sort direction**: **descending** by open time — the newest candle
//!    first, the opposite of every other source in this workspace. Sorted
//!    ascending here before returning, like every other source, so a
//!    caller never has to special-case this venue.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a JSON
//!    number, the first element of each row.
//! 3. **Closed-candle detection**: no flag of any kind. The newest row is
//!    closed only once its open time plus the requested interval has
//!    passed — compared against [`Clock::now`], never the wall clock
//!    directly (see `clock.rs`).
//! 4. **Row cap (tested)**: 1440 rows for `1m` — see "no time parameter at
//!    all" below for why this number is an artefact of the interval
//!    requested, not a fixed row count this venue promises.
//! 5. **Pagination direction**: **none.** There is no `start`, `end`,
//!    `since` or `limit` parameter anywhere in this endpoint's path or
//!    query string. Every call for a given symbol and interval returns the
//!    same fixed lookback window from "now", full stop.
//!
//! # This venue cannot backfill — say so, do not paper over it
//!
//! Point 5 above is not a pagination quirk to work around; it is a hard
//! limit of the endpoint. A caller asking for history older than what this
//! window reaches back to is not being told "no data here" by a confused
//! venue — the data may well exist, this endpoint simply cannot reach it.
//! [`GeminiBarSource::bars`] tells the two apart explicitly: a request
//! whose start precedes the oldest candle this call actually returned is
//! rejected with a message naming the boundary, never silently trimmed
//! down to whatever *did* come back. A chart that runs out of history on
//! Gemini is the venue's limit, not a bug in this source.
//!
//! The 1440-row, roughly one-day window measured for `1m` is *not* a
//! promise about other intervals: this endpoint has no row-count
//! parameter to test against, so whether it is a fixed row cap (meaning a
//! coarser interval reaches back proportionally further) or a fixed
//! wall-clock window (meaning every interval reaches back about the same
//! distance) was not established this session, and only `1m` is offered
//! as a result — see below.
//!
//! # Prices arrive as JSON numbers, never as strings
//!
//! Like Deribit's chart data in this same workspace, each row is a bare
//! JSON array of numbers, not decimal strings. This project never routes
//! a price through `f64`, not even transiently, so each field is decoded
//! as a [`RawValue`] — the workspace's `serde_json` `raw_value` feature —
//! which hands back the venue's exact digits with no float parsing
//! anywhere in between, then read by the same
//! [`senken_core::parse_scaled`] every other source uses.
//!
//! # Only one interval is offered
//!
//! This recording session could make exactly one live request (see the
//! task constraints this module was written under), so only `1m` is
//! offered. Gemini's other documented time frames (`5m`, `15m`, `30m`,
//! `1hr`, `6hr`, `1day` — note `1hr`, not this workspace's usual `1h`) are
//! unverified here and deliberately left out rather than guessed at.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde_json::value::RawValue;

const CANDLES_BASE_URL: &str = "https://api.gemini.com/v2/candles";

/// The tested cap for `1m` — see the module docs on why this is not a
/// promise about other intervals.
const MAX_ROWS: usize = 1440;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, time_frame)` mapping — see the module
/// docs on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Minute, "1m");

fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// Gemini's `time_frame` path segment for `spec`, or `None` when `spec` is
/// not the one interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

/// One row: `[timestamp_ms, open, high, low, close, volume]`, newest
/// first — see the module docs.
type RawCandle = (
    i64,
    Box<RawValue>,
    Box<RawValue>,
    Box<RawValue>,
    Box<RawValue>,
    Box<RawValue>,
);

/// Gemini bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (this endpoint sends no confirmation flag — see the module
/// docs).
#[derive(Clone)]
pub struct GeminiBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for GeminiBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl GeminiBarSource {
    /// Points this source at a different base URL — a local stand-in in
    /// tests. The full path (`/{symbol}/{time_frame}`) is appended at
    /// request time, so `with_url` takes the same bare host/prefix this
    /// source is constructed with.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// No query string at all — see the module docs on point 5. Gemini's
    /// symbols are lower case in the URL though upper case is what this
    /// workspace's normaliser and `SourceSymbol` carry.
    fn candles_url(&self, symbol: &str, time_frame: &str) -> String {
        format!("{}/{}/{time_frame}", self.url, symbol.to_lowercase())
    }
}

/// The Gemini bar source, registered under [`crate::SOURCE_ID`] — the same
/// source spot and perpetual instruments both register under, since this
/// endpoint takes any of the venue's symbols.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> GeminiBarSource {
    GeminiBarSource {
        url: CANDLES_BASE_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for GeminiBarSource {
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
        let time_frame = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;

        let url = self.candles_url(symbol.as_str(), time_frame);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(
            rows.iter()
                .flat_map(|row| [row.1.get(), row.2.get(), row.3.get(), row.4.get()]),
        );
        let qty_scale = common_scale(rows.iter().map(|row| row.5.get()));

        let now_nanos = self.clock.now().as_nanos();
        // Closed rows in ascending order — the venue answers newest first
        // (point 1), and the history-boundary check just below needs the
        // true oldest closed candle regardless of what `range` asked for.
        let mut closed = Vec::with_capacity(rows.len());
        for (ts_ms, open, high, low, close, volume) in &rows {
            let ts_open = UnixNanos::from_millis(*ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms} overflowed")))?;
            let close_nanos = ts_open.as_nanos().checked_add(bar_nanos).ok_or_else(|| {
                SourceError::decode(format!("close time for {ts_open} overflowed"))
            })?;
            if close_nanos > now_nanos {
                continue;
            }
            closed.push((ts_open, open, high, low, close, volume));
        }
        closed.sort_by_key(|(ts, ..)| *ts);

        // This endpoint takes no time parameter at all (point 5): the
        // fixed window it answers with either reaches back far enough to
        // cover `range.start()` or it does not, and there is no way to ask
        // it to go further. Silently handing back whatever *did* overlap
        // would be indistinguishable from "this venue has no data before
        // here", which is a claim this source has no basis for making —
        // see the module docs.
        if let Some((oldest, ..)) = closed.first()
            && range.start() < *oldest
        {
            return Err(SourceError::rejected(format!(
                "Gemini's fixed candle window for this symbol/interval starts at {oldest}; \
                 it cannot serve the requested range starting at {}",
                range.start()
            )));
        }

        let mut bars = Vec::with_capacity(closed.len());
        for (ts_open, open, high, low, close, volume) in closed {
            if !range.contains(ts_open) {
                continue;
            }
            bars.push(Bar {
                ts_open,
                open: scaled(open.get(), price_scale)?,
                high: scaled(high.get(), price_scale)?,
                low: scaled(low.get(), price_scale)?,
                close: scaled(close.get(), price_scale)?,
                volume: Volume::Real(scaled(volume.get(), qty_scale)?),
                // No quote-volume field on this endpoint.
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // Already ascending from the sort above; re-asserted here rather
        // than trusted, the same discipline every other source in this
        // workspace applies.
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses the raw JSON-number text `raw` at `scale`, mapping an
/// unparseable value — which should never happen given `scale` was
/// computed from this exact batch — to a decode error rather than
/// panicking or guessing.
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

    /// A real `GET /v2/candles/btcusd/1m` response, recorded 2026-09-02:
    /// 1440 one-minute rows, newest first, the newest one still forming
    /// relative to a clock set just before its close.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1m.json");

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
        SourceSymbol::assume("BTCUSD")
    }

    fn minute() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    /// Covers the whole fixture and then some.
    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_788_242_820_000).unwrap(),
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

    /// Strictly before the newest row's own close
    /// (`1_788_329_160_000 + 60_000 = 1_788_329_220_000`).
    fn clock_before_last_close() -> Arc<dyn Clock> {
        Arc::new(FixedClock(1_788_329_180_000))
    }

    fn clock_after_everything() -> Arc<dyn Clock> {
        Arc::new(FixedClock(4_102_444_800_000))
    }

    #[tokio::test]
    async fn the_still_forming_newest_candle_is_never_returned() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_before_last_close()).with_url(server.uri());

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 1439, "1440 rows, the newest one still forming");
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_millis(1_788_329_160_000).unwrap())
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_fourteen_forty_are_kept() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 1440);
    }

    #[tokio::test]
    async fn the_descending_response_is_returned_ascending() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_242_820_000).unwrap(),
            "the oldest row (last in the venue's own order) sorts first"
        );
    }

    #[tokio::test]
    async fn prices_are_read_from_bare_json_numbers_not_strings() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        // Oldest fixture row: open 79028.86, high 79035.26, low 79014.67,
        // close 79035.26, volume 0.00531414 — at scale (2, 8).
        let oldest = &bars[0];
        assert_eq!(oldest.open, 7_902_886);
        assert_eq!(oldest.high, 7_903_526);
        assert_eq!(oldest.low, 7_901_467);
        assert_eq!(oldest.close, 7_903_526);
        assert!(matches!(oldest.volume, Volume::Real(v) if v == 531_414));
    }

    #[tokio::test]
    async fn a_request_reaching_before_the_venues_fixed_window_is_reported_honestly() {
        // No `start`/`end` parameter exists on this endpoint at all: a
        // request older than what the fixed window reaches back to must
        // not be silently trimmed down to whatever did come back.
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());
        let reaches_before_history = TimeRange::new(
            UnixNanos::from_millis(1_700_000_000_000).unwrap(),
            UnixNanos::from_millis(1_788_400_000_000).unwrap(),
        )
        .unwrap();

        let error = source
            .bars(&symbol(), minute(), reaches_before_history)
            .await
            .expect_err("reaching before the fixed window must be reported, not trimmed");

        assert!(
            error.to_string().contains("cannot serve"),
            "the error must name the venue's own limit: {error}"
        );
    }

    #[tokio::test]
    async fn a_request_entirely_after_the_window_is_an_absence_not_an_error() {
        // The mirror image of the case above: asking for a time newer than
        // anything returned is not the venue ignoring the request, it is
        // simply data that does not exist yet.
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());
        let future = TimeRange::new(
            UnixNanos::from_millis(1_788_400_000_000).unwrap(),
            UnixNanos::from_millis(1_788_400_060_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), minute(), future).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(CANDLES).await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());
        // Only the second-oldest fixture row (1_788_242_880_000) falls
        // inside.
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_242_880_000).unwrap(),
            UnixNanos::from_millis(1_788_242_900_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), minute(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_242_880_000).unwrap()
        );
    }

    #[tokio::test]
    async fn an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let server = serving(b"[]").await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert!(bars.is_empty());
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Hour)).is_none());
        assert!(super::interval_of(BarSpec::new(5, BarUnit::Minute)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source(test_client(), clock_after_everything());
        for spec in source.supported() {
            assert!(super::interval_of(*spec).is_some());
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source(test_client(), clock_after_everything()).with_url(server.uri());
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_329_160_000).unwrap(),
            UnixNanos::from_millis(1_788_329_160_000).unwrap(),
        );

        if let Some(range) = inverted {
            assert!(
                source
                    .bars(&symbol(), minute(), range)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
