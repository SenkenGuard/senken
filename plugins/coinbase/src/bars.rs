//! Coinbase Exchange spot bar fetching —
//! `GET /products/{id}/candles`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against `BTC-USD`
//! (`granularity=3600`), except pagination direction — see point 5.
//!
//! 1. **Sort direction**: descending, newest first, with no request
//!    parameter to change it. Rows are re-sorted here anyway.
//! 2. **Timestamp representation**: epoch **seconds**, as a JSON number,
//!    field 0.
//! 3. **Closed-candle detection**: no flag of any kind. Closure is decided
//!    by adding the requested [`BarSpec`]'s own length to each row's open
//!    time and comparing against [`Clock::now`], exactly as
//!    `plugins/binance` does — see [`senken_plugin::SystemClock`].
//! 4. **Row cap**: 300, refused loudly beyond it — this workspace's own
//!    live audit of this venue conducted before this source was written,
//!    not a boundary this module's own fixture request reproduced.
//! 5. **Pagination direction**: `start`/`end`, ISO 8601, forward, is this
//!    module's own **documented assumption** — the one live recording
//!    behind this module's fixture used only `granularity`, matching the
//!    exact call this workspace's prior audit reported. `start`/`end` are
//!    Coinbase Exchange's long-published, stable parameter names for this
//!    endpoint, used here as the conservative default AGENTS.md calls for
//!    when a fact is not re-derived live; the defensive
//!    entirely-outside-the-range guard below still catches a window
//!    silently ignored.
//!
//! # The field order is neither OHLC nor this venue's own documented order
//!
//! A row is six positional values:
//! `[ time, low, high, open, close, volume ]` — **low and high come before
//! open and close**. Confirmed the same way Gate's own trap was: the
//! fixture's `high` is never below its `open` or `close` and its `low`
//! never above them under this reading, and consecutive rows' `close` and
//! `open` match to the cent.
//!
//! # No quote volume, no trade count, no taker volume
//!
//! This endpoint reports one volume figure — base-asset volume — and
//! nothing else `Bar` can otherwise hold; `quote_volume`, `trade_count` and
//! `taker_buy_volume` are always `None` here rather than a guess.
//!
//! # What was verified, and what is a documented assumption
//!
//! Only `granularity=3600` (one hour) was requested and measured. The
//! remaining entries in [`INTERVALS`] are Coinbase Exchange's own
//! published, fixed set of granularities in seconds (`60`, `300`, `900`,
//! `3600`, `21600`, `86400`) — not a value computed from an arbitrary
//! `BarSpec`, the same reasoning Gate's and Bitstamp's own tables apply.

use std::sync::Arc;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{Num, VenueClient, common_scale};

const PRODUCTS_URL: &str = "https://api.exchange.coinbase.com/products";

/// The row cap from this workspace's own prior live audit of this venue —
/// see the module docs' point 4 on why it is not independently re-tested by
/// this change's own fixture request.
const MAX_ROWS: usize = 300;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, the same
/// value every other bar source here uses, not a venue-documented number.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /products/{id}/candles`: `[time, low, high, open,
/// close, volume]` — see the module docs on why this is neither OHLC nor
/// the intuitive reading of the name "candles".
type RawCandle = (i64, Num, Num, Num, Num, Num);

/// Every `(step, unit, granularity)` this source has verified or trusts as
/// Coinbase Exchange's own published, fixed set of granularities.
const INTERVALS: &[(u32, BarUnit, u32)] = &[
    (1, BarUnit::Minute, 60),
    (5, BarUnit::Minute, 300),
    (15, BarUnit::Minute, 900),
    (1, BarUnit::Hour, 3600),
    (6, BarUnit::Hour, 21_600),
    (1, BarUnit::Day, 86_400),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// Coinbase's `granularity` seconds for `spec`, or `None` when `spec` is
/// not one this source serves.
fn granularity_of(spec: BarSpec) -> Option<u32> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, granularity)| granularity)
}

/// Coinbase Exchange spot bars, fetched through a [`VenueClient`] and
/// closed against a [`Clock`] (this endpoint sends no confirmation flag —
/// see the module docs).
#[derive(Clone)]
pub struct CoinbaseBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for CoinbaseBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoinbaseBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl CoinbaseBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `start`/`end` are ISO 8601, rounded to whole seconds the same way
    /// Gate's `from`/`to` are — down for `start`, up for `end` — and
    /// anything genuinely outside `range` is discarded again on the way
    /// back in [`BarSource::bars`] regardless.
    fn candles_url(&self, symbol: &str, granularity: u32, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let start_secs = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let end_secs = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        // `from_secs` only fails on `i64` overflow, which a `TimeRange`
        // already built from a valid `UnixNanos` cannot produce; the epoch
        // fallback is defensive, not a claim this path is ever taken.
        let start = UnixNanos::from_secs(start_secs).unwrap_or(UnixNanos::EPOCH);
        let end = UnixNanos::from_secs(end_secs).unwrap_or(UnixNanos::EPOCH);
        format!(
            "{}/{symbol}/candles?granularity={granularity}&start={start}&end={end}",
            self.url
        )
    }
}

/// Coinbase Exchange spot bars.
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> CoinbaseBarSource {
    CoinbaseBarSource {
        url: PRODUCTS_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for CoinbaseBarSource {
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
        let granularity = granularity_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        // Every entry in `INTERVALS` names a fixed-width `BarUnit`, so this
        // is always `Some` for a spec `granularity_of` just accepted.
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

        let url = self.candles_url(symbol.as_str(), granularity, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        // Fields 1-4 are low, high, open, close — see the module docs.
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
        for (ts_secs, low, high, open, close, volume) in rows {
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

        // Ascending regardless of what the venue returns — this endpoint's
        // default is descending, and this must not silently rely on that
        // ever changing.
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
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source_spot;

    /// A real `GET /products/BTC-USD/candles?granularity=3600` response,
    /// recorded 2026-09-02, trimmed to its five newest rows. Descending,
    /// the newest of which opened at `06:00:00Z` — still forming at the
    /// wall clock the recording was made against.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    /// The only sanctioned way to obtain a [`SourceSymbol`] is through an
    /// [`Instrument`].
    fn btcusd() -> SourceSymbol {
        Instrument::spot("BTCUSD", "BTC-USD", "BTC", "USD").source_symbol()
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
    async fn mock_source(now_ms: i64) -> (MockServer, super::CoinbaseBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(now_ms)))
            .with_url(format!("{}/products", server.uri()));
        (server, source)
    }

    #[tokio::test]
    async fn the_still_forming_top_row_is_never_returned() {
        // The fixture's newest row opened at 06:00:00Z (1_788_328_800 s); a
        // clock reading 06:30:00Z sits inside that same hour.
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
            UnixNanos::from_secs(1_788_314_400).unwrap()
        );
    }

    #[tokio::test]
    async fn low_and_high_are_read_from_the_venue_s_own_field_order_not_ohlc() {
        // Oldest row once sorted: time 1788314400, low 76900, high
        // 77292.96, open 76965.73, close 77277.15 — fields 1 and 2 are low
        // and high, not open and high.
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        assert_eq!(first.low, 7_690_000, "low is field 1, not open");
        assert_eq!(first.high, 7_729_296, "high is field 2");
        assert_eq!(first.open, 7_696_573);
        assert_eq!(first.close, 7_727_715);
        assert!(first.high >= first.open.max(first.close));
        assert!(first.low <= first.open.min(first.close));
        assert!(matches!(first.volume, Volume::Real(v) if v > 0));
    }

    #[tokio::test]
    async fn each_row_s_open_stays_close_to_the_previous_row_s_close() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        for pair in bars.windows(2) {
            let gap = (pair[1].open - pair[0].close).abs();
            assert!(
                gap <= 5,
                "open {} does not continue from close {}",
                pair[1].open,
                pair[0].close
            );
        }
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        // Only the fixture's 1_788_318_000 row falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_317_000).unwrap(),
            UnixNanos::from_secs(1_788_319_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&btcusd(), hour(), narrow).await.unwrap();

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
            .with_url(format!("{}/products", server.uri()));

        let bars = source.bars(&btcusd(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_granularity() {
        assert!(super::granularity_of(BarSpec::new(30, BarUnit::Minute)).is_none());
        assert!(super::granularity_of(BarSpec::new(1, BarUnit::Month)).is_none());
        assert!(super::granularity_of(BarSpec::new(2, BarUnit::Hour)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_granularity() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                super::granularity_of(*spec).is_some(),
                "{spec} is offered but has no granularity mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)))
            .with_url(format!("{}/products", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_318_000).unwrap(),
            UnixNanos::from_secs(1_788_318_000).unwrap(),
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
        assert_eq!(source.max_rows(), 300);
    }
}
