//! Binance spot bar fetching — `GET /api/v3/klines`.
//!
//! # Cross-venue traps
//!
//! 1. **Sort direction**: ascending by open time, as Binance itself
//!    returns it — this implementation still re-sorts defensively rather
//!    than trusting that to hold forever.
//! 2. **Timestamps**: JSON numbers (milliseconds), not strings.
//! 3. **Closed-candle detection**: Binance sets no flag at all. The last
//!    row is closed only once its close time (field 6) has passed —
//!    compared against [`Clock::now`], never the wall clock directly.
//! 4. **Row cap**: the *tested* cap is 1000 on spot (`limit=1500` returns
//!    HTTP 200 with 1000 rows, silently truncated) — the tested number is
//!    used, never the documented one.
//! 5. **Pagination**: `startTime`/`endTime`, both sent as inclusive
//!    milliseconds; Binance's own walk is a forward, ascending one.
//!
//! `symbol` is a [`senken_marketdata::SourceSymbol`]
//! : Binance's own wire format happens to already equal its
//! normalised symbol (`BTCUSDT` either way), which is presumably why the
//! plan's illustrative signature never had to say which one it meant — but
//! this source still takes the typed, venue-native form like every other
//! [`senken_plugin::BarSource`] implementation, so a caller cannot pass the
//! wrong one just because this particular venue would not have noticed.

use std::sync::Arc;

use async_trait::async_trait;
use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock};
use senken_venue::{VenueClient, common_scale};
use serde::de::IgnoredAny;

use crate::SPOT_ID;

const KLINES_URL: &str = "https://api.binance.com/api/v3/klines";

/// Binance spot's tested row cap: `limit=1500` returns HTTP 200
/// with only 1000 rows, no error. An implementation trusting the
/// documented value loses data silently; this is the tested one.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// for one klines call. Bar fetching is the request-hungry traffic this
/// project's rate limiting exists for — a klines page is not
/// free the way a once-a-day instrument fetch is — but no *documented*
/// Binance weight for this endpoint has been fetched and verified
/// only captured the *response* to one call, not a weight table), so this
/// stays a deliberately conservative, our-own-policy number rather than an
/// invented venue fact, matching the reasoning already recorded for
/// `senken_venue::HttpSource`'s `INSTRUMENT_FETCH_COST`.
const KLINES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v3/klines`, decoded positionally: a bare
/// array of 12 fields, mixing numbers and decimal strings. The trailing
/// `IgnoredAny` is field 11, documented by Binance as unused.
type RawKline = (
    i64,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    u32,
    String,
    String,
    IgnoredAny,
);

/// The specs this source maps to a Binance interval string. Only
/// 1-minute has actually been fetched and verified; the rest
/// follow Binance's public, standard interval syntax for a *request
/// parameter* — not a claim about a response shape — so an unsupported
/// combination surfaces immediately as an HTTP error from Binance, never
/// as silently wrong data.
fn supported_specs() -> Vec<BarSpec> {
    vec![
        BarSpec::new(1, BarUnit::Minute),
        BarSpec::new(3, BarUnit::Minute),
        BarSpec::new(5, BarUnit::Minute),
        BarSpec::new(15, BarUnit::Minute),
        BarSpec::new(30, BarUnit::Minute),
        BarSpec::new(1, BarUnit::Hour),
        BarSpec::new(2, BarUnit::Hour),
        BarSpec::new(4, BarUnit::Hour),
        BarSpec::new(6, BarUnit::Hour),
        BarSpec::new(12, BarUnit::Hour),
        BarSpec::new(1, BarUnit::Day),
        BarSpec::new(1, BarUnit::Week),
    ]
}

/// Binance's interval string for `spec`, e.g. `15m`, `1h`, `1d`. `None`
/// when `spec` is not one this source maps ([`supported_specs`]).
fn interval_of(spec: BarSpec) -> Option<String> {
    let suffix = match spec.unit {
        BarUnit::Minute => "m",
        BarUnit::Hour => "h",
        BarUnit::Day => "d",
        BarUnit::Week => "w",
        // `Second` and `Month` are not independently verified for this
        // venue; `BarUnit` is `#[non_exhaustive]`, so a
        // wildcard also catches any future unit this crate has never seen,
        // rather than guessing at it.
        _ => return None,
    };
    Some(format!("{}{suffix}", spec.step))
}

/// Binance spot bars, fetched through a [`VenueClient`] and closed against
/// a [`Clock`] (Binance sends no confirmation flag, so "now" must
/// come from somewhere — see the module docs).
#[derive(Clone)]
pub struct BinanceBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BinanceBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinanceBarSource")
            .field("url", &self.url)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl BinanceBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Binance spot bars.
#[must_use]
pub fn bar_source(client: VenueClient, clock: Arc<dyn Clock>) -> BinanceBarSource {
    BinanceBarSource {
        url: KLINES_URL.to_owned(),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait]
impl BarSource for BinanceBarSource {
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

        // `startTime` is inclusive; `range.end()` is exclusive
        // (`TimeRange`'s own half-open contract), so the last representable
        // millisecond strictly before it is the inclusive `endTime`.
        let url = format!(
            "{}?symbol={symbol}&interval={interval}&limit={MAX_ROWS}&startTime={}&endTime={}",
            self.url,
            range.start().as_millis(),
            range.end().as_millis() - 1,
        );
        let body = self.client.get(&url, KLINES_FETCH_COST).await?;
        let rows: Vec<RawKline> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(
            rows.iter()
                .flat_map(|row| [row.5.as_str(), row.7.as_str(), row.9.as_str()]),
        );

        let now_ms = self.clock.now().as_millis();
        let mut bars = Vec::with_capacity(rows.len());
        for (
            open_ms,
            open,
            high,
            low,
            close,
            volume,
            close_ms,
            quote_volume,
            trades,
            taker,
            _,
            _,
        ) in rows
        {
            // Binance sets no confirmation flag: a candle is
            // closed only once its own close time has passed.
            if close_ms >= now_ms {
                continue;
            }

            let ts_open = UnixNanos::from_millis(open_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {open_ms} overflowed")))?;
            bars.push(Bar {
                ts_open,
                open: scaled(&open, price_scale)?,
                high: scaled(&high, price_scale)?,
                low: scaled(&low, price_scale)?,
                close: scaled(&close, price_scale)?,
                volume: scaled(&volume, qty_scale)?,
                quote_volume: Some(scaled(&quote_volume, qty_scale)?),
                trade_count: Some(trades),
                taker_buy_volume: Some(scaled(&taker, qty_scale)?),
            });
        }

        // Ascending regardless of what the venue returns —
        // Binance already is, but this must not silently rely on that.
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
    use senken_marketdata::{Instrument, SourceSymbol};
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source;

    const KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1m.json");

    /// The only sanctioned way to obtain a [`SourceSymbol`]
    ///  is through an [`Instrument`] — Binance's own wire format happens
    /// to equal its normalised symbol, so both halves of this pair are
    /// `BTCUSDT`.
    fn btcusdt() -> SourceSymbol {
        Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT").source_symbol()
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

    fn test_client(url: &str) -> VenueClient {
        let _ = url;
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::EPOCH,
            UnixNanos::from_millis(4_102_444_800_000).unwrap(),
        )
        .unwrap()
    }

    /// Serves [`KLINES`] — a real, live-captured fixture — from a
    /// mock server and returns a source pointed at it, closed at `now_ms`.
    async fn mock_source(now_ms: i64) -> (MockServer, super::BinanceBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(KLINES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client(&server.uri()), Arc::new(FixedClock(now_ms)))
            .with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_with_correct_ohlcv_and_ascending_order() {
        // The real fixture's last row (open 1788081180000, close
        // 1788081239999) is not yet closed at this clock — chosen well
        // past every other row's close but before the last one's, so this
        // test proves both parsing *and* that the still-forming candle is
        // dropped, using one real fixture rather than a second, synthetic
        // one.
        let (_server, source) = mock_source(1_788_081_200_000).await;
        let bars = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Minute), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 4, "the 5th row is still forming at this clock");
        assert!(
            bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open),
            "must be ascending"
        );

        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_080_940_000).unwrap()
        );
        assert_eq!(first.open, 7_808_429);
        assert_eq!(first.high, 7_808_429);
        assert_eq!(first.low, 7_808_428);
        assert_eq!(first.close, 7_808_428);
        assert_eq!(first.trade_count, Some(225));
        assert!(first.quote_volume.is_some());
        assert!(first.taker_buy_volume.is_some());
    }

    #[tokio::test]
    async fn a_still_forming_candle_is_dropped() {
        // At the fixture's own capture instant every row but the last is
        // long closed; picking a clock before the last row's close time
        // (1788081239999) drops exactly that one row.
        let (_server, source) = mock_source(1_788_081_180_000).await;
        let bars = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Minute), wide_range())
            .await
            .unwrap();
        assert_eq!(bars.len(), 4);
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_081_180_000)
        );
    }

    #[tokio::test]
    async fn once_every_row_has_closed_all_five_are_kept() {
        let (_server, source) = mock_source(4_102_444_800_000).await;
        let bars = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Minute), wide_range())
            .await
            .unwrap();
        assert_eq!(bars.len(), 5);
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_guessed() {
        let source = bar_source(test_client("unused"), Arc::new(FixedClock(0)));
        let error = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Month), wide_range())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::SourceError::Rejected { .. }
        ));
    }

    #[tokio::test]
    async fn an_empty_range_is_never_fetched() {
        let source = bar_source(test_client("unused"), Arc::new(FixedClock(0)));
        let point = UnixNanos::from_millis(0).unwrap();
        let empty = TimeRange::new(point, point).unwrap();
        let bars = source
            .bars(&btcusdt(), BarSpec::new(1, BarUnit::Minute), empty)
            .await
            .unwrap();
        assert!(bars.is_empty());
    }

    #[test]
    fn max_rows_is_the_tested_cap_not_the_documented_one() {
        let source = bar_source(test_client("unused"), Arc::new(FixedClock(0)));
        assert_eq!(source.max_rows(), 1000);
    }

    #[test]
    fn source_id_is_the_spot_market() {
        let source = bar_source(test_client("unused"), Arc::new(FixedClock(0)));
        assert_eq!(source.source_id(), "binance-spot");
    }
}
