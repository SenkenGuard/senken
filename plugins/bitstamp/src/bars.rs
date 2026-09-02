//! Bitstamp bar fetching — `GET /api/v2/ohlc/{market_symbol}/`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against `btcusd`
//! (`step=3600&limit=5`), except pagination direction — see point 5.
//!
//! 1. **Sort direction**: ascending by open time, oldest row first. Rows
//!    are re-sorted here anyway, because a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a *string* —
//!    nested two levels deep, inside `data.ohlc[]`, not a bare array like
//!    Gate's or Bitfinex's rows.
//! 3. **Closed-candle detection**: no flag of any kind. Closure is decided
//!    by adding the requested [`BarSpec`]'s own length to each row's open
//!    time and comparing against [`Clock::now`], exactly as
//!    `plugins/binance` does — see [`senken_plugin::SystemClock`].
//! 4. **Row cap**: 1000, refused loudly beyond it — this workspace's own
//!    live audit of this venue conducted before this source was written,
//!    not a boundary this module's own fixture request (`limit=5`)
//!    reproduced.
//! 5. **Pagination direction**: `start`/`end`, both epoch seconds, is this
//!    module's own **documented assumption**, not something the one live
//!    recording behind this module's fixture exercised — that request only
//!    used `step`/`limit`, the exact call this workspace's prior audit
//!    reported.
//!    `start`/`end` are Bitstamp's long-published, stable parameter names
//!    for this endpoint, used here as the conservative default AGENTS.md
//!    calls for when a fact is not re-derived live: if wrong, an unknown
//!    query parameter is far more likely to be ignored or rejected outright
//!    than honoured with a different meaning, and the defensive
//!    entirely-outside-the-range guard below (shared with every other
//!    source in this workspace) still catches a window silently ignored.
//!
//! # Nested response, not a bare array
//!
//! The document is `{"data": {"pair": ..., "ohlc": [...]}}`; each element of
//! `ohlc` is an *object* with named fields (`timestamp`, `open`, `high`,
//! `low`, `close`, `volume`), unlike Gate's or Bitfinex's positional rows —
//! there is no field-order trap here, only the nesting.
//!
//! # No quote volume, no trade count, no taker volume
//!
//! This endpoint reports one volume figure — base-asset volume — and
//! nothing else `Bar` can otherwise hold; `quote_volume`, `trade_count` and
//! `taker_buy_volume` are always `None` here rather than a guess.
//!
//! # What was verified, and what is a documented assumption
//!
//! Only `step=3600` (one hour) was requested and measured. The remaining
//! entries in [`INTERVALS`] are Bitstamp's own published, fixed set of
//! `step` values in seconds — not a value computed from an arbitrary
//! `BarSpec`, which would let an unverified combination slip through as a
//! plausible-looking number Bitstamp might silently accept at some other
//! spacing. `259200` (three days) is included as `(3, Day)` because it is
//! fixed-width, unlike a calendar month; no month-length `step` is
//! published by this venue in the first place.

use std::sync::Arc;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const OHLC_URL: &str = "https://www.bitstamp.net/api/v2/ohlc";

/// The row cap from this workspace's own prior live audit of this venue —
/// see the module docs' point 4 on why it is not independently re-tested by
/// this change's own fixture request.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, the same
/// value every other bar source here uses, not a venue-documented number.
const CANDLES_FETCH_COST: u32 = 5;

/// The whole document: `{"data": {"pair": ..., "ohlc": [...]}}`.
#[derive(Debug, Deserialize)]
struct OhlcResponse {
    data: OhlcData,
}

#[derive(Debug, Deserialize)]
struct OhlcData {
    ohlc: Vec<RawCandle>,
}

/// One element of `data.ohlc`: named fields, not a positional row — see the
/// module docs.
#[derive(Debug, Deserialize)]
struct RawCandle {
    timestamp: String,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

/// Every `(step, unit, seconds)` this source has verified or trusts as
/// Bitstamp's own published, fixed set of `step` values.
const INTERVALS: &[(u32, BarUnit, u32)] = &[
    (1, BarUnit::Minute, 60),
    (3, BarUnit::Minute, 180),
    (5, BarUnit::Minute, 300),
    (15, BarUnit::Minute, 900),
    (30, BarUnit::Minute, 1800),
    (1, BarUnit::Hour, 3600),
    (2, BarUnit::Hour, 7200),
    (4, BarUnit::Hour, 14400),
    (6, BarUnit::Hour, 21600),
    (12, BarUnit::Hour, 43200),
    (1, BarUnit::Day, 86400),
    (3, BarUnit::Day, 259_200),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Bitstamp's `step` seconds for `spec`, or `None` when `spec` is not one
/// this source serves.
fn step_of(spec: BarSpec) -> Option<u32> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, seconds)| seconds)
}

/// Bitstamp bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (Bitstamp sends no confirmation flag — see the module docs).
#[derive(Clone)]
pub struct BitstampBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BitstampBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitstampBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl BitstampBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `start`/`end` are epoch seconds; `range` is nanoseconds, so the
    /// conversion truncates toward the epoch. `start` is rounded *down* and
    /// `end` rounded *up* deliberately — see Gate's identical reasoning for
    /// its own `from`/`to` — and anything genuinely outside `range` is
    /// discarded again on the way back in [`BarSource::bars`] regardless.
    fn candles_url(&self, symbol: &str, step: u32, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let start = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let end = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}/{symbol}/?step={step}&limit={MAX_ROWS}&start={start}&end={end}",
            self.url
        )
    }
}

/// Bitstamp bars.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> BitstampBarSource {
    BitstampBarSource {
        url: OHLC_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BitstampBarSource {
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
        let step = step_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        // Every entry in `INTERVALS` names a fixed-width `BarUnit`, so this
        // is always `Some` for a spec `step_of` just accepted.
        let Some(duration_nanos) = spec.duration_nanos() else {
            return Err(SourceError::rejected(format!(
                "{spec} has no fixed duration to close candles against"
            )));
        };
        let Ok(duration_nanos) = u64::try_from(duration_nanos) else {
            return Err(SourceError::rejected(format!(
                "{spec}'s duration does not fit a non-negative span"
            )));
        };
        let bar_length = Duration::from_nanos(duration_nanos);

        let url = self.candles_url(symbol.as_str(), step, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let response: OhlcResponse = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        let rows = response.data.ohlc;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.volume.as_str()));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for row in rows {
            let ts_secs: i64 = row.timestamp.parse().map_err(|_| {
                SourceError::decode(format!("{:?} is not a valid timestamp", row.timestamp))
            })?;
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;

            // No confirmation flag exists on this venue: a candle is closed
            // only once its own close boundary — open plus the requested
            // bar's length — has passed. A `None` here (overflow) is
            // treated as not yet closed, never as closed by default.
            let Some(ts_close) = ts_open.checked_add(bar_length) else {
                continue;
            };
            if ts_close > now {
                continue;
            }

            if !range.contains(ts_open) {
                outside += 1;
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: scaled(&row.open, price_scale)?,
                high: scaled(&row.high, price_scale)?,
                low: scaled(&row.low, price_scale)?,
                close: scaled(&row.close, price_scale)?,
                volume: Volume::Real(scaled(&row.volume, qty_scale)?),
                // Neither reported by this endpoint — see the module docs.
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard for why an answer made entirely of
        // out-of-range rows is reported rather than swallowed: it is the
        // one shape indistinguishable, from the caller's side, from a
        // venue that has genuinely nothing for the window asked.
        if bars.is_empty() && outside > 0 {
            return Err(SourceError::rejected(format!(
                "answered with {outside} closed bars, none inside the requested range — \
                 the range parameters were not honoured"
            )));
        }

        // Ascending regardless of what the venue returns — Bitstamp already
        // is, but this must not silently rely on that.
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
    use senken_series::{BarSpec, BarUnit, Clock};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source;

    /// A real `GET /api/v2/ohlc/btcusd/?step=3600&limit=5` response,
    /// recorded 2026-09-02. Five rows, ascending, the newest of which
    /// opened at `06:00:00Z` — still forming at the wall clock the
    /// recording was made against.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("btcusd")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_300_000).unwrap(),
            UnixNanos::from_secs(1_788_340_000).unwrap(),
        )
        .unwrap()
    }

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

    /// Serves [`CANDLES`] from a mock server and returns a source pointed at
    /// it, closed at `now_ms`.
    async fn mock_source(now_ms: i64) -> (MockServer, super::BitstampBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client(), Arc::new(FixedClock(now_ms))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn the_still_forming_top_row_is_never_returned() {
        // The fixture's newest row opened at 06:00:00Z (1_788_328_800 s); a
        // clock reading 06:30:00Z sits inside that same hour.
        let (_server, source) = mock_source(1_788_330_600_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 4, "the fixture holds 5 rows, one still forming");
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_328_800_000)
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_five_are_kept() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert_eq!(bars.len(), 5);
    }

    #[tokio::test]
    async fn ohlcv_decodes_from_the_nested_named_fields() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        assert_eq!(first.ts_open, UnixNanos::from_secs(1_788_314_400).unwrap());
        // "76972.14" / "77283.00" / "76901.09" / "77275.58" at the batch's
        // common scale of two decimals.
        assert_eq!(first.open, 7_697_214);
        assert_eq!(first.high, 7_728_300);
        assert_eq!(first.low, 7_690_109);
        assert_eq!(first.close, 7_727_558);
        assert!(matches!(first.volume, senken_series::Volume::Real(v) if v > 0));
    }

    #[tokio::test]
    async fn each_row_s_open_stays_close_to_the_previous_row_s_close() {
        // A real, independent check on the field mapping: on a continuous
        // venue the open of one hour sits near the close of the last. Not
        // exact — real trading has a genuine gap between the final trade of
        // one hour and the first of the next — so this is a tolerance, like
        // Gate's identical test on its own fixture.
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        for pair in bars.windows(2) {
            let gap = (pair[1].open - pair[0].close).abs();
            assert!(
                gap <= 3_000,
                "open {} does not continue from close {}",
                pair[1].open,
                pair[0].close
            );
        }
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        // Only the fixture's second row, 1_788_318_000, falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_317_000).unwrap(),
            UnixNanos::from_secs(1_788_319_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_318_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                &br#"{"data":{"pair":"BTC/USD","ohlc":[]}}"#[..],
                "application/json",
            ))
            .mount(&server)
            .await;
        let source = bar_source(test_client(), Arc::new(FixedClock(4_102_444_800_000)))
            .with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_step() {
        assert!(super::step_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(super::step_of(BarSpec::new(1, BarUnit::Month)).is_none());
        assert!(super::step_of(BarSpec::new(2, BarUnit::Day)).is_none());
    }

    #[test]
    fn a_three_day_step_is_offered_as_a_fixed_width_multiple() {
        assert_eq!(
            super::step_of(BarSpec::new(3, BarUnit::Day)).unwrap(),
            259_200
        );
    }

    #[test]
    fn every_supported_spec_maps_to_a_step() {
        let source = bar_source(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                super::step_of(*spec).is_some(),
                "{spec} is offered but has no step mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_318_000).unwrap(),
            UnixNanos::from_secs(1_788_318_000).unwrap(),
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

    #[test]
    fn max_rows_is_the_documented_tested_cap() {
        let source = bar_source(test_client(), Arc::new(FixedClock(0)));
        assert_eq!(source.max_rows(), 1000);
    }
}
