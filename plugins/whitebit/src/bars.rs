//! WhiteBIT bar fetching — `GET /api/v1/public/kline`.
//!
//! # Not the host this crate's own instrument catalog uses
//!
//! `/api/v4/public/kline`, the natural v4 sibling of this crate's
//! `/api/v4/public/markets`, answers HTTP 404 — WhiteBIT never shipped a v4
//! klines endpoint. Only the legacy `GET /api/v1/public/kline` is live, so
//! this source talks to a different API generation than the rest of this
//! plugin, verified by requesting both and comparing the results.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `market=BTC_USDT`.
//!
//! 1. **Sort direction**: ascending by open time. Sorted again here anyway,
//!    because a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a JSON number.
//! 3. **Closed-candle detection**: no flag and no server-time field at all.
//!    A candle is closed only once its own open time plus the spec's fixed
//!    duration has passed — compared against [`Clock::now`], exactly as
//!    `senken_plugin::clock` documents.
//! 4. **Row cap**: 1440 — the given, already-verified figure; requesting
//!    beyond it is refused loud rather than silently truncated.
//! 5. **Pagination direction**: `start`/`end`, both epoch seconds, bounding
//!    the window server-side; `start=1787000000&end=1787010800` returned
//!    exactly the four hourly candles covering that span.
//!
//! # The field order is not OHLC, exactly like this workspace's Gate source
//!
//! A row is seven positional values:
//!
//! ```text
//! [ ts, open, close, high, low, volume, quote_volume ]
//!    0    1     2     3    4      5          6
//! ```
//!
//! **Close comes before high and low.** Checked, not assumed: in the
//! recorded fixture, `high >= max(open, close)` and `low <= min(open,
//! close)` hold for every row read this way, and fail if fields 2 and 3
//! are swapped.
//!
//! # `message` is not always a string
//!
//! A rejected request (`interval=7m`, an unlisted value) answers
//! `{"success": false, "message": {"interval": ["Invalid interval."]},
//! "result": null}` — `message` is a JSON *object* here, not the plain
//! string every success-path caller might assume from its name. This
//! source reads it as a bare [`serde_json::Value`] and renders whatever
//! shape it turns out to be into the rejection text, rather than assuming
//! a string and failing to even report the venue's own reason.
//!
//! # What was verified, and what is a documented assumption
//!
//! The six intervals in [`supported_specs`] were each requested and the
//! spacing between returned candles measured, so every one of them is known
//! to be honoured rather than silently substituted; `interval=7m`, an
//! unlisted value, is the one that produced the rejection documented above
//! — evidence that guessing an interval wrong fails loud rather than
//! answering with the wrong width.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const KLINE_URL: &str = "https://whitebit.com/api/v1/public/kline";

/// The given, already-verified tested cap: WhiteBIT's klines refuse
/// `limit` beyond 1440 rather than silently truncating.
const MAX_ROWS: usize = 1440;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, matching
/// every other bar source in this workspace.
const KLINE_FETCH_COST: u32 = 5;

/// One row of `GET /api/v1/public/kline`: seven positional values, **not**
/// OHLC order — see the module docs.
type RawCandle = (i64, String, String, String, String, String, String);

/// The envelope every legacy WhiteBIT endpoint answers with. `message` is
/// not always a string (see the module docs), and `result` is `null`, not
/// an empty array, on a rejected request.
#[derive(Debug, Deserialize)]
struct KlineEnvelope {
    success: bool,
    #[serde(default)]
    message: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<Vec<RawCandle>>,
}

/// Every `(spec, interval)` pair this source has verified, and the only
/// ones it will ever ask WhiteBIT for.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (1, BarUnit::Hour, "1h"),
    (4, BarUnit::Hour, "4h"),
    (1, BarUnit::Day, "1d"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// WhiteBIT's `interval` string for `spec`, or `None` when `spec` is not
/// one this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, interval)| interval)
}

/// Parses `raw` at `scale`, mapping an unparseable value to a decode error
/// rather than panicking or guessing.
fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// WhiteBIT bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] — WhiteBIT sends no confirmation flag, so "now" must come from
/// somewhere (see `senken_plugin::clock`).
#[derive(Clone)]
pub struct WhitebitBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for WhitebitBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhitebitBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl WhitebitBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// `start`/`end` are epoch **seconds**, and `range` is nanoseconds, so
    /// the conversion truncates toward the epoch — `start` rounded *down*
    /// and `end` rounded *up*, matching every other source in this
    /// workspace that shares this shape.
    fn kline_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let start = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let end = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?market={symbol}&interval={interval}&start={start}&end={end}&limit={MAX_ROWS}",
            self.url,
        )
    }
}

/// WhiteBIT bars.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> WhitebitBarSource {
    WhitebitBarSource {
        url: KLINE_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for WhitebitBarSource {
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
        let interval = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let duration_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;
        let duration = Duration::from_nanos(u64::try_from(duration_nanos).unwrap_or(u64::MAX));

        let url = self.kline_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, KLINE_FETCH_COST).await?;
        let envelope: KlineEnvelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.success {
            let reason = envelope
                .message
                .map_or_else(|| "no message".to_owned(), |m| m.to_string());
            return Err(SourceError::rejected(reason));
        }
        let rows = envelope.result.unwrap_or_default();

        // Field 2 is the close and field 3 the high — see the module docs.
        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().flat_map(|row| [row.5.as_str(), row.6.as_str()]));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (ts, open, close, high, low, volume, quote_volume) in rows {
            let ts_open = UnixNanos::from_secs(ts)
                .ok_or_else(|| SourceError::decode(format!("open time {ts}s overflowed")))?;

            let still_forming = match ts_open.checked_add(duration) {
                Some(close_time) => close_time > now,
                None => true,
            };
            if still_forming {
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
                quote_volume: Some(scaled(&quote_volume, qty_scale)?),
                // Neither reported by this endpoint.
                trade_count: None,
                taker_buy_volume: None,
            });
        }

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::source::SourceError;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source, interval_of};

    /// A real `GET /api/v1/public/kline?market=BTC_USDT&interval=1h&limit=6`
    /// response, recorded 2026-09-02: six hourly candles.
    const KLINES: &[u8] = include_bytes!("../tests/fixtures/kline_1h.json");

    #[derive(Debug)]
    struct FixedClock(i64);

    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_secs(self.0).unwrap()
        }

        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("BTC_USDT")
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_000_000).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap()
    }

    async fn mock_source(now_secs: i64) -> (MockServer, super::WhitebitBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(KLINES, "application/json"))
            .mount(&server)
            .await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(now_secs))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_with_correct_ohlcv_and_ascending_order() {
        let (_server, source) = mock_source(4_102_444_800).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 6);
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        let first = bars[0];
        assert_eq!(first.ts_open, UnixNanos::from_secs(1_788_307_200).unwrap());
        // Row 0: open 77438.61, close 77216.26, high 77506.82, low 77137.78.
        assert_eq!(first.open, 7_743_861, "open is field 1, not field 2");
        assert_eq!(first.close, 7_721_626, "close is field 2, not field 1");
        assert_eq!(first.high, 7_750_682);
        assert_eq!(first.low, 7_713_778);
        assert!(first.high >= first.open.max(first.close));
        assert!(first.low <= first.open.min(first.close));
    }

    #[tokio::test]
    async fn a_candle_still_within_its_own_duration_of_now_is_never_returned() {
        // A clock set to the last row's own open time: that candle's own
        // close (open + 1h) has not happened yet.
        let (_server, source) = mock_source(1_788_325_200).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert_eq!(bars.len(), 5, "the sixth row has not closed at this clock");
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800).await;
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_310_800).unwrap(),
            UnixNanos::from_secs(1_788_314_400).unwrap(),
        )
        .unwrap();
        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_310_800).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = mock_source(4_102_444_800).await;
        let elsewhere = TimeRange::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            UnixNanos::from_secs(1_700_003_600).unwrap(),
        )
        .unwrap();
        let error = source.bars(&symbol(), hour(), elsewhere).await.unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }

    #[tokio::test]
    async fn an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"success":true,"message":null,"result":[]}"#.as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(4_102_444_800))).with_url(server.uri());
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_rejection_with_an_object_shaped_message_is_still_reported() {
        // The real shape of WhiteBIT's `interval=7m` (invalid) response:
        // `message` is an object, not a string.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"success":false,"message":{"interval":["Invalid interval."]},"result":null}"#
                    .as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(4_102_444_800))).with_url(server.uri());
        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
        assert!(error.to_string().contains("Invalid interval"));
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(4_102_444_800))).with_url(server.uri());
        let point = UnixNanos::from_secs(1_788_325_200).unwrap();
        if let Some(range) = TimeRange::new(point, point) {
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
    fn a_four_hour_interval_is_asked_for_as_4h() {
        assert_eq!(interval_of(BarSpec::new(4, BarUnit::Hour)).unwrap(), "4h");
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_interval() {
        assert!(interval_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(interval_of(BarSpec::new(1, BarUnit::Week)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval() {
        let source = bar_source(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(interval_of(*spec).is_some());
        }
    }
}
