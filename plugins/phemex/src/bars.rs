//! Phemex bar fetching — `GET /exchange/public/md/v2/kline/list`.
//!
//! One endpoint serves both of this plugin's sources: `kline/list` takes a
//! single `symbol`, and Phemex's leading `s` marks a spot one
//! (`sBTCUSDT`) apart from a perpetual (`BTCUSD`). What differs between
//! them is not the request but how the numbers in the answer are written
//! — see below.
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
//! # Three number shapes on one endpoint, told apart per symbol
//!
//! `kline/list` answers in whichever shape the symbol asked for uses, and
//! the only thing that says which is that symbol's `priceScale` in the
//! product list (see [`crate::scales`]). Recorded live 2026-09-02, same
//! endpoint, same hour, three symbols:
//!
//! ```text
//! BTCUSD    priceScale 4  ->  "791301000"        inverse perpetual
//! sBTCUSDT  priceScale 8  ->  "7917979000000"    spot
//! BTCUSDT   priceScale 0  ->  "79149.9"          linear perpetual, plain decimal
//! ```
//!
//! A scale of zero is not a missing value: it is the venue saying this
//! symbol is written the way every other venue writes prices. Both paths
//! are taken here, chosen by the catalogue rather than by market type —
//! six *spot* symbols use scale 4 while 1012 use scale 8, so even "spot"
//! is not a safe thing to branch on.
//!
//! # Which column is the base volume
//!
//! The row is `[ts, interval, lastClose, open, high, low, close, volume,
//! turnover, symbol]`, and what `volume` and `turnover` mean depends on
//! the family:
//!
//! - **Inverse perpetual**: `volume` counts $1 contracts — a quote-asset
//!   figure — and `turnover` is the base asset at the symbol's own
//!   `ratioScale`. Verified numerically: `10129906427` at `10^8` is
//!   101.299 BTC, which at that row's close of ~$78,700 is $7.97M,
//!   matching its 7,989,441 one-dollar contracts.
//! - **Spot**: `volume` is the base asset at the symbol's quantity scale
//!   and `turnover` the quote asset at its price scale. `26069896600` at
//!   `10^8` is 260.699 BTC against a turnover of 20,575,220 USDT.
//! - **Linear perpetual**: both are decimal text.
//!
//! Reading `volume` as a base amount on an inverse perpetual reports a BTC
//! figure some 78,000 times too large, and it looks entirely ordinary on a
//! volume histogram.
//!

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

/// Phemex bars, fetched through a [`VenueClient`]. Closure comes entirely
/// from the endpoint itself excluding the forming candle — see the module
/// docs — so no [`senken_series::Clock`] is taken here.
#[derive(Debug, Clone)]
pub struct PhemexBarSource {
    source_id: &'static str,
    url: String,
    client: VenueClient,
    /// How this symbol's numbers are written. Consulted per request
    /// because the answer differs per symbol, not per market.
    scales: crate::scales::ScaleCatalog,
    supported: Vec<BarSpec>,
}

impl PhemexBarSource {
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
pub fn bar_source_perp(
    client: VenueClient,
    scales: crate::scales::ScaleCatalog,
) -> PhemexBarSource {
    PhemexBarSource {
        source_id: crate::PERP_ID,
        url: KLINE_LIST_URL.to_owned(),
        client,
        scales,
        supported: supported_specs(),
    }
}

/// The Phemex spot bar source, registered under [`crate::SPOT_ID`]. Same
/// endpoint, same code — only the symbol's own scales differ.
#[must_use]
pub fn bar_source_spot(
    client: VenueClient,
    scales: crate::scales::ScaleCatalog,
) -> PhemexBarSource {
    PhemexBarSource {
        source_id: crate::SPOT_ID,
        url: KLINE_LIST_URL.to_owned(),
        client,
        scales,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for PhemexBarSource {
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
        let resolution = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;

        let scales = self.scales.get(symbol.as_str()).await?;
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

            let (base, quote) = if scales.is_decimal() {
                // Ordinary decimal text: `volume` is the base amount and
                // `turnover` the quote one, as on every other venue.
                (
                    decimal(&volume, DECIMAL_QTY_SCALE)?,
                    decimal(&turnover, DECIMAL_PRICE_SCALE)?,
                )
            } else if scales.quantity > 0 {
                // Spot: `volume` is the base asset at its own quantity
                // scale, `turnover` the quote asset at the price scale.
                (raw_int(&volume)?, raw_int(&turnover)?)
            } else {
                // Inverse perpetual: `turnover` is the base asset and
                // `volume` a count of one-dollar contracts.
                (raw_int(&turnover)?, raw_int(&volume)?)
            };

            bars.push(Bar {
                ts_open,
                open: price(&open, scales)?,
                high: price(&high, scales)?,
                low: price(&low, scales)?,
                close: price(&close, scales)?,
                volume: Volume::Real(base),
                quote_volume: Some(quote),
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

/// Scale a decimal-family quantity is stored at.
///
/// Phemex's linear perpetuals write sizes like `"129.745"`, and a `Bar`
/// carries no scale of its own — the series it joins does — so every row
/// in a page has to land on one. Eight digits is this workspace's usual
/// quantity scale and comfortably finer than the `qtyStepSize` of `0.001`
/// this family publishes; a size finer than that is refused rather than
/// rounded.
const DECIMAL_QTY_SCALE: u8 = 8;

/// One price field, read the way this symbol writes them.
fn price(raw: &str, scales: crate::scales::Scales) -> Result<i64, SourceError> {
    if scales.is_decimal() {
        decimal(raw, DECIMAL_PRICE_SCALE)
    } else {
        raw_int(raw)
    }
}

/// Scale a decimal-family price is stored at. Phemex's linear perpetuals
/// publish a `tickSize` of `0.1`, so four digits is far finer than the
/// venue quotes and leaves no rounding to do.
const DECIMAL_PRICE_SCALE: u8 = 4;

/// Parses `raw` as decimal text at exactly `scale` fractional digits,
/// refusing anything finer rather than rounding it.
fn decimal(raw: &str, scale: u8) -> Result<i64, SourceError> {
    senken_core::parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{bar_source_perp, bar_source_spot};
    use crate::scales::ScaleCatalog;

    /// Real `kline/list` responses recorded 2026-09-02, one per number
    /// shape this venue uses — the same endpoint and the same hour, asked
    /// about three different symbols.
    const KLINE_PERP: &[u8] = include_bytes!("../tests/fixtures/kline_1h.json");
    const KLINE_SPOT: &[u8] = include_bytes!("../tests/fixtures/kline_1h_spot.json");
    const KLINE_LINEAR: &[u8] = include_bytes!("../tests/fixtures/kline_1h_linear.json");
    const PRODUCTS: &[u8] = include_bytes!("../tests/fixtures/products.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
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

    /// A stand-in serving both documents this source now reads: the klines
    /// asked for, and the product list that says how to read them.
    async fn serving(klines: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/public/products"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(PRODUCTS, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/kline"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(klines, "application/json"))
            .mount(&server)
            .await;
        server
    }

    fn catalog(server: &MockServer) -> ScaleCatalog {
        ScaleCatalog::new(test_client()).with_url(format!("{}/public/products", server.uri()))
    }

    #[tokio::test]
    async fn every_row_the_venue_sends_is_kept_none_are_still_forming() {
        // Unlike every other source in this plugin set, this endpoint
        // excludes the forming candle itself, so no clock-based filtering
        // happens here — this asserts nothing was dropped that should not
        // have been.
        let server = serving(KLINE_PERP).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&SourceSymbol::assume("BTCUSD"), hour(), wide_range())
            .await
            .unwrap();

        assert_eq!(bars.len(), 71);
    }

    /// An inverse perpetual: `priceScale` 4, so the digits are used as
    /// they arrive.
    #[tokio::test]
    async fn an_inverse_perpetual_keeps_its_pre_scaled_price_digits() {
        let server = serving(KLINE_PERP).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&SourceSymbol::assume("BTCUSD"), hour(), wide_range())
            .await
            .unwrap();

        let first = &bars[0];
        assert_eq!(first.open, 782_460_000);
        assert_eq!(first.high, 782_460_000);
        assert_eq!(first.low, 780_556_000);
        assert_eq!(first.close, 780_556_000);
    }

    /// On an inverse perpetual `turnover` is the base asset and `volume`
    /// counts one-dollar contracts — the reverse of what their names
    /// suggest. Reading them the other way round reports a BTC figure
    /// some 78,000 times too large.
    #[tokio::test]
    async fn an_inverse_perpetual_takes_its_base_volume_from_turnover() {
        let server = serving(KLINE_PERP).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&SourceSymbol::assume("BTCUSD"), hour(), wide_range())
            .await
            .unwrap();

        let first = &bars[0];
        let Volume::Real(base) = first.volume else {
            panic!("this venue always reports a size");
        };
        let quote = first.quote_volume.expect("contracts are the quote figure");

        // `turnover` at 10^8 is BTC; `volume` counts dollars. Multiplying
        // the first by the row's own close must land on the second, which
        // only holds if the two were not swapped.
        let btc_hundred_millionths = i128::from(base);
        let close_ten_thousandths = i128::from(first.close);
        let dollars = btc_hundred_millionths * close_ten_thousandths / 1_000_000_000_000;
        let reported = i128::from(quote);
        assert!(
            (dollars - reported).abs() * 100 < reported,
            "turnover x close = {dollars} must match the {reported} one-dollar contracts \
             within a percent; swapping the two columns misses by ~78,000x"
        );
    }

    /// Spot: `priceScale` 8, and here `volume` really is the base asset
    /// while `turnover` is the quote one.
    #[tokio::test]
    async fn a_spot_symbol_reads_at_its_own_larger_scale() {
        let server = serving(KLINE_SPOT).await;
        let source = bar_source_spot(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&SourceSymbol::assume("sBTCUSDT"), hour(), wide_range())
            .await
            .unwrap();

        let first = &bars[0];
        assert_eq!(first.open, 7_918_397_000_000, "8 fractional digits, not 4");
        assert_eq!(first.volume, Volume::Real(26_069_896_600));
        assert_eq!(first.quote_volume, Some(2_057_522_020_148_435));
    }

    /// A linear perpetual publishes `priceScale: 0`, which means its
    /// fields are ordinary decimal text — the same endpoint, a different
    /// shape. Reading `"79144.9"` as an integer fails outright; reading it
    /// at the wrong scale would not.
    #[tokio::test]
    async fn a_linear_perpetual_is_read_as_decimal_text() {
        let server = serving(KLINE_LINEAR).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        let bars = source
            .bars(&SourceSymbol::assume("BTCUSDT"), hour(), wide_range())
            .await
            .unwrap();

        let first = &bars[0];
        // "79144.9" at this family's four-digit scale.
        assert_eq!(first.open, 791_449_000);
        assert_eq!(first.high, 791_449_000);
        // "129.745" base at eight digits.
        assert_eq!(first.volume, Volume::Real(12_974_500_000));
    }

    /// The catalogue is what tells the two apart, so a symbol it does not
    /// describe is refused rather than read at a guessed scale.
    #[tokio::test]
    async fn a_symbol_absent_from_the_product_list_is_refused() {
        let server = serving(KLINE_PERP).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        assert!(
            source
                .bars(&SourceSymbol::assume("NOTLISTED"), hour(), wide_range())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_silently_substituted() {
        let server = serving(KLINE_PERP).await;
        let source = bar_source_perp(test_client(), catalog(&server))
            .with_url(format!("{}/kline", server.uri()));

        assert!(
            source
                .bars(
                    &SourceSymbol::assume("BTCUSD"),
                    BarSpec::new(5, BarUnit::Minute),
                    wide_range()
                )
                .await
                .is_err()
        );
    }
}
