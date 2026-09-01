//! Bybit spot bar fetching — `GET /v5/market/kline`.
//!
//! # Cross-venue traps (plus one boundary verified in this
//! session — see below)
//!
//! 1. **Sort direction**: descending by open time (like OKX, opposite of
//!    Binance) — this implementation re-sorts to ascending before
//!    returning.
//! 2. **Timestamps**: JSON strings.
//! 3. **Closed-candle detection**: Bybit sets no confirmation flag either,
//!    but the response's own top-level `time` — server time in
//!    milliseconds — is "useful for closure checks", so this
//!    source compares each row's computed close time
//!    (`ts_open + spec duration`) against `time` rather than needing a
//!    [`senken_series::Clock`] at all.
//! 4. **Row cap**: 1000 — not covered by the earlier capture (which only fetched
//!    `limit=2`), so verified independently this session before relying on
//!    it: `limit=1500` on `BTCUSDT` returns HTTP 200, `retCode 0`, and
//!    exactly 1000 rows — the same silent-truncation shape Binance spot
//!    exhibits, and equally not to be trusted from documentation
//!    alone.
//! 5. **Pagination**: `start`/`end`, milliseconds. Also not covered by
//!    independently verified live: `start=1788081060000&
//!    end=1788081180000` on `BTCUSDT` returned exactly the three rows
//!    opening at `1788081060000`, `1788081120000` and `1788081180000` —
//!    **both ends inclusive**.
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

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const KLINE_URL: &str = "https://api.bybit.com/v5/market/kline";

/// Bybit spot's tested row cap for `/v5/market/kline` (verified this
/// session, module docs): `limit=1500` returns HTTP 200 with exactly 1000
/// rows, the same silent truncation Binance spot exhibits. The
/// tested number, not merely the documented one.
const MAX_ROWS: usize = 1000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this project's own conservative budget, not a
/// venue-documented weight (no weight/rate-limit headers were captured
/// for this endpoint).
const KLINE_FETCH_COST: u32 = 5;

/// One row of `GET /v5/market/kline`: seven positional strings —
/// open time, O, H, L, C, volume, turnover. No trade count.
type RawKline = (String, String, String, String, String, String, String);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KlineResponse {
    ret_code: i64,
    #[serde(default)]
    ret_msg: String,
    #[serde(default)]
    result: KlineResult,
    /// Server time in milliseconds — the "now" this source closes candles
    /// against ("useful for closure checks").
    time: i64,
}

#[derive(Debug, Default, Deserialize)]
struct KlineResult {
    #[serde(default)]
    list: Vec<RawKline>,
}

/// The specs this source maps to a Bybit `interval` string. Only
/// `interval=1` (1 minute) has actually been fetched and verified
/// ; the rest are Bybit's own enumerated, non-arbitrary set of valid
/// intervals for this endpoint — a step outside it has no mapping at all,
/// by construction, rather than being guessed at.
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

/// Bybit's `interval` string for `spec` — `"1"` for one minute, `"60"` for
/// one hour (Bybit counts every sub-day interval in minutes, not hours;
/// `interval="1"` is the one value fetched and verified), `"D"`/
/// `"W"` for a single day/week. `None` for anything outside Bybit's own
/// enumerated set ([`supported_specs`]), including `Month` — Bybit's
/// calendar month has no fixed duration, and this source's closure check
/// needs one (see [`Bar::ts_open`] docs on this module), so `Month` is
/// deliberately never offered rather than fetched with an unverified
/// closure rule.
fn interval_of(spec: BarSpec) -> Option<String> {
    match spec.unit {
        BarUnit::Minute => Some(spec.step.to_string()),
        BarUnit::Hour => Some((spec.step.get() * 60).to_string()),
        BarUnit::Day if spec.step.get() == 1 => Some("D".to_owned()),
        BarUnit::Week if spec.step.get() == 1 => Some("W".to_owned()),
        _ => None,
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
        // `duration_nanos` is `Some` for every spec `interval_of` maps —
        // `Month` is the only `None` case and is never offered above.
        let Some(duration_nanos) = spec.duration_nanos() else {
            return Err(SourceError::rejected(format!(
                "{spec} has no fixed duration to close candles against"
            )));
        };

        // Both ends inclusive, verified independently in this session
        // (module docs) rather than assumed from Bybit's own
        // documentation, per the "verify before use."
        let url = format!(
            "{}?category=spot&symbol={symbol}&interval={interval}&limit={MAX_ROWS}&start={}&end={}",
            self.url,
            range.start().as_millis(),
            range.end().as_millis() - 1,
        );
        let body = self.client.get(&url, KLINE_FETCH_COST).await?;
        let response: KlineResponse = serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if response.ret_code != 0 {
            return Err(SourceError::rejected(format!(
                "retCode {}: {}",
                response.ret_code, response.ret_msg
            )));
        }

        let price_scale = common_scale(response.result.list.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(
            response
                .result
                .list
                .iter()
                .flat_map(|row| [row.5.as_str(), row.6.as_str()]),
        );

        let server_now_ms = response.time;
        let mut bars = Vec::with_capacity(response.result.list.len());
        for (ts, open, high, low, close, volume, turnover) in response.result.list {
            let ts_ms: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms} overflowed")))?;

            // Bybit sets no confirmation flag: a candle is closed only once
            // its computed close time has passed the server's own clock
            //.
            let close_ms = ts_ms
                .checked_add(duration_nanos / 1_000_000)
                .ok_or_else(|| SourceError::decode("close time overflowed"))?;
            if close_ms > server_now_ms {
                continue;
            }
            if !range.contains(ts_open) {
                // Defensive: the query is bounded server-side already, but
                // never trust a venue's pagination boundaries alone.
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: scaled(&open, price_scale)?,
                high: scaled(&high, price_scale)?,
                low: scaled(&low, price_scale)?,
                close: scaled(&close, price_scale)?,
                volume: Volume::Real(scaled(&volume, qty_scale)?),
                quote_volume: Some(scaled(&turnover, qty_scale)?),
                // Never reported (the required test: this must be
                // `None`, never a false `0`).
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // Ascending regardless of what the venue returns —
        // Bybit is descending.
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
            super::interval_of(BarSpec::new(1, BarUnit::Minute)).as_deref(),
            Some("1")
        );
        assert_eq!(
            super::interval_of(BarSpec::new(1, BarUnit::Hour)).as_deref(),
            Some("60")
        );
        assert_eq!(
            super::interval_of(BarSpec::new(1, BarUnit::Day)).as_deref(),
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
