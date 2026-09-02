//! `BitMart`'s futures candles and depth — `contract/public/{kline,depth}`.
//!
//! # Neither shape is spot's
//!
//! `BitMart` answers its contract market on a different host
//! (`api-cloud-v2`) and in a different shape from its spot one. Recorded
//! live 2026-09-02:
//!
//! ```json
//! spot kline   ["1788329040","77667.25","77716.00","77667.25","77690.00","0.88092","68442.66"]
//! futures kline {"low_price":"78705.8","high_price":"79149.8","open_price":"79149.8","close_price":"78705.8","volume":"1355314","timestamp":1788242400}
//!
//! spot level    ["77655.08","0.16122"]
//! futures level ["76585.8","1306","1306"]
//! ```
//!
//! Positional strings against named fields for the candles; two cells
//! against three for a level — the third is a running total of the sizes
//! above it, which is a *cumulative* figure and must not be read as this
//! level's own size. A decoder that took the last cell would report the
//! deepest level's size for every level.
//!
//! # Why these bars carry no volume
//!
//! `volume` is a contract count. `BitMart`'s `BTCUSDT` contract is not one
//! bitcoin, so publishing `1355314` as a base amount would be wrong by
//! whatever the multiplier is — a figure this module does not fetch.
//! [`Volume::Absent`] says that honestly; the same choice this workspace's
//! MEXC and Gate contract sources already make.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

/// The contract host — spot lives on `api-cloud.bitmart.com`.
const CONTRACT_BASE: &str = "https://api-cloud-v2.bitmart.com/contract/public";

/// This project's own panel depth; the venue returns a deeper book and it
/// is truncated locally.
const MAX_DEPTH: usize = 20;

/// This workspace's conservative proactive budget, matching every other
/// source here.
const FETCH_COST: u32 = 5;

/// `BitMart`'s success code — `1000`, not `0`.
const OK: i64 = 1000;

/// Seconds are what this endpoint's window parameters take.
const NANOS_PER_SEC: i64 = 1_000_000_000;

#[derive(Debug, Deserialize)]
struct Envelope<T: Default> {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default = "Default::default")]
    data: T,
}

impl<T: Default> Envelope<T> {
    fn payload(self) -> Result<T, SourceError> {
        if self.code != OK {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                self.code, self.message
            )));
        }
        Ok(self.data)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawCandle {
    open_price: String,
    high_price: String,
    low_price: String,
    close_price: String,
    /// Epoch seconds of the candle's open.
    timestamp: i64,
}

/// `BitMart` futures bars.
#[derive(Clone)]
pub(crate) struct BitmartFuturesBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for BitmartFuturesBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BitmartFuturesBarSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl BitmartFuturesBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the futures bar source.
#[must_use]
pub(crate) fn bar_source_futures(
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> BitmartFuturesBarSource {
    BitmartFuturesBarSource {
        url: format!("{CONTRACT_BASE}/kline"),
        client,
        clock,
        supported: crate::bars::supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BitmartFuturesBarSource {
    fn source_id(&self) -> &str {
        crate::FUTURES_ID
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
        let step = crate::bars::step_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let from = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let to = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        let url = format!(
            "{}?symbol={}&step={step}&start_time={from}&end_time={to}",
            self.url,
            symbol.as_str()
        );

        let body = self.client.get(&url, FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice::<Envelope<Vec<RawCandle>>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.open_price.as_str(),
                row.high_price.as_str(),
                row.low_price.as_str(),
                row.close_price.as_str(),
            ]
        }));

        // No closed flag on this endpoint, so the forming candle is
        // excluded by a clock rather than by the venue saying so.
        let now = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_open = UnixNanos::from_secs(row.timestamp).ok_or_else(|| {
                SourceError::decode(format!("open time {}s overflowed", row.timestamp))
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
                open: at(&row.open_price, price_scale)?,
                high: at(&row.high_price, price_scale)?,
                low: at(&row.low_price, price_scale)?,
                close: at(&row.close_price, price_scale)?,
                // A contract count, not a base amount.
                volume: Volume::Absent,
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// One futures level: `[price, size, cumulative size]`. Only the first two
/// are read — see the module docs.
type RawLevel = (String, String, String);

#[derive(Debug, Default, Deserialize)]
struct RawBook {
    #[serde(default)]
    timestamp: i64,
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
}

/// `BitMart` futures depth.
#[derive(Debug, Clone)]
pub(crate) struct BitmartFuturesBookSource {
    url: String,
    client: VenueClient,
}

impl BitmartFuturesBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the futures book source.
#[must_use]
pub(crate) fn book_source_futures(client: VenueClient) -> BitmartFuturesBookSource {
    BitmartFuturesBookSource {
        url: format!("{CONTRACT_BASE}/depth"),
        client,
    }
}

#[async_trait::async_trait]
impl BookSource for BitmartFuturesBookSource {
    fn source_id(&self) -> &str {
        crate::FUTURES_ID
    }

    async fn book_snapshot(
        &self,
        symbol: &SourceSymbol,
        depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let depth = depth.clamp(1, MAX_DEPTH);
        let url = format!("{}?symbol={}", self.url, symbol.as_str());
        let body = self.client.get(&url, FETCH_COST).await?;
        let raw: RawBook = serde_json::from_slice::<Envelope<RawBook>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;

        // Both sides together, so an empty side cannot disagree with a
        // full one about the scale.
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

        let ts = UnixNanos::from_millis(raw.timestamp)
            .ok_or_else(|| SourceError::decode(format!("book ts {} overflowed", raw.timestamp)))?;

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

/// Reads the first two cells of each level. The third is a running total
/// of every size above it — reading it would report the deepest level's
/// depth for every level.
fn side(raw: &[RawLevel], price_scale: u8, qty_scale: u8) -> Result<Vec<BookLevel>, SourceError> {
    raw.iter()
        .map(|(price, size, _cumulative)| {
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
    use super::{bar_source_futures, book_source_futures};
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_marketdata::book::BookSource;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const KLINE: &[u8] = include_bytes!("../tests/fixtures/kline_1h_futures.json");
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book_futures.json");

    #[derive(Debug)]
    struct FixedClock;
    #[async_trait::async_trait]
    impl Clock for FixedClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_secs(1_788_400_000).unwrap()
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

    /// The named-field shape spot's positional parser cannot read.
    #[tokio::test]
    async fn a_futures_page_decodes_to_bars() {
        let server = serving(KLINE).await;
        let source = bar_source_futures(client(), Arc::new(FixedClock)).with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("BTCUSDT"),
                BarSpec::new(1, BarUnit::Hour),
                TimeRange::new(
                    UnixNanos::from_secs(1_788_240_000).unwrap(),
                    UnixNanos::from_secs(1_788_340_000).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(!bars.is_empty());
        assert_eq!(bars[0].open, 791_498);
        assert_eq!(bars[0].close, 787_058);
        assert!(
            bars.iter().all(|bar| bar.volume == Volume::Absent),
            "`volume` counts contracts, not base asset"
        );
    }

    /// A level's third cell is the running total of everything above it.
    /// Reading it as this level's own size reports the deepest level's
    /// depth at every price.
    #[tokio::test]
    async fn a_levels_size_is_its_own_not_the_running_total() {
        let server = serving(BOOK).await;
        let source = book_source_futures(client()).with_url(server.uri());

        let snapshot = source
            .book_snapshot(&SourceSymbol::assume("BTCUSDT"), 5)
            .await
            .unwrap();

        // First two asks: ["76585.8","1306","1306"] then
        // ["76593","19992","21298"] — the second's own size is 19992, and
        // 21298 is 1306 + 19992.
        assert_eq!(snapshot.qty_scale, 0, "contract counts are whole");
        assert_eq!(snapshot.asks[0].size, 1_306);
        assert_eq!(snapshot.asks[1].size, 19_992);
        assert_ne!(
            snapshot.asks[1].size, 21_298,
            "21298 is the running total of the two sizes above and must not be read as a size"
        );
    }

    #[tokio::test]
    async fn an_application_error_inside_http_200_is_a_rejection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"code":30000,"message":"Not found","data":null}"#),
            )
            .mount(&server)
            .await;
        let source = book_source_futures(client()).with_url(server.uri());

        assert!(
            source
                .book_snapshot(&SourceSymbol::assume("BTCUSDT"), 5)
                .await
                .is_err()
        );
    }
}
