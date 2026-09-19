//! Bybit response parsing and symbol normalisation, shared by the native
//! adapter (`plugins/bybit`) and its `wasm32-wasip2` component
//! (`plugins/bybit/wasm`) — the one place either side decodes a Bybit
//! document, so the two are never two independent parsers that happen to
//! agree by coincidence. See `plugins/bybit/src/lib.rs` and
//! `plugins/bybit/wasm/src/lib.rs` for the two callers.
//!
//! Every constant here (`KLINE_MAX_ROWS`, the closure rule against the
//! response's own `time` field, the supported bar specs, the pagination
//! parameters) is copied from `plugins/bybit/src/bars.rs`'s own
//! already-fixture-tested values, never written from Bybit's documentation
//! — see that module's own doc comments for the citations, most of them
//! verified live in the same session that wrote them.
//!
//! This crate depends on `senken-core` (confirmed to compile for
//! `wasm32-wasip2` before `okx-core` first took the dependency, and again
//! here) for the fixed-point decimal parser every other venue plugin in
//! this workspace already uses, and for
//! [`senken_core::UnixNanos`]/[`senken_core::TimeRange`] — not on
//! `senken-venue`, `senken-marketdata` or `senken-series`, all of which
//! either pull in `reqwest`/`tokio` or are simply not needed here; a wasm
//! guest has none of them to name.

use senken_core::{TimeRange, UnixNanos, decimal_places, parse_increment, parse_scaled};
use serde::Deserialize;
use serde::de::{self, Visitor};

/// Bybit's own tested cap for `/v5/market/kline`, copied from
/// `plugins/bybit/src/bars.rs`'s own `MAX_ROWS` — "`limit=1500` returns
/// HTTP 200 with exactly 1000 rows", verified live rather than only
/// documented. Both the native adapter and the wasm component use this
/// exact number, from this one place.
pub const KLINE_MAX_ROWS: u32 = 1000;

/// The separator this venue's normalised catalog strips — copied from
/// `plugins/bybit/src/lib.rs`'s own call to `senken_venue::normalise_symbol`
/// (`&['-']`): spot symbols carry none (`BTCUSDT`), but option symbols do
/// (`BTC-26SEP25-…`), and `plugins/bybit/src/feed.rs`'s own module docs
/// name the same separator for the same reason. The one input to
/// [`normalise_bybit_symbol`].
const SEPARATORS: [char; 1] = ['-'];

/// Normalises a Bybit symbol (`BTCUSDT`, `BTC-26SEP25-160000-C-USDT`) into
/// the cross-venue symbol.
///
/// **The one function this venue normalises a symbol through.** Called from
/// [`parse_spot_instruments`] (building the instrument catalog) and from
/// `plugins/bybit/src/feed.rs` (decoding a live frame) — `AGENTS.md`
/// documents Deribit and Crypto.com each shipping with the two call sites
/// silently disagreeing on separators, found only by comparing them by
/// hand. Keeping both call sites behind this one function, rather than two
/// copies of the same trim-and-uppercase, is what makes that class of bug
/// impossible here rather than merely avoided today. Mirrors
/// `senken_venue::normalise_symbol(symbol, &['-'])` exactly — reimplemented
/// here, not imported, for the same reason `RawNum` is (see that type's
/// own docs).
#[must_use]
pub fn normalise_bybit_symbol(symbol: &str) -> String {
    symbol
        .chars()
        .filter(|c| !c.is_whitespace() && !SEPARATORS.contains(c))
        .flat_map(char::to_uppercase)
        .collect()
}

/// Why parsing a Bybit document failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// The bytes were not the shape this parser expects.
    #[error("could not decode Bybit response: {0}")]
    Decode(String),
    /// Bybit answered with a non-zero `retCode` inside a `200`.
    #[error("Bybit rejected the request: {0}")]
    Rejected(String),
}

/// A number as Bybit actually sends it: a JSON string, a JSON number, or
/// either in scientific notation. Mirrors `senken_venue::Num` field-for-
/// field — reimplemented here, not imported, because `senken-venue`
/// depends on `reqwest`/`tokio` and would not compile for `wasm32-wasip2`
/// for the sake of one decimal parser (the same trade-off `okx-core`'s own
/// `RawNum` already documents for itself).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RawNum(String);

impl RawNum {
    /// The `(scale, size)` pair this value implies as a price tick or
    /// quantity step. `None` when not a usable increment — empty,
    /// unparseable, or non-positive.
    fn increment(&self) -> Option<(u8, i64)> {
        parse_increment(&self.0)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<'de> Deserialize<'de> for RawNum {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RawNumVisitor;

        impl Visitor<'_> for RawNumVisitor {
            type Value = RawNum;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a number, as a JSON number or a decimal string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<RawNum, E> {
                Ok(RawNum(
                    senken_core::plain_decimal(value)
                        .map(std::borrow::Cow::into_owned)
                        .unwrap_or_default(),
                ))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<RawNum, E> {
                Ok(RawNum(value.to_string()))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<RawNum, E> {
                Ok(RawNum(value.to_string()))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<RawNum, E> {
                if value.is_finite() {
                    Ok(RawNum(value.to_string()))
                } else {
                    Err(E::custom("number is not finite"))
                }
            }
        }

        deserializer.deserialize_any(RawNumVisitor)
    }
}

/// One spot instrument, field-for-field what `wit/senken.wit`'s
/// `venue::instrument` record needs and what
/// `senken_marketdata::Instrument::spot` is built from — a caller on
/// either side of the native/wasm boundary maps this onto its own type
/// with a plain field copy, never a second parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotInstrument {
    /// Normalised symbol (`BTCUSDT`).
    pub symbol: String,
    /// Bybit's own symbol, sent back to [`kline_query`] verbatim — spot's
    /// wire format happens to equal its normalised form (both `BTCUSDT`),
    /// unlike OKX's dashed `instId`, but this crate still carries it
    /// through its own field rather than assuming the two always coincide.
    pub source_symbol: String,
    /// Human-readable name (`BTC / USDT`).
    pub name: String,
    /// Base asset code.
    pub base: String,
    /// Quote asset code.
    pub quote: String,
    /// Decimal places in a price.
    pub price_scale: u8,
    /// Minimum price increment at `price_scale`.
    pub tick_size: i64,
    /// Decimal places in a quantity.
    pub qty_scale: u8,
    /// Minimum quantity increment at `qty_scale`.
    pub step_size: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstrumentsResponse {
    ret_code: i64,
    #[serde(default)]
    ret_msg: String,
    #[serde(default)]
    result: InstrumentsResult,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstrumentsResult {
    #[serde(default)]
    list: Vec<RawSpotInstrument>,
    /// Empty when this response is the whole catalog. The native adapter
    /// follows this to the end; a caller that cannot must not quietly
    /// return the first page as if it were everything.
    #[serde(default, rename = "nextPageCursor")]
    next_page_cursor: String,
}

/// The fields `GET /v5/market/instruments-info?category=spot` sends that a
/// spot instrument actually needs. Mirrors
/// `plugins/bybit/src/api.rs`'s own `RawInstrument`, trimmed to the spot
/// branch only — this crate never sees `linear`/`inverse`/`option`
/// documents, since `wit/senken.wit`'s `instrument` record has no field for
/// a derivative's contract terms (the same boundary `okx-core` documents
/// for its own `RawSpotInstrument`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSpotInstrument {
    symbol: String,
    #[serde(default)]
    base_coin: String,
    #[serde(default)]
    quote_coin: String,
    status: String,
    #[serde(default)]
    price_filter: RawPriceFilter,
    #[serde(default)]
    lot_size_filter: RawLotSizeFilter,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawPriceFilter {
    #[serde(default)]
    tick_size: RawNum,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLotSizeFilter {
    /// Spot names the quantity step `basePrecision`; every derivative
    /// category names the same thing `qtyStep` — copied from
    /// `plugins/bybit/src/api.rs`'s own `LotSizeFilter::step`, though this
    /// crate only ever parses the spot branch.
    #[serde(default)]
    base_precision: RawNum,
    #[serde(default)]
    qty_step: RawNum,
}

impl RawLotSizeFilter {
    fn step(&self) -> &RawNum {
        if self.qty_step.is_empty() {
            &self.base_precision
        } else {
            &self.qty_step
        }
    }
}

/// Parses `GET /v5/market/instruments-info?category=spot`'s body into
/// [`SpotInstrument`]s, skipping (never failing on) a row this parser
/// cannot represent, and — the same choice `okx-core`'s own
/// `parse_spot_instruments` makes, established by
/// `crates/plugin-host/tests/fixtures/venue-example` — omitting outright
/// any row that is not currently tradable (`status == "Trading"`) rather
/// than listing it with a status this crate's output has no field to carry
/// (`wit/senken.wit`'s `instrument` record has none).
///
/// # Errors
/// [`CoreError::Decode`] if the bytes are not this response's shape;
/// [`CoreError::Rejected`] if Bybit answered with a non-zero `retCode`.
/// The cursor `GET /v5/market/instruments-info` reports for the page after
/// this one, or `None` when this response is the last page.
///
/// Split out from [`parse_spot_instruments`] rather than returned beside the
/// instruments because a caller that follows the cursor and one that only
/// needs to know whether it exists want different things, and a partial
/// catalogue returned as if it were complete is the failure this exists to
/// make impossible: a reader cannot tell a venue with few pairs from one
/// whose catalogue was cut off after the first page.
///
/// # Errors
///
/// Returns [`CoreError::Decode`] when the body is not the shape this venue
/// documents, and [`CoreError::Rejected`] when it carries a non-zero
/// `retCode`.
pub fn spot_next_page_cursor(body: &[u8]) -> Result<Option<String>, CoreError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(|error| CoreError::Decode(error.to_string()))?;
    if response.ret_code != 0 {
        return Err(CoreError::Rejected(format!(
            "retCode {}: {}",
            response.ret_code, response.ret_msg
        )));
    }
    let cursor = response.result.next_page_cursor.trim().to_owned();
    Ok((!cursor.is_empty()).then_some(cursor))
}

/// Every tradable spot instrument in one `GET
/// /v5/market/instruments-info?category=spot` response, in the order the
/// venue sent them. Rows the venue is not currently trading are dropped
/// rather than flagged: the boundary this crosses has no field to carry a
/// status, so a row that survives here is one that can be traded.
///
/// This reads a single response. Whether that response is the whole
/// catalogue is [`spot_next_page_cursor`]'s question, and a caller serving
/// instruments to anyone must ask it.
///
/// # Errors
///
/// Returns [`CoreError::Decode`] when the body is not the shape this venue
/// documents, and [`CoreError::Rejected`] when it carries a non-zero
/// `retCode`.
pub fn parse_spot_instruments(body: &[u8]) -> Result<Vec<SpotInstrument>, CoreError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(|error| CoreError::Decode(error.to_string()))?;
    if response.ret_code != 0 {
        return Err(CoreError::Rejected(format!(
            "retCode {}: {}",
            response.ret_code, response.ret_msg
        )));
    }
    Ok(response
        .result
        .list
        .into_iter()
        .filter_map(to_spot_instrument)
        .collect())
}

fn to_spot_instrument(raw: RawSpotInstrument) -> Option<SpotInstrument> {
    if raw.symbol.trim().is_empty() {
        return None;
    }
    if raw.base_coin.is_empty() || raw.quote_coin.is_empty() {
        return None;
    }
    if raw.status != "Trading" {
        return None;
    }
    let (tick_scale, tick_size) = raw.price_filter.tick_size.increment()?;
    let (qty_scale, step_size) = raw.lot_size_filter.step().increment()?;

    Some(SpotInstrument {
        symbol: normalise_bybit_symbol(&raw.symbol),
        name: format!("{} / {}", raw.base_coin, raw.quote_coin),
        source_symbol: raw.symbol,
        base: raw.base_coin,
        quote: raw.quote_coin,
        price_scale: tick_scale,
        tick_size,
        qty_scale,
        step_size,
    })
}

/// Mirrors `senken_series::BarUnit`'s cases Bybit ever maps
/// ([`bybit_interval`]), without depending on that crate — the wasm side
/// has no `senken-series` to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarUnit {
    /// One second — never mapped by [`bybit_interval`]; carried only so
    /// this type mirrors every unit the two real callers know about.
    Second,
    /// One minute.
    Minute,
    /// One hour.
    Hour,
    /// One calendar day.
    Day,
    /// Seven days.
    Week,
    /// One calendar month — never mapped by [`bybit_interval`]: Bybit's
    /// calendar month has no fixed duration, and this crate's closure
    /// check needs one (see [`duration_nanos`]).
    Month,
}

/// A bar timeframe, step and unit, the same shape
/// `senken_series::BarSpec`/the WIT `bar-spec` record already are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarSpec {
    /// How many `unit`s per bar.
    pub step: u32,
    /// The unit `step` counts.
    pub unit: BarUnit,
}

/// The specs Bybit's kline endpoint maps to a request `interval` string,
/// copied from `plugins/bybit/src/bars.rs`'s own `supported_specs`. Only
/// `interval=1` (one minute) has actually been fetched and verified
/// against a real response; the rest follow that already-shipped native
/// decision (Bybit's own enumerated, non-arbitrary set of valid intervals
/// for this endpoint), not a new claim about Bybit's documentation.
#[must_use]
pub fn supported_bar_specs() -> Vec<BarSpec> {
    vec![
        BarSpec {
            step: 1,
            unit: BarUnit::Minute,
        },
        BarSpec {
            step: 3,
            unit: BarUnit::Minute,
        },
        BarSpec {
            step: 5,
            unit: BarUnit::Minute,
        },
        BarSpec {
            step: 15,
            unit: BarUnit::Minute,
        },
        BarSpec {
            step: 30,
            unit: BarUnit::Minute,
        },
        BarSpec {
            step: 1,
            unit: BarUnit::Hour,
        },
        BarSpec {
            step: 2,
            unit: BarUnit::Hour,
        },
        BarSpec {
            step: 4,
            unit: BarUnit::Hour,
        },
        BarSpec {
            step: 6,
            unit: BarUnit::Hour,
        },
        BarSpec {
            step: 12,
            unit: BarUnit::Hour,
        },
        BarSpec {
            step: 1,
            unit: BarUnit::Day,
        },
        BarSpec {
            step: 1,
            unit: BarUnit::Week,
        },
    ]
}

/// Bybit's `interval` request-parameter string for `spec` — `"1"` for one
/// minute, `"60"` for one hour (Bybit counts every sub-day interval in
/// minutes, not hours), `"D"`/`"W"` for a single day/week. `None` for
/// anything outside Bybit's own enumerated set
/// ([`supported_bar_specs`]), including every `Month` — Bybit's calendar
/// month has no fixed duration, and [`parse_klines`]'s closure check needs
/// one, so `Month` is deliberately never offered rather than fetched with
/// an unverified closure rule. Copied verbatim from
/// `plugins/bybit/src/bars.rs`'s own `interval_of`.
#[must_use]
pub fn bybit_interval(spec: BarSpec) -> Option<String> {
    match spec.unit {
        BarUnit::Minute => Some(spec.step.to_string()),
        BarUnit::Hour => Some((spec.step * 60).to_string()),
        BarUnit::Day if spec.step == 1 => Some("D".to_owned()),
        BarUnit::Week if spec.step == 1 => Some("W".to_owned()),
        _ => None,
    }
}

/// `spec`'s fixed duration in nanoseconds, `None` only for `Month` — the
/// same formula `senken_series::BarSpec::duration_nanos` already computes
/// (`crates/series/src/spec.rs`), reimplemented here for the same reason
/// `RawNum` is: a wasm guest has no `senken-series` to name. Used by
/// [`parse_klines`]'s own closure check, the same way
/// `plugins/bybit/src/bars.rs`'s native `bars` method uses the
/// `senken-series` original.
#[must_use]
pub fn duration_nanos(spec: BarSpec) -> Option<i64> {
    let unit_nanos: i64 = match spec.unit {
        BarUnit::Second => 1_000_000_000,
        BarUnit::Minute => 60_000_000_000,
        BarUnit::Hour => 3_600_000_000_000,
        BarUnit::Day => 86_400_000_000_000,
        BarUnit::Week => 7 * 86_400_000_000_000,
        BarUnit::Month => return None,
    };
    Some(unit_nanos * i64::from(spec.step))
}

/// Builds a [`TimeRange`] from a pair of raw nanosecond instants — the
/// shape a `venue-plugin` guest's `bars` export receives them in (WIT's
/// `instant` is a plain `s64` alias for nanoseconds since the epoch, the
/// same unit `senken_core::UnixNanos` is). Exposed here so `wasm/` never
/// has to name `senken-core` as a direct dependency of its own just to
/// construct the one value every other function in this crate already
/// takes as a `TimeRange`.
///
/// `None` when `start >= end` — the same "empty, not an error" case
/// `plugins/bybit/src/bars.rs`'s own `bars` checks before ever reaching
/// this crate.
#[must_use]
pub fn time_range_from_instants(start_nanos: i64, end_nanos: i64) -> Option<TimeRange> {
    TimeRange::new(
        UnixNanos::from_nanos(start_nanos),
        UnixNanos::from_nanos(end_nanos),
    )
}

/// The query string (no leading `?`) for one `bars()` call against
/// `/v5/market/kline` — copied verbatim from
/// `plugins/bybit/src/bars.rs`'s own inline URL construction.
///
/// Both ends inclusive — verified live (that module's own docs):
/// `start=1788081060000&end=1788081180000` on `BTCUSDT` returned exactly
/// the three rows opening at those two timestamps and the one between
/// them.
#[must_use]
pub fn kline_query(source_symbol: &str, interval: &str, range: TimeRange) -> String {
    format!(
        "category=spot&symbol={source_symbol}&interval={interval}&limit={KLINE_MAX_ROWS}&start={}&end={}",
        range.start().as_millis(),
        range.end().as_millis() - 1,
    )
}

/// One OHLCV candle, field-for-field what `wit/senken.wit`'s `bar` record
/// and `senken_series::Bar` both need modulo their own volume/quote-volume
/// wrapping — Bybit always reports both, so a caller on either side wraps
/// `volume`/`quote_volume` in whatever "real volume, always present" shape
/// its own boundary expects. Bybit reports no trade count at all, so
/// neither side of this boundary can populate one — `None`, never a false
/// `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandleBar {
    /// The bar's open time.
    pub ts_open: UnixNanos,
    /// Decimal places shared by `open`/`high`/`low`/`close`.
    pub price_scale: u8,
    /// Opening price at `price_scale`.
    pub open: i64,
    /// High price at `price_scale`.
    pub high: i64,
    /// Low price at `price_scale`.
    pub low: i64,
    /// Closing price at `price_scale`.
    pub close: i64,
    /// Decimal places shared by `volume`/`quote_volume`.
    pub qty_scale: u8,
    /// Base-asset volume traded, at `qty_scale`.
    pub volume: i64,
    /// Quote-asset volume traded (Bybit's own `turnover`), at `qty_scale`.
    pub quote_volume: i64,
}

/// One row of `GET /v5/market/kline`: seven positional strings — open
/// time, O, H, L, C, volume, turnover. No trade count, mirroring
/// `plugins/bybit/src/bars.rs`'s own `RawKline`.
type RawKline = (String, String, String, String, String, String, String);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KlineResponse {
    ret_code: i64,
    #[serde(default)]
    ret_msg: String,
    #[serde(default)]
    result: KlineResult,
    /// Server time in milliseconds — the "now" [`parse_klines`] closes
    /// candles against, since Bybit sets no per-row confirmation flag.
    time: i64,
}

#[derive(Debug, Default, Deserialize)]
struct KlineResult {
    #[serde(default)]
    list: Vec<RawKline>,
}

/// The smallest common scale that represents every value in `values`
/// without losing precision — the maximum of each value's own
/// [`senken_core::decimal_places`]. Reimplemented from
/// `senken_venue::common_scale` for the same reason [`RawNum`] is: that
/// crate is not a dependency this crate can take.
fn common_scale<'a>(values: impl IntoIterator<Item = &'a str>) -> u8 {
    values.into_iter().map(decimal_places).max().unwrap_or(0)
}

/// Parses `GET /v5/market/kline`'s body into ascending [`CandleBar`]s
/// within `range`, dropping any row whose computed close time has not yet
/// passed the response's own server clock — copied verbatim from
/// `plugins/bybit/src/bars.rs`'s own `bars` method. `spec` supplies the
/// bar's fixed duration ([`duration_nanos`]) that closure needs; a `spec`
/// [`bybit_interval`] would reject is never passed in by either caller.
///
/// # Errors
/// [`CoreError::Decode`] if the bytes are not this response's shape, a
/// row's own fields do not parse at the scale this batch requires, or
/// `spec` has no fixed duration to close candles against; [`CoreError::Rejected`]
/// if Bybit answered with a non-zero `retCode`.
pub fn parse_klines(
    body: &[u8],
    spec: BarSpec,
    range: TimeRange,
) -> Result<Vec<CandleBar>, CoreError> {
    let response: KlineResponse =
        serde_json::from_slice(body).map_err(|error| CoreError::Decode(error.to_string()))?;
    if response.ret_code != 0 {
        return Err(CoreError::Rejected(format!(
            "retCode {}: {}",
            response.ret_code, response.ret_msg
        )));
    }
    let duration = duration_nanos(spec)
        .ok_or_else(|| CoreError::Decode(format!("{spec:?} has no fixed duration")))?;

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
            .map_err(|_| CoreError::Decode(format!("{ts:?} is not a valid timestamp")))?;
        let ts_open = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| CoreError::Decode(format!("open time {ts_ms} overflowed")))?;

        // Bybit sets no confirmation flag: a candle is closed only once its
        // computed close time has passed the server's own clock.
        let close_ms = ts_ms
            .checked_add(duration / 1_000_000)
            .ok_or_else(|| CoreError::Decode("close time overflowed".to_owned()))?;
        if close_ms > server_now_ms {
            continue;
        }
        if !range.contains(ts_open) {
            // Defensive: the query is bounded server-side already, but
            // never trust a venue's pagination boundaries alone.
            continue;
        }

        bars.push(CandleBar {
            ts_open,
            price_scale,
            open: scaled(&open, price_scale)?,
            high: scaled(&high, price_scale)?,
            low: scaled(&low, price_scale)?,
            close: scaled(&close, price_scale)?,
            qty_scale,
            volume: scaled(&volume, qty_scale)?,
            quote_volume: scaled(&turnover, qty_scale)?,
        });
    }

    // Ascending regardless of what the venue returns — Bybit is descending.
    bars.sort_by_key(|bar| bar.ts_open);
    Ok(bars)
}

/// Parses `raw` at `scale`, mapping an unparseable value — which should
/// never happen given `scale` was computed from this exact batch of
/// strings — to a decode error rather than panicking or guessing.
fn scaled(raw: &str, scale: u8) -> Result<i64, CoreError> {
    parse_scaled(raw, scale)
        .ok_or_else(|| CoreError::Decode(format!("{raw:?} does not parse at scale {scale}")))
}

#[cfg(test)]
mod tests {
    use super::{
        BarSpec, BarUnit, common_scale, duration_nanos, kline_query, normalise_bybit_symbol,
        parse_klines, parse_spot_instruments,
    };
    use senken_core::{TimeRange, UnixNanos};

    const SPOT: &[u8] = include_bytes!("../../tests/fixtures/spot.json");
    const KLINE: &[u8] = include_bytes!("../../tests/fixtures/kline_1m.json");

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::EPOCH,
            UnixNanos::from_millis(4_102_444_800_000).unwrap(),
        )
        .unwrap()
    }

    fn one_minute() -> BarSpec {
        BarSpec {
            step: 1,
            unit: BarUnit::Minute,
        }
    }

    #[test]
    fn a_dash_and_whitespace_both_disappear_and_the_result_is_upper_case() {
        assert_eq!(normalise_bybit_symbol("BTCUSDT"), "BTCUSDT");
        assert_eq!(
            normalise_bybit_symbol("BTC-26SEP25-160000-C-USDT"),
            "BTC26SEP25160000CUSDT"
        );
    }

    #[test]
    fn a_non_trading_status_is_omitted_not_merely_unflagged() {
        // The real recorded fixture (`spot.json`) carries only one, already
        // `"Trading"`, row — this crate has no field to carry a non-
        // tradable status across the wasm boundary at all (`wit/senken.
        // wit`'s `instrument` record has none), the same omission
        // `okx-core`'s own `parse_spot_instruments` already established.
        // The status string and field names here are the fixture's own
        // (`spot.json`'s `status: "Trading"`) and native's own already-
        // verified vocabulary (`plugins/bybit/src/lib.rs`'s `map_status`
        // maps `"Closed"`), not invented — only the second row is
        // synthetic, isolating the one branch the real fixture cannot
        // exercise.
        let body = br#"{"retCode":0,"result":{"list":[
            {"symbol":"BTCUSDT","baseCoin":"BTC","quoteCoin":"USDT","status":"Trading","priceFilter":{"tickSize":"0.1"},"lotSizeFilter":{"basePrecision":"0.000001"}},
            {"symbol":"OLDUSDT","baseCoin":"OLD","quoteCoin":"USDT","status":"Closed","priceFilter":{"tickSize":"0.1"},"lotSizeFilter":{"basePrecision":"0.000001"}}
        ]}}"#;
        let instruments = parse_spot_instruments(body).unwrap();
        assert_eq!(instruments.len(), 1);
        assert!(instruments.iter().all(|i| i.symbol != "OLDUSDT"));
    }

    #[test]
    fn spot_normalises_to_the_fixed_point_contract() {
        let instruments = parse_spot_instruments(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();
        assert_eq!(btc.source_symbol, "BTCUSDT");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.name, "BTC / USDT");
        assert!(btc.tick_size >= 1 && btc.step_size >= 1);
    }

    #[test]
    fn klines_decode_ascending_with_the_still_forming_row_dropped() {
        // The real fixture's top-level `time` (1788081203987) is before the
        // newest row's computed close (1788081180000 + 60000 =
        // 1788081240000), so that row must be dropped; the other four have
        // already closed.
        let bars = parse_klines(KLINE, one_minute(), wide_range()).unwrap();
        assert_eq!(
            bars.len(),
            4,
            "the still-forming newest row must be dropped"
        );
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_080_940_000).unwrap()
        );
        assert_eq!(first.open, 780_777);
        assert_eq!(first.high, 780_778);
        assert_eq!(first.low, 780_777);
        assert_eq!(first.close, 780_777);
    }

    #[test]
    fn the_pagination_window_is_both_ends_inclusive() {
        let range = TimeRange::new(
            UnixNanos::from_millis(1_788_081_060_000).unwrap(),
            UnixNanos::from_millis(1_788_081_180_000).unwrap(),
        )
        .unwrap();
        let query = kline_query("BTCUSDT", "1", range);
        assert!(query.contains("start=1788081060000"), "{query}");
        assert!(query.contains("end=1788081179999"), "{query}");
    }

    #[test]
    fn duration_is_none_only_for_month() {
        assert_eq!(
            duration_nanos(BarSpec {
                step: 1,
                unit: BarUnit::Minute
            }),
            Some(60_000_000_000)
        );
        assert_eq!(
            duration_nanos(BarSpec {
                step: 1,
                unit: BarUnit::Week
            }),
            Some(7 * 86_400_000_000_000)
        );
        assert_eq!(
            duration_nanos(BarSpec {
                step: 1,
                unit: BarUnit::Month
            }),
            None
        );
    }

    #[test]
    fn common_scale_of_an_empty_batch_is_zero() {
        assert_eq!(common_scale(std::iter::empty()), 0);
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::{CoreError, spot_next_page_cursor};

    #[test]
    fn a_last_page_reports_no_cursor() {
        let body = br#"{"retCode":0,"retMsg":"OK","result":{"list":[],"nextPageCursor":""}}"#;
        assert_eq!(spot_next_page_cursor(body).unwrap(), None);
    }

    #[test]
    fn a_page_that_has_more_reports_the_cursor_the_venue_sent() {
        let body =
            br#"{"retCode":0,"retMsg":"OK","result":{"list":[],"nextPageCursor":"page%3D2"}}"#;
        assert_eq!(
            spot_next_page_cursor(body).unwrap(),
            Some("page%3D2".to_owned())
        );
    }

    #[test]
    fn a_rejected_body_is_rejected_here_too_rather_than_read_as_a_last_page() {
        let body = br#"{"retCode":10001,"retMsg":"params error","result":{"list":[]}}"#;
        assert!(matches!(
            spot_next_page_cursor(body),
            Err(CoreError::Rejected(_))
        ));
    }
}
