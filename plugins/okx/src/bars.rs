//! OKX bar fetching — `GET /api/v5/market/history-candles`.
//!
//! # Cross-venue traps
//!
//! 1. **Sort direction**: descending by open time (opposite of Binance) —
//!    this implementation re-sorts to ascending before returning.
//! 2. **Timestamps**: JSON strings, including the timestamp itself — "all
//!    strings including the timestamp".
//! 3. **Closed-candle detection**: the `confirm` field — `"1"` closed,
//!    `"0"` still forming — verified present even on the history endpoint
//!    used here.
//! 4. **Row cap**: 100, the tested cap of `/market/history-candles` — used
//!    uniformly (see below), never the 300 the plain `/market/candles`
//!    accepts.
//! 5. **Pagination**: `after=X` returns candles strictly **older** than
//!    `X` (the opposite of what the name suggests); `before=X` is the
//!    newer direction. Both are used together here to bound a request to
//!    exactly one `TimeRange`.
//!
//! # Why `/market/history-candles`, not `/market/candles`
//!
//! OKX splits recent and historical candles across two endpoints
//! . Switching between them by recency would need a "how old is old"
//! decision this crate has no verified answer for, and — since OKX's
//! `confirm` flag is verified present "even on the history endpoint" — the
//! history endpoint alone already serves both backfill and the newest,
//! still-forming candle without ever needing to know what time it is.
//! Using it uniformly, at its lower, verified cap of 100 rather than the
//! other endpoint's 300, is therefore not a loss of capability, only of an
//! optimisation this stage leaves for later.
//!
//! # The anchor
//!
//! OKX's plain `1D`/`1W`/`1M` open at 16:00 UTC (00:00 Hong Kong), not UTC
//! midnight — a real, silent 8-hour shift. This source always requests the
//! `utc` variant (`1Dutc`, `1Wutc`, `1Mutc`) for Day and above, so every
//! bar it ever returns is UTC-anchored and the anchor never needs to reach
//! a store path token (its fallback path) at all.
//!
//! # What `symbol` means here
//!
//! The plan's M7.1 sketch did not say whether `bars`'s `symbol` is the
//! cross-venue normalised form (`Instrument::symbol`, e.g. `BTCUSDT`) or
//! the venue's own identifier (`Instrument::source_symbol()`, e.g.
//! `BTC-USDT`). Reconstructing OKX's dashed `instId` from the normalised
//! form is not generally possible without guessing where the dash goes
//! (`normalise_symbol` deliberately discards separator position), so this
//! implementation takes `symbol` to **be** the venue's own identifier —
//! sent to OKX verbatim as `instId`. This is no longer only a doc comment: a
//! [`senken_marketdata::SourceSymbol`] is obtainable only from
//! `Instrument::source_symbol()`, so a caller that reaches for
//! `Instrument::symbol` instead gets a compile error, not a wrong `instId`.

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

const HISTORY_CANDLES_URL: &str = "https://www.okx.com/api/v5/market/history-candles";

/// The tested cap of `/market/history-candles`: "verified,
/// returned exactly 100 rows". Deliberately not the 300 the sibling
/// `/market/candles` endpoint accepts — see the module docs for why this
/// source never calls that endpoint at all.
const MAX_ROWS: usize = 100;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call. OKX's public endpoints send no rate-limit headers to
/// reconcile against, so this is purely this project's own,
/// deliberately conservative proactive budget, not a venue-documented
/// number — the same value every venue's own bar source in this workspace
/// uses for the same reason (see e.g. `senken-plugin-binance`'s
/// `KLINES_FETCH_COST`), so the difference between venues is never mistaken
/// for a claim about their relative real cost.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v5/market/history-candles`: nine positional
/// strings — open time, O, H, L, C, volume, quote volume, a
/// second quote-volume variant (unused here — see the module docs on A4's
/// own labelling), and `confirm`.
type RawCandle = (
    String,
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
struct CandlesResponse {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Vec<RawCandle>,
}

/// The specs this source maps to an OKX `bar` string. Only 1-minute has
/// actually been fetched and verified; the rest follow OKX's
/// public interval syntax for a *request parameter*, preferring the `utc`
/// variant for Day and above.
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
        BarSpec::new(1, BarUnit::Day),
        BarSpec::new(1, BarUnit::Week),
    ]
}

/// OKX's `bar` string for `spec`, e.g. `15m`, `4H`, `1Dutc`. `None` when
/// `spec` is not one this source maps ([`supported_specs`]).
fn interval_of(spec: BarSpec) -> Option<String> {
    let step = spec.step;
    match spec.unit {
        BarUnit::Minute => Some(format!("{step}m")),
        BarUnit::Hour => Some(format!("{step}H")),
        // Finding F3: always the UTC variant, never the plain `D`/`W`/`M`
        // that opens at 16:00 UTC.
        BarUnit::Day => Some(format!("{step}Dutc")),
        BarUnit::Week => Some(format!("{step}Wutc")),
        BarUnit::Month => Some(format!("{step}Mutc")),
        // `Second` is not offered by OKX's documented interval set, and
        // `BarUnit` is `#[non_exhaustive]`: a wildcard also catches any
        // future unit this crate has never seen, rather than guessing.
        _ => None,
    }
}

/// OKX bars, fetched through a [`VenueClient`]. Closure is determined
/// entirely from the response's own `confirm` field — no
/// [`senken_series::Clock`] is needed, unlike Binance.
#[derive(Debug, Clone)]
pub struct OkxBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl OkxBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `after=X` returns candles strictly **older** than `X`; `before=X`
    /// is the newer direction ("the single most commonly
    /// mis-implemented parameter in this API"). Passing both bounds the
    /// request to exactly this half-open `range` server-side: everything
    /// strictly older than `range.end()` and strictly newer than one
    /// millisecond before `range.start()`.
    fn candles_url(&self, symbol: &str, bar: &str, range: TimeRange) -> String {
        format!(
            "{}?instId={symbol}&bar={bar}&limit={MAX_ROWS}&after={}&before={}",
            self.url,
            range.end().as_millis(),
            range.start().as_millis() - 1,
        )
    }
}

/// OKX bars, spanning every instrument type OKX's candles endpoint serves
/// (it addresses by `instId`, not by a market-specific path) — registered
/// under [`crate::SPOT_ID`] since only a spot symbol has been fetched and
/// verified.
#[must_use]
pub fn bar_source(client: VenueClient) -> OkxBarSource {
    OkxBarSource {
        url: HISTORY_CANDLES_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for OkxBarSource {
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
        let bar = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let url = self.candles_url(symbol.as_str(), &bar, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let response: CandlesResponse =
            serde_json::from_slice(&body).map_err(SourceError::decode)?;
        if response.code != "0" {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                response.code, response.msg
            )));
        }

        let price_scale = common_scale(response.data.iter().flat_map(|row| {
            [
                row.1.as_str(),
                row.2.as_str(),
                row.3.as_str(),
                row.4.as_str(),
            ]
        }));
        let qty_scale = common_scale(
            response
                .data
                .iter()
                .flat_map(|row| [row.5.as_str(), row.6.as_str()]),
        );

        let mut bars = Vec::with_capacity(response.data.len());
        for (ts, open, high, low, close, volume, quote_volume, _quote_volume_variant, confirm) in
            response.data
        {
            // `confirm == "0"` on the newest row, verified present even on
            // this history endpoint: never persist it.
            if confirm != "1" {
                continue;
            }

            let ts_ms: i64 = ts
                .parse()
                .map_err(|_| SourceError::decode(format!("{ts:?} is not a valid timestamp")))?;
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms} overflowed")))?;
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
                quote_volume: Some(scaled(&quote_volume, qty_scale)?),
                // Neither reported by this endpoint.
                trade_count: None,
                taker_buy_volume: None,
            });
        }

        // Ascending regardless of what the venue returns — OKX
        // is descending.
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

    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1m.json");

    /// The only sanctioned way to obtain a [`SourceSymbol`]
    ///  is through an [`Instrument`] — OKX's own wire format is the
    /// dashed `BTC-USDT`, distinct from its normalised `BTCUSDT`.
    fn btc_usdt() -> SourceSymbol {
        Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT").source_symbol()
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

    async fn mock_source() -> (MockServer, super::OkxBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client()).with_url(server.uri());
        (server, source)
    }

    #[tokio::test]
    async fn fixture_rows_decode_with_correct_ohlcv_and_ascending_order_and_drop_the_unconfirmed_row()
     {
        // The real fixture's newest row carries `confirm == "0"`; the
        // other four carry `"1"`.
        let (_server, source) = mock_source().await;
        let bars = source
            .bars(&btc_usdt(), BarSpec::new(1, BarUnit::Minute), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 4, "the unconfirmed newest row must be dropped");
        assert!(
            bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open),
            "must be ascending despite OKX returning descending"
        );
        assert!(
            bars.iter()
                .all(|b| b.ts_open.as_millis() < 1_788_083_280_000),
            "the dropped row was the newest"
        );

        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_083_040_000).unwrap()
        );
        assert_eq!(first.open, 780_343);
        assert_eq!(first.high, 780_401);
        assert_eq!(first.low, 780_342);
        assert_eq!(first.close, 780_401);
        assert_eq!(first.trade_count, None, "OKX does not report a trade count");
        assert_eq!(
            first.taker_buy_volume, None,
            "OKX does not report a taker-buy split"
        );
        assert!(first.quote_volume.is_some());
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_guessed() {
        let source = bar_source(test_client());
        let error = source
            .bars(&btc_usdt(), BarSpec::new(1, BarUnit::Second), wide_range())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            senken_marketdata::SourceError::Rejected { .. }
        ));
    }

    #[test]
    fn the_pagination_cursor_walks_backwards_correctly() {
        // `after=X` must be the range's *end* (the boundary candles must
        // be strictly older than) and `before=X` the range's start minus
        // one millisecond (strictly newer than) — the inversion
        // calls "the single most commonly mis-implemented parameter in
        // this API". Getting these two swapped would silently walk the
        // wrong direction through history while still compiling and often
        // still returning *some* rows.
        let source = bar_source(test_client());
        let range = TimeRange::new(
            UnixNanos::from_millis(1_788_066_600_000).unwrap(),
            UnixNanos::from_millis(1_788_066_720_000).unwrap(),
        )
        .unwrap();
        let url = source.candles_url("BTC-USDT", "1m", range);
        assert!(
            url.contains("after=1788066720000"),
            "after must be the range's end: {url}"
        );
        assert!(
            url.contains("before=1788066599999"),
            "before must be one millisecond before the range's start: {url}"
        );
    }

    #[test]
    fn day_and_above_always_request_the_utc_variant() {
        assert_eq!(
            super::interval_of(BarSpec::new(1, BarUnit::Day)).as_deref(),
            Some("1Dutc")
        );
        assert_eq!(
            super::interval_of(BarSpec::new(1, BarUnit::Week)).as_deref(),
            Some("1Wutc")
        );
    }

    #[test]
    fn max_rows_is_the_history_endpoints_tested_cap() {
        let source = bar_source(test_client());
        assert_eq!(source.max_rows(), 100);
    }

    #[test]
    fn source_id_is_the_spot_market() {
        let source = bar_source(test_client());
        assert_eq!(source.source_id(), "okx-spot");
    }
}
