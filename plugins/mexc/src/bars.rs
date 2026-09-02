//! MEXC bar fetching: `GET /api/v3/klines` for spot, `GET
//! /api/v1/contract/kline/{symbol}` on the dedicated `contract.mexc.com`
//! host for futures.
//!
//! # `contract.mexc.com` answers market data fine unauthenticated
//!
//! This crate's own module docs once claimed the dedicated futures host
//! "answers 403 to plain clients" and routed the futures *instrument list*
//! through `api.mexc.com` instead. That 403 is real for the instrument-list
//! endpoint, but it does not extend to market data: `GET
//! /api/v1/contract/kline/{symbol}` on `contract.mexc.com` answered every
//! request below with no credentials at all. The two markets' klines are
//! different enough — one row-oriented with decimal strings, the other
//! column-oriented with bare JSON numbers — that they need their own
//! sources regardless of which host serves them.
//!
//! # Spot: the five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `symbol=BTCUSDT`.
//!
//! 1. **Sort direction**: ascending by open time.
//! 2. **Timestamps**: JSON numbers (milliseconds).
//! 3. **Closed-candle detection**: an explicit `closeTime` field, the
//!    seventh of eight positional values — but it is a timestamp, not a
//!    flag, so it is still compared against [`Clock::now`] rather than
//!    trusted on its own (a row can carry a `closeTime` in the future).
//! 4. **Row cap**: 500.
//! 5. **Pagination**: `startTime`/`endTime`, both milliseconds; an
//!    out-of-reach window (verified separately for futures below, and
//!    assumed to hold here too since both markets share the venue's
//!    infrastructure) answers empty rather than substituting recent data.
//!
//! # Futures: the same five, and a sixth trap unique to this market
//!
//! 1. **Sort direction**: ascending by `time`.
//! 2. **Timestamps**: JSON numbers (**seconds**, not milliseconds — unlike
//!    spot on the very same venue).
//! 3. **Closed-candle detection**: no flag and no server-time field at all;
//!    closed against [`Clock::now`] the same way BitMart and WhiteBIT are.
//! 4. **Row cap**: 1-minute candles answered identically capped at 1992
//!    rows across two separate requests with different `start` values but
//!    the same `end` — a retention ceiling on how far back this endpoint's
//!    finest granularity reaches, not a per-request `limit` parameter (this
//!    endpoint takes none). A window older than that retention answers
//!    empty, not a truncated or substituted one: `start=1700000000&
//!    end=1700003600` (2023) returned zero rows.
//! 5. **Pagination**: `start`/`end`, both epoch seconds.
//! 6. **The plain `open`/`close`/`high`/`low` fields are not real trade
//!    prices.** In the recorded fixture, every row's plain `open` equals
//!    the *previous* row's plain `close` **exactly**, while `realOpen`
//!    differs from that same previous close by a few cents — the plain
//!    series is a continuity-glued display line, and `realOpen`/
//!    `realClose`/`realHigh`/`realLow` are the genuine per-candle trade
//!    extremes. This source reads the `real*` fields.
//!
//! # Numbers, not decimal strings — on both markets' futures response and
//! all of Upbit's, but not MEXC spot
//!
//! Every price and volume field in the futures kline response is a bare
//! JSON number (`77683.8`), not a quoted decimal string the way every other
//! source in this workspace decodes them. A `f64` field would silently
//! violate this project's exact-integer-money rule the moment `serde_json`
//! parsed it, before this module ever saw a value — the loss happens inside
//! the parser, not in application code, so no downstream check can catch
//! it. Each field is instead read as a [`Box<RawValue>`], which captures
//! the JSON number's own source text verbatim; [`RawValue::get`] hands that
//! text to the same [`parse_scaled`] every string-based source already
//! uses, and no `f64` is ever constructed.
//!
//! # `vol` is contracts, not base-asset quantity
//!
//! The futures response's `vol` column and `amount` column are not a
//! base/quote pair the way every other source's volume fields are:
//! `vol ÷ price` does not reconcile with `amount`, but `vol × price ×
//! 0.0001` roughly does, consistent with MEXC's own contract multiplier for
//! `BTC_USDT` being 0.0001 BTC per contract. Converting `vol` into a base
//! quantity would require that multiplier, which lives on the instrument,
//! not in a klines response this source has no access to. Reporting it as
//! [`Volume::Real`] regardless would be silently wrong by exactly that
//! factor, so this source reports [`Volume::Absent`] for futures base
//! volume and keeps only `amount` (genuine quote-currency turnover) as
//! `quote_volume`.

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
use serde_json::value::RawValue;

use crate::{FUTURES_ID, SPOT_ID};

const SPOT_KLINES_URL: &str = "https://api.mexc.com/api/v3/klines";
const FUTURES_KLINES_URL: &str = "https://contract.mexc.com/api/v1/contract/kline";

/// The tested cap: MEXC spot's klines answer at most 500 rows per call.
const SPOT_MAX_ROWS: usize = 500;

/// The tested cap for the futures endpoint's finest granularity — see the
/// module docs for why this is a retention ceiling, not a request limit.
const FUTURES_MAX_ROWS: usize = 1992;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, matching
/// every other bar source in this workspace.
const KLINES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v3/klines`: a bare array of eight values, mixing
/// numbers and decimal strings. `closeTime` is field 6.
type RawSpotKline = (i64, String, String, String, String, String, i64, String);

/// Every `(spec, interval)` pair MEXC spot has verified, and the only ones
/// this source will ever ask for.
const SPOT_INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (30, BarUnit::Minute, "30m"),
    // MEXC spot has no `1h`; a one-hour candle is asked for as 60 minutes.
    (1, BarUnit::Hour, "60m"),
    (4, BarUnit::Hour, "4h"),
    (1, BarUnit::Day, "1d"),
];

/// Every `(spec, interval)` pair MEXC futures has verified. `Hour1` is
/// deliberately absent: it was tried first, by analogy with spot's `60m`,
/// and refused outright (`{"success":false,"code":600,"message":"Parameter
/// error"}`) — `Min60` is this venue's actual spelling for one hour.
const FUTURES_INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "Min1"),
    (5, BarUnit::Minute, "Min5"),
    (15, BarUnit::Minute, "Min15"),
    (30, BarUnit::Minute, "Min30"),
    (1, BarUnit::Hour, "Min60"),
    (4, BarUnit::Hour, "Hour4"),
    (1, BarUnit::Day, "Day1"),
];

fn specs_of(table: &[(u32, BarUnit, &str)]) -> Vec<BarSpec> {
    table
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

fn interval_of(table: &[(u32, BarUnit, &'static str)], spec: BarSpec) -> Option<&'static str> {
    table
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

/// The still-forming check every source without a boolean flag shares: a
/// candle is closed only once its own open time plus the spec's fixed
/// duration has passed.
fn still_forming(ts_open: UnixNanos, duration: Duration, now: UnixNanos) -> bool {
    match ts_open.checked_add(duration) {
        Some(close_time) => close_time > now,
        None => true,
    }
}

// ---------------------------------------------------------------------
// Spot
// ---------------------------------------------------------------------

/// MEXC spot bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`]: `closeTime` is a timestamp to compare, not a flag to trust —
/// see the module docs.
#[derive(Clone)]
pub struct MexcSpotBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for MexcSpotBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MexcSpotBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl MexcSpotBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn klines_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        format!(
            "{}?symbol={symbol}&interval={interval}&limit={SPOT_MAX_ROWS}&startTime={}&endTime={}",
            self.url,
            range.start().as_millis(),
            range.end().as_millis().saturating_sub(1),
        )
    }
}

/// MEXC spot bars.
#[must_use]
pub fn spot_bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> MexcSpotBarSource {
    MexcSpotBarSource {
        url: SPOT_KLINES_URL.to_owned(),
        client,
        clock,
        supported: specs_of(SPOT_INTERVALS),
    }
}

#[async_trait]
impl BarSource for MexcSpotBarSource {
    fn source_id(&self) -> &str {
        SPOT_ID
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        SPOT_MAX_ROWS
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
        let interval = interval_of(SPOT_INTERVALS, spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;

        let url = self.klines_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, KLINES_FETCH_COST).await?;
        let rows: Vec<RawSpotKline> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().flat_map(|row| [row.5.as_str(), row.7.as_str()]));

        let now_ms = self.clock.now().as_millis();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (open_ms, open, high, low, close, volume, close_ms, quote_volume) in rows {
            // `closeTime` is a timestamp, not a flag: it must still be
            // compared against "now" rather than trusted on its own.
            if close_ms >= now_ms {
                continue;
            }
            let ts_open = UnixNanos::from_millis(open_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {open_ms} overflowed")))?;
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

// ---------------------------------------------------------------------
// Futures
// ---------------------------------------------------------------------

/// One column of `GET /api/v1/contract/kline/{symbol}`: the fields this
/// source actually reads. `open`/`close`/`high`/`low`/`vol` are declared
/// nowhere here and simply ignored by `serde` — see the module docs for why
/// the `real*` fields are read instead.
#[derive(Debug, Deserialize)]
struct FuturesColumns {
    time: Vec<i64>,
    #[serde(rename = "realOpen")]
    real_open: Vec<Box<RawValue>>,
    #[serde(rename = "realHigh")]
    real_high: Vec<Box<RawValue>>,
    #[serde(rename = "realLow")]
    real_low: Vec<Box<RawValue>>,
    #[serde(rename = "realClose")]
    real_close: Vec<Box<RawValue>>,
    amount: Vec<Box<RawValue>>,
}

/// The envelope every `contract.mexc.com` endpoint answers with. `data` is
/// absent, not present-and-empty, when `success` is `false`.
#[derive(Debug, Deserialize)]
struct FuturesEnvelope {
    success: bool,
    code: i64,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    data: Option<FuturesColumns>,
}

/// MEXC futures bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] — this endpoint reports no confirmation flag at all.
#[derive(Clone)]
pub struct MexcFuturesBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for MexcFuturesBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MexcFuturesBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl MexcFuturesBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    /// The symbol is appended as a path segment, so `url` is the endpoint
    /// *without* the trailing `/{symbol}`.
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
            "{}/{symbol}?interval={interval}&start={start}&end={end}",
            self.url
        )
    }
}

/// MEXC futures bars.
#[must_use]
pub fn futures_bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> MexcFuturesBarSource {
    MexcFuturesBarSource {
        url: FUTURES_KLINES_URL.to_owned(),
        client,
        clock,
        supported: specs_of(FUTURES_INTERVALS),
    }
}

#[async_trait]
impl BarSource for MexcFuturesBarSource {
    fn source_id(&self) -> &str {
        FUTURES_ID
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        FUTURES_MAX_ROWS
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
        let interval = interval_of(FUTURES_INTERVALS, spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let duration_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;
        let duration = Duration::from_nanos(u64::try_from(duration_nanos).unwrap_or(u64::MAX));

        let url = self.kline_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, KLINES_FETCH_COST).await?;
        let envelope: FuturesEnvelope =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.success {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code,
                envelope.message.as_deref().unwrap_or("no message")
            )));
        }
        let Some(columns) = envelope.data else {
            return Ok(Vec::new());
        };

        // The five columns are independent JSON arrays with no length
        // enforced between them by the venue's own shape; indexing any of
        // them without checking first could panic on a malformed response.
        let len = columns.time.len();
        if [
            columns.real_open.len(),
            columns.real_high.len(),
            columns.real_low.len(),
            columns.real_close.len(),
            columns.amount.len(),
        ]
        .iter()
        .any(|&column_len| column_len != len)
        {
            return Err(SourceError::decode(
                "futures kline columns have mismatched lengths",
            ));
        }

        let price_scale = common_scale(
            columns
                .real_open
                .iter()
                .chain(&columns.real_high)
                .chain(&columns.real_low)
                .chain(&columns.real_close)
                .map(|v| v.get()),
        );
        let qty_scale = common_scale(columns.amount.iter().map(|v| v.get()));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(len);
        let mut outside = 0usize;
        for i in 0..len {
            let ts_open = UnixNanos::from_secs(columns.time[i]).ok_or_else(|| {
                SourceError::decode(format!("open time {}s overflowed", columns.time[i]))
            })?;
            if still_forming(ts_open, duration, now) {
                continue;
            }
            if !range.contains(ts_open) {
                outside += 1;
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: scaled(columns.real_open[i].get(), price_scale)?,
                high: scaled(columns.real_high[i].get(), price_scale)?,
                low: scaled(columns.real_low[i].get(), price_scale)?,
                close: scaled(columns.real_close[i].get(), price_scale)?,
                // `vol` is contracts, not base-asset quantity — see the
                // module docs for why this cannot be `Volume::Real`.
                volume: Volume::Absent,
                quote_volume: Some(scaled(columns.amount[i].get(), qty_scale)?),
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
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{
        FUTURES_INTERVALS, SPOT_INTERVALS, futures_bar_source, interval_of, spot_bar_source,
    };

    const SPOT_KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1m.json");
    const FUTURES_KLINES: &[u8] = include_bytes!("../tests/fixtures/futures_kline_1m.json");

    #[derive(Debug)]
    struct FixedClock(i64);

    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_nanos(self.0)
        }

        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    fn symbol(raw: &str) -> SourceSymbol {
        SourceSymbol::assume(raw)
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_000_000).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap()
    }

    // -- spot --

    async fn mock_spot_source(now_ms: i64) -> (MockServer, super::MexcSpotBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(SPOT_KLINES, "application/json"))
            .mount(&server)
            .await;
        let source = spot_bar_source(test_client(), Arc::new(FixedClock(now_ms * 1_000_000)))
            .with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn spot_fixture_rows_decode_with_correct_ohlcv_and_ascending_order() {
        // The fixture's five rows run 1788328800000..1788329040000 ms apart
        // by a minute; a clock well past the last one's closeTime keeps all
        // five.
        let (_server, source) = mock_spot_source(4_102_444_800_000).await;
        let bars = source
            .bars(
                &symbol("BTCUSDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap();

        assert_eq!(bars.len(), 5);
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_328_800_000).unwrap()
        );
        // Row 0: open 77645.6, high 77668.51, low 77637.67, close 77668.51.
        assert_eq!(first.open, 7_764_560);
        assert_eq!(first.high, 7_766_851);
        assert_eq!(first.low, 7_763_767);
        assert_eq!(first.close, 7_766_851);
        assert!(matches!(first.volume, Volume::Real(v) if v > 0));
    }

    #[tokio::test]
    async fn spot_close_time_is_compared_to_the_clock_not_trusted_as_a_flag() {
        // The fixture's last row has `closeTime` 1788329100000. A clock set
        // to exactly that instant has not yet passed it.
        let (_server, source) = mock_spot_source(1_788_329_100_000).await;
        let bars = source
            .bars(
                &symbol("BTCUSDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap();
        assert_eq!(
            bars.len(),
            4,
            "the fifth row's closeTime has not passed yet"
        );
    }

    #[tokio::test]
    async fn spot_an_unsupported_spec_is_rejected() {
        let (_server, source) = mock_spot_source(4_102_444_800_000).await;
        let error = source
            .bars(
                &symbol("BTCUSDT"),
                BarSpec::new(1, BarUnit::Month),
                wide_range(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }

    #[tokio::test]
    async fn spot_an_inverted_range_asks_the_venue_nothing() {
        let server = MockServer::start().await;
        let source = spot_bar_source(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        let point = UnixNanos::from_secs(1_788_329_100).unwrap();
        if let Some(range) = TimeRange::new(point, point) {
            assert!(
                source
                    .bars(&symbol("BTCUSDT"), BarSpec::new(1, BarUnit::Minute), range)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn spot_an_hour_is_asked_for_as_sixty_minutes() {
        assert_eq!(
            interval_of(SPOT_INTERVALS, BarSpec::new(1, BarUnit::Hour)).unwrap(),
            "60m"
        );
    }

    #[test]
    fn spot_a_spec_this_venue_does_not_serve_has_no_interval() {
        assert!(interval_of(SPOT_INTERVALS, BarSpec::new(1, BarUnit::Week)).is_none());
        assert!(interval_of(SPOT_INTERVALS, BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn spot_every_supported_spec_maps_to_an_interval() {
        let source = spot_bar_source(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(interval_of(SPOT_INTERVALS, *spec).is_some());
        }
    }

    // -- futures --

    async fn mock_futures_source(now_secs: i64) -> (MockServer, super::MexcFuturesBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(FUTURES_KLINES, "application/json"),
            )
            .mount(&server)
            .await;
        let source = futures_bar_source(
            test_client(),
            Arc::new(FixedClock(now_secs * 1_000_000_000)),
        )
        .with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn futures_fixture_rows_use_the_real_fields_not_the_continuity_glued_ones() {
        // The fixture's first row: plain open 77683.8, but realOpen 77683.7
        // — the previous-close-glued series and the genuine trade price
        // disagree by a cent, and only the real one should come out.
        let (_server, source) = mock_futures_source(4_102_444_800).await;
        let bars = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap();

        assert_eq!(bars.len(), 11);
        let first = bars[0];
        assert_eq!(
            first.open, 776_837,
            "realOpen 77683.7, not plain open 77683.8"
        );
        assert_eq!(first.close, 776_309);
        assert!(
            matches!(first.volume, Volume::Absent),
            "vol is contracts, not base qty"
        );
        assert!(first.quote_volume.is_some_and(|q| q > 0));
    }

    #[tokio::test]
    async fn futures_a_still_forming_candle_is_never_returned() {
        // The fixture's last row opens at 1788329100; its own one-minute
        // close (1788329160) has not passed at that same clock instant.
        let (_server, source) = mock_futures_source(1_788_329_100).await;
        let bars = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap();
        assert_eq!(bars.len(), 10, "the eleventh row has not closed yet");
    }

    #[tokio::test]
    async fn futures_bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_futures_source(4_102_444_800).await;
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_328_620).unwrap(),
            UnixNanos::from_secs(1_788_328_680).unwrap(),
        )
        .unwrap();
        let bars = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                narrow,
            )
            .await
            .unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_328_620).unwrap()
        );
    }

    #[tokio::test]
    async fn futures_an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"success":true,"code":0,"data":{"time":[],"open":[],"close":[],"high":[],"low":[],"vol":[],"amount":[],"realOpen":[],"realClose":[],"realHigh":[],"realLow":[]}}"#
                    .as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source = futures_bar_source(
            test_client(),
            Arc::new(FixedClock(4_102_444_800_000_000_000)),
        )
        .with_url(server.uri());
        let bars = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap();
        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn futures_an_unsuccessful_envelope_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"success":false,"code":600,"message":"Parameter error"}"#.as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source = futures_bar_source(
            test_client(),
            Arc::new(FixedClock(4_102_444_800_000_000_000)),
        )
        .with_url(server.uri());
        let error = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Rejected { .. }));
    }

    #[tokio::test]
    async fn futures_mismatched_column_lengths_are_a_decode_error_not_a_panic() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                br#"{"success":true,"code":0,"data":{"time":[1,2],"realOpen":["1.0"],"realHigh":["1.0"],"realLow":["1.0"],"realClose":["1.0"],"amount":["1.0"]}}"#
                    .as_slice(),
                "application/json",
            ))
            .mount(&server)
            .await;
        let source = futures_bar_source(
            test_client(),
            Arc::new(FixedClock(4_102_444_800_000_000_000)),
        )
        .with_url(server.uri());
        let error = source
            .bars(
                &symbol("BTC_USDT"),
                BarSpec::new(1, BarUnit::Minute),
                wide_range(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SourceError::Decode { .. }));
    }

    #[test]
    fn futures_an_hour_is_asked_for_as_min60_not_hour1() {
        assert_eq!(
            interval_of(FUTURES_INTERVALS, BarSpec::new(1, BarUnit::Hour)).unwrap(),
            "Min60"
        );
    }

    #[test]
    fn futures_every_supported_spec_maps_to_an_interval() {
        let source = futures_bar_source(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(interval_of(FUTURES_INTERVALS, *spec).is_some());
        }
    }
}
