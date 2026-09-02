//! BitMEX bar fetching — `GET /api/v1/trade/bucketed`.
//!
//! # The five cross-venue traps, answered from a real response
//!
//! Every fact below was observed live on 2026-09-02, 06:05 UTC, against
//! `symbol=XBTUSD&binSize=1h&count=5&reverse=true`, except the row cap —
//! see point 4.
//!
//! 1. **Sort direction**: the recording behind this module's fixture asked
//!    for `reverse=true` (descending) to get a compact, recent sample; the
//!    documented default with `reverse` omitted is ascending. Rows are
//!    re-sorted here regardless of which way a request asked.
//! 2. **Timestamp representation**: an ISO 8601 string — but **it labels
//!    the *end* of the bucket, not the start**. This is BitMEX's own,
//!    well-known inversion of the convention every other venue in this
//!    workspace uses, and it was confirmed from the data rather than
//!    assumed: the recording ran at `06:05:16Z`, with `partial` left at its
//!    default (`false`, excluding the still-forming bucket); the newest row
//!    this endpoint returned is labelled `06:00:00.000Z`. Read as a
//!    *start* label, that would be the `06:00`–`07:00` bucket — still
//!    forming at `06:05`, and therefore impossible for a `partial=false`
//!    response to include. The only reading consistent with what was
//!    actually returned is that `06:00:00.000Z` labels the **end** of the
//!    `05:00`–`06:00` bucket, which had genuinely closed five minutes
//!    earlier. [`BarSource::bars`] performs the subtraction this implies.
//! 3. **Closed-candle detection**: BitMEX excludes the still-forming bucket
//!    by default; `partial=true` would include it, and this source never
//!    sends that parameter. No [`senken_series::Clock`] is needed as a
//!    result — the one bar source in this batch of four that doesn't.
//! 4. **Row cap**: approximately 12 000, refused loudly beyond it. This
//!    number comes from this workspace's own prior live audit of this
//!    venue, not a boundary this module's own recording (`count=5`)
//!    reproduced — nor is the exact refusal text quoted here, since
//!    reproducing it would need a second live request past the cap, and
//!    this module's fixture recording is limited to one request per
//!    endpoint.
//! 5. **Pagination direction**: `startTime`/`endTime`, ISO 8601, forward —
//!    but shifted forward by one bucket length from the requested
//!    [`TimeRange`], to compensate for the end-of-bucket labelling in point
//!    2: a bucket whose *end* label falls in `[start + length, end +
//!    length)` is exactly the bucket whose *open* — what this workspace's
//!    model actually stores — falls in `[start, end)`. [`BarSource::bars`]
//!    still drops anything that lands outside the requested range
//!    regardless, in case the shift or the venue's own filtering is ever
//!    imperfect.
//!
//! # No quote volume, no taker volume
//!
//! `trades` is unambiguous — a literal count — and is kept as
//! [`Bar::trade_count`]. `turnover`, `homeNotional` and `foreignNotional`
//! are also in the response, but their meaning shifts across this venue's
//! three settlement kinds (linear, inverse, quanto — see `lib.rs`'s own
//! module docs on why quanto exists at all here), and getting that wrong
//! silently would misreport quote volume rather than merely omit it; they
//! are left unread, and `quote_volume`/`taker_buy_volume` stay `None`.
//!
//! # What was verified, and what is a documented assumption
//!
//! Only `1h` was requested and measured. `1m`, `5m` and `1d` are BitMEX's
//! own published, fixed enum of bucket sizes — this venue has no `3h`,
//! `15m`, or any other combination Binance-style venues offer, so unlike
//! `plugins/binance`'s wider assumption this is a **closed** list with
//! nothing arithmetic about it.

use senken_core::{TimeRange, UnixNanos, parse_scaled};
use senken_marketdata::SourceSymbol;
use senken_marketdata::source::SourceError;
use senken_plugin::BarSource;
use senken_series::{Bar, BarSpec, BarUnit, Volume};
use senken_venue::{Num, VenueClient, common_scale, iso8601_ms};
use serde::Deserialize;

const BUCKETED_URL: &str = "https://www.bitmex.com/api/v1/trade/bucketed";

/// The row cap from this workspace's own prior live audit of this venue —
/// approximate, and not independently re-tested by this change's own
/// fixture request. See the module docs' point 4.
const MAX_ROWS: usize = 12_000;

/// The weight charged against this source's [`senken_venue::LimitGroup`]
/// per call — this workspace's own conservative proactive budget, the same
/// value every other bar source here uses, not a venue-documented number.
const CANDLES_FETCH_COST: u32 = 5;

/// One row of `GET /api/v1/trade/bucketed`: a JSON object, unlike the
/// positional rows Gate and Bitfinex send. `timestamp` labels the bucket's
/// **end** — see the module docs.
#[derive(Debug, Deserialize)]
struct RawCandle {
    timestamp: String,
    open: Num,
    high: Num,
    low: Num,
    close: Num,
    volume: Num,
    trades: u32,
}

/// Every `(step, unit, binSize)` this source serves — BitMEX's own
/// published, fixed set of bucket sizes, with nothing arithmetic about it
/// (see the module docs).
const INTERVALS: &[(u32, BarUnit, &str)] = &[
    (1, BarUnit::Minute, "1m"),
    (5, BarUnit::Minute, "5m"),
    (1, BarUnit::Hour, "1h"),
    (1, BarUnit::Day, "1d"),
];

/// The specs this source can fetch — every entry of [`INTERVALS`], and
/// nothing else.
fn supported_specs() -> Vec<BarSpec> {
    INTERVALS
        .iter()
        .map(|&(step, unit, _)| BarSpec::new(step, unit))
        .collect()
}

/// BitMEX's `binSize` for `spec`, or `None` when `spec` is not one of the
/// four this venue serves.
fn bin_size_of(spec: BarSpec) -> Option<&'static str> {
    INTERVALS
        .iter()
        .find(|&&(step, unit, _)| step == spec.step.get() && unit == spec.unit)
        .map(|&(_, _, bin_size)| bin_size)
}

/// BitMEX bars, fetched through a [`VenueClient`]. Closure comes entirely
/// from the venue's own default exclusion of the still-forming bucket
/// (`partial` is never sent — see the module docs), so no
/// [`senken_series::Clock`] is needed.
#[derive(Debug, Clone)]
pub struct BitmexBarSource {
    url: String,
    client: VenueClient,
    supported: Vec<BarSpec>,
}

impl BitmexBarSource {
    /// Points this source at a different URL — a regional host, a mirror,
    /// or a local stand-in in tests. Mirrors `HttpSource::with_url`.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Builds the request URL for one `bars()` call.
    ///
    /// `startTime`/`endTime` are `range` shifted forward by one bucket
    /// length — see the module docs' point 5 on why the end-labelled
    /// bucket timestamp needs that shift to line up with the open-time
    /// window this source is actually asked for.
    fn candles_url(
        &self,
        symbol: &str,
        bin_size: &str,
        start: UnixNanos,
        end: UnixNanos,
    ) -> String {
        format!(
            "{}?binSize={bin_size}&symbol={symbol}&count={MAX_ROWS}&startTime={start}&\
             endTime={end}",
            self.url
        )
    }
}

/// BitMEX bars.
#[must_use]
pub fn bar_source(client: VenueClient) -> BitmexBarSource {
    BitmexBarSource {
        url: BUCKETED_URL.to_owned(),
        client,
        supported: supported_specs(),
    }
}

#[async_trait::async_trait]
impl BarSource for BitmexBarSource {
    fn source_id(&self) -> &str {
        crate::SOURCE_ID
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
        let bin_size = bin_size_of(spec)
            .ok_or_else(|| SourceError::rejected(format!("unsupported bar spec {spec}")))?;
        // Every entry in `INTERVALS` names a fixed-width `BarUnit`, so this
        // is always `Some` for a spec `bin_size_of` just accepted.
        let Some(duration_nanos) = spec.duration_nanos() else {
            return Err(SourceError::rejected(format!(
                "{spec} has no fixed duration to shift the query window by"
            )));
        };

        // Shift the query window forward by one bucket length to compensate
        // for the end-of-bucket labelling — see the module docs' point 5.
        let Some(query_start) = range.start().as_nanos().checked_add(duration_nanos) else {
            return Err(SourceError::rejected(
                "requested range overflowed shifting for BitMEX's end-labelled buckets",
            ));
        };
        let Some(query_end) = range.end().as_nanos().checked_add(duration_nanos) else {
            return Err(SourceError::rejected(
                "requested range overflowed shifting for BitMEX's end-labelled buckets",
            ));
        };
        let url = self.candles_url(
            symbol.as_str(),
            bin_size,
            UnixNanos::from_nanos(query_start),
            UnixNanos::from_nanos(query_end),
        );
        let body = self.client.get(&url, CANDLES_FETCH_COST).await?;
        let rows: Vec<RawCandle> = serde_json::from_slice(&body).map_err(SourceError::decode)?;

        let price_scale = common_scale(rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }));
        let qty_scale = common_scale(rows.iter().map(|row| row.volume.as_str()));

        let mut bars = Vec::with_capacity(rows.len());
        let mut outside = 0usize;
        for row in rows {
            let Some(label_ms) = iso8601_ms(&row.timestamp) else {
                return Err(SourceError::decode(format!(
                    "{:?} is not a valid timestamp",
                    row.timestamp
                )));
            };
            let Some(label) = UnixNanos::from_millis(label_ms) else {
                return Err(SourceError::decode(format!(
                    "bucket end time {label_ms}ms overflowed"
                )));
            };
            // The venue's own field names the bucket's *end* — see the
            // module docs — so the open this workspace's model stores is
            // one bucket length earlier.
            let Some(ts_open) = label
                .as_nanos()
                .checked_sub(duration_nanos)
                .map(UnixNanos::from_nanos)
            else {
                return Err(SourceError::decode(format!(
                    "bucket end time {label} underflowed subtracting {spec}'s length"
                )));
            };

            if !range.contains(ts_open) {
                outside += 1;
                continue;
            }

            bars.push(Bar {
                ts_open,
                open: scaled(&row.open, price_scale)?,
                high: scaled(&row.high, price_scale)?,
                low: scaled(&row.low, price_scale)?,
                close: scaled(&row.close, price_scale)?,
                volume: Volume::Real(scaled(&row.volume, qty_scale)?),
                trade_count: Some(row.trades),
                // Neither reported unambiguously by this endpoint — see the
                // module docs.
                quote_volume: None,
                taker_buy_volume: None,
            });
        }

        // See Gate's identical guard for why an answer made entirely of
        // out-of-range rows is reported rather than swallowed: it is the
        // one shape indistinguishable, from the caller's side, from a
        // venue that has genuinely nothing for the window asked.
        if bars.is_empty() && outside > 0 {
            return Err(SourceError::rejected(format!(
                "answered with {outside} closed bars, none inside the requested range — \
                 the range parameters were not honoured"
            )));
        }

        // Ascending regardless of what the venue returns for this request's
        // own `reverse` value.
        bars.sort_by_key(|bar| bar.ts_open);
        Ok(bars)
    }
}

/// Parses `raw` at `scale`, mapping an unparseable value — which should
/// never happen given `scale` was computed from this exact batch of
/// values — to a decode error rather than panicking or guessing.
fn scaled(raw: &Num, scale: u8) -> Result<i64, SourceError> {
    parse_scaled(raw.as_str(), scale)
        .ok_or_else(|| SourceError::decode(format!("{raw} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use senken_core::{TimeRange, UnixNanos};
    use senken_marketdata::SourceSymbol;
    use senken_plugin::BarSource;
    use senken_series::{BarSpec, BarUnit, Volume};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::bar_source;

    /// A real `GET
    /// /api/v1/trade/bucketed?binSize=1h&symbol=XBTUSD&count=5&reverse=true`
    /// response, recorded 2026-09-02T06:05:16Z. Five rows, descending by
    /// their end-of-bucket label, `partial` left at its default (excluded).
    const CANDLES: &[u8] = include_bytes!("../tests/fixtures/candles_1h.json");

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("XBTUSD")
    }

    fn hour() -> BarSpec {
        BarSpec::new(1, BarUnit::Hour)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(1_788_300_000).unwrap(),
            UnixNanos::from_secs(1_788_340_000).unwrap(),
        )
        .unwrap()
    }

    async fn mock_source() -> (MockServer, super::BitmexBarSource) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client()).with_url(format!("{}/bucketed", server.uri()));
        (server, source)
    }

    #[tokio::test]
    async fn the_end_labelled_timestamp_is_converted_to_an_open_time() {
        // The fixture's newest row is labelled 06:00:00Z. Read as an open
        // time that bucket would still be forming; this venue excludes
        // forming buckets by default, so the label must mean the bucket's
        // end instead — the open this source stores is one hour earlier.
        let (_server, source) = mock_source().await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        let newest = bars.last().unwrap();
        assert_eq!(newest.ts_open, UnixNanos::from_secs(1_788_325_200).unwrap());
    }

    #[tokio::test]
    async fn no_forming_candle_is_ever_returned_because_none_was_asked_for() {
        // This source never sends `partial=true`; every row the fixture
        // carries is one BitMEX itself already considers closed, so all
        // five survive.
        let (_server, source) = mock_source().await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();
        assert_eq!(bars.len(), 5);
    }

    #[tokio::test]
    async fn rows_come_back_ascending_even_though_the_venue_sent_them_descending() {
        let (_server, source) = mock_source().await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_310_800).unwrap(),
            "the oldest bucket, end-labelled 02:00:00Z, opens at 01:00:00Z"
        );
    }

    #[tokio::test]
    async fn ohlcv_and_trade_count_decode_from_the_named_fields() {
        let (_server, source) = mock_source().await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        // Oldest row once sorted: end label 02:00:00Z, open 77191.8, high
        // 77248.9, low 76634.8, close 76835.1, trades 87, at the batch's
        // common price scale of one decimal.
        let first = &bars[0];
        assert_eq!(first.open, 771_918);
        assert_eq!(first.high, 772_489);
        assert_eq!(first.low, 766_348);
        assert_eq!(first.close, 768_351);
        assert_eq!(first.trade_count, Some(87));
        assert!(matches!(first.volume, Volume::Real(v) if v > 0));
        assert!(
            first.quote_volume.is_none(),
            "meaning shifts by settlement kind — see the module docs"
        );
    }

    #[tokio::test]
    async fn each_row_s_open_matches_the_previous_row_s_close_exactly() {
        // BitMEX's buckets tile with no gap: unlike Bitstamp's fixture,
        // this one is exactly continuous.
        let (_server, source) = mock_source().await;
        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        for pair in bars.windows(2) {
            assert_eq!(pair[1].open, pair[0].close);
        }
    }

    #[tokio::test]
    async fn bars_outside_the_requested_range_are_dropped() {
        let (_server, source) = mock_source().await;
        // Only the bucket opening at 03:00:00Z (1_788_318_000) falls inside.
        let narrow = TimeRange::new(
            UnixNanos::from_secs(1_788_317_000).unwrap(),
            UnixNanos::from_secs(1_788_319_000).unwrap(),
        )
        .unwrap();

        let bars = source.bars(&symbol(), hour(), narrow).await.unwrap();

        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].ts_open,
            UnixNanos::from_secs(1_788_318_000).unwrap()
        );
    }

    #[tokio::test]
    async fn a_venue_that_ignores_the_requested_range_is_reported_not_swallowed() {
        let (_server, source) = mock_source().await;
        let elsewhere = TimeRange::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            UnixNanos::from_secs(1_700_003_600).unwrap(),
        )
        .unwrap();

        let error = source
            .bars(&symbol(), hour(), elsewhere)
            .await
            .expect_err("an answer entirely outside the range is a failure, not an absence");

        assert!(
            error.to_string().contains("not honoured"),
            "the error must say the range was ignored: {error}"
        );
    }

    #[tokio::test]
    async fn an_empty_answer_inside_a_valid_range_is_an_absence_not_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(&b"[]"[..], "application/json"))
            .mount(&server)
            .await;
        let source = bar_source(test_client()).with_url(format!("{}/bucketed", server.uri()));

        let bars = source.bars(&symbol(), hour(), wide_range()).await.unwrap();

        assert!(bars.is_empty());
    }

    #[test]
    fn a_spec_this_venue_does_not_serve_has_no_bin_size() {
        // BitMEX's set is closed and small: no 3h, no 15m, no 30m.
        assert!(super::bin_size_of(BarSpec::new(3, BarUnit::Hour)).is_none());
        assert!(super::bin_size_of(BarSpec::new(15, BarUnit::Minute)).is_none());
        assert!(super::bin_size_of(BarSpec::new(1, BarUnit::Week)).is_none());
    }

    #[test]
    fn every_supported_spec_maps_to_a_bin_size() {
        let source = bar_source(test_client());
        for spec in source.supported() {
            assert!(
                super::bin_size_of(*spec).is_some(),
                "{spec} is offered but has no binSize mapping"
            );
        }
    }

    #[tokio::test]
    async fn an_inverted_range_asks_the_venue_nothing_at_all() {
        let server = MockServer::start().await;
        let source = bar_source(test_client()).with_url(format!("{}/bucketed", server.uri()));
        let inverted = TimeRange::new(
            UnixNanos::from_secs(1_788_318_000).unwrap(),
            UnixNanos::from_secs(1_788_318_000).unwrap(),
        );

        if let Some(range) = inverted {
            assert!(
                source
                    .bars(&symbol(), hour(), range)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn max_rows_is_the_documented_approximate_cap() {
        let source = bar_source(test_client());
        assert_eq!(source.max_rows(), 12_000);
    }
}
