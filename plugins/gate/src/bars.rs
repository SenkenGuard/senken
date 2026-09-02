//! Gate spot bar fetching — `GET /api/v4/spot/candlesticks`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `currency_pair=BTC_USDT`, not read from documentation.
//!
//! 1. **Sort direction**: ascending by open time. Sorted again here anyway,
//!    because a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a *string* — not
//!    milliseconds, which is what the neighbouring venues in this
//!    workspace use. [`UnixNanos::from_secs`] names the unit so the
//!    difference cannot be lost at a call site.
//! 3. **Closed-candle detection**: an explicit `window_closed` flag, the
//!    eighth field, the string `"true"` or `"false"`. No wall clock is
//!    needed, and no [`senken_series::Clock`] is taken here as a result.
//! 4. **Row cap (tested)**: 1000. `limit=1001` is refused with HTTP 400 and
//!    `Invalid request parameter `limit` value: 1001`; `limit=5000`
//!    likewise. Nothing is silently truncated.
//! 5. **Pagination direction**: `from`/`to`, both epoch seconds, bounding
//!    the window server-side. A range spanning more than 1000 candles is
//!    refused outright — `Candlestick range too broad. Maximum 1000 data
//!    points are allowed per request` — rather than quietly returning a
//!    different window.
//!
//! # The field order is not OHLC
//!
//! A row is eight positional strings:
//!
//! ```text
//! [ ts, quote_volume, close, high, low, open, base_volume, window_closed ]
//!    0       1          2      3    4     5        6             7
//! ```
//!
//! **Close comes before high, low and open.** This is the single easiest
//! thing to get wrong here, and getting it wrong is invisible: open and
//! close would simply be swapped, every candle would still be internally
//! consistent, and the chart would look entirely plausible until someone
//! compared it against another venue. The order was confirmed from the
//! data rather than assumed — in the recorded fixture each row's field 5
//! equals the previous row's field 2 to within a tick, which only holds if
//! 5 is the open and 2 is the close.
//!
//! # What was verified, and what is a documented assumption
//!
//! - The twelve intervals in [`supported_specs`] were each requested and
//!   the spacing between returned candles measured, so every one of them
//!   is known to be honoured rather than silently substituted.
//! - `30d` is deliberately **absent**: it is accepted by the venue, but the
//!   observed spacing was 2 678 400 seconds — 31 days, a calendar month,
//!   not the fixed 30 days its name implies. A fixed-width [`BarSpec`]
//!   cannot represent it faithfully, and guessing would misalign every
//!   bucket, so it is not offered at all.
//! - **History depth is capped**: a window older than 10 000 candles is
//!   refused with `Candlestick too long ago. Maximum 10000 points ago are
//!   allowed`. Loud, so a caller can act on it; at one hour that is a
//!   little over a year.
//! - Only the spot market is served here. Gate's futures candlesticks
//!   answer with an entirely different shape — an array of *objects*
//!   (`{t, o, h, l, c, v, sum}`) with no `window_closed` flag at all — so
//!   they need their own source rather than a branch in this one.

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{VenueClient, common_scale};

const SPOT_CANDLES_URL: &str = "https://api.gateio.ws/api/v4/spot/candlesticks";

/// The tested cap: 1000 rows came back for `limit=1000`, and `limit=1001`
/// was refused with HTTP 400 rather than silently truncated.
pub(crate) const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call. Gate's public endpoints send no rate-limit headers to
/// reconcile against, so this is this project's own conservative proactive
/// budget, not a venue-documented number — the same value every other bar
/// source in this workspace uses, so a difference between venues is never
/// mistaken for a claim about their relative real cost.
pub(crate) const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v4/spot/candlesticks`: eight positional strings,
/// in the order this module's docs spell out — **not** OHLC order.
type RawCandle = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

/// Every `(spec, interval)` pair this source has verified, and the only
/// ones it will ever ask Gate for.
///
/// One table rather than a `supported_specs()` list beside a formatting
/// function, because Gate makes the usual arrangement dangerous: it answers
/// **HTTP 200 for interval strings it does not honour**, so a mapping that
/// built its string arithmetically (`format!("{step}m")`) would happily
/// send `7m`, be told everything was fine, and store bars of the wrong
/// width for ever. Deriving both the offered list and the lookup from this
/// single table makes the two incapable of disagreeing, and makes an
/// unverified interval impossible to send rather than merely unlikely.
///
/// Each entry was requested live and the spacing between the returned
/// candles measured against the interval asked for — accepted is not the
/// same as honoured, and only the measurement distinguishes them.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (3, BarUnit::Minute, "3m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (30, BarUnit::Minute, "30m"),
    (1, BarUnit::Hour, "1h"),
    (2, BarUnit::Hour, "2h"),
    (4, BarUnit::Hour, "4h"),
    (8, BarUnit::Hour, "8h"),
    (1, BarUnit::Day, "1d"),
    // Gate honours `1w` identically — both measured 604 800 seconds — but
    // one spelling keeps the mapping single-valued.
    (1, BarUnit::Week, "7d"),
    // `30d` is deliberately absent: accepted, but measured 31 days apart.
    // See the module docs.
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
pub(crate) fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Gate's `interval` string for `spec`, or `None` when `spec` is not one
/// this source has verified.
///
/// A lookup, never a computation — see [`INTERVALS`] for why the
/// difference matters on this venue specifically.
pub(crate) fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, interval)| interval)
}

/// Gate spot bars, fetched through a [`VenueClient`]. Closure comes
/// entirely from each row's own `window_closed` flag, so no
/// [`senken_series::Clock`] is needed.
#[derive(Debug, Clone)]
pub struct GateBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl GateBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `from`/`to` are epoch **seconds**, and `range` is nanoseconds, so
    /// the conversion truncates toward the epoch. `from` is rounded *down*
    /// and `to` rounded *up* deliberately: a candle whose open time falls
    /// inside `range` but whose second-precision form would land just
    /// outside it must still be asked for, and anything genuinely outside
    /// is discarded on the way back in [`BarSource::bars`] anyway.
    fn candles_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let from = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let to = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?currency_pair={symbol}&interval={interval}&from={from}&to={to}",
            self.url,
        )
    }
}

/// The Gate spot bar source, registered under [`crate::SPOT_ID`] — the one
/// Gate market whose candlestick shape this source decodes (see the module
/// docs on why futures cannot share it).
#[must_use]
pub fn bar_source_spot(client: VenueClient) -> GateBarSource {
    GateBarSource {
        url: SPOT_CANDLES_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for GateBarSource {
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
        let interval = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let url = self.candles_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        // Field 2 is the close and field 5 the open — see the module docs.
        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
                row.5.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().flat_map(|row| [row.6.as_str(), row.1.as_str()]));

        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (ts, quote_volume, close, high, low, open, base_volume, window_closed) in rows {
            // The still-forming candle is the venue's own answer, not a
            // guess from a clock, and it must never be persisted.
            if window_closed != "true" {
                continue;
            }

            let ts_secs: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;
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
                volume: Volume::Real(scaled(&base_volume, qty_scale)?),
                quote_volume: Some(scaled(&quote_volume, qty_scale)?),
                // Neither reported by this endpoint.
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // Nine of the twenty venues audited for this workspace answer an
        // out-of-reach window with HTTP 200 and their *newest* candles
        // instead of the ones asked for. Gate is not one of them — it
        // refuses such a request outright — but discarding stray rows
        // silently is what would hide it if that ever changed, or if this
        // code were copied to a venue that does it: the caller would get an
        // empty result, indistinguishable from "this venue has no data
        // here", and a gap could be cached permanently over data that
        // actually exists. An answer made *entirely* of rows outside the
        // requested range is that failure, and it is reported, not swallowed.
        if bars.is_empty() && outside > 0 {
            return Err(SourceError::rejected(format!(
                "answered with {outside} closed bars, none inside the requested range — \
                 the range parameters were not honoured"
            )));
        }

        // Ascending regardless of what the venue returns. Gate is already
        // ascending; that is an observation, not a guarantee.
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
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source_spot, interval_of};

    /// A real `GET /api/v4/spot/candlesticks?currency_pair=BTC_USDT&interval=1h`
    /// response, recorded 2026-09-02. Four rows, the last of which the
    /// venue itself marks as still forming — which is exactly what makes it
    /// worth keeping: a hand-written fixture would not have thought to
    /// include one.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

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
            UnixNanos::from_secs(1_788_300_000).unwrap(),
            UnixNanos::from_secs(1_788_320_000).unwrap(),
        )
        .unwrap()
    }

    async fn serving(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/candles"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn the_still_forming_candle_the_venue_flags_is_never_returned() {
        // The fixture's fourth row carries `window_closed: "false"`. A bar
        // source that persisted it would write a half-finished candle into
        // storage, where nothing downstream could tell it from a real one.
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 3, "the fixture holds 4 rows, one still forming");
        let newest = bars.last().unwrap();
        assert_eq!(newest.ts_open, UnixNanos::from_secs(1_788_310_800).unwrap());
    }

    #[tokio::test]
    async fn open_and_close_are_read_from_the_venue_s_own_field_order_not_ohlc() {
        // Field 2 is the close and field 5 the open. Read as OHLC the two
        // would swap, every candle would still look internally consistent,
        // and only a comparison against another venue would reveal it — so
        // this asserts the actual recorded numbers, not a relationship.
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // Row 0 of the fixture: close 77437.4, high 77630.2, low 77211.6,
        // open 77276.2 — at the batch's common scale of one decimal.
        assert_eq!(first.open, 772_762, "open is field 5, not field 2");
        assert_eq!(first.close, 774_374, "close is field 2, not field 5");
        assert_eq!(first.high, 776_302);
        assert_eq!(first.low, 772_116);
        assert!(first.high >= first.open.max(first.close));
        assert!(first.low <= first.open.min(first.close));
    }

    #[tokio::test]
    async fn each_row_s_open_matches_the_previous_row_s_close() {
        // The independent check that the field order above is right: on a
        // continuous venue the open of one hour is the close of the last.
        // If open and close were swapped this would fail across the board.
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        for pair in bars.windows(2) {
            let gap = (pair[1].open - pair[0].close).abs();
            assert!(
                gap <= 10,
                "open {} does not continue from close {}",
                pair[1].open,
                pair[0].close
            );
        }
    }

    #[tokio::test]
    async fn timestamps_are_read_as_seconds_not_milliseconds() {
        // Every neighbouring venue in this workspace reports milliseconds.
        // Reading Gate's seconds as milliseconds would place every bar in
        // January 1970 without failing anything.
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_303_600).unwrap()
        );
        assert_eq!(
            bars[1].ts_open.as_nanos() - bars[0].ts_open.as_nanos(),
            3_600 * 1_000_000_000,
            "one hour apart, so the unit was read correctly"
        );
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));
        // Only the fixture's second closed row falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_307_200).unwrap(),
            UnixNanos::from_secs(1_788_310_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_307_200).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        // Nine of the twenty venues audited answer an out-of-reach window
        // with their newest candles and HTTP 200. Silently dropping those
        // rows would hand the caller an empty result that reads as "no data
        // exists here" — and a permanent cached gap over data that does.
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));
        // A window the fixture's rows are nowhere near.
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
        // The other half of the rule above: a venue that genuinely has
        // nothing for a window must not be reported as broken.
        let server = serving(b"[]").await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn volume_comes_from_the_base_asset_column() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // Field 6 is base volume (166.513681), field 1 quote volume.
        assert!(matches!(bars[0].volume, Volume::Real(v) if v > 0));
        assert!(bars[0].quote_volume.is_some_and(|q| q > 0));
    }

    #[test]
    fn a_week_is_asked_for_as_seven_days() {
        assert_eq!(interval_of(BarSpec::new(1, BarUnit::Week)).unwrap(), "7d");
    }

    #[test]
    fn a_multiple_of_a_verified_interval_is_still_not_verified() {
        // `14d` is arithmetically obvious and completely unevidenced. The
        // old mapping computed its string and would have sent it; Gate
        // answers HTTP 200 to interval strings it does not honour, so the
        // reply would have looked like success while the bars came back at
        // some other width entirely.
        assert!(interval_of(BarSpec::new(2, BarUnit::Week)).is_none());
        assert!(interval_of(BarSpec::new(6, BarUnit::Hour)).is_none());
        assert!(interval_of(BarSpec::new(2, BarUnit::Day)).is_none());
    }

    #[test]
    fn the_calendar_month_gate_calls_30d_is_not_offered() {
        // Gate accepts `30d` and spaces it 31 days apart — a calendar
        // month. Mapping a fixed-width spec onto it would misalign every
        // bucket, so it is refused rather than approximated.
        assert!(interval_of(BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_interval_string() {
        assert!(interval_of(BarSpec::new(7, BarUnit::Minute)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        // A spec offered by `supported()` that `interval_of` then refuses
        // would be a promise the source cannot keep.
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
        // No mock is mounted: any request would fail the test.
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client()).with_url(format!("{}/candles", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_310_800).unwrap(),
            UnixNanos::from_secs(1_788_310_800).unwrap(),
        );

        // `TimeRange::new` may itself reject an empty range; either way the
        // source must not reach the venue for one.
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
