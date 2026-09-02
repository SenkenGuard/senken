//! Upbit bar fetching — `GET /v1/candles/minutes/{unit}`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against `market=KRW-BTC`.
//!
//! 1. **Sort direction**: **descending** by open time — the opposite of
//!    every other source in this workspace. Sorted ascending here before
//!    returning, same as every other source, but this is the one venue
//!    where that sort is not merely defensive.
//! 2. **Timestamp representation**: no single numeric field for the
//!    candle's own open time at all. `candle_date_time_utc` is an ISO-style
//!    string with no `Z` suffix (`"2026-09-02T06:00:00"`), parsed here by
//!    hand with [`senken_core::days_from_civil`] rather than pulling in a
//!    date-time crate this plugin does not otherwise need.
//! 3. **Closed-candle detection**: no flag. The response's own `timestamp`
//!    field is the last **trade** time inside the bucket, not the bucket's
//!    end — it is earlier than the bucket boundary whether the candle is
//!    closed or not, so it cannot distinguish the two. Closure is decided
//!    by comparing the parsed open time plus the spec's own duration
//!    against [`Clock::now`], exactly as `senken_plugin::clock` documents.
//! 4. **Row cap (tested)**: 200 (`count`).
//! 5. **Pagination direction**: backward from a `to` cursor — Upbit has no
//!    `from`/start parameter at all. `to=2026-08-01T00:00:00Z&count=3`
//!    answered the three candles strictly before that instant, confirming
//!    `to` is an **exclusive** upper bound, matching [`TimeRange`]'s own
//!    half-open convention exactly.
//!
//! # Numbers, not decimal strings
//!
//! Every price and volume field (`opening_price`, `trade_price`,
//! `candle_acc_trade_volume`, …) is a bare JSON number, not a quoted
//! decimal string the way BitMart, WhiteBIT and MEXC spot report them. A
//! `f64` field would silently violate this project's exact-integer-money
//! rule inside `serde_json`'s own parser, before this module ever saw a
//! value. Each field is instead read as a [`Box<RawValue>`], which captures
//! the JSON number's own source text verbatim, and that text is handed to
//! [`parse_scaled`] exactly as every string-based source's decimal already
//! is.
//!
//! # `market` is already venue-native
//!
//! Upbit's own market identifier (`KRW-BTC`, quote first) is what this
//! crate's instrument catalog stores verbatim as
//! [`Instrument::source_symbol`](senken_marketdata::Instrument::source_symbol) —
//! see this crate's own module docs on why the pair is written backwards —
//! so it is passed straight through to the `market` query parameter with no
//! reversal needed at this layer.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, days_from_civil, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

const CANDLES_URL: &str = "https://api.upbit.com/v1/candles/minutes";

/// The tested cap: Upbit's minute candles answer at most 200 rows per call.
const MAX_ROWS: usize = 200;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, matching
/// every other bar source in this workspace.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /v1/candles/minutes/{unit}`: the fields this source
/// actually reads. `timestamp` and `unit` are ignored — see the module
/// docs for why `timestamp` cannot be used to detect closure.
#[derive(Debug, Deserialize)]
struct RawCandle {
    candle_date_time_utc: String,
    opening_price: Box<RawValue>,
    high_price: Box<RawValue>,
    low_price: Box<RawValue>,
    trade_price: Box<RawValue>,
    candle_acc_trade_price: Box<RawValue>,
    candle_acc_trade_volume: Box<RawValue>,
}

/// Every `(spec, unit)` pair this source has verified, and the only ones it
/// will ever ask Upbit for. `unit` is the URL path segment — minutes, as an
/// integer.
///
/// Only the endpoint's day/week/month siblings are out of scope: they live
/// at entirely different paths (`/v1/candles/days`, `/weeks`, `/months`)
/// with a different response shape, not merely a different `unit`, so they
/// would need their own source rather than a wider table here.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1"),
    (5, BarUnit::Minute, "5"),
    (30, BarUnit::Minute, "30"),
    (1, BarUnit::Hour, "60"),
    (4, BarUnit::Hour, "240"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Upbit's `unit` path segment for `spec`, or `None` when `spec` is not one
/// this source has verified.
fn unit_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, path_unit)| path_unit)
}

/// Parses Upbit's `candle_date_time_utc`, e.g. `"2026-09-02T06:00:00"` —
/// UTC, but with no `Z` and no offset to say so — using this project's own
/// calendar arithmetic rather than a date-time crate this plugin would
/// otherwise never need.
fn parse_utc_datetime(s: &str) -> Option<UnixNanos> {
    let (date, time) = s.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)?
        .checked_add(hour.checked_mul(3600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)?;
    UnixNanos::from_secs(secs)
}

/// Parses `raw` at `scale`, mapping an unparseable value to a decode error
/// rather than panicking or guessing.
fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// Upbit bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] — Upbit sends no confirmation flag, so "now" must come from
/// somewhere (see `senken_plugin::clock`).
#[derive(Clone)]
pub struct UpbitBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for UpbitBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpbitBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl UpbitBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    /// The `unit` is appended as a path segment, so `url` is the endpoint
    /// *without* the trailing `/{unit}`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call: `to` is `range.end()`
    /// truncated to whole seconds (Upbit's cursor has no finer
    /// resolution), and `count` asks for exactly as many candles as
    /// `range`'s span needs at `spec`'s width, capped at [`MAX_ROWS`] —
    /// this endpoint pages backward from `to` and has no start parameter,
    /// so a caller wanting more history makes another call with an
    /// earlier `to`.
    fn candles_url(
        &self,
        symbol: &str,
        unit: &str,
        spec: BarSpec,
        range: TimeRange,
    ) -> Option<String> {
        let to_secs = range.end().as_nanos().div_euclid(1_000_000_000);
        let to = UnixNanos::from_secs(to_secs)?;

        let span_nanos = range.end().as_nanos() - range.start().as_nanos();
        let duration_nanos = spec.duration_nanos()?;
        // Ceiling division written out: `i64::div_ceil` is still unstable,
        // and a floor would ask for one bar too few whenever the range does
        // not land exactly on a bucket boundary — the last bar of a chart's
        // window is the one a reader is looking at.
        let bars_needed = (span_nanos + duration_nanos - 1) / duration_nanos;
        let count = bars_needed.clamp(1, i64::try_from(MAX_ROWS).unwrap_or(i64::MAX));

        Some(format!(
            "{}/{unit}?market={symbol}&count={count}&to={to}",
            self.url,
        ))
    }
}

/// The Upbit market.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> UpbitBarSource {
    UpbitBarSource {
        url: CANDLES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for UpbitBarSource {
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
        let unit = unit_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let duration_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;
        let duration = Duration::from_nanos(u64::try_from(duration_nanos).unwrap_or(u64::MAX));
        let url = self
            .candles_url(symbol.as_str(), unit, spec, range)
            .ok_or_else(|| SourceError::rejected(format!("{spec} could not be requested")))?;

        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.opening_price.get(),
                row.high_price.get(),
                row.low_price.get(),
                row.trade_price.get(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.candle_acc_trade_volume.get(),
                row.candle_acc_trade_price.get(),
            ]
        }));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for row in rows {
            let ts_open = parse_utc_datetime(&row.candle_date_time_utc).ok_or_else(|| {
                SourceError::decode(format!(
                    "{:?} is not a valid candle_date_time_utc",
                    row.candle_date_time_utc
                ))
            })?;

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
                open: scaled(row.opening_price.get(), price_scale)?,
                high: scaled(row.high_price.get(), price_scale)?,
                low: scaled(row.low_price.get(), price_scale)?,
                close: scaled(row.trade_price.get(), price_scale)?,
                volume: Volume::Real(scaled(row.candle_acc_trade_volume.get(), qty_scale)?),
                quote_volume: Some(scaled(row.candle_acc_trade_price.get(), qty_scale)?),
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

        // Descending is Upbit's own order; ascending is this trait's
        // contract regardless of what the venue returns.
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

    use super::{bar_source, parse_utc_datetime, unit_of};

    /// A real `GET /v1/candles/minutes/60?market=KRW-BTC&count=5` response,
    /// recorded 2026-09-02: five hourly candles, newest first, the newest
    /// of which was still forming at capture time.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_60m.json");

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
        SourceSymbol::assume("KRW-BTC")
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

    async fn mock_source(now_secs: i64) -> (MockServer, super::UpbitBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(now_secs))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_ascending_despite_the_venue_s_own_descending_order() {
        // The fixture's five rows run 02:00..=06:00 UTC, newest first. A
        // clock well past every candle's own close keeps all five, in
        // ascending order.
        let (_server, source) = mock_source(4_102_444_800).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 5);
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        let first = bars[0];
        assert_eq!(first.ts_open, UnixNanos::from_secs(1_788_314_400).unwrap());
        // Row (02:00 UTC): opening_price 106086000, high 106450000,
        // low 106010000, trade_price 106411000 — all whole-number KRW.
        assert_eq!(first.open, 106_086_000);
        assert_eq!(first.high, 106_450_000);
        assert_eq!(first.low, 106_010_000);
        assert_eq!(first.close, 106_411_000);
    }

    #[tokio::test]
    async fn a_candle_still_within_its_own_duration_of_now_is_never_returned() {
        // A clock set to the newest row's own open time (06:00 UTC): that
        // candle's own close (07:00 UTC) has not happened yet.
        let (_server, source) = mock_source(1_788_328_800).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert_eq!(bars.len(), 4, "the newest row has not closed at this clock");
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800).await;
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_318_000).unwrap(),
            UnixNanos::from_secs(1_788_318_001).unwrap(),
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
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(b"[]".as_slice(), "application/json"),
            )
            .mount(&server)
            .await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(4_102_444_800))).with_url(server.uri());
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source =
            bar_source(test_client(), Arc::new(FixedClock(4_102_444_800))).with_url(server.uri());
        let point = UnixNanos::from_secs(1_788_328_800).unwrap();
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
    fn a_four_hour_unit_is_asked_for_as_240_minutes() {
        assert_eq!(unit_of(BarSpec::new(4, BarUnit::Hour)).unwrap(), "240");
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_unit() {
        assert!(unit_of(BarSpec::new(1, BarUnit::Day)).is_none());
        assert!(unit_of(BarSpec::new(15, BarUnit::Minute)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_unit() {
        let source = bar_source(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(unit_of(*spec).is_some());
        }
    }

    #[test]
    fn the_hand_rolled_datetime_parser_matches_the_recorded_fixture() {
        assert_eq!(
            parse_utc_datetime("2026-09-02T06:00:00"),
            Some(UnixNanos::from_secs(1_788_328_800).unwrap())
        );
        assert_eq!(
            parse_utc_datetime("2026-09-02T02:00:00"),
            Some(UnixNanos::from_secs(1_788_314_400).unwrap())
        );
    }

    #[test]
    fn the_datetime_parser_rejects_garbage_rather_than_guessing() {
        assert_eq!(parse_utc_datetime("not a date"), None);
        assert_eq!(parse_utc_datetime("2026-09-02"), None);
    }
}
