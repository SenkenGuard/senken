//! Deribit bar fetching — `GET
//! /public/get_tradingview_chart_data`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `instrument_name=BTC-PERPETUAL`,
//! `resolution=60`, a ten-day window.
//!
//! 1. **Sort direction**: ascending by open time (`ticks` increases
//!    monotonically). Sorted again here anyway — a venue's order is not a
//!    promise.
//! 2. **Timestamp representation**: epoch **milliseconds**, as JSON
//!    numbers, in the parallel `ticks` array.
//! 3. **Closed-candle detection**: no flag of any kind. The last tick is
//!    closed only once its open time plus the requested resolution has
//!    passed — compared against [`Clock::now`], never the wall clock
//!    directly (see `clock.rs`).
//! 4. **Row cap**: the task brief this source was written against records
//!    a **silent** truncation to roughly 5000 rows on a sufficiently wide
//!    window, from an earlier live audit. This recording session's
//!    one-request-per-endpoint limit made a second call to reconfirm that
//!    number impossible, so `5000` is used here as a cited, not
//!    independently reconfirmed, figure — never rounded up.
//! 5. **Pagination direction**: `start_timestamp`/`end_timestamp`, both
//!    inclusive epoch milliseconds. The ten-day window requested here came
//!    back in full (241 hourly candles, none missing), so ordinary
//!    pagination is honoured; it is the *silent* truncation on a much
//!    wider window that is the trap (point 4), not this endpoint refusing
//!    an out-of-range request outright the way Gate does.
//!
//! # Column-oriented, not row-oriented
//!
//! `result` is six parallel arrays (`ticks`, `open`, `high`, `low`,
//! `close`, `volume`, plus `cost`), index `i` of one naming the same
//! candle as index `i` of every other — never an array of per-candle
//! objects. `cost` is the traded value in the *quote* currency (USD on a
//! USD-margined instrument); `volume` is the base-currency amount. This
//! matches Deribit's own documented naming for this endpoint specifically
//! (as opposed to this module's five *undocumented* traps above, which
//! were all confirmed from the live response, not read off a page).
//!
//! # Prices arrive as JSON numbers, never as strings
//!
//! Every other bar source in this workspace decodes prices from decimal
//! *strings*, where [`senken_core::parse_scaled`] can read the venue's
//! exact digits directly. Deribit's chart data instead sends bare JSON
//! numbers (`76076.5`, not `"76076.5"`), and this project never routes a
//! price through `f64` — not even transiently — to get from one form to
//! the other (see this crate's own top-level `AGENTS.md`). Each array
//! element is decoded as a [`RawValue`], the workspace's `serde_json`
//! `raw_value` feature, which hands back the exact bytes the venue sent
//! with no float parsing anywhere in between; those bytes are then read by
//! the same [`senken_core::parse_scaled`] every other source uses.
//!
//! # Only one interval is offered
//!
//! This recording session could make exactly one live request (see the
//! task constraints this module was written under), so only `resolution=60`
//! (one hour) is offered; Deribit's other documented resolutions are
//! unverified here and deliberately left out rather than guessed at.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::api::RpcError;

const CHART_URL: &str = "https://www.deribit.com/api/v2/public/get_tradingview_chart_data";

/// Cited from a prior live audit, not independently reconfirmed this
/// session — see the module docs' point 4.
const MAX_ROWS: usize = 5000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, resolution-in-minutes)` mapping — see
/// the module docs on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Hour, "60");

fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// Deribit's `resolution` string (minutes) for `spec`, or `None` when
/// `spec` is not the one interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    result: Option<ChartResult>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// The column-oriented candle payload — see the module docs.
#[derive(Debug, Deserialize)]
struct ChartResult {
    /// `"ok"` on success; Deribit's other documented value for this
    /// endpoint, `"no_data"`, is treated the same as an empty `ticks` —
    /// anything else is a rejection. Not independently reconfirmed this
    /// session (see the module docs' point 4 on this source's one-request
    /// limit).
    status: String,
    #[serde(default)]
    ticks: Vec<i64>,
    #[serde(default)]
    open: Vec<Box<RawValue>>,
    #[serde(default)]
    high: Vec<Box<RawValue>>,
    #[serde(default)]
    low: Vec<Box<RawValue>>,
    #[serde(default)]
    close: Vec<Box<RawValue>>,
    #[serde(default)]
    volume: Vec<Box<RawValue>>,
    #[serde(default)]
    cost: Vec<Box<RawValue>>,
}

/// Deribit bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (this endpoint sends no confirmation flag — see the module
/// docs).
#[derive(Clone)]
pub struct DeribitBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for DeribitBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeribitBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl DeribitBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// `start_timestamp`/`end_timestamp` are both inclusive epoch
    /// milliseconds; `range.end()` is exclusive (`TimeRange`'s own
    /// half-open contract), so the last representable millisecond strictly
    /// before it is the inclusive end sent here.
    fn chart_url(&self, symbol: &str, resolution: &str, range: TimeRange) -> String {
        format!(
            "{}?instrument_name={symbol}&resolution={resolution}&start_timestamp={}&end_timestamp={}",
            self.url,
            range.start().as_millis(),
            range.end().as_millis() - 1,
        )
    }
}

/// The Deribit bar source, registered under [`crate::SOURCE_ID`] — the one
/// document this venue answers chart data for, covering spot, perpetual,
/// dated futures and options alike by `instrument_name`.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> DeribitBarSource {
    DeribitBarSource {
        url: CHART_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for DeribitBarSource {
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
        let resolution = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;

        let url = self.chart_url(symbol.as_str(), resolution, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if let Some(error) = envelope.error {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                error.code, error.message
            )));
        }
        let result = envelope
            .result
            .ok_or_else(|| SourceError::decode("response carried neither result nor error"))?;
        if result.status != "ok" && result.status != "no_data" {
            return Err(SourceError::rejected(format!(
                "unexpected status {:?}",
                result.status
            )));
        }
        let n = result.ticks.len();
        if result.open.len() != n
            || result.high.len() != n
            || result.low.len() != n
            || result.close.len() != n
            || result.volume.len() != n
        {
            return Err(SourceError::decode(
                "the column arrays disagree on their length",
            ));
        }

        let price_scale = common_scale(
            result
                .open
                .iter()
                .chain(&result.high)
                .chain(&result.low)
                .chain(&result.close)
                .map(|v| v.get()),
        );
        let qty_scale = common_scale(result.volume.iter().map(|v| v.get()));
        // `cost` is optional relative to the other columns: an
        // instrument-less error path aside, every observed response
        // carries it, but nothing about the other five columns implies it
        // must.
        let has_cost = result.cost.len() == n;
        let cost_scale = if has_cost {
            common_scale(result.cost.iter().map(|v| v.get()))
        } else {
            0
        };

        let now_nanos = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(n);
        let mut outside = 0usize;
        for i in 0..n {
            let ts_ms = result.ticks[i];
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms} overflowed")))?;

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

            let quote_volume = if has_cost {
                Some(scaled(result.cost[i].get(), cost_scale)?)
            } else {
                None
            };

            bars.push(Bar {
                ts_open,
                open: scaled(result.open[i].get(), price_scale)?,
                high: scaled(result.high[i].get(), price_scale)?,
                low: scaled(result.low[i].get(), price_scale)?,
                close: scaled(result.close[i].get(), price_scale)?,
                volume: Volume::Real(scaled(result.volume[i].get(), qty_scale)?),
                quote_volume,
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard, in this same workspace, for why an
        // answer made entirely of rows outside the requested range is
        // reported rather than swallowed: a wide-enough window is
        // documented (module docs, point 4) to be silently truncated on
        // this endpoint, and a caller reaching past that truncation must
        // not be told "no data here".
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

    /// A real `GET get_tradingview_chart_data?instrument_name=BTC-PERPETUAL
    /// &resolution=60` response, recorded 2026-09-02: 241 hourly candles,
    /// the last one still forming at capture time.
    const CHART: &[u8] = include_bytes!("../tests/fixtures/chart_1h.json");

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
        SourceSymbol::assume("BTC-PERPETUAL")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_787_464_800_000).unwrap(),
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

    /// Strictly before the fixture's last tick closes
    /// (`1_788_328_800_000 + 3_600_000 = 1_788_332_400_000`).
    fn clock_before_last_close() -> Arc<dyn Clock> {
        Arc::new(FixedClock(1_788_329_200_000))
    }

    fn clock_after_everything() -> Arc<dyn Clock> {
        Arc::new(FixedClock(4_102_444_800_000))
    }

    #[tokio::test]
    async fn the_still_forming_last_candle_is_never_returned() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_before_last_close())
            .with_url(format!("{}/chart", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars.len(),
            240,
            "the fixture holds 241 ticks, one still forming"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_millis(1_788_328_800_000).unwrap())
        );
    }

    #[tokio::test]
    async fn once_every_tick_has_closed_all_two_hundred_forty_one_are_kept() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 241);
    }

    #[tokio::test]
    async fn columns_are_read_positionally_not_as_row_objects() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // Index 0: open 76076.5, high 76371.5, low 75907.0, close 76111.5,
        // volume 115.94348719, cost 8830360 — at scales (1, 8, 0).
        assert_eq!(first.open, 760_765);
        assert_eq!(first.high, 763_715);
        assert_eq!(first.low, 759_070);
        assert_eq!(first.close, 761_115);
        assert!(matches!(first.volume, Volume::Real(v) if v == 11_594_348_719));
        assert_eq!(first.quote_volume, Some(8_830_360));
    }

    #[tokio::test]
    async fn timestamps_are_read_as_milliseconds_and_land_an_hour_apart() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_787_464_800_000).unwrap()
        );
        assert_eq!(
            bars[1].ts_open.as_nanos() - bars[0].ts_open.as_nanos(),
            3_600 * 1_000_000_000
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));
        // Only the fixture's second tick (1_787_468_400_000) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_787_468_400_000).unwrap(),
            UnixNanos::from_millis(1_787_470_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_787_468_400_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let server = serving(CHART).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));
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
        let body = br#"{"result":{"status":"no_data","ticks":[],"open":[],"high":[],"low":[],"close":[],"volume":[],"cost":[]}}"#;
        let server = serving(body).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_json_rpc_error_is_a_rejection() {
        let body = br#"{"jsonrpc":"2.0","error":{"code":10009,"message":"not_enough_funds"}}"#;
        let server = serving(body).await;
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("10009"));
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Minute)).is_none());
        assert!(super::interval_of(BarSpec::new(4, BarUnit::Hour)).is_none());
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
        let source = bar_source(test_client(), clock_after_everything())
            .with_url(format!("{}/chart", server.uri()));
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
