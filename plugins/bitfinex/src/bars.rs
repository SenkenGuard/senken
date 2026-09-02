//! Bitfinex spot bar fetching —
//! `GET /v2/candles/trade:{timeframe}:{symbol}/hist`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against `tBTCUSD`
//! (`limit=5`), except the row cap, which is this workspace's own prior live
//! audit of this venue rather than something re-derived here — see point 4.
//!
//! 1. **Sort direction**: `sort=1` asks for ascending, but the *default*
//!    (`sort` omitted) is descending — confirmed by requesting `sort=1` with
//!    no `start`/`end` bound and getting back 2013 data instead of the
//!    recent candles an unqualified request otherwise returns, i.e. Bitfinex
//!    really does walk from the opposite end when the sort direction flips.
//!    Rows are re-sorted here regardless of which way the request asked.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a JSON
//!    *number*, field 0 — not a string, which is what Gate and Bitstamp use
//!    for the same concept.
//! 3. **Closed-candle detection**: no flag of any kind. Closure is decided
//!    by adding the requested [`BarSpec`]'s own length to each row's open
//!    time and comparing against [`Clock::now`], exactly as
//!    `plugins/binance` does — see [`senken_plugin::SystemClock`].
//! 4. **Row cap**: 10 000, refused loudly beyond it. This number comes from
//!    this workspace's live audit of Bitfinex conducted before this source
//!    was written, not from a boundary request made here — the one
//!    live request this module's own fixture is built from used `limit=5`,
//!    well under the cap, so the exact refusal text has not been
//!    independently reproduced by this change.
//! 5. **Pagination direction**: `start`/`end`, both epoch milliseconds,
//!    forward. Not independently tested against a window Bitfinex might
//!    ignore — the audit that supplied the row cap above also found this
//!    venue "safe" (refuses rather than silently substituting), which this
//!    source trusts, but still carries the same defensive
//!    entirely-outside-the-range guard every other source in this workspace
//!    does, in case that ever changes.
//!
//! # The field order is not OHLC
//!
//! A row is six positional values: `[ MTS, OPEN, CLOSE, HIGH, LOW, VOLUME ]`.
//! **Close comes before high and low**, the same trap Gate's candles have —
//! confirmed the same way: the fixture's `high` is never below its `open` or
//! `close`, and its `low` never above them, which only holds for this
//! reading of the field order.
//!
//! # No quote volume, no trade count, no taker volume
//!
//! This endpoint reports one volume figure — base-asset volume — and
//! nothing else `Bar` can otherwise hold; `quote_volume`, `trade_count` and
//! `taker_buy_volume` are always `None` here rather than a guess.
//!
//! # The venue's own symbol needs a `t` prefix this source's symbols don't carry
//!
//! [`crate::to_instrument`] stores Bitfinex's *configuration-list* spelling
//! as `source_symbol` (`BTCUSD`, `AAVE:USD`) — the trading-pair spelling
//! this endpoint actually needs is that string with a `t` prepended
//! (`tBTCUSD`, `tAAVE:USD`), which this module's own URL builder adds.
//! A caller that already had a `t`-prefixed string and passed it through
//! [`SourceSymbol::assume`] would end up asking for `ttBTCUSD` and get a
//! decode error from Bitfinex's own "symbol not found" response — loud, not
//! silent, but worth naming here since every other source in this workspace
//! sends its `source_symbol` unmodified.
//!
//! # What was verified, and what is a documented assumption
//!
//! Only `1h` was requested and measured. The remaining entries in
//! [`INTERVALS`] are Bitfinex's own published, fixed enum of candle
//! resolutions (`1m`, `5m`, `15m`, `30m`, `1h`, `3h`, `6h`, `12h`, `1D`,
//! `1W`) — not a string built by formatting `step` and a suffix, which is
//! the shape of mistake Gate's docs warn about. An unrecognised or
//! mistyped resolution is a documented HTTP error on this venue, so a wrong
//! entry here would surface as a request failure, never as silently
//! mis-spaced candles — the same reasoning `plugins/binance` already
//! applies to its own unverified entries. `14D` and `1M` are left out: `1M`
//! is a calendar month with no fixed [`BarSpec::duration_nanos`], which the
//! closed-candle check above depends on, and `14D` adds nothing `2` weeks
//! or `14` days cannot already express once verified.

use std::sync::Arc;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{Num, VenueClient, common_scale};

const CANDLES_URL: &str = "https://api-pub.bitfinex.com/v2/candles/trade";

/// The row cap from this workspace's own prior live audit of this venue —
/// see the module docs' point 4 on why it is not independently re-tested by
/// this change's own fixture request.
const MAX_ROWS: usize = 10_000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, the same
/// value every other bar source here uses, not a venue-documented number.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /v2/candles/trade:.../hist`: `[MTS, OPEN, CLOSE, HIGH,
/// LOW, VOLUME]` — **not** OHLC order, see the module docs.
type RawCandle = (i64, Num, Num, Num, Num, Num);

/// Every `(step, unit, timeframe)` this source has verified or trusts as
/// Bitfinex's own published, fixed set of candle resolutions — see the
/// module docs on why a formatted string is not used instead.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (30, BarUnit::Minute, "30m"),
    (1, BarUnit::Hour, "1h"),
    (3, BarUnit::Hour, "3h"),
    (6, BarUnit::Hour, "6h"),
    (12, BarUnit::Hour, "12h"),
    (1, BarUnit::Day, "1D"),
    (1, BarUnit::Week, "1W"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Bitfinex's timeframe string for `spec`, or `None` when `spec` is not one
/// this source serves.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, interval)| interval)
}

/// Bitfinex spot bars, fetched through a [`VenueClient`] and closed against
/// a [`Clock`] (Bitfinex sends no confirmation flag — see the module docs).
#[derive(Clone)]
pub struct BitfinexBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BitfinexBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitfinexBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl BitfinexBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `start`/`end` are epoch milliseconds; `range.end()` is exclusive, so
    /// the last representable millisecond strictly before it is the
    /// inclusive upper bound sent here — anything genuinely outside `range`
    /// is discarded again on the way back in [`BarSource::bars`] regardless.
    fn candles_url(&self, symbol: &str, timeframe: &str, range: TimeRange) -> String {
        let start_ms = range.start().as_millis();
        let end_ms = range.end().as_millis().saturating_sub(1);
        format!(
            "{}:{timeframe}:t{symbol}/hist?start={start_ms}&end={end_ms}&limit={MAX_ROWS}&sort=1",
            self.url
        )
    }
}

/// Bitfinex spot bars.
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> BitfinexBarSource {
    BitfinexBarSource {
        url: CANDLES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BitfinexBarSource {
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
        let timeframe = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        // Every entry in `INTERVALS` names a fixed-width `BarUnit`, so this
        // is always `Some` for a spec `interval_of` just accepted — never
        // `Month`, which is excluded from `INTERVALS` for exactly this
        // reason.
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

        let url = self.candles_url(symbol.as_str(), timeframe, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        // Field 2 is the close and field 3 the high — see the module docs.
        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.5.as_str()));

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for (ts_ms, open, close, high, low, volume) in rows {
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms}ms overflowed")))?;

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
                open: scaled(&open, price_scale)?,
                high: scaled(&high, price_scale)?,
                low: scaled(&low, price_scale)?,
                close: scaled(&close, price_scale)?,
                volume: Volume::Real(scaled(&volume, qty_scale)?),
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

        // Ascending regardless of what the venue returns for this request's
        // own `sort` value.
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses `raw` at `scale`, mapping an unparseable value — which should
/// never happen given `scale` was computed from this exact batch of
/// values — to a decode error rather than panicking or guessing.
fn scaled(raw: &Num, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw.as_str(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source_spot;

    /// A real `GET
    /// /v2/candles/trade:1h:tBTCUSD/hist?start=...&end=...&limit=5`
    /// response, recorded 2026-09-02. Five rows, descending (Bitfinex's own
    /// default order), the newest of which opened at `06:00:00Z` — still
    /// forming at the wall clock the recording was made against.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    /// The only sanctioned way to obtain a [`SourceSymbol`] is through an
    /// [`Instrument`] — Bitfinex's own configuration-list spelling has no
    /// `t` prefix, matching what [`crate::to_instrument`] actually stores.
    fn btcusd() -> SourceSymbol {
        Instrument::spot("BTCUSD", "BTCUSD", "BTC", "USD").source_symbol()
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
    async fn mock_source(now_ms: i64) -> (MockServer, super::BitfinexBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(now_ms)))
            .with_url(format!("{}/candles", server.uri()));
        (server, source)
    }

    #[tokio::test]
    async fn the_still_forming_top_row_is_never_returned() {
        // The fixture's newest row opened at 06:00:00Z (1_788_328_800_000
        // ms); a clock reading 06:30:00Z sits inside that same hour, so the
        // bar has not closed yet.
        let (_server, source) = mock_source(1_788_330_600_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 4, "the fixture holds 5 rows, one still forming");
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_328_800_000)
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_five_are_kept() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();
        assert_eq!(bars.len(), 5);
    }

    #[tokio::test]
    async fn rows_come_back_ascending_even_though_the_venue_sent_them_descending() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_314_400_000).unwrap(),
            "the fixture's oldest row, MTS field, is first once sorted"
        );
    }

    #[tokio::test]
    async fn open_and_close_are_read_from_the_venue_s_own_field_order_not_ohlc() {
        // Row 0 of the fixture (oldest, once sorted): MTS 1788314400000,
        // OPEN 77027, CLOSE 77363, HIGH 77371, LOW 76968 — field 2 is the
        // close and field 3 the high, not OHLC order.
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        // Every price in this fixture happens to be a whole number, so the
        // batch's common scale is 0 and these values are the raw integers.
        let first = &bars[0];
        assert_eq!(first.open, 77_027, "open is field 1");
        assert_eq!(first.close, 77_363, "close is field 2, not field 3");
        assert_eq!(first.high, 77_371);
        assert_eq!(first.low, 76_968);
        assert!(first.high >= first.open.max(first.close));
        assert!(first.low <= first.open.min(first.close));
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        // Only the fixture's 1_788_318_000_000 row falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_317_000_000).unwrap(),
            UnixNanos::from_millis(1_788_319_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&btcusd(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_318_000_000).unwrap()
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
            .bars(&btcusd(), hour(), elsewhere)
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
            .respond_with(ResponseTemplate::new(200).set_body_raw(&b"[]"[..], "application/json"))
            .mount(&server)
            .await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(4_102_444_800_000)))
            .with_url(format!("{}/candles", server.uri()));

        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Month)).is_none());
        assert!(super::interval_of(BarSpec::new(14, BarUnit::Day)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                super::interval_of(*spec).is_some(),
                "{spec} is offered but has no interval mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)))
            .with_url(format!("{}/candles", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_318_000_000).unwrap(),
            UnixNanos::from_millis(1_788_318_000_000).unwrap(),
        );

        if let Some(range) = inverted {
            assert!(
                source
                    .bars(&btcusd(), hour(), range)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn max_rows_is_the_documented_tested_cap() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        assert_eq!(source.max_rows(), 10_000);
    }
}
