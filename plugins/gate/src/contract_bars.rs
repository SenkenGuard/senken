//! Gate's contract-market candles — `futures/{settle}` and
//! `delivery/{settle}`.
//!
//! # A different shape from spot, on purpose
//!
//! Gate's spot candles are an array of eight positional **strings**. Its
//! contract candles are an array of **objects**, and the two share nothing
//! but a name. Recorded live 2026-09-02:
//!
//! ```json
//! futures/usdt   {"o":"77579.1","v":7827633,"t":1788332400,"c":"77439.8","l":"77436.1","h":"77730","sum":"60724438.34676"}
//! futures/btc    {"o":"77544.7","v":23844,"t":1788332400,"c":"77434.2","l":"77434.2","h":"77656.6","sum":"23844"}
//! delivery/usdt  {"t":1788332400,"o":"77840.3","v":1893,"h":"77969.9","c":"77547.1","l":"77471.2"}
//! ```
//!
//! Three things follow, and each is why this cannot be a parameter on the
//! spot source:
//!
//! - **`v` is a contract count, not a base amount.** Gate's `BTC_USDT`
//!   perpetual is 0.0001 BTC per contract, so `7827633` contracts is
//!   about 783 BTC — publishing the raw figure as base volume would be
//!   wrong by four orders of magnitude and would look ordinary on a
//!   histogram. It is reported as the **quote** figure it is not, either:
//!   see below.
//! - **`sum` is absent on delivery.** The two futures paths carry a quote
//!   turnover; the delivery one does not.
//! - **No `window_closed` flag.** Spot's rows say whether they are final;
//!   these do not, so the forming candle has to be excluded by a clock.
//!
//! # Why these bars carry no volume
//!
//! `v` counts contracts and the base amount would be `v x
//! quanto_multiplier`, a per-contract figure published on a different
//! endpoint. Rather than fetch and multiply — or worse, assume one
//! multiplier — the volume is reported as
//! [`Volume::Absent`](senken_series::Volume::Absent), which is what it
//! honestly is: a size this source did not establish. The same choice
//! this workspace's MEXC futures bars already make, for the same reason.
//!
//! `sum`, where present, is a genuine quote turnover and is carried as
//! one.

use std::sync::Arc;

use senken_core::{TimeRange, UnixNanos};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, Clock, Volume};
use senken_venue::{VenueClient, common_scale};
use serde::Deserialize;

use crate::bars::{CANDLES_FETCH_COST, MAX_ROWS, interval_of, supported_specs};

/// One contract candle. `v` is a bare JSON number; every price is a
/// decimal string.
#[derive(Debug, Deserialize)]
struct RawCandle {
    /// Epoch seconds of the candle's open.
    t: i64,
    o: String,
    h: String,
    l: String,
    c: String,
    /// Quote turnover. Absent on the delivery path.
    #[serde(default)]
    sum: Option<String>,
}

/// Gate contract-market bars, closed against a [`Clock`]: unlike spot's
/// rows these carry no "closed" flag, so "now" has to come from
/// somewhere.
#[derive(Clone)]
pub(crate) struct GateContractBarSource {
    source_id: &'static str,
    url: String,
    client: VenueClient,
    clock: Arc<dyn Clock>,
    supported: Vec<BarSpec>,
}

impl std::fmt::Debug for GateContractBarSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GateContractBarSource")
            .field("source_id", &self.source_id)
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl GateContractBarSource {
    /// Points this source at a different URL — a local stand-in in tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn candles_url(&self, symbol: &str, interval: &str, range: TimeRange) -> String {
        const NANOS_PER_SEC: i64 = 1_000_000_000;
        let from = range.start().as_nanos().div_euclid(NANOS_PER_SEC);
        let to = range
            .end()
            .as_nanos()
            .div_euclid(NANOS_PER_SEC)
            .saturating_add(1);
        format!(
            "{}?contract={symbol}&interval={interval}&from={from}&to={to}",
            self.url,
        )
    }
}

/// Builds a bar source for one of Gate's contract markets.
///
/// `path` is the segment between the API root and `/candlesticks` —
/// `futures/usdt`, `futures/btc` or `delivery/usdt`. All three answer with
/// the same object shape, confirmed live.
#[must_use]
pub(crate) fn bar_source_contract(
    source_id: &'static str,
    path: &str,
    client: VenueClient,
    clock: Arc<dyn Clock>,
) -> GateContractBarSource {
    GateContractBarSource {
        source_id,
        url: format!("{}/{path}/candlesticks", crate::BASE_URL),
        client,
        clock,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for GateContractBarSource {
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
        let interval = interval_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;

        let url = self.candles_url(symbol.as_str(), interval, range);
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        // One scale for the whole page, across every price column: rows
        // formatted differently by magnitude must still share a column.
        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.o.as_str(),
                row.h.as_str(),
                row.l.as_str(),
                row.c.as_str(),
            ]
        }));
        let quote_scale = common_scale(rows.iter().filter_map(|row| row.sum.as_deref()));

        // These rows carry no "closed" flag, so the forming candle is
        // excluded by the clock rather than by the venue saying so.
        let now = self.clock.now();
        let width = spec
            .duration_nanos()
            .ok_or_else(|| SourceError::rejected(format!("{spec} has no fixed width")))?;

        let mut bars = Vec::with_capacity(rows.len());
        for row in rows {
            let ts_open = UnixNanos::from_secs(row.t)
                .ok_or_else(|| SourceError::decode(format!("open time {}s overflowed", row.t)))?;
            if !range.contains(ts_open) {
                continue;
            }
            let Some(ts_close) = ts_open.as_nanos().checked_add(width) else {
                continue;
            };
            if ts_close > now.as_nanos() {
                continue;
            }
            bars.push(Bar {
                ts_open,
                open: at(&row.o, price_scale)?,
                high: at(&row.h, price_scale)?,
                low: at(&row.l, price_scale)?,
                close: at(&row.c, price_scale)?,
                // `v` counts contracts — see the module docs.
                volume: Volume::Absent,
                quote_volume: row
                    .sum
                    .as_deref()
                    .map(|sum| at(sum, quote_scale))
                    .transpose()?,
                trade_count: None,
                taker_buy_volume: None,
            });
        }
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

fn at(raw: &str, scale: u8) -> Result<i64, SourceError> {
    senken_core::parse_scaled(raw.trim(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use super::bar_source_contract;
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Clock, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Real responses recorded 2026-09-02, one per contract path.
    const USDT_PERP: &[u8] = include_bytes!("../tests/fixtures/candles_1h_usdt_perp.json");
    const BTC_PERP: &[u8] = include_bytes!("../tests/fixtures/candles_1h_btc_perp.json");
    const DELIVERY: &[u8] = include_bytes!("../tests/fixtures/candles_1h_usdt_delivery.json");

    /// A clock fixed well after the fixtures, so every row in them counts
    /// as closed and none is dropped for being the forming one.
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

    fn range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_300_000).unwrap(),
            UnixNanos::from_secs(1_788_400_000).unwrap(),
        )
        .unwrap()
    }

    async fn bars_from(body: &'static [u8], source_id: &'static str) -> Vec<senken_series::Bar> {
        let server = serving(body).await;
        let source = bar_source_contract(source_id, "futures/usdt", client(), Arc::new(FixedClock))
            .with_url(server.uri());
        source
            .bars(
                &SourceSymbol::assume("BTC_USDT"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap()
    }

    /// The object shape spot's positional-string parser cannot read at
    /// all — this is the whole reason the two are separate sources.
    #[tokio::test]
    async fn a_usdt_perpetual_page_decodes_to_bars() {
        let bars = bars_from(USDT_PERP, crate::USDT_PERP_ID).await;

        assert!(!bars.is_empty());
        let first = &bars[0];
        assert_eq!(first.ts_open.as_nanos() / 1_000_000_000, 1_788_332_400);
        // "77579.1" at the page's shared one-digit scale.
        assert_eq!(first.open, 775_791);
        assert_eq!(first.close, 774_398);
    }

    /// `v` counts contracts, and this plugin's perpetual is 0.0001 BTC
    /// each — so publishing it as a base amount would be four orders of
    /// magnitude out. Absent is the honest answer.
    #[tokio::test]
    async fn a_contract_count_is_never_published_as_a_base_volume() {
        let bars = bars_from(USDT_PERP, crate::USDT_PERP_ID).await;
        assert!(bars.iter().all(|bar| bar.volume == Volume::Absent));
        assert!(
            bars[0].quote_volume.is_some(),
            "`sum` is a genuine quote turnover and is carried"
        );
    }

    /// The inverse path answers the same shape.
    #[tokio::test]
    async fn a_btc_settled_perpetual_page_decodes_to_bars() {
        let bars = bars_from(BTC_PERP, crate::BTC_PERP_ID).await;
        assert!(!bars.is_empty());
        assert_eq!(bars[0].open, 775_447);
    }

    /// Delivery carries no `sum` at all, so its quote volume is absent
    /// rather than zero — a zero would read as "nothing traded".
    #[tokio::test]
    async fn a_delivery_page_has_no_quote_turnover() {
        let bars = bars_from(DELIVERY, crate::USDT_DELIVERY_ID).await;
        assert!(!bars.is_empty());
        assert!(bars.iter().all(|bar| bar.quote_volume.is_none()));
        assert_eq!(bars[0].open, 778_403);
    }

    /// These rows carry no closed flag, so a clock inside the fixture's
    /// own span must drop the rows that have not finished.
    #[tokio::test]
    async fn the_forming_candle_is_excluded_by_the_clock() {
        #[derive(Debug)]
        struct EarlyClock;
        #[async_trait::async_trait]
        impl Clock for EarlyClock {
            fn now(&self) -> UnixNanos {
                // Part-way through the second candle of the fixture.
                UnixNanos::from_secs(1_788_338_000).unwrap()
            }

            async fn sleep_until(&self, _t: UnixNanos) {}
        }
        let server = serving(USDT_PERP).await;
        let source = bar_source_contract(
            crate::USDT_PERP_ID,
            "futures/usdt",
            client(),
            Arc::new(EarlyClock),
        )
        .with_url(server.uri());

        let bars = source
            .bars(
                &SourceSymbol::assume("BTC_USDT"),
                BarSpec::new(1, BarUnit::Hour),
                range(),
            )
            .await
            .unwrap();

        assert_eq!(
            bars.len(),
            1,
            "only the candle that had finished by then survives"
        );
    }

    #[tokio::test]
    async fn an_unsupported_spec_is_rejected_not_silently_substituted() {
        let server = serving(USDT_PERP).await;
        let source = bar_source_contract(
            crate::USDT_PERP_ID,
            "futures/usdt",
            client(),
            Arc::new(FixedClock),
        )
        .with_url(server.uri());

        assert!(
            source
                .bars(
                    &SourceSymbol::assume("BTC_USDT"),
                    BarSpec::new(7, BarUnit::Minute),
                    range()
                )
                .await
                .is_err()
        );
    }
}
