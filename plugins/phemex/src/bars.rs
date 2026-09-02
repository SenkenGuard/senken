//! Phemex perpetual bar fetching — `GET /exchange/public/md/v2/kline/list`.
//!
//! Perpetuals only. `kline/list` takes one `symbol`, and Phemex's spot and
//! perpetual markets are already two separate sources in this crate (see
//! the crate root docs on the leading `s` marker); covering spot too would
//! have meant a second live request under this task's
//! one-request-per-endpoint limit, so it is left for a follow-up.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Recorded live on 2026-09-02 against `symbol=BTCUSD&resolution=3600`, a
//! three-day window.
//!
//! 1. **Sort direction**: ascending by open time. Sorted again here anyway
//!    — a venue's order is not a promise.
//! 2. **Timestamp representation**: epoch **seconds**, as a JSON number,
//!    the first element of each row.
//! 3. **Closed-candle detection**: this endpoint excludes the currently
//!    forming candle itself — the fixture's last row closes strictly
//!    before its own capture instant, with no partial hour after it. No
//!    [`senken_series::Clock`] is needed as a result, unlike every other
//!    source in this plugin set.
//! 4. **Row cap**: not established this session — the three-day window
//!    requested came back in full (71 rows, one short of 72 because the
//!    forming hour is excluded) with no sign of truncation, but nothing
//!    wider was tried. [`MAX_ROWS`] is therefore a conservative, commented
//!    assumption, not a tested figure — see its own doc comment.
//! 5. **Pagination direction**: **two endpoints exist, and the obvious one
//!    is the trap.** `/exchange/public/md/v2/kline` is documented (task
//!    brief, not tested directly here — testing the broken one would have
//!    spent this source's one live request proving a negative) to
//!    silently misbehave; `/kline/list`, used here, requires explicit
//!    `from`/`to` unix seconds and rejects a bare `limit` with code 30000
//!    rather than guessing. The window requested here came back
//!    correctly bounded.
//!
//! # Prices are already scaled integers, not decimal text
//!
//! Unlike every other source in this workspace, Phemex's kline fields are
//! **not** decimal strings to run through [`senken_core::parse_scaled`] —
//! they are already-scaled integers, written as strings with no decimal
//! point at all (`"782460000"`). Each field's raw text is parsed as a
//! plain `i64` and used verbatim, at the fixed scales the two sections
//! below establish for price and volume respectively.
//!
//! **The price scale is a single-symbol assumption, not a generic
//! solution.** Phemex's own convention (well known from wider use of this
//! API, not from this session's one fetch) is that the scale is
//! per-symbol — this crate's own `api.rs` does not currently capture that
//! per-symbol `priceScale` from the product catalogue at all, and the
//! products fixture already checked into this crate carries no such field
//! for `BTCUSD` either. A scale of `4` was inferred here by comparing this
//! row's raw integer against BTC's simultaneously observed real price on
//! other venues in this same recording session (about $78,000:
//! `782460000 / 10^4 = 78246.0`) — evidence, not documentation, but
//! evidence for `BTCUSD` specifically, and applied only implicitly, by
//! using every price field's raw digits verbatim rather than rescaling
//! them. **This source will silently misprice any other Phemex perpetual
//! whose `priceScale` differs**, and that is this module's most important
//! open gap: a real implementation needs `priceScale` sourced per symbol
//! from the product catalogue, which is a follow-up, not something this
//! session could safely guess at for every listed contract.
//!
//! [`Volume`] is split the same way inverse contracts are conventionally
//! split: the `volume` field counts whole USD-denominated contracts (one
//! contract is $1 on `BTCUSD`), so it is the *quote*-asset amount;
//! `turnover` is the base-asset (BTC) amount. This was checked against the
//! fixture, not assumed: `turnover / volume` for the first row is close to
//! the row's own price, which only holds if `turnover` is priced in BTC
//! and `volume` in USD.
//!
//! # Only one interval is offered
//!
//! This recording session could make exactly one live request (see the
//! task constraints this module was written under), so only `resolution=3600`
//! (one hour) is offered; Phemex's other documented resolutions are
//! unverified here and deliberately left out rather than guessed at.

use senken_core::{TimeRange, UnixNanos};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::VenueClient;
use serde::Deserialize;

const KLINE_LIST_URL: &str = "https://api.phemex.com/exchange/public/md/v2/kline/list";

/// Not independently tested this session — see the module docs' point 4.
/// Chosen as a round, conservative figure comfortably above the 71 rows a
/// real three-day hourly window actually returned, never as a claim about
/// where this endpoint truncates.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, not a
/// venue-documented number, matching every other bar source here.
const CANDLES_FETCH_COST: u32 = 5;

/// The one verified `(step, unit, resolution-in-seconds)` mapping — see the
/// module docs on why only one is offered.
const INTERVAL: (u32, BarUnit, &str) = (1, BarUnit::Hour, "3600");

fn supported_specs() -> Vec<BarSpec> {
    vec![BarSpec::new(INTERVAL.0, INTERVAL.1)]
}

/// Phemex's `resolution` string (seconds) for `spec`, or `None` when
/// `spec` is not the one interval this source has verified.
fn interval_of(spec: BarSpec) -> Option<&'static str> {
    (spec.step.get() == INTERVAL.0 && spec.unit == INTERVAL.1).then_some(INTERVAL.2)
}

/// One row: `[timestamp, interval, lastClose, open, high, low, close,
/// volume, turnover, symbol]`. `lastClose` and `symbol` are read but not
/// carried into [`Bar`] — `lastClose` exists only to let a reader confirm
/// candle continuity, which this source's own tests do once.
type RawRow = (
    i64,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

#[derive(Debug, Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: KlineData,
}

#[derive(Debug, Default, Deserialize)]
struct KlineData {
    #[serde(default)]
    rows: Vec<RawRow>,
}

/// Phemex perpetual bars, fetched through a [`VenueClient`]. Closure comes
/// entirely from the endpoint itself excluding the forming candle — see
/// the module docs — so no [`senken_series::Clock`] is taken here.
#[derive(Debug, Clone)]
pub struct PhemexPerpBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl PhemexPerpBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// `from`/`to` are both explicit, required unix seconds on this
    /// endpoint — see the module docs' point 5.
    fn kline_url(&self, symbol: &str, resolution: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let from = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let to = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?symbol={symbol}&resolution={resolution}&from={from}&to={to}",
            self.url,
        )
    }
}

/// The Phemex perpetual bar source, registered under [`crate::PERP_ID`].
#[must_use]
pub fn bar_source_perp(client: VenueClient) -> PhemexPerpBarSource {
    PhemexPerpBarSource {
        url: KLINE_LIST_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for PhemexPerpBarSource {
    fn source_id(&self) -> &str {
        crate::PERP_ID
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
        let resolution = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;

        let url = self.kline_url(symbol.as_str(), resolution, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let envelope: Envelope = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if envelope.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                envelope.code, envelope.msg
            )));
        }

        let mut bars = Vec::with_capacity(envelope.data.rows.len());
        let mut outside = 0usize;
        for (ts_secs, _interval, _last_close, open, high, low, close, volume, turnover, _symbol) in
            envelope.data.rows
        {
            let ts_open = UnixNanos::from_secs(ts_secs)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_secs}s overflowed")))?;
            if !range.contains(ts_open) {
                outside += 1;
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: raw_int(&open)?,
                high: raw_int(&high)?,
                low: raw_int(&low)?,
                close: raw_int(&close)?,
                // See the module docs on why `turnover` (BTC) is the base
                // amount and `volume` (contracts, $1 each) the quote one —
                // the reverse of their field order.
                volume: Volume::Real(raw_int(&turnover)?),
                quote_volume: Some(raw_int(&volume)?),
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard, in this same workspace, for why an
        // answer made entirely of rows outside the requested range is
        // reported rather than swallowed. This endpoint is documented
        // (module docs, point 5) to honour explicit `from`/`to`, so this
        // is a defensive backstop rather than a known trap here — the same
        // discipline Gate itself applies despite its own pagination being
        // reliable.
        if bars.is_empty() && outside > 0 {
            return Err(SourceError::rejected(format!(
                "answered with {outside} bars, none inside the requested range — \
                 the range parameters were not honoured"
            )));
        }

        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses `raw` — already a scaled integer's exact digits, never a decimal
/// string — as a plain `i64`. See the module docs on why this is not
/// [`senken_core::parse_scaled`].
fn raw_int(raw: &str) -> Result<i64, SourceError> {
    raw.parse()
        .map_err(|_| SourceError::decode(format!("{raw:?} is not an integer")))
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

    use super::bar_source_perp;

    /// A real `GET kline/list?symbol=BTCUSD&resolution=3600` response,
    /// recorded 2026-09-02: 71 hourly rows, none of them the currently
    /// forming candle.
    const KLINE: &[u8] = include_bytes!("../tests/fixtures/kline_1h.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("BTCUSD")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_073_200).unwrap(),
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

    #[tokio::test]
    async fn every_row_the_venue_sends_is_kept_none_are_still_forming() {
        // Unlike every other source in this plugin set, this endpoint
        // excludes the forming candle itself, so no clock-based filtering
        // happens here — this asserts nothing was dropped that should not
        // have been.
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(bars.len(), 71);
    }

    #[tokio::test]
    async fn prices_are_read_as_pre_scaled_integers_not_decimal_text() {
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // Row 0: open 782460000, high 782460000, low 780556000,
        // close 780556000 — used verbatim, at this symbol's own scale of 4
        // (see the module docs' caveat on why that is not a general rule).
        assert_eq!(first.open, 782_460_000);
        assert_eq!(first.high, 782_460_000);
        assert_eq!(first.low, 780_556_000);
        assert_eq!(first.close, 780_556_000);
    }

    #[tokio::test]
    async fn volume_and_turnover_are_swapped_into_base_and_quote() {
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let first = &bars[0];
        // `turnover` (12718939112, BTC) is the base amount; `volume`
        // (9939402, USD contracts) is the quote amount — the reverse of
        // the field order in the response.
        assert!(matches!(first.volume, Volume::Real(v) if v == 12_718_939_112));
        assert_eq!(first.quote_volume, Some(9_939_402));
    }

    #[tokio::test]
    async fn each_rows_open_is_close_to_the_previous_rows_close() {
        // The independent check that field order was not scrambled: on a
        // continuous venue the open of one hour is close to the last
        // hour's close.
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        for pair in bars.windows(2) {
            let gap = (pair[1].open - pair[0].close).abs();
            assert!(
                gap < 1_000_000,
                "open {} does not roughly continue from close {}",
                pair[1].open,
                pair[0].close
            );
        }
    }

    #[tokio::test]
    async fn timestamps_are_read_as_seconds_and_land_an_hour_apart() {
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_073_200).unwrap()
        );
        assert_eq!(
            bars[1].ts_open.as_nanos() - bars[0].ts_open.as_nanos(),
            3_600 * 1_000_000_000
        );
    }

    #[tokio::test]
    async fn rows_outside_the_requested_range_are_dropped() {
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));
        // Only the fixture's second row (1_788_076_800) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_076_800).unwrap(),
            UnixNanos::from_secs(1_788_080_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_076_800).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let server = serving(KLINE).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));
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
        let body = br#"{"code":0,"msg":"OK","data":{"total":-1,"rows":[]}}"#;
        let server = serving(body).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[tokio::test]
    async fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":30000,"msg":"limit not allowed","data":{"total":-1,"rows":[]}}"#;
        let server = serving(body).await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));

        let error = source
            .bars(&symbol(), hour(), wide_range())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("30000"));
    }

    #[test]
    fn a_spec_this_venue_is_not_verified_for_has_no_interval_string() {
        assert!(super::interval_of(BarSpec::new(1, BarUnit::Minute)).is_none());
        assert!(super::interval_of(BarSpec::new(4, BarUnit::Hour)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_an_interval_string() {
        let source = bar_source_perp(test_client());
        for spec in source.supported() {
            assert!(super::interval_of(*spec).is_some());
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source_perp(test_client()).with_url(format!("{}/kline", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_073_200).unwrap(),
            UnixNanos::from_secs(1_788_073_200).unwrap(),
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
