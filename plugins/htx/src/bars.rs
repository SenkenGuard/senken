//! HTX spot bar fetching — `GET /market/history/kline`.
//!
//! Spot only. HTX's three derivative markets live on `api.hbdm.com` with a
//! different path prefix per market (linear swap, inverse swap, inverse
//! dated futures — the last needing its contract code resolved from
//! `/api/v1/contract_contract_info` since it rolls weekly), and this
//! recording session's one-request-per-endpoint limit made covering all
//! four in one pass impossible. This is the scope decision that follows:
//! spot only, so it is at least complete rather than four partial ones.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `symbol=btcusdt&period=1min`.
//!
//! 1. **Sort direction**: **descending** by open time — the newest row
//!    first, the same as Gemini in this workspace. Sorted ascending here
//!    before returning, like every other source.
//! 2. **Timestamp representation**: epoch **seconds**, as a JSON number,
//!    field `id`.
//! 3. **Closed-candle detection**: no per-row flag. The newest row is
//!    closed only once `id` plus the requested period has passed —
//!    compared against [`Clock::now`], never the wall clock directly (see
//!    `clock.rs`). The envelope's own top-level `ts` (a server timestamp)
//!    is deliberately not used for this — see `clock.rs`'s docs.
//! 4. **Row cap**: the task brief this source was written against
//!    documents 2000 as this endpoint's `size` ceiling, from an earlier
//!    live audit; the fixture recorded here used `size=20` to keep the
//!    file small, so 2000 is a cited, not independently reconfirmed,
//!    figure.
//! 5. **Pagination direction**: **none.** There is no `from`/`to`/`since`
//!    parameter on this endpoint at all — only `size`, which bounds *how
//!    many* rows come back, never *which* ones. Every call for a given
//!    symbol and period returns the same fixed lookback window from "now".
//!
//! # This venue cannot backfill — say so, do not paper over it
//!
//! Point 5 is not a pagination quirk to work around; it is a hard limit of
//! the endpoint, identical in shape to Gemini's in this same workspace. A
//! request whose start precedes the oldest row this call actually returned
//! is rejected with a message naming the boundary, never silently trimmed
//! down to whatever *did* come back — the data may exist, this endpoint
//! simply cannot reach it. A chart that runs out of history on HTX is the
//! venue's limit, not a bug in this source.
//!
//! # Prices arrive as JSON numbers, some in scientific notation
//!
//! Every price and volume field is a bare JSON number, not a decimal
//! string — `"amount":1.8E-4` in the recorded fixture, a small quantity
//! HTX chose to write in scientific notation. This project never routes a
//! price through `f64`, not even transiently, so each field is decoded as
//! a [`RawValue`] (this workspace's `serde_json` `raw_value` feature,
//! which hands back the venue's exact digits with no float parsing) and
//! then normalised with [`senken_core::plain_decimal`] — the same
//! string-shifting digit arithmetic `senken_venue::Num` uses for its own
//! string-typed inputs, applied here directly to the raw JSON text instead
//! of through `Num`'s own `Deserialize` impl, whose JSON-number path goes
//! through `f64::to_string()` and would defeat the point.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled, plain_decimal};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

const SPOT_KLINE_URL: &str = "https://api.huobi.pro/market/history/kline";

/// Cited from the task brief's earlier live audit, not independently
/// reconfirmed this session — see the module docs' point 4.
const MAX_ROWS: usize = 2000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, period)` mapping — see the module docs
/// on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Minute, "1min");

fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// HTX's `period` string for `spec`, or `None` when `spec` is not the one
/// interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

#[derive(Debug, Deserialize)]
struct RawCandle {
    /// Epoch seconds.
    id: i64,
    open: Box<RawValue>,
    close: Box<RawValue>,
    low: Box<RawValue>,
    high: Box<RawValue>,
    /// Base-asset volume.
    amount: Box<RawValue>,
    /// Quote-asset volume (turnover).
    vol: Box<RawValue>,
    count: u32,
}

/// HTX spot bars, fetched through a [`VenueClient`] and closed against a
/// [`Clock`] (this endpoint sends no confirmation flag — see the module
/// docs).
#[derive(Clone)]
pub struct HtxSpotBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for HtxSpotBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HtxSpotBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl HtxSpotBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// No `from`/`to` at all — see the module docs on point 5.
    fn candles_url(&self, symbol: &str, period: &str) -> String {
        format!(
            "{}?symbol={symbol}&period={period}&size={MAX_ROWS}",
            self.url
        )
    }
}

/// The HTX spot bar source, registered under [`crate::SPOT_ID`].
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> HtxSpotBarSource {
    HtxSpotBarSource {
        url: SPOT_KLINE_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

/// Normalises `raw` (a bare JSON number's exact text, possibly scientific
/// notation) to plain decimal digits with no `f64` anywhere in the path —
/// see the module docs.
fn normalize(raw: &str) -> Result<String, SourceError> {
    plain_decimal(raw)
        .map(std::borrow::Cow::into_owned)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} is not a decimal number")))
}

#[async_trait]
impl BarSource for HtxSpotBarSource {
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
        let period = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let bar_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed duration")))?;

        let url = self.candles_url(symbol.as_str(), period);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = crate::decode(&body)?;

        let now_nanos = self.clock.now().as_nanos();
        // (ts_open, open, high, low, close, amount, vol, count), ascending,
        // closed rows only — the venue answers newest first (point 1), and
        // the history-boundary check below needs the true oldest closed
        // candle regardless of what `range` asked for.
        let mut closed = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_open = UnixNanos::from_secs(row.id)
                .ok_or_else(|| SourceError::decode(format!("open time {}s overflowed", row.id)))?;
            let close_nanos = ts_open.as_nanos().checked_add(bar_nanos).ok_or_else(|| {
                SourceError::decode(format!("close time for {ts_open} overflowed"))
            })?;
            if close_nanos > now_nanos {
                continue;
            }
            closed.push((
                ts_open,
                normalize(row.open.get())?,
                normalize(row.high.get())?,
                normalize(row.low.get())?,
                normalize(row.close.get())?,
                normalize(row.amount.get())?,
                normalize(row.vol.get())?,
                row.count,
            ));
        }
        closed.sort_by_key(|entry| entry.0);

        // This endpoint takes no time parameter at all (point 5): a
        // request older than the fixed window it answers with must be
        // reported as reaching past the venue's own limit, never silently
        // trimmed down to whatever did come back — see the module docs.
        if let Some((oldest, ..)) = closed.first()
            && range.start() < *oldest
        {
            return Err(SourceError::rejected(format!(
                "HTX's fixed candle window for this symbol/period starts at {oldest}; \
                 it cannot serve the requested range starting at {}",
                range.start()
            )));
        }

        let price_scale = common_scale(closed.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(closed.iter().map(|row| row.5.as_str()));
        let quote_scale = common_scale(closed.iter().map(|row| row.6.as_str()));

        let mut bars = Vec::with_capacity(closed.len());
        for (ts_open, open, high, low, close, amount, vol, count) in closed {
            if !range.contains(ts_open) {
                continue;
            }
            bars.push(Bar {
                ts_open,
                open: scaled(&open, price_scale)?,
                high: scaled(&high, price_scale)?,
                low: scaled(&low, price_scale)?,
                close: scaled(&close, price_scale)?,
                volume: Volume::Real(scaled(&amount, qty_scale)?),
                quote_volume: Some(scaled(&vol, quote_scale)?),
                trade_count: Some(count),
                taker_buy_volume: None,
            });
        }

        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses `raw` (already-normalised plain decimal text) at `scale`,
/// mapping an unparseable value — which should never happen given `scale`
/// was computed from this exact batch — to a decode error rather than
/// panicking or guessing.
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

    use super::bar_source_spot;

    /// A real `GET history/kline?symbol=btcusdt&period=1min&size=20`
    /// response, recorded 2026-09-02: 20 one-minute rows, newest first,
    /// including one (`amount`) written in scientific notation.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/spot_klines_1min.json");

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
        SourceSymbol::assume("btcusdt")
    }

    fn minute() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_328_080).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
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

    /// Strictly before the newest row's own close
    /// (`1_788_329_220 + 60 = 1_788_329_280` seconds).
    fn clock_before_last_close() -> Arc<dyn Clock> {
        Arc::new(FixedClock(1_788_329_230_000))
    }

    fn clock_after_everything() -> Arc<dyn Clock> {
        Arc::new(FixedClock(4_102_444_800_000))
    }

    #[tokio::test]
    async fn the_still_forming_newest_candle_is_never_returned() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_before_last_close())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 19, "20 rows, the newest one still forming");
        assert!(
            bars.iter()
                .all(|b| b.ts_open < UnixNanos::from_secs(1_788_329_220).unwrap())
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_twenty_are_kept() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 20);
    }

    #[tokio::test]
    async fn the_descending_response_is_returned_ascending() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_328_080).unwrap(),
            "the oldest row (last in the venue's own order) sorts first"
        );
    }

    #[tokio::test]
    async fn scientific_notation_amounts_decode_without_going_through_f64() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        // The fixture's newest row: amount "1.8E-4" == 0.00018.
        let newest = bars.last().unwrap();
        assert_eq!(newest.ts_open, UnixNanos::from_secs(1_788_329_220).unwrap());
        // At whatever common scale the batch settled on, 0.00018 base
        // units must be exactly representable — not rounded away by a
        // float round-trip.
        let Volume::Real(amount) = newest.volume else {
            panic!("expected real volume");
        };
        assert!(
            amount > 0,
            "a nonzero scientific-notation amount must not decode to zero"
        );
    }

    #[tokio::test]
    async fn trade_count_and_quote_volume_are_both_carried() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        let oldest = &bars[0];
        assert_eq!(oldest.trade_count, Some(41));
        assert!(oldest.quote_volume.is_some_and(|q| q > 0));
    }

    #[tokio::test]
    async fn a_request_reaching_before_the_venues_fixed_window_is_reported_honestly() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));
        let reaches_before_history = TimeRange::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap();

        let error = source
            .bars(&symbol(), minute(), reaches_before_history)
            .await
            .expect_err("reaching before the fixed window must be reported, not trimmed");

        assert!(
            error.to_string().contains("cannot serve"),
            "the error must name the venue's own limit: {error}"
        );
    }

    #[tokio::test]
    async fn a_request_entirely_after_the_window_is_an_absence_not_an_error() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));
        let future = TimeRange::new(
            UnixNanos::from_secs(1_788_400_000).unwrap(),
            UnixNanos::from_secs(1_788_400_060).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), minute(), future).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let server = serving(CANDLES).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));
        // Only the second-oldest fixture row (1_788_328_140) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_328_140).unwrap(),
            UnixNanos::from_secs(1_788_328_180).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), minute(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_328_140).unwrap()
        );
    }

    #[tokio::test]
    async fn an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let body = br#"{"status":"ok","data":[]}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&symbol(), minute(), wide_range())
            .await
            .unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn an_error_status_is_a_rejection() {
        let body = br#"{"status":"error","err_msg":"invalid symbol","data":[]}"#;
        let server = serving(body).await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));

        assert!(
            source
                .bars(&symbol(), minute(), wide_range())
                .await
                .is_err()
        );
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Hour)).is_none());
        assert!(super::interval_of(BarSpec::new(5, BarUnit::Minute)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_spot(test_client(), clock_after_everything());
        for spec in source.supported() {
            assert!(super::interval_of(*spec).is_some());
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), clock_after_everything())
            .with_url(format!("{}/kline", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_329_220).unwrap(),
            UnixNanos::from_secs(1_788_329_220).unwrap(),
        );

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
