//! Poloniex spot bar fetching — `GET /markets/{symbol}/candles`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `BTC_USDT`, not read from documentation.
//!
//! 1. **Sort direction**: ascending by open time.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a JSON
//!    number — field 12 (`startTime`), not field 9 (see below).
//! 3. **Closed-candle detection**: this task was scoped on the
//!    assumption that Poloniex carries no flag at all, matching the
//!    uniform "compare a close time against a [`Clock`]" answer this
//!    task gave for Bitget and KuCoin. The recorded response
//!    contradicts that assumption in one respect worth recording
//!    plainly rather than silently working around: **field 13 is a
//!    `closeTime`**, and on the response's own still-open final row it
//!    reads *after* the moment the response was captured — so it is not
//!    a static echo of `startTime + interval`, it behaves like a real
//!    close time. This source still compares it against an injected
//!    [`Clock`] rather than trusting its mere presence as a boolean
//!    flag, for the same reason `plugins/binance` does: nothing in the
//!    response says "trust me", only a timestamp to compare. Using the
//!    venue's own `closeTime` rather than recomputing `ts_open +
//!    spec.duration_nanos()` also sidesteps the one case that duration
//!    cannot express — [`senken_series::BarUnit::Month`] — for free,
//!    though this source does not currently offer a monthly spec.
//! 4. **Row cap**: 500, confirmed both by this task's own scoping and
//!    independently by Poloniex's public API reference ("the default
//!    value is 100 and the max value is 500"), and understood to be
//!    **silent** beyond it per the live audit this task was grouped
//!    under — not independently reproduced here to avoid a second live
//!    request for an already-established fact.
//! 5. **Pagination direction**: `startTime`/`endTime`, both epoch
//!    milliseconds, confirmed live: a five-hour window bounded by both
//!    returned exactly the five candles inside it, ascending.
//!
//! # The field order
//!
//! A row is fourteen positional values:
//!
//! ```text
//! [ low, high, open, close, amount, quantity, buyTakerAmount,
//!   buyTakerQuantity, tradeCount, ts, weightedAverage, interval,
//!   startTime, closeTime ]
//! ```
//!
//! `ts` (field 9) and `startTime` (field 12) coincide in every row this
//! source has observed, but only `startTime` is documented as the
//! candle's own open time — `ts` is described elsewhere in Poloniex's
//! API as a generic "message time" shared across endpoints, so this
//! source reads `startTime`, never `ts`, as `Bar::ts_open`.
//!
//! # What was verified, and what is a documented assumption
//!
//! - `HOUR_1` was requested and the spacing between rows measured:
//!   exactly 3 600 000 ms apart, and `closeTime - startTime == 3 599 999`
//!   on every row.
//! - The remaining specs in [`INTERVALS`] are the enum Poloniex's own
//!   public API reference documents for this parameter, fetched live
//!   from that reference during this work — not requested against the
//!   candles endpoint itself and measured, so still marked as
//!   documented-not-response-verified, the same distinction this
//!   project draws for `plugins/binance`.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::de::IgnoredAny;

use crate::SPOT_ID;

/// The `/markets` prefix every candle request is nested under:
/// `{MARKETS_URL}/{symbol}/candles`.
const MARKETS_URL: &str = "https://api.poloniex.com/markets";

/// The row cap this task was scoped with and Poloniex's own public API
/// reference confirms independently. See the module docs.
const MAX_ROWS: usize = 500;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source in this
/// workspace.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /markets/{symbol}/candles`: fourteen positional
/// values — see the module docs for the field order. `tradeCount` and
/// `ts` are read but not carried into the returned [`Bar`] (`ts_open`
/// comes from `startTime`, field 12, not `ts`, field 9 — see the module
/// docs); `weightedAverage` and `interval` are ignored entirely.
type RawCandle = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u32,
    IgnoredAny,
    IgnoredAny,
    IgnoredAny,
    i64,
    i64,
);

/// Every `(step, unit, interval)` this source will ask Poloniex for.
/// `HOUR_1` was requested and measured live; every other entry is the
/// enum Poloniex's own public API reference documents for this
/// parameter — see the module docs.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "MINUTE_1"),
    (5, BarUnit::Minute, "MINUTE_5"),
    (15, BarUnit::Minute, "MINUTE_15"),
    (30, BarUnit::Minute, "MINUTE_30"),
    // Measured live: 3,600,000 ms apart.
    (1, BarUnit::Hour, "HOUR_1"),
    (4, BarUnit::Hour, "HOUR_4"),
    (1, BarUnit::Day, "DAY_1"),
    (1, BarUnit::Week, "WEEK_1"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`].
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Poloniex's `interval` string for `spec`, or `None` when `spec` is not
/// one this source has mapped.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, interval)| interval)
}

/// Poloniex spot bars, fetched through a [`VenueClient`] and closed
/// against a [`Clock`] compared to each row's own `closeTime` — see the
/// module docs on why this is not treated as a boolean confirmation
/// flag.
#[derive(Clone)]
pub struct PoloniexBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for PoloniexBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoloniexBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl PoloniexBarSource {
    /// Points this source at a different `/markets`-equivalent prefix — a
    /// mirror, or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn candles_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        format!(
            "{}/{symbol}/candles?interval={interval}&limit={MAX_ROWS}&startTime={}&endTime={}",
            self.url,
            range.start().as_millis(),
            // `endTime` is inclusive on this venue; `range.end()` is
            // exclusive (`TimeRange`'s own half-open contract), so the
            // last representable millisecond strictly before it is sent.
            range.end().as_millis() - 1,
        )
    }
}

/// The Poloniex spot bar source.
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> PoloniexBarSource {
    PoloniexBarSource {
        url: MARKETS_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for PoloniexBarSource {
    fn source_id(&self) -> &str {
        SPOT_ID
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
        let url = self.candles_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.2.as_str(),
                row.1.as_str(),
                row.0.as_str(),
                row.3.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().flat_map(|row| [row.5.as_str(), row.4.as_str()]));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (
            low,
            high,
            open,
            close,
            amount,
            quantity,
            _buy_taker_amount,
            _buy_taker_quantity,
            trade_count,
            _ts,
            _weighted_average,
            _interval,
            start_time,
            close_time,
        ) in rows
        {
            // No boolean confirmation flag on this endpoint: a candle is
            // closed only once its own `closeTime` has passed — see the
            // module docs on why this is read from the venue's own field
            // rather than recomputed from `spec`'s length.
            let close_time = UnixNanos::from_millis(close_time).ok_or_else(|| {
                SourceError::decode(format!("close time {close_time}ms overflowed"))
            })?;
            if close_time > now {
                continue;
            }

            let ts_open = UnixNanos::from_millis(start_time).ok_or_else(|| {
                SourceError::decode(format!("open time {start_time}ms overflowed"))
            })?;
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
                volume: Volume::Real(scaled(&quantity, qty_scale)?),
                quote_volume: Some(scaled(&amount, qty_scale)?),
                trade_count: Some(trade_count),
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard for why an answer made entirely of
        // rows outside the requested range is reported, not swallowed —
        // exactly the silent wide-window failure this venue was grouped
        // for.
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source_spot, interval_of};

    /// A real `GET /markets/BTC_USDT/candles?interval=HOUR_1` response,
    /// recorded 2026-09-02. Ten rows, ascending; the last row's
    /// `closeTime` (1,788,332,399,999) is still ahead of the capture
    /// instant, which is exactly the still-forming candle this source
    /// must never return.
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
            UnixNanos::from_millis(1_788_290_000_000).unwrap(),
            UnixNanos::from_millis(1_788_340_000_000).unwrap(),
        )
        .unwrap()
    }

    async fn serving_at(now_ms: i64) -> (MockServer, super::PoloniexBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/BTC_USDT/candles"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source =
            bar_source_spot(test_client(), Arc::new(FixedClock(now_ms))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn the_still_forming_candle_the_close_time_reveals_is_never_returned() {
        // The fixture's last row's closeTime is 1,788,332,399,999; a
        // clock reading before that must drop it.
        let (_server, source) = serving_at(1_788_329_100_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars.len(),
            9,
            "the fixture holds 10 rows, one still forming"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_328_800_000)
        );
    }

    #[tokio::test]
    async fn once_the_clock_passes_every_close_time_all_rows_are_kept() {
        let (_server, source) = serving_at(1_788_400_000_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 10);
    }

    #[tokio::test]
    async fn open_time_is_read_from_start_time_not_the_generic_ts_field() {
        let (_server, source) = serving_at(1_788_400_000_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // Fixture row 0: low 76909.47, high 77510.66, open 77438.93,
        // close 77199.3, startTime 1788296400000.
        let first = &bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_296_400_000).unwrap()
        );
        assert_eq!(first.low, 7_690_947);
        assert_eq!(first.high, 7_751_066);
        assert_eq!(first.open, 7_743_893);
        assert_eq!(first.close, 7_719_930);
    }

    #[tokio::test]
    async fn rows_are_ascending() {
        let (_server, source) = serving_at(1_788_400_000_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = serving_at(1_788_400_000_000).await;
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_303_500_000).unwrap(),
            UnixNanos::from_millis(1_788_304_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_303_600_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = serving_at(1_788_400_000_000).await;
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/BTC_USDT/candles"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(&b"[]"[..], "application/json"))
            .mount(&server)
            .await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(1_788_400_000_000)))
            .with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn volume_and_quote_volume_come_from_quantity_and_amount() {
        let (_server, source) = serving_at(1_788_400_000_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(matches!(bars[0].volume, Volume::Real(v) if v > 0));
        assert!(bars[0].quote_volume.is_some_and(|q| q > 0));
        assert!(bars[0].trade_count.is_some_and(|c| c > 0));
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_interval_string() {
        assert!(interval_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(interval_of(BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                interval_of(*spec).is_some(),
                "{spec} is offered but has no interval mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_296_400_000).unwrap(),
            UnixNanos::from_millis(1_788_296_400_000).unwrap(),
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
