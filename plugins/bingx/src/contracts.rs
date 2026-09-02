//! `BingX`'s perpetual candles and depth.
//!
//! # Two markets, two hosts, one shape between them
//!
//! `BingX` serves its linear and inverse perpetuals from different API
//! roots, and neither answers the shape spot does. Recorded live
//! 2026-09-02:
//!
//! ```json
//! spot     [1788240000000, 77651.07, …]                            positional numbers
//! linear   {"open":"76647.1","close":"76587.4","high":"76672.4","low":"76480.0","volume":"96.9307","time":1788346800000}
//! inverse  {"open":"76603.2","close":"76543.6","high":"76633.2","low":"76441.8","volume":"6876.00","time":1788346800000}
//!
//! swap/v3/quote/klines        linear   symbol=BTC-USDT
//! cswap/v1/market/klines      inverse  symbol=BTC-USD
//! ```
//!
//! Named fields against positional cells, and decimal strings against
//! bare numbers — so this cannot be a parameter on the spot source.
//!
//! # What `volume` means, and why only one market publishes it
//!
//! On the **linear** market `volume` is the base asset: `96.9307` BTC
//! against a close of `$76,587` is $7.4M for an hour, which is the right
//! order of magnitude for this contract. On the **inverse** one it is a
//! contract count — `6876.00` against the same close would be half a
//! billion dollars of bitcoin in an hour, which it is not.
//!
//! So the linear market carries its volume and the inverse one reports
//! [`Volume::Absent`]. That asymmetry is the venue's, not this module's,
//! and inventing a multiplier to make the second look like the first is
//! how a volume histogram ends up confidently wrong.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

/// This project's own panel depth.
const MAX_DEPTH: usize = 20;

/// This workspace's conservative proactive budget.
const FETCH_COST: u32 = 5;

/// Which perpetual market a source serves — they differ in host, and in
/// whether `volume` is a base amount at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Market {
    /// USDT-margined; `volume` is the base asset.
    Linear,
    /// Coin-margined; `volume` is a contract count.
    Inverse,
}

impl Market {
    /// This market's source id, for callers outside this module.
    pub(crate) const fn source_id_public(self) -> &'static str {
        self.source_id()
    }

    const fn source_id(self) -> &'static str {
        match self {
            Self::Linear => crate::LINEAR_ID,
            Self::Inverse => crate::INVERSE_ID,
        }
    }

    fn klines_url(self) -> &'static str {
        match self {
            Self::Linear => "https://open-api.bingx.com/openApi/swap/v3/quote/klines",
            Self::Inverse => "https://open-api.bingx.com/openApi/cswap/v1/market/klines",
        }
    }

    fn depth_url(self) -> &'static str {
        match self {
            Self::Linear => "https://open-api.bingx.com/openApi/swap/v2/quote/depth",
            Self::Inverse => "https://open-api.bingx.com/openApi/cswap/v1/market/depth",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope<T: Default> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: T,
}

impl<T: Default> Envelope<T> {
    fn payload(self) -> Result<T, SourceError> {
        if self.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                self.code, self.msg
            )));
        }
        Ok(self.data)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawCandle {
    open: String,
    high: String,
    low: String,
    close: String,
    #[serde(default)]
    volume: String,
    /// Epoch milliseconds of the candle's open.
    time: i64,
}

/// `BingX` perpetual bars.
#[derive(Clone)]
pub(crate) struct BingxContractBarSource {
    market: Market,
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BingxContractBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BingxContractBarSource")
            .field("market", &self.market)
            .finish_non_exhaustive()
    }
}

impl BingxContractBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a perpetual bar source for `market`.
#[must_use]
pub(crate) fn bar_source_contract(
    market: Market,
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> BingxContractBarSource {
    BingxContractBarSource {
        market,
        url: market.klines_url().to_owned(),
        client,
        clock,
        supported: crate::bars::supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BingxContractBarSource {
    fn source_id(&self) -> &str {
        self.market.source_id()
    }

    fn supported(&self) -> &[BarSpec] {
        &self.supported
    }

    fn max_rows(&self) -> usize {
        crate::bars::MAX_ROWS
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
        let interval = crate::bars::interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let url = format!(
            "{}?symbol={}&interval={interval}&limit={}&startTime={}&endTime={}",
            self.url,
            symbol.as_str(),
            crate::bars::MAX_ROWS,
            range.start().as_millis(),
            range.end().as_millis(),
        );
        let body = self.client.get(&url, FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice::<Envelope<Vec<RawCandle>>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.volume.as_str()));

        // No closed flag here, so a clock decides.
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
                // Only the linear market's `volume` is a base amount —
                // see the module docs.
                volume: match self.market {
                    Market::Linear => Volume::Real(at(&row.volume, qty_scale)?),
                    Market::Inverse => Volume::Absent,
                },
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// One level: `[price, size]`, both decimal strings on both markets.
type RawLevel = (String, String);

#[derive(Debug, Default, Deserialize)]
struct RawBook {
    /// Epoch milliseconds.
    #[serde(rename = "T", default)]
    ts: i64,
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
}

/// `BingX` perpetual depth.
#[derive(Debug, Clone)]
pub(crate) struct BingxContractBookSource {
    market: Market,
    url: String,
    client: VenueClient,
}

impl BingxContractBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds a perpetual book source for `market`.
#[must_use]
pub(crate) fn book_source_contract(market: Market, client: VenueClient) -> BingxContractBookSource {
    BingxContractBookSource {
        market,
        url: market.depth_url().to_owned(),
        client,
    }
}

#[async_trait::async_trait]
impl BookSource for BingxContractBookSource {
    fn source_id(&self) -> &str {
        self.market.source_id()
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}&limit={depth}", self.url, symbol.as_str());
        let body = self.client.get(&url, FETCH_COST).await?;
        let raw: RawBook = serde_json::from_slice::<Envelope<RawBook>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;

        let price_scale = common_scale(
            raw.bids
                .iter()
                .chain(raw.asks.iter())
                .map(|level| level.0.as_str()),
        );
        let qty_scale = common_scale(
            raw.bids
                .iter()
                .chain(raw.asks.iter())
                .map(|level| level.1.as_str()),
        );

        let mut bids = side(&raw.bids, price_scale, qty_scale)?;
        let mut asks = side(&raw.asks, price_scale, qty_scale)?;
        bids.truncate(depth);
        asks.truncate(depth);

        let ts = UnixNanos::from_millis(raw.ts)
            .ok_or_else(|| SourceError::decode(format!("book ts {} overflowed", raw.ts)))?;

        BookSnapshot::new(
            ts,
            bids,
            price_scale,
            qty_scale,
            asks,
            price_scale,
            qty_scale,
        )
        .map_err(|source| SourceError::rejected(source.to_string()))
    }
}

fn side(raw: &[RawLevel], price_scale: u8, qty_scale: u8) -> Result<Vec<BookLevel>, SourceError> {
    raw.iter()
        .map(|(price, size)| {
            Ok(BookLevel {
                price: at(price, price_scale)?,
                size: at(size, qty_scale)?,
            })
        })
        .collect()
}

fn at(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use super::{Market, bar_source_contract, book_source_contract};
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LINEAR_KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1h_linear.json");
    const INVERSE_KLINES: &[u8] = include_bytes!("../tests/fixtures/klines_1h_inverse.json");
    const LINEAR_BOOK: &[u8] = include_bytes!("../tests/fixtures/book_linear.json");

    #[derive(Debug)]
    struct FixedClock;
    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_millis(1_788_400_000_000).unwrap()
        }
        async fn sleep_until(&self, _t: UnixNanos) {}
    }

    fn client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    async fn serving(body: &'static [u8]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        server
    }

    fn range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_millis(1_788_300_000_000).unwrap(),
            UnixNanos::from_millis(1_788_400_000_000).unwrap(),
        )
        .unwrap()
    }

    async fn bars(body: &'static [u8], market: Market) -> Vec<senken_series::Bar> {
        let server = serving(body).await;
        bar_source_contract(market, client(), Arc::new(FixedClock))
            .with_url(server.uri())
            .bars(
                &SourceSymbol::assume("BTC-USDT"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap()
    }

    /// The named-field shape spot's positional parser cannot read.
    #[tokio::test]
    async fn a_linear_page_decodes_to_bars_with_a_base_volume() {
        let bars = bars(LINEAR_KLINES, Market::Linear).await;

        // The venue sends newest first; this source sorts ascending, so
        // the fixture's last row is the first bar.
        assert_eq!(bars.len(), 5);
        assert_eq!(bars[0].close, 774_428, "the oldest row, `77442.8`");
        assert_eq!(bars[4].close, 765_874, "the newest row, `76587.4`");
        let Volume::Real(volume) = bars[4].volume else {
            panic!("the linear market publishes a base volume");
        };
        assert_eq!(volume, 969_307, "`96.9307` BTC at the page's scale of 4");
    }

    /// The inverse market's `volume` is a contract count: `6876.00` BTC
    /// in an hour would be half a billion dollars, which it is not. So
    /// no base volume is claimed.
    #[tokio::test]
    async fn an_inverse_page_publishes_no_base_volume() {
        let bars = bars(INVERSE_KLINES, Market::Inverse).await;

        assert_eq!(bars.len(), 5);
        assert_eq!(bars[4].close, 765_436, "the newest row, `76543.6`");
        assert!(bars.iter().all(|bar| bar.volume == Volume::Absent));
    }

    #[tokio::test]
    async fn a_perpetual_book_decodes_to_levels() {
        let server = serving(LINEAR_BOOK).await;
        let source = book_source_contract(Market::Linear, client()).with_url(server.uri());

        let snapshot = source
            .book_snapshot(&SourceSymbol::assume("BTC-USDT"), 5)
            .await
            .unwrap();

        assert_eq!(snapshot.bids[0].price, 765_983, "`76598.3` at scale 1");
        assert_eq!(snapshot.price_scale, 1);
        assert!(snapshot.ts.as_millis() > 0, "the instant is under `T`");
    }

    #[tokio::test]
    async fn an_application_error_inside_http_200_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"code":100400,"msg":"symbol not exist","data":null}"#),
            )
            .mount(&server)
            .await;
        let source = book_source_contract(Market::Linear, client()).with_url(server.uri());

        assert!(
            source
                .book_snapshot(&SourceSymbol::assume("NOPE"), 5)
                .await
                .is_err()
        );
    }
}
