//! Poloniex's perpetual market — candles, depth and the live trade stream.
//!
//! # A third API version, with its own shapes
//!
//! Poloniex's spot market is v2 and its perpetual one is v3, and the two
//! disagree about nearly everything. Recorded live 2026-09-02:
//!
//! ```json
//! spot candle  ["76909.47","77510.66","77438.93","77199.3","19124572.06","247.858776","9422743.83","122.120976",15282,1788296400000,"77159.36","HOUR_1",1788296400000,1788299999999]
//! perp candle  ["77444.45","77719.58","77599.33","77444.45","84361.5254","1088","159","1788332400000","1788335999999"]
//! ```
//!
//! Fourteen cells against nine, and the perpetual's are wrapped in a
//! `{code, msg, data}` envelope the spot endpoint does not use. The
//! leading four are the same in both — `[low, high, open, close]`, which
//! is *not* OHLC order and is the trap this venue is known for here — but
//! nothing after that lines up, so a tuple built for one rejects the
//! other outright.
//!
//! # Why these bars and ticks carry no base volume
//!
//! On the perpetual, cell 5 (`1088`) is a **contract count** and cell 4
//! (`84361.5254`) the quote turnover: 1088 BTC in an hour would be $84
//! million at that hour's own close, which is precisely what cell 4 says
//! — so cell 4 is money and cell 5 is contracts. The base amount would
//! need this contract's multiplier, which this module does not fetch, so
//! the base volume is [`Volume::Absent`] and the turnover is carried as
//! the quote figure it is.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_marketdata::{InstrumentId, SourceSymbol};
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::{VenueClient, common_scale, normalise_symbol};
use serde::Deserialize;

/// The v3 root — spot lives under `/markets`.
const V3_BASE: &str = "https://api.poloniex.com/v3/market";

/// `wss://ws.poloniex.com/ws/v3/public` — confirmed live 2026-09-02.
const PERP_WS_URL: &str = "wss://ws.poloniex.com/ws/v3/public";

/// Poloniex joins the legs of a perpetual with `_`.
const SEPARATOR: char = '_';

/// This project's own panel depth.
const MAX_DEPTH: usize = 20;

/// This workspace's conservative proactive budget.
const FETCH_COST: u32 = 5;

/// Rows this source will accept from one call.
const MAX_ROWS: usize = 500;

/// Every `(step, unit, interval)` this source offers.
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "MINUTE_1"),
    (5, BarUnit::Minute, "MINUTE_5"),
    (15, BarUnit::Minute, "MINUTE_15"),
    (1, BarUnit::Hour, "HOUR_1"),
    (4, BarUnit::Hour, "HOUR_4"),
    (1, BarUnit::Day, "DAY_1"),
];

fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|(step, unit, _)| BarSpec::new(*step, *unit))
        .collect()
}

fn interval_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|(step, unit, _)| *step == spec.step.get() && *unit == spec.unit)
        .map(|(_, _, interval)| *interval)
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
        if self.code != 200 && self.code != 0 {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                self.code, self.msg
            )));
        }
        Ok(self.data)
    }
}

/// One perpetual candle: `[low, high, open, close, turnover, contracts,
/// tradeCount, startTime, closeTime]`. **Not OHLC order** — see the
/// module docs.
type RawCandle = Vec<String>;

/// Cells a row must carry before it can be read.
const REQUIRED_CELLS: usize = 8;

/// Poloniex perpetual bars.
#[derive(Clone)]
pub(crate) struct PoloniexPerpBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for PoloniexPerpBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoloniexPerpBarSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl PoloniexPerpBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the perpetual bar source.
#[must_use]
pub(crate) fn bar_source_perp(client: VenueClient, clock: Arc<dyn Clock>) -> PoloniexPerpBarSource {
    PoloniexPerpBarSource {
        url: format!("{V3_BASE}/candles"),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for PoloniexPerpBarSource {
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
        let interval = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let url = format!(
            "{}?symbol={}&interval={interval}&limit={MAX_ROWS}&startTime={}&endTime={}",
            self.url,
            symbol.as_str(),
            range.start().as_millis(),
            range.end().as_millis(),
        );
        let body = self.client.get(&url, FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice::<Envelope<Vec<RawCandle>>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;
        let rows: Vec<RawCandle> = rows
            .into_iter()
            .filter(|row| row.len() >= REQUIRED_CELLS)
            .collect();

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row[0].as_str(),
                row[1].as_str(),
                row[2].as_str(),
                row[3].as_str(),
            ]
        }));
        let quote_scale = common_scale(rows.iter().map(|row| row[4].as_str()));

        let now = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_ms: i64 = row[7]
                .trim()
                .parse()
                .map_err(|_| SourceError::decode(format!("{:?} is not a timestamp", row[7])))?;
            let ts_open = UnixNanos::from_millis(ts_ms)
                .ok_or_else(|| SourceError::decode(format!("open time {ts_ms}ms overflowed")))?;
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
                // `[low, high, open, close]`, not OHLC.
                low: at(&row[0], price_scale)?,
                high: at(&row[1], price_scale)?,
                open: at(&row[2], price_scale)?,
                close: at(&row[3], price_scale)?,
                // Cell 5 counts contracts — see the module docs.
                volume: Volume::Absent,
                quote_volume: Some(at(&row[4], quote_scale)?),
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawBook {
    #[serde(default)]
    ts: i64,
    #[serde(default)]
    asks: Vec<(String, String)>,
    #[serde(default)]
    bids: Vec<(String, String)>,
}

/// Poloniex perpetual depth.
#[derive(Clone)]
pub(crate) struct PoloniexPerpBookSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for PoloniexPerpBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoloniexPerpBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl PoloniexPerpBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the perpetual book source.
///
/// Takes a clock because this endpoint carries no instant of its own on
/// the recorded response: rather than claim 1970, the snapshot says when
/// it was fetched.
#[must_use]
pub(crate) fn book_source_perp(
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> PoloniexPerpBookSource {
    PoloniexPerpBookSource {
        url: format!("{V3_BASE}/orderBook"),
        client,
        clock,
    }
}

#[async_trait::async_trait]
impl BookSource for PoloniexPerpBookSource {
    fn source_id(&self) -> &str {
        crate::PERP_ID
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
                .map(|(price, _)| price.as_str()),
        );
        let qty_scale = common_scale(
            raw.bids
                .iter()
                .chain(raw.asks.iter())
                .map(|(_, size)| size.as_str()),
        );

        let mut bids = side(&raw.bids, price_scale, qty_scale)?;
        let mut asks = side(&raw.asks, price_scale, qty_scale)?;
        bids.truncate(depth);
        asks.truncate(depth);

        let ts = if raw.ts > 0 {
            UnixNanos::from_millis(raw.ts)
                .ok_or_else(|| SourceError::decode(format!("book ts {} overflowed", raw.ts)))?
        } else {
            self.clock.now()
        };

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

fn side(
    raw: &[(String, String)],
    price_scale: u8,
    qty_scale: u8,
) -> Result<Vec<BookLevel>, SourceError> {
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

/// Poloniex's v3 perpetual trade stream.
///
/// A different socket and a different frame from spot's: the fields are
/// abbreviated (`px`, `qty`) where spot spells them out. Confirmed live
/// 2026-09-02.
pub(crate) struct PoloniexPerpProtocol {
    symbols: Arc<dyn SymbolMap>,
}

impl VenueProtocol for PoloniexPerpProtocol {
    fn url(&self) -> &str {
        PERP_WS_URL
    }

    fn venue(&self) -> &'static str {
        "poloniex-perp"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(frame("subscribe", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(frame("unsubscribe", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<TradeFrame>(text) else {
            return Vec::new();
        };
        if frame.channel != "trades" {
            return Vec::new();
        }
        frame
            .data
            .iter()
            .filter_map(|row| {
                let instrument =
                    InstrumentId::new(crate::PERP_ID, &normalise_symbol(&row.s, &[SEPARATOR]))
                        .ok()?;
                let ts = UnixNanos::from_millis(row.ts)?;
                let (price, price_scale) = senken_plugin::live::scaled(&row.px)?;
                Some((
                    instrument,
                    LiveUpdate::Price(senken_subscription::PriceUpdate {
                        ts,
                        price,
                        price_scale,
                        // `qty` counts contracts — see the module docs.
                        qty: Volume::Absent,
                        qty_scale: 0,
                    }),
                ))
            })
            .collect()
    }

    fn keepalive(&self) -> Option<(std::time::Duration, String)> {
        Some((
            std::time::Duration::from_secs(20),
            r#"{"event":"ping"}"#.to_owned(),
        ))
    }
}

impl PoloniexPerpProtocol {
    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Poloniex native symbol known for {instrument}"))
        })
    }
}

fn frame(event: &str, symbol: &str) -> String {
    format!(r#"{{"event":"{event}","channel":["trades"],"symbols":["{symbol}"]}}"#)
}

#[derive(Debug, Deserialize)]
struct TradeFrame {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    data: Vec<PerpTrade>,
}

#[derive(Debug, Deserialize)]
struct PerpTrade {
    /// The contract's own symbol.
    s: String,
    /// Price, abbreviated — spot spells it `price`.
    px: String,
    /// Epoch milliseconds.
    ts: i64,
}

/// Poloniex's perpetual live-feed registration.
pub(crate) struct PoloniexPerpFeedSource {
    source_ids: Vec<String>,
}

impl PoloniexPerpFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::PERP_ID.to_owned()],
        }
    }
}

impl FeedSource for PoloniexPerpFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(PoloniexPerpProtocol { symbols })
    }
}

#[cfg(test)]
mod tests {
    use super::{PoloniexPerpProtocol, bar_source_perp, book_source_perp};
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::book::BookSource;
    use senken_marketdata::{InstrumentId, SourceSymbol};
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h_perp.json");
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book_perp.json");

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

    /// Nine cells against spot's fourteen, wrapped in an envelope spot
    /// does not use — and the leading four are `[low, high, open,
    /// close]`, which is not OHLC order.
    #[tokio::test]
    async fn a_perpetual_page_decodes_with_the_low_high_open_close_order() {
        let server = serving(CANDLES).await;
        let source = bar_source_perp(client(), Arc::new(FixedClock)).with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("BTC_USDT_PERP"),
                BarSpec::new(1, BarUnit::Hour),
                TimeRange::new(
                    UnixNanos::from_millis(1_788_300_000_000).unwrap(),
                    UnixNanos::from_millis(1_788_400_000_000).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(!bars.is_empty());
        let first = bars
            .iter()
            .find(|bar| bar.ts_open.as_millis() == 1_788_332_400_000)
            .expect("the fixture's first row is inside the range");
        // Row: low 77444.45, high 77719.58, open 77599.33, close 77444.45
        assert_eq!(first.low, 7_744_445);
        assert_eq!(first.high, 7_771_958);
        assert_eq!(first.open, 7_759_933);
        assert_eq!(first.close, 7_744_445);
        assert!(
            first.low <= first.open && first.open <= first.high,
            "reading the cells in OHLC order would invert this"
        );
        assert_eq!(
            first.volume,
            Volume::Absent,
            "cell 5 counts contracts, not base asset"
        );
        assert!(first.quote_volume.is_some(), "cell 4 is real money");
    }

    #[tokio::test]
    async fn a_perpetual_book_decodes_to_levels() {
        let server = serving(BOOK).await;
        let source = book_source_perp(client(), Arc::new(FixedClock)).with_url(server.uri());

        let snapshot = source
            .book_snapshot(&SourceSymbol::assume("BTC_USDT_PERP"), 5)
            .await
            .unwrap();

        assert_eq!(snapshot.bids[0].price, 7_637_995);
        assert!(!snapshot.asks.is_empty());
        assert!(
            snapshot.ts.as_millis() > 0,
            "an absent venue instant falls back to the reader's clock, never to 1970"
        );
    }

    fn protocol() -> PoloniexPerpProtocol {
        PoloniexPerpProtocol {
            symbols: Arc::new(IdentitySymbolMap),
        }
    }

    /// Byte-for-byte a frame from this module's live capture. Its fields
    /// are abbreviated where spot's are spelled out, so spot's decoder
    /// reads nothing from it.
    #[tokio::test]
    async fn the_captured_perpetual_frame_decodes_to_a_price() {
        let frame = r#"{"channel":"trades","data":[{"id":110522035,"ts":1788345463960,"s":"BTC_USDT_PERP","px":"76512.81","qty":"8","amt":"612.10248","side":"sell","cT":1788345463950}]}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades frame must decode to a price update");
        };
        assert_eq!(
            id,
            &InstrumentId::new(crate::PERP_ID, "BTCUSDTPERP").unwrap()
        );
        assert_eq!(update.price, 7_651_281);
        assert_eq!(update.ts.as_millis(), 1_788_345_463_960);
        assert_eq!(update.qty, Volume::Absent, "`qty` counts contracts");
    }

    #[test]
    fn an_acknowledgement_and_garbage_yield_nothing() {
        assert!(
            protocol()
                .parse_message(
                    r#"{"event":"subscribe","channel":"trades","symbols":["BTC_USDT_PERP"]}"#
                )
                .is_empty()
        );
        assert!(protocol().parse_message("not json").is_empty());
    }
}
