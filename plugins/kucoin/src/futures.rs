//! `KuCoin` Futures — candles, depth and the live execution stream.
//!
//! # A separate API from spot's, on a separate host
//!
//! Everything about `api-futures.kucoin.com` differs from the spot host,
//! and none of it is a parameter on the spot code. Recorded live
//! 2026-09-02:
//!
//! ```json
//! spot candle   ["1788329220","77634.61","77634.61","77634.61", …]   strings, seconds
//! futures candle [1788242400000,79140.8,79140.8,78712.0,78712.0,49623,3915961.5089]  numbers, milliseconds
//!
//! spot level    ["77550.9","0.0106"]
//! futures level [76387.7,70]
//!
//! spot trade    {"subject":"trade.l3match","data":{…,"time":"1788335265722000000"}}
//! futures trade {"subject":"match","data":{…,"size":2,"price":"76468.6","ts":1788348620000000000}}
//! ```
//!
//! Its token endpoint is separate too — `api-futures.kucoin.com`'s own
//! `bullet-public`, not the spot one — so the two markets cannot share a
//! socket even though the handshake is identical.
//!
//! # Why these bars and ticks carry no volume
//!
//! `XBTUSDTM`'s size is a **contract count**: `49623` for an hour that
//! traded $3.9M at ~$78,700 works out at 0.001 BTC per contract, which is
//! a multiplier this module does not fetch. Publishing the raw count as a
//! base amount would be three orders of magnitude out, so the base volume
//! is [`Volume::Absent`] and the venue's own `turnover` is carried as the
//! quote figure it genuinely is.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
use senken_marketdata::source::SourceError;
use senken_marketdata::{InstrumentId, SourceSymbol};
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Clock, Volume};
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;
use serde_json::value::RawValue;

/// The futures host — spot lives on `api.kucoin.com`.
const FUTURES_BASE: &str = "https://api-futures.kucoin.com/api/v1";

/// This project's own panel depth.
const MAX_DEPTH: usize = 20;

/// This workspace's conservative proactive budget.
const FETCH_COST: u32 = 5;

/// Rows this source will accept from one call — this project's own cap,
/// not a venue-documented one.
const MAX_ROWS: usize = 500;

/// Every `(step, unit, granularity in minutes)` this source offers.
const INTERVALS: &[(u32, BarUnit, u32)] = &[
    (1, BarUnit::Minute, 1),
    (5, BarUnit::Minute, 5),
    (15, BarUnit::Minute, 15),
    (1, BarUnit::Hour, 60),
    (4, BarUnit::Hour, 240),
    (1, BarUnit::Day, 1440),
];

fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|(step, unit, _)| BarSpec::new(*step, *unit))
        .collect()
}

fn granularity_of(spec: BarSpec) -> Option<u32> {
    INTERVALS
        .iter()
        .find(|(step, unit, _)| *step == spec.step.get() && *unit == spec.unit)
        .map(|(_, _, granularity)| *granularity)
}

#[derive(Debug, Deserialize)]
struct Envelope<T: Default> {
    #[serde(default)]
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: T,
}

impl<T: Default> Envelope<T> {
    fn payload(self) -> Result<T, SourceError> {
        if !self.code.is_empty() && self.code != "200000" {
            return Err(SourceError::rejected(format!(
                "code {}: {}",
                self.code, self.msg
            )));
        }
        Ok(self.data)
    }
}

/// One candle: `[ts_ms, open, high, low, close, volume, turnover]`, every
/// cell a bare JSON number.
type RawCandle = Vec<Box<RawValue>>;

/// Cells a row must have before it can be read.
const REQUIRED_CELLS: usize = 7;

/// One cell's digits, whether the venue quoted them or not.
fn plain(raw: &RawValue) -> String {
    let trimmed = raw.get().trim().trim_matches('"');
    senken_core::plain_decimal(trimmed)
        .map_or_else(|| trimmed.to_owned(), std::borrow::Cow::into_owned)
}

/// `KuCoin` Futures bars.
#[derive(Clone)]
pub(crate) struct KucoinFuturesBarSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for KucoinFuturesBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KucoinFuturesBarSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl KucoinFuturesBarSource {
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
) -> KucoinFuturesBarSource {
    KucoinFuturesBarSource {
        url: format!("{FUTURES_BASE}/kline/query"),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for KucoinFuturesBarSource {
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
        let granularity = granularity_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let url = format!(
            "{}?symbol={}&granularity={granularity}&from={}&to={}",
            self.url,
            symbol.as_str(),
            range.start().as_millis(),
            range.end().as_millis(),
        );
        let body = self.client.get(&url, FETCH_COST).await?;
        let raw: Vec<RawCandle> = serde_json::from_slice::<Envelope<Vec<RawCandle>>>(&body)
            .map_err(SourceError::decode)?
            .payload()?;

        let rows: Vec<Vec<String>> = raw
            .iter()
            .filter(|row| row.len() >= REQUIRED_CELLS)
            .map(|row| row.iter().map(|cell| plain(cell)).collect())
            .collect();

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row[1].as_str(),
                row[2].as_str(),
                row[3].as_str(),
                row[4].as_str(),
            ]
        }));
        let quote_scale = common_scale(rows.iter().map(|row| row[6].as_str()));

        let now = self.clock.now().as_nanos();
        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_ms: i64 = row[0]
                .parse()
                .map_err(|_| SourceError::decode(format!("{:?} is not a timestamp", row[0])))?;
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
                open: at(&row[1], price_scale)?,
                high: at(&row[2], price_scale)?,
                low: at(&row[3], price_scale)?,
                close: at(&row[4], price_scale)?,
                // `row[5]` counts contracts — see the module docs.
                volume: Volume::Absent,
                quote_volume: Some(at(&row[6], quote_scale)?),
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// One level: `[price, size]`, both bare JSON numbers.
type RawLevel = (Box<RawValue>, Box<RawValue>);

#[derive(Debug, Default, Deserialize)]
struct RawBook {
    #[serde(default)]
    ts: i64,
    #[serde(default)]
    asks: Vec<RawLevel>,
    #[serde(default)]
    bids: Vec<RawLevel>,
}

/// `KuCoin` Futures depth.
#[derive(Clone)]
pub(crate) struct KucoinFuturesBookSource {
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for KucoinFuturesBookSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KucoinFuturesBookSource")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl KucoinFuturesBookSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }
}

/// Builds the futures book source.
///
/// Takes a clock because this endpoint's `ts` was observed absent on the
/// recorded response: an instant of zero would read as 1970, so the
/// reader's own clock stands in and the snapshot says when it was
/// fetched rather than claiming a time the venue did not give.
#[must_use]
pub(crate) fn book_source_futures(
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> KucoinFuturesBookSource {
    KucoinFuturesBookSource {
        url: format!("{FUTURES_BASE}/level2/depth20"),
        client,
        clock,
    }
}

#[async_trait::async_trait]
impl BookSource for KucoinFuturesBookSource {
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

        let cells: Vec<(String, String, bool)> = raw
            .bids
            .iter()
            .map(|(p, s)| (plain(p), plain(s), true))
            .chain(raw.asks.iter().map(|(p, s)| (plain(p), plain(s), false)))
            .collect();
        let price_scale = common_scale(cells.iter().map(|(price, _, _)| price.as_str()));
        let qty_scale = common_scale(cells.iter().map(|(_, size, _)| size.as_str()));

        let mut bids = Vec::new();
        let mut asks = Vec::new();
        for (price, size, is_bid) in &cells {
            let level = BookLevel {
                price: at(price, price_scale)?,
                size: at(size, qty_scale)?,
            };
            let side = if *is_bid { &mut bids } else { &mut asks };
            side.push(level);
        }
        bids.truncate(depth);
        asks.truncate(depth);

        let ts = if raw.ts > 0 {
            UnixNanos::from_nanos(raw.ts)
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

fn at(raw: &str, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

/// `KuCoin` Futures' `execution` channel.
pub(crate) struct KucoinFuturesProtocol {
    symbols: Arc<dyn SymbolMap>,
    client: VenueClient,
    bullet_url: String,
}

impl KucoinFuturesProtocol {
    fn topic(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no KuCoin native symbol known for {instrument}"))
        })?;
        Ok(format!("/contractMarket/execution:{symbol}"))
    }
}

#[async_trait::async_trait]
impl VenueProtocol for KucoinFuturesProtocol {
    /// A fallback only: dialling without a token is refused, and the dial
    /// path uses [`endpoint`](VenueProtocol::endpoint).
    fn url(&self) -> &str {
        &self.bullet_url
    }

    async fn endpoint(&self) -> Result<String, ConnectionError> {
        let body = self
            .client
            .post(&self.bullet_url, 1)
            .await
            .map_err(|source| {
                ConnectionError::new(format!(
                    "KuCoin Futures refused a WebSocket token: {source}"
                ))
            })?;
        let bullet: Bullet = serde_json::from_slice(&body).map_err(|source| {
            ConnectionError::new(format!("KuCoin's token response did not decode: {source}"))
        })?;
        let server = bullet.data.instance_servers.first().ok_or_else(|| {
            ConnectionError::new("KuCoin's token response named no WebSocket endpoint")
        })?;
        Ok(format!(
            "{}?token={}&connectId=senken",
            server.endpoint, bullet.data.token
        ))
    }

    fn venue(&self) -> &'static str {
        "kucoin-futures"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let topic = self.topic(instrument)?;
        Ok(format!(
            r#"{{"id":"{topic}","type":"subscribe","topic":"{topic}","privateChannel":false,"response":true}}"#
        ))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let topic = self.topic(instrument)?;
        Ok(format!(
            r#"{{"id":"{topic}","type":"unsubscribe","topic":"{topic}","privateChannel":false,"response":true}}"#
        ))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<MatchFrame>(text) else {
            return Vec::new();
        };
        if frame.subject != "match" {
            return Vec::new();
        }
        let Some(data) = frame.data else {
            return Vec::new();
        };
        let Ok(instrument) = InstrumentId::new(crate::FUTURES_ID, &data.symbol) else {
            return Vec::new();
        };
        let Some((price, price_scale)) = senken_plugin::live::scaled(&data.price) else {
            return Vec::new();
        };
        vec![(
            instrument,
            LiveUpdate::Price(senken_subscription::PriceUpdate {
                // `ts` is epoch nanoseconds here, as on spot.
                ts: UnixNanos::from_nanos(data.ts),
                price,
                price_scale,
                // `size` counts contracts — see the module docs.
                qty: Volume::Absent,
                qty_scale: 0,
            }),
        )]
    }

    fn keepalive(&self) -> Option<(std::time::Duration, String)> {
        Some((
            std::time::Duration::from_secs(15),
            r#"{"id":"senken","type":"ping"}"#.to_owned(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct Bullet {
    data: BulletData,
}

#[derive(Debug, Deserialize)]
struct BulletData {
    token: String,
    #[serde(rename = "instanceServers")]
    instance_servers: Vec<InstanceServer>,
}

#[derive(Debug, Deserialize)]
struct InstanceServer {
    endpoint: String,
}

#[derive(Debug, Deserialize)]
struct MatchFrame {
    #[serde(default)]
    subject: String,
    #[serde(default)]
    data: Option<MatchData>,
}

#[derive(Debug, Deserialize)]
struct MatchData {
    symbol: String,
    price: String,
    /// Epoch **nanoseconds**.
    ts: i64,
}

/// `KuCoin` Futures' live-feed registration.
pub(crate) struct KucoinFuturesFeedSource {
    source_ids: Vec<String>,
    client: VenueClient,
}

impl KucoinFuturesFeedSource {
    pub(crate) fn new(client: VenueClient) -> Self {
        Self {
            source_ids: vec![crate::FUTURES_ID.to_owned()],
            client,
        }
    }
}

impl FeedSource for KucoinFuturesFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(KucoinFuturesProtocol {
            symbols,
            client: self.client.clone(),
            bullet_url: format!("{FUTURES_BASE}/bullet-public"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{KucoinFuturesProtocol, bar_source_futures, book_source_futures};
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

    const KLINE: &[u8] = include_bytes!("../tests/fixtures/kline_1h_futures.json");
    const BOOK: &[u8] = include_bytes!("../tests/fixtures/book_futures.json");

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

    /// Numbers rather than strings, milliseconds rather than seconds —
    /// the spot decoder reads neither.
    #[tokio::test]
    async fn a_futures_page_decodes_to_bars() {
        let server = serving(KLINE).await;
        let source = bar_source_futures(client(), Arc::new(FixedClock)).with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("XBTUSDTM"),
                BarSpec::new(1, BarUnit::Hour),
                TimeRange::new(
                    UnixNanos::from_millis(1_788_240_000_000).unwrap(),
                    UnixNanos::from_millis(1_788_340_000_000).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(!bars.is_empty());
        assert_eq!(bars[0].ts_open.as_millis(), 1_788_242_400_000);
        assert_eq!(bars[0].open, 791_408);
        assert!(
            bars.iter().all(|bar| bar.volume == Volume::Absent),
            "`volume` counts contracts, not base asset"
        );
        assert!(bars[0].quote_volume.is_some(), "`turnover` is real money");
    }

    #[tokio::test]
    async fn a_futures_book_decodes_from_bare_numbers() {
        let server = serving(BOOK).await;
        let source = book_source_futures(client(), Arc::new(FixedClock)).with_url(server.uri());

        let snapshot = source
            .book_snapshot(&SourceSymbol::assume("XBTUSDTM"), 5)
            .await
            .unwrap();

        assert_eq!(snapshot.bids[0].price, 763_877);
        assert_eq!(snapshot.bids[0].size, 70);
        assert_eq!(snapshot.bids.len(), 5);
    }

    fn protocol() -> KucoinFuturesProtocol {
        KucoinFuturesProtocol {
            symbols: Arc::new(IdentitySymbolMap),
            client: client(),
            bullet_url: "https://api-futures.kucoin.com/api/v1/bullet-public".to_owned(),
        }
    }

    #[test]
    fn the_subscribe_names_the_execution_channel() {
        let frame: serde_json::Value = serde_json::from_str(
            &protocol()
                .subscribe_frame(&InstrumentId::new(crate::FUTURES_ID, "XBTUSDTM").unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(frame["type"], "subscribe");
        assert_eq!(frame["topic"], "/contractMarket/execution:XBTUSDTM");
    }

    /// Byte-for-byte a frame from this module's live capture. Its
    /// `subject` is `match`, not spot's `trade.l3match`, and `ts` is a
    /// bare integer rather than a string.
    #[test]
    fn the_captured_execution_frame_decodes_to_a_price() {
        let frame = r#"{"topic":"/contractMarket/execution:XBTUSDTM","type":"message","subject":"match","sn":1942648200120,"data":{"symbol":"XBTUSDTM","sequence":1942648200120,"side":"buy","size":2,"price":"76468.6","takerOrderId":"484590665674690560","makerOrderId":"484590651955064832","tradeId":"1942648200120","ts":1788348620000000000}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("an execution frame must decode to a price update");
        };
        assert_eq!(
            id,
            &InstrumentId::new(crate::FUTURES_ID, "XBTUSDTM").unwrap()
        );
        assert_eq!(update.price, 764_686);
        assert_eq!(update.ts.as_millis(), 1_788_348_620_000);
        assert_eq!(
            update.qty,
            Volume::Absent,
            "`size` counts contracts, not base asset"
        );
    }

    #[test]
    fn the_welcome_and_ack_yield_nothing() {
        assert!(
            protocol()
                .parse_message(r#"{"id":"senken","type":"welcome"}"#)
                .is_empty()
        );
        assert!(protocol().parse_message("not json").is_empty());
    }
}
