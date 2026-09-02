//! KuCoin spot bar fetching — `GET /api/v1/market/candles`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `symbol=BTC-USDT`, not read from documentation.
//!
//! 1. **Sort direction**: descending by open time — newest row first.
//!    Sorted ascending before returning, never trusted as-is.
//! 2. **Timestamp representation**: epoch **seconds**, as a *string* —
//!    the first element of each row, unlike the milliseconds every
//!    neighbouring venue in this workspace reports.
//! 3. **Closed-candle detection**: no flag at all. Closure is determined
//!    by comparing `ts_open` plus the spec's own fixed length against a
//!    [`senken_series::Clock`], the same pattern
//!    `plugins/binance/src/bars.rs` uses for the identical reason.
//! 4. **Row cap**: this task was scoped with 1500 as the tested cap, and
//!    a **silent** wide-window failure — a request wider than the
//!    venue's real coverage answers HTTP 200 with a narrower window, not
//!    an error. Not independently re-derived here (a second live request
//!    for an already-established fact was judged not worth it); the
//!    guard at the end of this module's `bars()` implementation exists
//!    regardless of whether any one run happens to trip it.
//! 5. **Pagination direction**: `startAt`/`endAt`, both epoch **seconds**
//!    (matching the row timestamp's own unit, unlike every millisecond
//!    venue elsewhere in this workspace), confirmed live: a five-hour
//!    window bounded by both returned exactly the five candles inside
//!    it.
//!
//! # The field order is not OHLC
//!
//! A row is seven positional strings:
//!
//! ```text
//! [ ts, open, close, high, low, volume, turnover ]
//!    0    1     2     3    4     5         6
//! ```
//!
//! **Close comes before high and low**, the same trap Gate's
//! `candlesticks` carries under a different arrangement. Confirmed from
//! the data itself, not assumed: in the recorded fixture, each row's
//! field 1 (open) sits within a tick of the row *before* it (KuCoin's own
//! descending order, so "before" here means the next-newer row) field 2
//! (close) — which only holds if 1 is open and 2 is close.
//!
//! # What was verified, and what is a documented assumption
//!
//! - `1hour` was requested and the spacing between rows measured: exactly
//!   3 600 seconds apart.
//! - The remaining specs in [`INTERVALS`] follow KuCoin's own documented
//!   `type` syntax for this endpoint but were **not** individually
//!   requested and measured — an explicit, commented assumption, kept
//!   deliberately small.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale, exact_common_scale};

use crate::api::Envelope;
use crate::{OK, SPOT_ID};

const CANDLES_URL: &str = "https://api.kucoin.com/api/v1/market/candles";

/// The tested cap this task was scoped with: 1500, and silent beyond it.
/// See the module docs.
const MAX_ROWS: usize = 1500;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source in this
/// workspace.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v1/market/candles`: seven positional strings —
/// `[ts, open, close, high, low, volume, turnover]`. **Not** OHLC order
/// — see the module docs.
type RawCandle = (String, String, String, String, String, String, String);

/// Every `(step, unit, type)` this source will ask KuCoin for. See the
/// module docs for which entry was measured against a live response and
/// which follows documented syntax only.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1min"),
    (5, BarUnit::Minute, "5min"),
    (15, BarUnit::Minute, "15min"),
    (30, BarUnit::Minute, "30min"),
    // Measured live: 3,600 s apart.
    (1, BarUnit::Hour, "1hour"),
    (4, BarUnit::Hour, "4hour"),
    (1, BarUnit::Day, "1day"),
    (1, BarUnit::Week, "1week"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`].
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// KuCoin's `type` string for `spec`, or `None` when `spec` is not one
/// this source has mapped.
fn type_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, ty)| ty)
}

/// KuCoin spot bars, fetched through a [`VenueClient`] and closed against
/// a [`Clock`] — this endpoint carries no confirmation flag (see the
/// module docs).
#[derive(Clone)]
pub struct KucoinBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for KucoinBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KucoinBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl KucoinBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn candles_url(&self, symbol: &str, ty: &str, range: TimeRange) -> String {
        // Mirrors Gate's own seconds conversion: `start_at` rounds down
        // and `end_at` rounds *up*, deliberately. `range` is nanoseconds
        // and this venue's window is whole seconds, so a candle whose
        // open time falls inside `range` but whose second-precision form
        // would land just outside it must still be asked for — anything
        // genuinely outside is discarded on the way back in `bars()`
        // regardless, so over-asking here is free and under-asking is
        // not.
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let start_at = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let end_at = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?type={ty}&symbol={symbol}&startAt={start_at}&endAt={end_at}",
            self.url
        )
    }
}

/// The KuCoin spot bar source.
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> KucoinBarSource {
    KucoinBarSource {
        url: CANDLES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for KucoinBarSource {
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
        let ty = type_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_len_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed length")))?;

        let url = self.candles_url(symbol.as_str(), ty, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope<RawCandle> =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.code.is_empty() && envelope.code != OK {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        // Field 2 is the close and field 3 the high — see the module
        // docs on why this is not OHLC order.
        let price_scale = common_scale(envelope.data.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        // `None` when this batch's quantities do not fit an `i64` at the
        // venue's own scale — see `quantity_scale`.
        let qty_scale = exact_common_scale(
            envelope
                .data
                .iter()
                .flat_map(|row| [row.5.as_str(), row.6.as_str()]),
        );
        if qty_scale.is_none() {
            tracing::warn!(
                source = SPOT_ID,
                "KuCoin reported quantities finer than a scaled i64 can hold; \
                 these bars carry prices but no volume"
            );
        }

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(envelope.data.len());
        let mut outside = 0usize;
        for (ts, open, close, high, low, volume, turnover) in envelope.data {
            let ts_secs: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;

            // No confirmation flag on this endpoint: a candle is closed
            // only once its own close time — open plus the spec's fixed
            // length — has passed.
            let close_time =
                UnixNanos::from_nanos(ts_open.as_nanos().saturating_add(bar_len_nanos));
            if close_time > now {
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
                volume: match qty_scale {
                    Some(scale) => Volume::Real(scaled(&volume, scale)?),
                    None => Volume::Absent,
                },
                quote_volume: qty_scale.and_then(|scale| parse_scaled(&turnover, scale)),
                trade_count: None,
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
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source_spot, type_of};

    /// A real `GET /api/v1/market/candles?type=1hour&symbol=BTC-USDT`
    /// response, recorded 2026-09-02, unbounded (no `startAt`/`endAt`).
    /// Descending by open time; the newest row opens at 1,788,328,800 and
    /// was still forming at capture.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

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

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("BTC-USDT")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    /// The whole window the fixture covers, and then some.
    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_200_000).unwrap(),
            UnixNanos::from_secs(1_788_340_000).unwrap(),
        )
        .unwrap()
    }

    async fn serving_at(
        body: &'static [u8],
        now_secs: i64,
    ) -> (MockServer, super::KucoinBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source =
            bar_source_spot(test_client(), Arc::new(FixedClock(now_secs))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn the_still_forming_newest_row_is_dropped_at_a_mid_hour_clock() {
        // The fixture's newest row opens at 1,788,328,800 and closes an
        // hour later; a clock reading before that close must drop it.
        let (_server, source) = serving_at(CANDLES, 1_788_329_100).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_nanos() / 1_000_000_000 < 1_788_328_800),
            "the still-forming row must not appear"
        );
    }

    #[tokio::test]
    async fn once_the_clock_passes_the_newest_close_time_it_is_kept_too() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(
            bars.iter()
                .any(|b| b.ts_open == UnixNanos::from_secs(1_788_328_800).unwrap())
        );
    }

    #[tokio::test]
    async fn open_and_close_are_read_from_the_venue_s_own_field_order_not_ohlc() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // The newest fixture row: open 77625, close 77674.6, high 77689,
        // low 77625.
        let newest = bars
            .iter()
            .find(|b| b.ts_open == UnixNanos::from_secs(1_788_328_800).unwrap())
            .unwrap();
        assert_eq!(newest.open, 776_250, "open is field 1, not field 2");
        assert_eq!(newest.close, 776_746, "close is field 2, not field 1");
        assert_eq!(newest.high, 776_890);
        assert_eq!(newest.low, 776_250);
        assert!(newest.high >= newest.open.max(newest.close));
        assert!(newest.low <= newest.open.min(newest.close));
    }

    #[tokio::test]
    async fn rows_are_returned_ascending_even_though_the_venue_sends_descending() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
    }

    #[tokio::test]
    async fn timestamps_are_read_as_seconds_not_milliseconds() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let a = bars
            .iter()
            .find(|b| b.ts_open == UnixNanos::from_secs(1_788_325_200).unwrap())
            .unwrap();
        let b = bars
            .iter()
            .find(|b| b.ts_open == UnixNanos::from_secs(1_788_328_800).unwrap())
            .unwrap();
        assert_eq!(
            b.ts_open.as_nanos() - a.ts_open.as_nanos(),
            3_600 * 1_000_000_000,
            "one hour apart, so the unit was read correctly"
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_321_600).unwrap(),
            UnixNanos::from_secs(1_788_323_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_321_600).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;
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
        let body = br#"{"code":"200000","data":[]}"#;
        let (_server, source) = serving_at(body, 1_788_400_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":"400100","msg":"Invalid parameter","data":[]}"#;
        let (_server, source) = serving_at(body, 1_788_400_000).await;

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("400100"));
    }

    /// One row of the recorded response, verbatim — the same bytes, just
    /// fewer of them. Used where a whole batch would be decided by its
    /// widest quantity (see `quantity_scale`) and this test is about
    /// something else.
    fn one_recorded_row(index: usize) -> &'static [u8] {
        let envelope: serde_json::Value = serde_json::from_slice(CANDLES).expect("recorded JSON");
        let row = envelope["data"][index].clone();
        let body = serde_json::json!({ "code": "200000", "data": [row] }).to_string();
        Box::leak(body.into_bytes().into_boxed_slice())
    }

    #[tokio::test]
    async fn volume_and_turnover_are_read_from_the_final_two_columns() {
        // Row 0 of the recording, whose quantities fit an `i64` at their own
        // scale. The whole-batch case is the test below.
        let (_server, source) = serving_at(one_recorded_row(0), 1_788_400_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(matches!(bars[0].volume, Volume::Real(v) if v > 0));
        assert!(bars[0].quote_volume.is_some_and(|q| q > 0));
    }

    #[tokio::test]
    async fn a_quantity_finer_than_an_i64_can_hold_is_absent_not_rounded() {
        // The recorded response really does contain
        // `89.56968223943530450117` — twenty decimals, 8.9e21 at that
        // scale, against an `i64` ceiling of 9.2e18. There is no smaller
        // scale to fall back to: `parse_scaled` refuses to drop digits
        // rather than rounding silently. So the bars carry exact prices and
        // an honest absence of volume, never a rounded number.
        let (_server, source) = serving_at(CANDLES, 1_788_400_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(!bars.is_empty(), "the prices still come through");
        assert!(bars.iter().all(|bar| bar.close > 0), "prices are exact");
        assert!(
            bars.iter().all(|bar| matches!(bar.volume, Volume::Absent)),
            "a quantity that cannot be represented exactly must be absent"
        );
        assert!(bars.iter().all(|bar| bar.quote_volume.is_none()));
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_type_string() {
        assert!(type_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(type_of(BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_type_string() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                type_of(*spec).is_some(),
                "{spec} is offered but has no type mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
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
