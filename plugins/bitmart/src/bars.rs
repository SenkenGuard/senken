//! BitMart spot bar fetching — `GET /spot/quotation/v3/klines`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `symbol=BTC_USDT`, not read from documentation. The `/spot/v1/symbols/kline`
//! endpoint this project's own instrument catalog comment once pointed at is
//! dead — it now answers HTTP 200 with `code: 30031` ("This endpoint has
//! been deprecated") — so this source uses the v3 replacement instead.
//!
//! 1. **Sort direction**: ascending by open time. Sorted again here anyway,
//!    because a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a *string*.
//! 3. **Closed-candle detection**: no flag and no server-time field at all.
//!    A candle is closed only once its own open time plus the spec's fixed
//!    duration has passed — compared against [`Clock::now`], never the wall
//!    clock directly, exactly as `senken_plugin::clock` documents.
//! 4. **Row cap (tested)**: 200.
//! 5. **Pagination direction**: `after`/`before`, both epoch seconds,
//!    bounding the window server-side; `after=1787000000&before=1787010000`
//!    returned exactly the candles overlapping that span. An invalid `step`
//!    is refused loud (`code: 71005, "request step is invalid"`, `data:
//!    null`) rather than silently answered with the nearest supported one.
//!
//! # The field order **is** OHLC, unlike this workspace's Gate and WhiteBIT
//! sources
//!
//! A row is seven positional strings, in the order the name suggests:
//!
//! ```text
//! [ ts, open, high, low, close, volume, quote_volume ]
//!    0    1     2    3     4      5          6
//! ```
//!
//! Checked, not assumed: in the recorded fixture, `high >= max(open, close)`
//! and `low <= min(open, close)` hold for every row read this way, and fail
//! if the middle two fields are swapped.
//!
//! # What was verified, and what is a documented assumption
//!
//! The six intervals in [`supported_specs`] were each requested and the
//! spacing between returned candles measured, so every one of them is known
//! to be honoured rather than silently substituted (`step=7`, an unlisted
//! value, is the one that produced the loud `71005` rejection above — the
//! evidence that guessing a `step` wrong fails loud rather than answering
//! with the wrong width).

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

const KLINES_URL: &str = "https://api-cloud.bitmart.com/spot/quotation/v3/klines";

/// The tested cap: BitMart spot's klines answer at most 200 rows per call.
pub(crate) const MAX_ROWS: usize = 200;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source in this
/// workspace.
const KLINES_FETCH_COST: u32 = 5;

/// One row of `GET /spot/quotation/v3/klines`: seven positional strings, in
/// OHLC order — see the module docs for why that is worth stating rather
/// than assuming.
type RawCandle = (String, String, String, String, String, String, String);

/// The envelope every BitMart quotation endpoint answers with. `data` is
/// `null`, not an empty array, when the venue rejects the request (a bad
/// `step` answers `code: 71005` this way).
#[derive(Debug, Deserialize)]
struct KlinesEnvelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Option<Vec<RawCandle>>,
}

/// Every `(spec, step)` pair this source has verified, and the only ones it
/// will ever ask BitMart for. `step` is BitMart's own request parameter: an
/// integer count of minutes.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1"),
    (5, BarUnit::Minute, "5"),
    (15, BarUnit::Minute, "15"),
    (1, BarUnit::Hour, "60"),
    (4, BarUnit::Hour, "240"),
    (1, BarUnit::Day, "1440"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
pub(crate) fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// BitMart's `step` string for `spec`, or `None` when `spec` is not one this
/// source has verified.
pub(crate) fn step_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, step)| step)
}

/// BitMart spot bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] — BitMart sends no confirmation flag, so "now" must come from
/// somewhere (see `senken_plugin::clock`).
#[derive(Clone)]
pub struct BitmartBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BitmartBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmartBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl BitmartBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `after`/`before` are epoch **seconds**, and `range` is nanoseconds,
    /// so the conversion truncates toward the epoch. `after` is rounded
    /// *down* and `before` rounded *up* deliberately: a candle whose open
    /// time falls inside `range` but whose second-precision form would land
    /// just outside it must still be asked for, and anything genuinely
    /// outside is discarded on the way back in [`BarSource::bars`] anyway.
    fn candles_url(&self, symbol: &str, step: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let after = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let before = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?symbol={symbol}&step={step}&after={after}&before={before}&limit={MAX_ROWS}",
            self.url,
        )
    }
}

/// BitMart spot bars.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> BitmartBarSource {
    BitmartBarSource {
        url: KLINES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for BitmartBarSource {
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
        let step = step_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        // Every supported spec has a fixed duration (`Minute`/`Hour`/`Day`
        // only), so this is never `None` in practice; handled rather than
        // unwrapped in case that ever changes.
        let duration_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;
        let duration = Duration::from_nanos(u64::try_from(duration_nanos).unwrap_or(u64::MAX));

        let url = self.candles_url(symbol.as_str(), step, range);
        let body = self.client.get(&url, KLINES_FETCH_COST).await?;
        let envelope: KlinesEnvelope =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if envelope.code != 1000 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.message
            )));
        }
        let rows = envelope.data.unwrap_or_default();

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
        for (ts, open, high, low, close, volume, quote_volume) in rows {
            let ts_secs: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;

            // No confirmation flag: a candle is closed only once its own
            // open time plus the spec's duration has passed.
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

        // Same defensive check Gate's source makes: an answer made entirely
        // of rows outside the requested range means the range parameters
        // were not honoured, and that must be reported rather than handed
        // back as an empty (and therefore cacheable-as-absent) result.
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
    use senken_marketdata::source::SourceError;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source, step_of};

    /// A real `GET /spot/quotation/v3/klines?symbol=BTC_USDT&step=1&limit=10`
    /// response, recorded 2026-09-02: ten one-minute candles.
    const KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1m.json");

    /// A `Clock` a test fully controls.
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

    fn minute() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_000_000).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap()
    }

    /// Serves [`KLINES`] and returns a source pointed at it, closed at
    /// `now_secs`.
    async fn mock_source(now_secs: i64) -> (MockServer, super::BitmartBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/klines"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(KLINES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(
            VenueClient::new(reqwest::Client::new(), LimitGroup::new("test")),
            Arc::new(FixedClock(now_secs)),
        )
        .with_url(format!("{}/klines", server.uri()));
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_with_correct_ohlcv_and_ascending_order() {
        // The fixture's ten rows run 1788329040..=1788329580 one minute
        // apart. A clock set well past the last one's close keeps all ten.
        let (_server, source) = mock_source(4_102_444_800).await;
        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 10);
        assert!(
            bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open),
            "must be ascending"
        );

        let first = bars[0];
        assert_eq!(first.ts_open, UnixNanos::from_secs(1_788_329_040).unwrap());
        // Row 0: open 77667.25, high 77716.00, low 77667.25, close 77690.00.
        assert_eq!(first.open, 7_766_725);
        assert_eq!(first.high, 7_771_600);
        assert_eq!(first.low, 7_766_725);
        assert_eq!(first.close, 7_769_000);
        assert!(first.high >= first.open.max(first.close));
        assert!(first.low <= first.open.min(first.close));
    }

    #[tokio::test]
    async fn a_candle_still_within_its_own_duration_of_now_is_never_returned() {
        // A clock set to exactly the last row's own open time: that
        // candle's own close (open + 60s) has not happened yet.
        let (_server, source) = mock_source(1_788_329_580).await;
        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 9, "the tenth row has not closed at this clock");
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_secs(1_788_329_580).unwrap())
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800).await;
        // Only the fixture's second row falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_329_100).unwrap(),
            UnixNanos::from_secs(1_788_329_160).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), minute(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_329_100).unwrap()
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

        let error = source
            .bars(&symbol(), minute(), elsewhere)
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
            .and(path("/klines"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    br#"{"data":[],"code":1000,"message":"success","trace":"t","subCode":null}"#
                        .as_slice(),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        let source = bar_source(
            VenueClient::new(reqwest::Client::new(), LimitGroup::new("test")),
            Arc::new(FixedClock(4_102_444_800)),
        )
        .with_url(format!("{}/klines", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_rejection_code_with_a_null_data_field_is_reported() {
        // The real shape of BitMart's `step=7` (invalid) response.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/klines"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"data":null,"subCode":null,"trace":"t","code":71005,"message":"request step is invalid"}"#
                    .as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source = bar_source(
            VenueClient::new(reqwest::Client::new(), LimitGroup::new("test")),
            Arc::new(FixedClock(4_102_444_800)),
        )
        .with_url(format!("{}/klines", server.uri()));

        let error = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }

    #[test]
    fn a_four_hour_step_is_asked_for_as_240_minutes() {
        assert_eq!(step_of(BarSpec::new(4, BarUnit::Hour)).unwrap(), "240");
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_step() {
        assert!(step_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(step_of(BarSpec::new(1, BarUnit::Week)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_step() {
        let source = bar_source(
            VenueClient::new(reqwest::Client::new(), LimitGroup::new("test")),
            Arc::new(FixedClock(0)),
        );
        for spec in source.supported() {
            assert!(
                step_of(*spec).is_some(),
                "{spec} is offered but has no step mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source(
            VenueClient::new(reqwest::Client::new(), LimitGroup::new("test")),
            Arc::new(FixedClock(4_102_444_800)),
        )
        .with_url(format!("{}/klines", server.uri()));
        let point = UnixNanos::from_secs(1_788_329_580).unwrap();
        let inverted = TimeRange::new(point, point);

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
