//! Bybit spot bar fetching — `GET /v5/market/kline`.
//!
//! Parsing the response body — row shape, closed-candle detection against
//! the response's own `time`, the fixed-point scale batching, the sort
//! direction — lives in `bybit-core`, shared with `wasm/`'s `bybit-venue`
//! component: this module is the native HTTP/`BarSource` wrapper around
//! [`bybit_core::parse_klines`], not a second implementation of it. See
//! that crate's own module docs for the cross-venue traps it documents
//! once for both callers, most of them verified live in the same session
//! that first wrote them:
//!
//! 1. **Sort direction**: descending by open time (like OKX, opposite of
//!    Binance).
//! 2. **Timestamps**: JSON strings.
//! 3. **Closed-candle detection**: Bybit sets no confirmation flag either,
//!    but the response's own top-level `time` — server time in
//!    milliseconds — is "useful for closure checks", so
//!    [`bybit_core::parse_klines`] compares each row's computed close time
//!    (`ts_open + spec duration`) against `time` rather than needing a
//!    [`senken_series::Clock`] at all.
//! 4. **Row cap**: 1000 — verified independently this session before
//!    relying on it: `limit=1500` on `BTCUSDT` returns HTTP 200, `retCode
//!    0`, and exactly 1000 rows — the same silent-truncation shape Binance
//!    spot exhibits, and equally not to be trusted from documentation
//!    alone.
//! 5. **Pagination**: `start`/`end`, milliseconds, **both ends inclusive**
//!    — verified live: `start=1788081060000&end=1788081180000` on
//!    `BTCUSDT` returned exactly the three rows opening at
//!    `1788081060000`, `1788081120000` and `1788081180000`.
//!
//! Bybit reports no trade count at all (the required test: this
//! must decode to `None`, never `0`, since `0` would be a false claim that
//! no trades occurred). `turnover` — Bybit's own name for quote-denominated
//! volume — is mapped onto [`senken_series::Bar::quote_volume`].
//!
//! `symbol` is a [`senken_marketdata::SourceSymbol`]
//! , obtainable only from `Instrument::source_symbol()` — Bybit's
//! own wire format happens to equal its normalised symbol (both `BTCUSDT`),
//! but this source still takes the typed, venue-native form like every
//! other [`senken_plugin::BarSource`] implementation.

use bybit_core::{BarSpec as CoreBarSpec, BarUnit as CoreBarUnit};
use senken_core::TimeRange;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::VenueClient;

const KLINE_URL: &str = "https://api.bybit.com/v5/market/kline";

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative budget, not a
/// venue-documented weight (no weight/rate-limit headers were captured
/// for this endpoint).
const KLINE_FETCH_COST: u32 = 5;

/// `senken_series::BarSpec` -> `bybit_core::BarSpec`, the boundary crossing
/// every call into `bybit-core` makes so that crate never has to depend on
/// `senken-series` (a wasm guest has no such crate to name).
fn core_spec(spec: BarSpec) -> CoreBarSpec {
    let unit = match spec.unit {
        BarUnit::Second => CoreBarUnit::Second,
        BarUnit::Minute => CoreBarUnit::Minute,
        BarUnit::Hour => CoreBarUnit::Hour,
        BarUnit::Day => CoreBarUnit::Day,
        BarUnit::Week => CoreBarUnit::Week,
        // `BarUnit` is `#[non_exhaustive]`; a wildcard also catches any
        // future unit this crate has never seen, mapped to a spec
        // `bybit_interval` already rejects (`None`) rather than guessed.
        _ => CoreBarUnit::Month,
    };
    CoreBarSpec {
        step: spec.step.get(),
        unit,
    }
}

/// The specs this source maps to a Bybit `interval` string — see
/// `bybit_core::supported_bar_specs`'s own docs for provenance.
fn supported_specs() -> Vec<BarSpec> {
    bybit_core::supported_bar_specs()
        .into_iter()
        .map(|spec| {
            let unit = match spec.unit {
                CoreBarUnit::Second => BarUnit::Second,
                CoreBarUnit::Minute => BarUnit::Minute,
                CoreBarUnit::Hour => BarUnit::Hour,
                CoreBarUnit::Day => BarUnit::Day,
                CoreBarUnit::Week => BarUnit::Week,
                CoreBarUnit::Month => BarUnit::Month,
            };
            BarSpec::new(spec.step, unit)
        })
        .collect()
}

/// Maps a [`SourceError`] onto `bybit_core`'s decode/rejected split.
fn source_error(error: bybit_core::CoreError) -> SourceError {
    match error {
        bybit_core::CoreError::Decode(message) => SourceError::decode(message),
        bybit_core::CoreError::Rejected(message) => SourceError::rejected(message),
    }
}

/// Bybit spot bars, fetched through a [`VenueClient`]. Closure is
/// determined from the response's own top-level `time` field —
/// no [`senken_series::Clock`] is needed, unlike Binance.
#[derive(Debug, Clone)]
pub struct BybitBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl BybitBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Bybit spot bars.
#[must_use]
pub fn bar_source(client: VenueClient) -> BybitBarSource {
    BybitBarSource {
        url: KLINE_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BybitBarSource {
    fn source_id(&self) -> &str {
        crate::SPOT_ID
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        bybit_core::KLINE_MAX_ROWS as usize
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
        let bybit_spec = core_spec(spec);
        let interval = bybit_core::bybit_interval(bybit_spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let url = format!(
            "{}?{}",
            self.url,
            bybit_core::kline_query(symbol.as_str(), &interval, range)
        );
        let body = self.client.get(&url, KLINE_FETCH_COST).await?;
        let candles = bybit_core::parse_klines(&body, bybit_spec, range).map_err(source_error)?;

        Ok(candles
            .into_iter()
            .map(|candle| Bar {
                ts_open: candle.ts_open,
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                volume: Volume::Real(candle.volume),
                quote_volume: Some(candle.quote_volume),
                // Never reported (the required test: this must be
                // `None`, never a false `0`).
                trade_count: None,
                taker_buy_volume: None,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source;

    const KLINE: &[u8] = include_bytes!("../tests/fixtures/kline_1m.json");

    /// The only sanctioned way to obtain a [`SourceSymbol`]
    ///  is through an [`Instrument`] — Bybit's own wire format happens
    /// to equal its normalised symbol, so both halves of this pair are
    /// `BTCUSDT`.
    fn btcusdt() -> SourceSymbol {
        Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT").source_symbol()
    }

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::EPOCH,
            UnixNanos::from_millis(4_102_444_800_000).unwrap(),
        )
        .unwrap()
    }

    async fn mock_source() -> (MockServer, super::BybitBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(KLINE, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_with_correct_ohlcv_ascending_order_and_no_trade_count() {
        // The real fixture's top-level `time` (1788081203987) is before
        // the newest row's computed close (1788081180000 + 60000 =
        // 1788081240000), so that row is still forming and must be
        // dropped; the other four have already closed.
        let (_server, source) = mock_source().await;
        let bars = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Minute), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 4, "the still-forming newest row is dropped");
        assert!(
            bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open),
            "must be ascending despite Bybit returning descending"
        );
        assert!(
            bars.iter().all(|b| b.trade_count.is_none()),
            "Bybit never reports a trade count: None, never a false 0"
        );

        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_080_940_000).unwrap()
        );
        assert_eq!(first.open, 780_777);
        assert_eq!(first.high, 780_778);
        assert_eq!(first.low, 780_777);
        assert_eq!(first.close, 780_777);
        assert!(
            first.quote_volume.is_some(),
            "turnover maps to quote_volume"
        );
        assert_eq!(first.taker_buy_volume, None);
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_guessed() {
        let source = bar_source(test_client());
        let error = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Month), wide_range())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::SourceError::Rejected { .. }
        ));
    }

    #[test]
    fn interval_counts_hours_in_minutes() {
        assert_eq!(
            bybit_core::bybit_interval(super::core_spec(BarSpec::new(1, BarUnit::Minute)))
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            bybit_core::bybit_interval(super::core_spec(BarSpec::new(1, BarUnit::Hour))).as_deref(),
            Some("60")
        );
        assert_eq!(
            bybit_core::bybit_interval(super::core_spec(BarSpec::new(1, BarUnit::Day))).as_deref(),
            Some("D")
        );
    }

    #[test]
    fn max_rows_is_bybits_documented_page_size() {
        let source = bar_source(test_client());
        assert_eq!(source.max_rows(), 1000);
    }

    #[test]
    fn source_id_is_the_spot_market() {
        let source = bar_source(test_client());
        assert_eq!(source.source_id(), "bybit-spot");
    }
}
