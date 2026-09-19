//! OKX bar fetching — `GET /api/v5/market/history-candles`.
//!
//! Parsing the response body — row shape, closed-candle detection, the
//! fixed-point scale batching, the pagination direction — lives in
//! `okx-core`, shared with `wasm/`'s `okx-venue` component: this module is
//! the native HTTP/`BarSource` wrapper around
//! [`okx_core::parse_history_candles`], not a second implementation of it.
//! See that crate's own module docs for the cross-venue traps (sort
//! direction, timestamp shape, `confirm`, the 100-row cap, the `after`/
//! `before` inversion) it documents once for both callers.
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

use okx_core::{BarSpec as CoreBarSpec, BarUnit as CoreBarUnit};
use senken_core::TimeRange;
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::VenueClient;

const HISTORY_CANDLES_URL: &str = "https://www.okx.com/api/v5/market/history-candles";

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call. OKX's public endpoints send no rate-limit headers to
/// reconcile against, so this is purely this project's own,
/// deliberately conservative proactive budget, not a venue-documented
/// number — the same value every venue's own bar source in this workspace
/// uses for the same reason (see e.g. `senken-plugin-binance`'s
/// `KLINES_FETCH_COST`), so the difference between venues is never mistaken
/// for a claim about their relative real cost.
const CANDLES_FETCH_COST: u32 = 5;

/// `senken_series::BarSpec` -> `okx_core::BarSpec`, the boundary crossing
/// every call into `okx-core` makes so that crate never has to depend on
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
        // `okx_interval` already rejects (`None`) rather than guessed.
        _ => CoreBarUnit::Month,
    };
    CoreBarSpec {
        step: spec.step.get(),
        unit,
    }
}

/// The specs this source maps to an OKX `bar` string — see
/// `okx_core::supported_bar_specs`'s own docs for provenance.
fn supported_specs() -> Vec<BarSpec> {
    okx_core::supported_bar_specs()
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

/// Maps a [`SourceError`] onto `okx_core`'s decode/rejected split.
fn source_error(error: okx_core::CoreError) -> SourceError {
    match error {
        okx_core::CoreError::Decode(message) => SourceError::decode(message),
        okx_core::CoreError::Rejected(message) => SourceError::rejected(message),
    }
}

/// OKX bars, fetched through a [`VenueClient`]. Closure is determined
/// entirely from the response's own `confirm` field — no
/// [`senken_series::Clock`] is needed, unlike Binance.
#[derive(Debug, Clone)]
pub struct OkxBarSource {
    source_id: &'static str,
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

    /// Builds the request URL for one `bars()` call — the pagination
    /// query itself comes from `okx_core::history_candles_query`, the one
    /// place either this native source or the wasm component spells out
    /// OKX's `after`/`before` inversion.
    fn candles_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        format!(
            "{}?{}",
            self.url,
            okx_core::history_candles_query(symbol, interval, range)
        )
    }
}

/// OKX bars for `source_id`.
///
/// One endpoint serves every market: it addresses by `instId` and takes no
/// market in its path, so spot, perpetual and dated-future candles come
/// back from the same call with the same row shape. Confirmed live
/// 2026-09-02 by fetching all three.
#[must_use]
pub fn bar_source(source_id: &'static str, client: VenueClient) -> OkxBarSource {
    OkxBarSource {
        source_id,
        url: HISTORY_CANDLES_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for OkxBarSource {
    fn source_id(&self) -> &str {
        self.source_id
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        okx_core::HISTORY_CANDLES_MAX_ROWS as usize
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
        let interval = okx_core::okx_interval(core_spec(spec))
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let url = self.candles_url(symbol.as_str(), &interval, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let candles = okx_core::parse_history_candles(&body, range).map_err(source_error)?;

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
                // Neither reported by this endpoint.
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
        let source = bar_source(crate::SPOT_ID, test_client()).with_url(server.uri());
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
        let source = bar_source(crate::SPOT_ID, test_client());
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
        let source = bar_source(crate::SPOT_ID, test_client());
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
            okx_core::okx_interval(super::core_spec(BarSpec::new(1, BarUnit::Day))).as_deref(),
            Some("1Dutc")
        );
        assert_eq!(
            okx_core::okx_interval(super::core_spec(BarSpec::new(1, BarUnit::Week))).as_deref(),
            Some("1Wutc")
        );
    }

    #[test]
    fn max_rows_is_the_history_endpoints_tested_cap() {
        let source = bar_source(crate::SPOT_ID, test_client());
        assert_eq!(source.max_rows(), 100);
    }

    #[test]
    fn source_id_is_the_spot_market() {
        let source = bar_source(crate::SPOT_ID, test_client());
        assert_eq!(source.source_id(), "okx-spot");
    }
}
