//! BingX spot bar fetching — `GET /openApi/spot/v2/market/kline`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `symbol=BTC-USDT`, not read from documentation.
//!
//! 1. **Sort direction**: descending by open time — newest row first,
//!    the opposite of every other source in this workspace. Sorted
//!    ascending before returning, never trusted as-is.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a JSON
//!    number, for both `ts` (open time) and `closeTime`.
//! 3. **Closed-candle detection**: no per-row flag, but the envelope
//!    itself carries a `timestamp` field — the venue's own notion of
//!    "now" for this response, the same shape [`BarSource::bars`]'s own
//!    docs describe for Bybit. A row is closed when its `closeTime` is
//!    strictly before that envelope timestamp. Using the venue's own
//!    clock rather than the caller's wall clock means a slow round trip
//!    can never make an unclosed candle look closed.
//! 4. **Row cap (tested)**: 1000 rows for `limit=1000`. This module does
//!    not itself reproduce the wide-window failure — the live audit this
//!    task was scoped against recorded it as **silent**: a request wider
//!    than the venue's real coverage answers HTTP 200 with whatever
//!    window BingX has, not an error, so [`BarSource::bars`] never trusts
//!    an answer that misses the requested range entirely — see the guard
//!    at the end of this module's `bars()` implementation.
//! 5. **Pagination direction**: `startTime`/`endTime`, both inclusive
//!    epoch milliseconds, confirmed live: a five-hour window returned
//!    exactly the five candles inside it.
//!
//! # The field order
//!
//! A row is eight positional values, numbers and decimal strings mixed:
//!
//! ```text
//! [ ts, open, high, low, close, volume, closeTime, quoteVolume ]
//! ```
//!
//! Already OHLC order, unlike Gate's `candlesticks` — but the envelope
//! wraps the array in `{code, timestamp, data}`, and `code` is checked
//! before the data is trusted, the same as every other BingX endpoint in
//! this plugin.
//!
//! # What was verified, and what is a documented assumption
//!
//! - `1h` was requested and the spacing between rows measured: exactly
//!   3 600 000 ms apart, and `closeTime - ts == 3 599 999` on every row.
//! - The remaining specs in [`INTERVALS`] follow BingX's own documented
//!   interval syntax for this endpoint (the same `1m`/`4h`/`1d`-style
//!   strings used by every other Binance-derived venue in this
//!   workspace) but were **not** individually requested and measured —
//!   an explicit, commented assumption per this project's rule against
//!   inventing venue facts, kept deliberately small rather than offering
//!   the full documented range unverified.
//! - The `limit=1000` row cap is the number this task's own live audit
//!   recorded (`limit=1001` etc. was not re-tried here to avoid a second
//!   live request for a fact already established).

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::SPOT_ID;

const SPOT_KLINE_URL: &str = "https://open-api.bingx.com/openApi/spot/v2/market/kline";

/// The tested cap: this task's live audit recorded 1000 rows for
/// `limit=1000` on this endpoint, with the wide-window case answering
/// HTTP 200 and a *different* window rather than an error — see the
/// module docs.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source in this
/// workspace.
const KLINE_FETCH_COST: u32 = 5;

/// One row of `GET /openApi/spot/v2/market/kline`: `[ts, open, high,
/// low, close, volume, closeTime, quoteVolume]`, already OHLC order.
///
/// Prices and sizes arrive as **bare JSON numbers**, not strings — the
/// recorded fixture shows `77651.07`, not `"77651.07"`. They are read as
/// [`RawValue`] so the exact digits the venue wrote reach
/// [`parse_scaled`] untouched: deserialising them as `f64` and formatting
/// back would be a float on the money path, which this project does not do
/// (`AGENTS.md`). `RawValue` borrows from the response body, so this type
/// carries its lifetime.
type RawKline<'a> = (
    i64,
    &'a RawValue,
    &'a RawValue,
    &'a RawValue,
    &'a RawValue,
    &'a RawValue,
    i64,
    &'a RawValue,
);

/// The envelope every BingX endpoint wraps its payload in. `timestamp` is
/// the venue's own server time for this response — used here as "now"
/// for closed-candle detection instead of the caller's wall clock.
#[derive(Debug, Deserialize)]
struct Envelope<'a> {
    code: i64,
    #[serde(default)]
    msg: String,
    timestamp: i64,
    #[serde(borrow)]
    data: Vec<RawKline<'a>>,
}

/// Every `(step, unit, interval)` this source will ask BingX for. See the
/// module docs for which entry was measured against a live response and
/// which follows documented syntax only.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (30, BarUnit::Minute, "30m"),
    // Measured live: 3,600,000 ms apart.
    (1, BarUnit::Hour, "1h"),
    (4, BarUnit::Hour, "4h"),
    (1, BarUnit::Day, "1d"),
    (1, BarUnit::Week, "1w"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`].
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// BingX's interval string for `spec`, or `None` when `spec` is not one
/// this source has mapped.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, interval)| interval)
}

/// BingX spot bars, fetched through a [`VenueClient`]. Closure comes from
/// the envelope's own `timestamp` field compared against each row's
/// `closeTime` — see the module docs on why that needs no injected
/// [`senken_series::Clock`].
#[derive(Debug, Clone)]
pub struct BingxBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl BingxBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn kline_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        format!(
            "{}?symbol={symbol}&interval={interval}&limit={MAX_ROWS}&startTime={}&endTime={}",
            self.url,
            range.start().as_millis(),
            // `endTime` is inclusive on this venue; `range.end()` is
            // exclusive (`TimeRange`'s own half-open contract), so the
            // last representable millisecond strictly before it is sent.
            range.end().as_millis() - 1,
        )
    }
}

/// The BingX spot bar source.
#[must_use]
pub fn bar_source_spot(client: VenueClient) -> BingxBarSource {
    BingxBarSource {
        url: SPOT_KLINE_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BingxBarSource {
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
        let url = self.kline_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, KLINE_FETCH_COST).await?;
        let envelope: Envelope<'_> = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if envelope.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        let price_scale = common_scale(
            envelope
                .data
                .iter()
                .flat_map(|row| [row.1.get(), row.2.get(), row.3.get(), row.4.get()]),
        );
        let qty_scale = common_scale(
            envelope
                .data
                .iter()
                .flat_map(|row| [row.5.get(), row.7.get()]),
        );

        let now_ms = envelope.timestamp;
        let mut bars = Vec::with_capacity(envelope.data.len());
        let mut outside = 0usize;
        for &(open_ms, open, high, low, close, volume, close_ms, quote_volume) in &envelope.data {
            // No per-row flag; the venue's own response timestamp is the
            // only trustworthy "now" — see the module docs.
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
                open: scaled(open.get(), price_scale)?,
                high: scaled(high.get(), price_scale)?,
                low: scaled(low.get(), price_scale)?,
                close: scaled(close.get(), price_scale)?,
                volume: Volume::Real(scaled(volume.get(), qty_scale)?),
                quote_volume: Some(scaled(quote_volume.get(), qty_scale)?),
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard for why an answer made entirely of
        // rows outside the requested range is reported, not swallowed:
        // this is the exact silent wide-window failure this venue was
        // grouped for.
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

/// Parses `raw` — the venue's own digits, verbatim from the response body —
/// at `scale`, mapping an unparseable value to a decode error rather than
/// panicking or guessing. Should never fire: `scale` was computed from this
/// exact batch.
fn scaled(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source_spot, interval_of};

    /// A real `GET /openApi/spot/v2/market/kline?symbol=BTC-USDT&interval=1h`
    /// response, recorded 2026-09-02. Ten rows, descending by open time;
    /// the first (newest) row's `closeTime` is still ahead of the
    /// envelope's own `timestamp` — captured mid-hour, so it is exactly
    /// the still-forming candle this source must never return.
    const KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1h.json");

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
            UnixNanos::from_millis(1_788_290_000_000).unwrap(),
            UnixNanos::from_millis(1_788_340_000_000).unwrap(),
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

    #[tokio::test]
    async fn the_still_forming_candle_the_envelope_timestamp_reveals_is_never_returned() {
        // The fixture's newest row has closeTime 1,788,332,399,999 but the
        // envelope's own `timestamp` is 1,788,329,099,653 — still mid-hour.
        // A source trusting the row count alone would return it.
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars.len(),
            9,
            "the fixture holds 10 rows, one still forming"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_328_800_000),
            "the still-forming row's open time must not appear"
        );
    }

    #[tokio::test]
    async fn rows_decode_in_ohlc_order_not_gate_s_reordered_one() {
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // Second-newest fixture row: 1788325200000,77561.81,77757.29,
        // 77437.96,77651.07 — open, high, low, close, in that order.
        let newest = bars.last().unwrap();
        assert_eq!(
            newest.ts_open,
            UnixNanos::from_millis(1_788_325_200_000).unwrap()
        );
        assert_eq!(newest.open, 7_756_181);
        assert_eq!(newest.high, 7_775_729);
        assert_eq!(newest.low, 7_743_796);
        assert_eq!(newest.close, 7_765_107);
    }

    #[tokio::test]
    async fn rows_are_returned_ascending_even_though_the_venue_sends_descending() {
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());
        // Only the fixture's third-newest closed row falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_320_000_000).unwrap(),
            UnixNanos::from_millis(1_788_322_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_321_600_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        // This task's live audit found BingX silently answers a
        // wide-window request with whatever coverage it has, HTTP 200, no
        // error. Dropping rows entirely outside the asked-for range
        // silently would turn that into an empty result indistinguishable
        // from "no data here" — and a permanently cached gap.
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());
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
        let body = br#"{"code":0,"timestamp":1788329099653,"data":[]}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_nonzero_code_is_a_rejection_even_with_http_200() {
        let body = br#"{"code":100202,"msg":"Insufficient balance","timestamp":0,"data":[]}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("100202"));
    }

    #[tokio::test]
    async fn volume_comes_from_field_five_not_quote_volume() {
        let server = serving(KLINES).await;
        let source = bar_source_spot(test_client()).with_url(server.uri());

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(matches!(bars[0].volume, Volume::Real(v) if v > 0));
        assert!(bars[0].quote_volume.is_some_and(|q| q > 0));
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_interval_string() {
        assert!(interval_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(interval_of(BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_spot(test_client());
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
        let source = bar_source_spot(test_client()).with_url(server.uri());
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_325_200_000).unwrap(),
            UnixNanos::from_millis(1_788_325_200_000).unwrap(),
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
