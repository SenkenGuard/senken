//! Kraken Futures candles — `GET /api/charts/v1/trade/{product}/{resolution}`.
//!
//! # A different API from spot's, on a different host
//!
//! Kraken's spot candles come from `api.kraken.com/0/public/OHLC` as
//! positional arrays inside a result keyed by the pair's canonical name.
//! Its futures candles come from `futures.kraken.com` as named objects
//! under `candles`. Recorded live 2026-09-02:
//!
//! ```json
//! {"candles":[{"time":1788242400000,"open":"79167","high":"79167","low":"78729","close":"78729","volume":"133.1038"}]}
//! ```
//!
//! `time` is epoch **milliseconds** — spot's is seconds — and the
//! resolution is a path segment (`1h`), not a query parameter in minutes.
//!
//! # No closed flag
//!
//! This endpoint returns the forming candle along with the finished ones,
//! so a [`Clock`] decides which to keep, the same way this workspace's
//! other flagless sources do.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

/// The charts host — spot lives on `api.kraken.com`.
const CHARTS_URL: &str = "https://futures.kraken.com/api/charts/v1/trade";

/// This workspace's conservative proactive budget.
const FETCH_COST: u32 = 5;

/// The `from` parameter takes whole seconds.
const NANOS_PER_SEC: i64 = 1_000_000_000;

/// Rows this source will accept from one call. Not a venue-documented
/// ceiling: the venue's own cap was not established, and a page larger
/// than this is truncated by the loader rather than trusted whole.
const MAX_ROWS: usize = 5000;

/// Every `(step, unit, path segment)` this source has verified. The
/// resolution is a path segment on this API, so an unverified one would
/// be a 404 rather than a silently different width — but it is still
/// enumerated rather than formatted, for the same reason every other
/// source here enumerates.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (15, BarUnit::Minute, "15m"),
    (1, BarUnit::Hour, "1h"),
    (4, BarUnit::Hour, "4h"),
    (1, BarUnit::Day, "1d"),
];

fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|(step, unit, _)| BarSpec::new(*step, *unit))
        .collect()
}

fn resolution_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|(step, unit, _)| *step == spec.step.get() && *unit == spec.unit)
        .map(|(_, _, resolution)| *resolution)
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    candles: Vec<RawCandle>,
}

#[derive(Debug, Deserialize)]
struct RawCandle {
    /// Epoch milliseconds — spot's own field is seconds.
    time: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    #[serde(default)]
    volume: String,
}

/// Kraken Futures bars.
#[derive(Clone)]
pub(crate) struct KrakenFuturesBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for KrakenFuturesBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KrakenFuturesBarSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl KrakenFuturesBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the Kraken Futures bar source.
#[must_use]
pub(crate) fn bar_source_futures(
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> KrakenFuturesBarSource {
    KrakenFuturesBarSource {
        url: CHARTS_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for KrakenFuturesBarSource {
    fn source_id(&self) -> &str {
        crate::FUTURES_ID
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
        let resolution = resolution_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let from = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let url = format!("{}/{}/{resolution}?from={from}", self.url, symbol.as_str());
        let body = self.client.get(&url, FETCH_COST).await?;
        let response: Response = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        let rows = response.candles;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.volume.as_str()));

        let now = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_open = UnixNanos::from_millis(row.time).ok_or_else(|| {
                SourceError::decode(format!("open time {}ms overflowed", row.time))
            })?;
            if !range.contains(ts_open) {
                continue;
            }
            let Some(ts_close) = ts_open.as_nanos().checked_add(width) else {
                continue;
            };
            if ts_close > now {
                continue;
            }
            bars.push(Bar {
                ts_open,
                open: at(&row.open, price_scale)?,
                high: at(&row.high, price_scale)?,
                low: at(&row.low, price_scale)?,
                close: at(&row.close, price_scale)?,
                volume: Volume::Real(at(&row.volume, qty_scale)?),
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

fn at(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use super::bar_source_futures;
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A real `charts/v1/trade/PF_XBTUSD/1h` response, recorded
    /// 2026-09-02.
    const CHARTS: &[u8] = include_bytes!("../tests/fixtures/charts_1h_futures.json");

    #[derive(Debug)]
    struct FixedClock(i64);
    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_millis(self.0).unwrap()
        }
        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    fn client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn serving() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CHARTS, "application/json"))
            .mount(&server)
            .await;
        server
    }

    fn range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_788_240_000_000).unwrap(),
            UnixNanos::from_millis(1_788_400_000_000).unwrap(),
        )
        .unwrap()
    }

    /// The named-object shape spot's positional parser cannot read, from
    /// a different host entirely.
    #[tokio::test]
    async fn a_futures_page_decodes_to_bars() {
        let server = serving().await;
        let source = bar_source_futures(client(), Arc::new(FixedClock(1_788_400_000_000)))
            .with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("PF_XBTUSD"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap();

        assert!(!bars.is_empty());
        assert_eq!(bars[0].open, 79_167);
        assert_eq!(bars[0].close, 78_729);
        assert!(matches!(bars[0].volume, Volume::Real(v) if v > 0));
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
    }

    /// `time` is milliseconds here and seconds on spot. Reading it as
    /// seconds would date every candle fifty-six thousand years out.
    #[tokio::test]
    async fn the_open_time_is_read_as_milliseconds() {
        let server = serving().await;
        let source = bar_source_futures(client(), Arc::new(FixedClock(1_788_400_000_000)))
            .with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("PF_XBTUSD"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap();

        assert_eq!(bars[0].ts_open.as_millis(), 1_788_242_400_000);
    }

    /// This endpoint returns the forming candle too, so the clock has to
    /// exclude it.
    #[tokio::test]
    async fn the_forming_candle_is_excluded_by_the_clock() {
        let server = serving().await;
        // Part-way through the fixture's second hour.
        let source = bar_source_futures(client(), Arc::new(FixedClock(1_788_248_000_000)))
            .with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("PF_XBTUSD"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap();

        assert_eq!(bars.len(), 1, "only the hour that had finished by then");
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_silently_substituted() {
        let server = serving().await;
        let source = bar_source_futures(client(), Arc::new(FixedClock(1_788_400_000_000)))
            .with_url(server.uri());

        assert!(
            source
                .bars(
                    &SourceSymbol::assume("PF_XBTUSD"),
                    BarSpec::new(7, BarUnit::Minute),
                    range()
                )
                .await
                .is_err()
        );
    }
}
