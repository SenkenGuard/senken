//! Bitget spot bar fetching — `GET /api/v2/spot/market/history-candles`.
//!
//! # Two endpoints, and the obvious one is the trap
//!
//! Bitget spot exposes both `/api/v2/spot/market/candles` and
//! `/api/v2/spot/market/history-candles`. The plain `candles` endpoint is
//! the one this task's live audit found silently answers a request wider
//! than its real coverage with HTTP 200 and the wrong window. Only
//! `history-candles` is used here, and it additionally **requires**
//! `endTime` — a request with no `endTime` at all is rejected outright
//! (HTTP 400, code `400172`, confirmed live), which is itself a small
//! mercy: a caller cannot forget the parameter that bounds the window.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02 against
//! `symbol=BTCUSDT`, not read from documentation.
//!
//! 1. **Sort direction**: ascending by open time.
//! 2. **Timestamp representation**: epoch **milliseconds**, as a
//!    *string* — the first element of each row.
//! 3. **Closed-candle detection**: no flag at all. `history-candles` was
//!    also observed to withhold the currently-open candle by itself —
//!    requesting with `endTime` set an hour into the future still
//!    stopped at the last fully closed hour — but that is an
//!    observation about this one response, not a guarantee for every
//!    future one, so this source still compares `ts_open` plus the
//!    spec's own length against a [`senken_series::Clock`], exactly the
//!    pattern `plugins/binance/src/bars.rs` uses for the same reason.
//! 4. **Row cap**: `history-candles` documents 1000 for spot (this
//!    task's own audit) and 200 for the futures product lines, which
//!    this source does not fetch — futures candlesticks are a different
//!    shape behind a `productType` query parameter and would need their
//!    own source, the same reasoning that keeps Gate's futures
//!    candlesticks out of its own spot bar source.
//! 5. **Pagination direction**: `startTime`/`endTime`, both epoch
//!    milliseconds, confirmed live: a five-hour window bounded by both
//!    returned exactly the candles inside it, ascending.
//!
//! # Granularity casing
//!
//! Spot accepts lowercase (`1h`); Bitget's own futures endpoints demand
//! the same string **uppercase** (`1H`) — a fact this task was scoped
//! with, not independently re-derived, since this source never calls the
//! futures endpoint. Only the lowercase spot form is used here.
//!
//! # What was verified, and what is a documented assumption
//!
//! - `1h` was requested and the spacing between rows measured: exactly
//!   3 600 000 ms apart.
//! - The remaining specs in [`INTERVALS`] follow Bitget's own documented
//!   granularity syntax for this endpoint but were **not** individually
//!   requested and measured — an explicit, commented assumption, kept
//!   deliberately small.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde_json::value::RawValue;

use crate::api::Envelope;
use crate::{OK, SPOT_ID};

const HISTORY_CANDLES_URL: &str = "https://api.bitget.com/api/v2/spot/market/history-candles";

/// The tested cap for spot `history-candles`: this task's live audit
/// recorded 1000. Not re-derived here (see the module docs) to avoid a
/// second live request for an already-established fact.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source in this
/// workspace.
const CANDLES_FETCH_COST: u32 = 5;

/// One row: `[ts, open, high, low, close, baseVolume, quoteVolume]`,
/// with a trailing `usdtVolume` on spot that duplicates `quoteVolume` on
/// every USDT pair and is not decoded separately.
///
/// A `Vec` rather than a fixed tuple because the two markets send
/// different lengths — spot eight, every futures product type seven
/// (confirmed live 2026-09-02) — and a tuple of eight rejects the shorter
/// row outright. Each cell is a [`RawValue`] because the two also differ
/// in *type*: spot writes its numbers as strings and futures depth writes
/// bare numbers, and the venue is free to change which without telling
/// anyone.
type RawCandle = Vec<Box<RawValue>>;

/// Fields a row must have before it can be read: through `quoteVolume`.
const REQUIRED_CELLS: usize = 7;

/// One cell's digits, whether the venue wrote them as a JSON string or a
/// bare number.
fn plain(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"');
    senken_core::plain_decimal(trimmed)
        .map_or_else(|| trimmed.to_owned(), std::borrow::Cow::into_owned)
}

/// Every `(step, unit, granularity)` this source will ask Bitget for. See
/// the module docs for which entry was measured against a live response
/// and which follows documented syntax only.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1min"),
    (5, BarUnit::Minute, "5min"),
    (15, BarUnit::Minute, "15min"),
    (30, BarUnit::Minute, "30min"),
    // Measured live: 3,600,000 ms apart.
    (1, BarUnit::Hour, "1h"),
    (4, BarUnit::Hour, "4h"),
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

/// Bitget's `granularity` string for `spec`, or `None` when `spec` is not
/// one this source has mapped.
fn granularity_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, granularity)| granularity)
}

/// Bitget spot bars, fetched through a [`VenueClient`] and closed against
/// a [`Clock`] — `history-candles` carries no confirmation flag (see the
/// module docs).
#[derive(Clone)]
pub struct BitgetBarSource {
    source_id: &'static str,
    /// Everything the futures endpoint needs beyond the spot one — its
    /// `productType`. Empty for spot.
    extra_query: String,
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BitgetBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitgetBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl BitgetBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn history_candles_url(&self, symbol: &str, granularity: &str, range: TimeRange) -> String {
        format!(
            "{}?symbol={symbol}&granularity={granularity}&limit={MAX_ROWS}&startTime={}&endTime={}{}",
            self.url,
            range.start().as_millis(),
            // `endTime` is inclusive on this venue; `range.end()` is
            // exclusive (`TimeRange`'s own half-open contract), so the
            // last representable millisecond strictly before it is sent.
            range.end().as_millis() - 1,
            self.extra_query,
        )
    }
}

/// The Bitget spot bar source.
#[must_use]
pub fn bar_source_spot(client: VenueClient, clock: Arc<dyn Clock>) -> BitgetBarSource {
    BitgetBarSource {
        source_id: SPOT_ID,
        extra_query: String::new(),
        url: HISTORY_CANDLES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

/// A bar source for one of Bitget's three futures product types.
///
/// A different endpoint on the same host, taking a `productType`, and
/// answering rows of **seven** cells where spot sends eight — spot's
/// trailing `usdtVolume` has no counterpart here. Confirmed live
/// 2026-09-02 for all three product types.
#[must_use]
pub fn bar_source_futures(
    source_id: &'static str,
    product_type: &str,
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> BitgetBarSource {
    BitgetBarSource {
        source_id,
        extra_query: format!("&productType={product_type}"),
        url: "https://api.bitget.com/api/v2/mix/market/history-candles".to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BitgetBarSource {
    fn source_id(&self) -> &str {
        self.source_id
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
        // A candle not yet closed at `spec`'s own length has no place
        // being requested at all — mirrors `plugins/binance`'s reasoning,
        // and keeps the still-forming filter below purely defensive
        // rather than load-bearing for correctness.
        let bar_len_nanos = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed length")))?;

        let url = self.history_candles_url(symbol.as_str(), granularity, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope<RawCandle> =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if !envelope.code.is_empty() && envelope.code != OK {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }
        let rows: Vec<Vec<String>> = envelope
            .data
            .iter()
            .filter(|row| row.len() >= REQUIRED_CELLS)
            .map(|row| row.iter().map(|cell| plain(cell.get())).collect())
            .collect();

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row[1].as_str(),
                row[2].as_str(),
                row[3].as_str(),
                row[4].as_str(),
            ]
        }));
        let qty_scale = common_scale(
            rows.iter()
                .flat_map(|row| [row[5].as_str(), row[6].as_str()]),
        );

        let now = self.clock.now();
        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for row in rows {
            let (ts, open, high, low, close, base_volume, quote_volume) = (
                &row[0], &row[1], &row[2], &row[3], &row[4], &row[5], &row[6],
            );
            let ts_ms: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms}ms overflowed")))?;

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
                open: scaled(open, price_scale)?,
                high: scaled(high, price_scale)?,
                low: scaled(low, price_scale)?,
                close: scaled(close, price_scale)?,
                volume: Volume::Real(scaled(base_volume, qty_scale)?),
                quote_volume: Some(scaled(quote_volume, qty_scale)?),
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard for why an answer made entirely of
        // rows outside the requested range is reported, not swallowed —
        // this is the exact silent wide-window failure this venue was
        // grouped for, even though `history-candles` specifically was
        // found safe by this task's own audit: the guard costs nothing
        // when the venue behaves, and is the only thing standing between
        // a misbehaving future response and a permanently cached gap.
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

    use super::{bar_source_spot, granularity_of};

    /// A real `GET /api/v2/spot/market/history-candles?symbol=BTCUSDT&
    /// granularity=1h&endTime=<now>&limit=10` response, recorded
    /// 2026-09-02. Ten fully closed rows — `history-candles` withheld the
    /// currently-open hour by itself, so closure here is exercised via
    /// the injected clock rather than a naturally-forming fixture row.
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/history_candles_1h.json");

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
        SourceSymbol::assume("BTCUSDT")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    /// The whole window the fixture covers, and then some.
    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_788_290_000_000).unwrap(),
            UnixNanos::from_millis(1_788_330_000_000).unwrap(),
        )
        .unwrap()
    }

    async fn serving_at(body: &'static [u8], now_ms: i64) -> (MockServer, super::BitgetBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let source =
            bar_source_spot(test_client(), Arc::new(FixedClock(now_ms))).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn a_candle_not_yet_closed_at_the_clock_is_dropped() {
        // The fixture's last row opens at 1,788,325,200,000 and closes an
        // hour later. A clock reading just before that close must drop
        // it even though the venue itself already sent it as data.
        let (_server, source) = serving_at(CANDLES, 1_788_328_000_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars.len(),
            9,
            "the still-forming-per-clock row must be dropped"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_325_200_000)
        );
    }

    #[tokio::test]
    async fn once_the_clock_passes_every_close_time_all_rows_are_kept() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 10);
    }

    #[tokio::test]
    async fn rows_decode_with_correct_ohlcv() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // Fixture row 0: ts 1788292800000, open 77310.4, high 77474.99,
        // low 77236.85, close 77455.01 — at the batch's common scale.
        let first = &bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_292_800_000).unwrap()
        );
        assert_eq!(first.open, 7_731_040);
        assert_eq!(first.high, 7_747_499);
        assert_eq!(first.low, 7_723_685);
        assert_eq!(first.close, 7_745_501);
        assert!(matches!(first.volume, Volume::Real(v) if v > 0));
        assert!(first.quote_volume.is_some_and(|q| q > 0));
    }

    #[tokio::test]
    async fn rows_are_ascending() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000_000).await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000_000).await;
        let narrow = TimeRange::new(
            UnixNanos::from_millis(1_788_300_000_000).unwrap(),
            UnixNanos::from_millis(1_788_302_000_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_millis(1_788_300_000_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = serving_at(CANDLES, 1_788_400_000_000).await;
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
        let body = br#"{"code":"00000","msg":"success","requestTime":0,"data":[]}"#;
        let (_server, source) = serving_at(body, 1_788_400_000_000).await;

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":"40034","msg":"Parameter does not exist","data":[]}"#;
        let (_server, source) = serving_at(body, 1_788_400_000_000).await;

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("40034"));
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_granularity_string() {
        assert!(granularity_of(BarSpec::new(7, BarUnit::Minute)).is_none());
        assert!(granularity_of(BarSpec::new(1, BarUnit::Month)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_granularity_string() {
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0)));
        for spec in source.supported() {
            assert!(
                granularity_of(*spec).is_some(),
                "{spec} is offered but has no granularity mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_spot(test_client(), Arc::new(FixedClock(0))).with_url(server.uri());
        let inverted = TimeRange::new(
            UnixNanos::from_millis(1_788_300_000_000).unwrap(),
            UnixNanos::from_millis(1_788_300_000_000).unwrap(),
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
