//! OKX response parsing and symbol normalisation, shared by the native
//! adapter (`plugins/okx`) and its `wasm32-wasip2` component
//! (`plugins/okx/wasm`) — the one place either side decodes an OKX
//! document, so the two are never two independent parsers that happen to
//! agree by coincidence. See `plugins/okx/src/lib.rs` and
//! `plugins/okx/wasm/src/lib.rs` for the two callers.
//!
//! Every constant here (`HISTORY_CANDLES_MAX_ROWS`, the pagination
//! direction in [`history_candles_query`], the supported bar specs) is
//! copied from `plugins/okx/src/bars.rs`'s own already-fixture-tested
//! values, never written from OKX's documentation — see that module's own
//! doc comments for the citations.
//!
//! This crate depends on `senken-core` (confirmed to compile for
//! `wasm32-wasip2` before this crate took the dependency) for the fixed-
//! point decimal parser every other venue plugin in this workspace already
//! uses, and for [`senken_core::UnixNanos`]/[`senken_core::TimeRange`] —
//! not on `senken-venue` or `senken-marketdata`, both of which pull in
//! `reqwest`/`tokio` and would not compile for a wasm guest at all.

use senken_core::{TimeRange, UnixNanos, decimal_places, parse_increment, parse_scaled};
use serde::Deserialize;
use serde::de::{self, Visitor};

/// The tested cap of `/market/history-candles`, copied from
/// `plugins/okx/src/bars.rs`'s own `MAX_ROWS` — "verified, returned exactly
/// 100 rows", deliberately not the 300 the sibling `/market/candles`
/// endpoint accepts. Both the native adapter and the wasm component use
/// this exact number, from this one place, rather than each hand-writing
/// it.
pub const HISTORY_CANDLES_MAX_ROWS: u32 = 100;

/// The separator OKX uses between an instrument's legs. The one input to
/// [`normalise_okx_symbol`] — see that function's own docs for why a
/// venue's symbol rule must never be spelled out a second time at another
/// call site.
const SEPARATOR: char = '-';
/// OKX's own perpetual-swap suffix, stripped before normalising — a swap's
/// `instId` is `BTC-USD-SWAP`, and the cross-venue symbol drops the
/// `-SWAP` the same way the plain pair's dash is dropped.
const SWAP_SUFFIX: &str = "-SWAP";

/// Normalises an OKX `instId` (`BTC-USDT`, `BTC-USD-SWAP`) into the
/// cross-venue symbol (`BTCUSDT`, `BTCUSD`).
///
/// **The one function this venue normalises a symbol through.** Called
/// from [`parse_spot_instruments`] (building the instrument catalog) and
/// from `plugins/okx/src/feed.rs` (decoding a live frame) — `AGENTS.md`
/// documents Deribit and Crypto.com each shipping with the two call sites
/// silently disagreeing on separators, found only by comparing them by
/// hand. Keeping both call sites behind this one function, rather than
/// two copies of the same trim-and-uppercase, is what makes that class of
/// bug impossible here rather than merely avoided today.
#[must_use]
pub fn normalise_okx_symbol(inst_id: &str) -> String {
    inst_id
        .trim_end_matches(SWAP_SUFFIX)
        .chars()
        .filter(|c| !c.is_whitespace() && *c != SEPARATOR)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Why parsing an OKX document failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    /// The bytes were not the shape this parser expects.
    #[error("could not decode OKX response: {0}")]
    Decode(String),
    /// OKX answered with an application-level error code inside a `200`.
    #[error("OKX rejected the request: {0}")]
    Rejected(String),
}

/// A number as OKX actually sends it: a JSON string, a JSON number, or
/// either in scientific notation (`SCI-USDT`'s `lotSz` is `1e-8` in the
/// real recorded fixture). Mirrors `senken_venue::Num` field-for-field —
/// reimplemented here, not imported, because `senken-venue` depends on
/// `reqwest`/`tokio` and would not compile for `wasm32-wasip2` for the sake
/// of one decimal parser (the same trade-off
/// `crates/plugin-host/tests/fixtures/venue-example` already documents for
/// itself).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RawNum(String);

impl RawNum {
    /// The `(scale, size)` pair this value implies as a price tick or
    /// quantity step. `None` when not a usable increment — empty,
    /// unparseable, or non-positive.
    fn increment(&self) -> Option<(u8, i64)> {
        parse_increment(&self.0)
    }

    /// The value truncated to a whole integer — mirrors
    /// `senken_venue::Num::as_i64`. Used for OKX's `expTime`, always a
    /// whole millisecond count but sent as a decimal string like every
    /// other field this type wraps.
    fn as_i64(&self) -> Option<i64> {
        let text = self.0.trim();
        let text = text.split_once('.').map_or(text, |(whole, _)| whole);
        text.parse().ok()
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
///
/// Kept alongside the newer, kind-aware [`Instrument`] rather than folded
/// into it: `plugins/okx/wasm`'s existing spot component only ever names
/// this narrower shape, and [`parse_spot_instruments`] still produces it —
/// changing its public fields would be an unrelated break to a caller this
/// slice of work does not touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotInstrument {
    /// Normalised symbol (`BTCUSDT`).
    pub symbol: String,
    /// OKX's own `instId` (`BTC-USDT`), sent back to [`history_candles_query`]
    /// verbatim.
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

/// What kind of contract an instrument is — mirrors
/// `senken_marketdata::instrument::InstrumentKind`/`wit/senken.wit`'s
/// `instrument-kind` case for case. Reimplemented rather than imported:
/// this crate depends on neither `senken-marketdata` nor the plugin
/// bindings, so that native and WASM can share it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentKind {
    /// Immediate exchange of base for quote.
    Spot,
    /// Dated future.
    Future,
    /// Option contract.
    Option,
    /// Perpetual swap.
    Perpetual,
}

/// Which OKX instrument-catalog endpoint a document came from, and so which
/// [`InstrumentKind`] every row in it maps to — mirrors
/// `plugins/okx/src`'s own (now-removed) private `Market` enum, moved here
/// so both the native adapter and the wasm component select it the same
/// way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Market {
    /// `instType=SPOT`.
    Spot,
    /// `instType=SWAP` — perpetual swaps.
    Swap,
    /// `instType=FUTURES` — dated futures.
    Futures,
    /// `instType=OPTION`.
    Option,
}

impl Market {
    /// The [`InstrumentKind`] every row from this market's endpoint has.
    #[must_use]
    pub fn kind(self) -> InstrumentKind {
        match self {
            Self::Spot => InstrumentKind::Spot,
            Self::Swap => InstrumentKind::Perpetual,
            Self::Futures => InstrumentKind::Future,
            Self::Option => InstrumentKind::Option,
        }
    }
}

/// Whether the venue currently accepts orders for an instrument — mirrors
/// `senken_marketdata::instrument::InstrumentStatus` case for case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstrumentStatus {
    /// Orders are accepted and matched.
    Trading,
    /// Temporarily suspended; expected to resume.
    Halted,
    /// Listed but not yet trading.
    PreOpen,
    /// Delisted or otherwise permanently closed.
    Closed,
    /// A venue test symbol; never real liquidity.
    Test,
    /// OKX reported a state this parser does not recognise.
    Unknown,
}

/// What a derivative is collateralised and settled in — mirrors
/// `senken_marketdata::instrument::Settlement` case for case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    /// Settled in the quote currency (`USDT`-margined).
    Linear,
    /// Settled in the base currency (coin-margined).
    Inverse,
    /// Settled in a third currency that is neither leg of the pair. OKX
    /// never reports this itself (`ctType` is only ever `linear` or
    /// `inverse`); kept for the same reason
    /// `senken_marketdata::instrument::Settlement` carries it — the shape
    /// this parser's result is mapped onto has the case, and OKX's absence
    /// of it is a venue fact about OKX, not a reason to drop the case here.
    Quanto,
}

/// Which side an option confers — mirrors
/// `senken_marketdata::instrument::OptionRight` case for case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionRight {
    /// The right to buy.
    Call,
    /// The right to sell.
    Put,
}

/// The strike of an option, as fixed-point at its own scale — mirrors
/// `senken_marketdata::instrument::OptionTerms` field for field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionTerms {
    /// Whether the option is a call or a put.
    pub right: OptionRight,
    /// Decimal places in `strike`.
    pub strike_scale: u8,
    /// Strike price at `strike_scale`.
    pub strike: i64,
}

/// The terms only a derivative carries — mirrors
/// `senken_marketdata::instrument::Contract` field for field. Present on
/// [`Instrument::contract`] exactly when [`Instrument::kind`] is not
/// [`InstrumentKind::Spot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    /// Currency positions settle in: `USDT` for a linear contract, `BTC`
    /// for an inverse one.
    pub settle: String,
    /// Linear or inverse.
    pub settlement: Settlement,
    /// Expiry instant, UTC. `None` for a perpetual, which never expires —
    /// never a sentinel far-future date.
    pub expiry: Option<UnixNanos>,
    /// Decimal places in `contract_size`.
    pub size_scale: u8,
    /// Units of the underlying one contract represents, at `size_scale`.
    pub contract_size: i64,
    /// Strike and right, for an option.
    pub option: Option<OptionTerms>,
}

/// One instrument this venue lists — spot or any derivative kind, field for
/// field what `wit/senken.wit`'s `venue::instrument` record needs and what
/// `senken_marketdata::Instrument` is built from, for every market OKX
/// serves. A caller on either side of the native/wasm boundary maps this
/// onto its own type with a plain field copy plus a case-for-case enum
/// translation, never a second parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    /// Normalised symbol (`BTCUSDT`, `BTCUSD`).
    pub symbol: String,
    /// OKX's own `instId` (`BTC-USDT`, `BTC-USD-SWAP`), sent back to
    /// [`history_candles_query`] verbatim.
    pub source_symbol: String,
    /// Human-readable name (`BTC / USDT`, `BTC / USD perpetual`).
    pub name: String,
    /// Base asset code.
    pub base: String,
    /// Quote asset code.
    pub quote: String,
    /// Contract type.
    pub kind: InstrumentKind,
    /// Current venue status.
    pub status: InstrumentStatus,
    /// Decimal places in a price.
    pub price_scale: u8,
    /// Minimum price increment at `price_scale`.
    pub tick_size: i64,
    /// Decimal places in a quantity.
    pub qty_scale: u8,
    /// Minimum quantity increment at `qty_scale`.
    pub step_size: i64,
    /// Derivative terms. `Some` exactly when `kind` is not `Spot`; `None`
    /// for spot.
    pub contract: Option<Contract>,
}

#[derive(Debug, Deserialize)]
struct InstrumentsResponse {
    code: String,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    data: Vec<RawInstrument>,
}

/// The fields OKX's `GET /api/v5/public/instruments` sends that any market
/// this crate parses needs. One shape covers every `instType`: `baseCcy`/
/// `quoteCcy` are populated on spot and empty on every derivative, which
/// instead carries the pair in `uly` (or, for some index-tracking swaps,
/// only `instFamily`) — see [`pair_of`]. Every container defaults to empty
/// so a field absent on one market's rows never breaks decoding another
/// market's.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawInstrument {
    inst_id: String,
    #[serde(default)]
    base_ccy: String,
    #[serde(default)]
    quote_ccy: String,
    /// The underlying pair, `BTC-USD`. Where a derivative's base and quote
    /// are read from, since `baseCcy`/`quoteCcy` are empty there.
    #[serde(default)]
    uly: String,
    /// The instrument family, `BTC-USD`. Some index-tracking swaps fill
    /// this in and leave `uly` empty.
    #[serde(default)]
    inst_family: String,
    /// What the contract settles in.
    #[serde(default)]
    settle_ccy: String,
    /// `linear` or `inverse`; empty on spot.
    #[serde(default)]
    ct_type: String,
    /// Units of `ctValCcy` per contract.
    #[serde(default)]
    ct_val: RawNum,
    /// Expiry in Unix milliseconds; empty on spot and perpetual swaps.
    #[serde(default)]
    exp_time: RawNum,
    /// `C` or `P` on options; empty otherwise.
    #[serde(default)]
    opt_type: String,
    /// Option strike.
    #[serde(default)]
    stk: RawNum,
    tick_sz: RawNum,
    lot_sz: RawNum,
    state: String,
}

/// Parses `GET /api/v5/public/instruments?instType=SPOT`'s body into
/// [`SpotInstrument`]s, skipping (never failing on) a row this parser
/// cannot represent — the same "one bad row is not fatal" discipline
/// `senken_venue::skip` documents for the native side, and the same
/// **only a currently tradable row survives** rule
/// `crates/plugin-host/tests/fixtures/venue-example` already established
/// for this exact pipeline: unlike a compiled-in `MarketDataSource`, the
/// WIT `instrument` record this crate's *wasm caller* ultimately feeds
/// (`plugins/okx/wasm`'s existing spot component) has no status field to
/// carry a suspended instrument's state on, so this narrower catalog omits
/// it outright rather than listing it with a status it has no way to
/// express. [`parse_instruments`] is the newer, kind-aware parser that
/// *can* carry status, and does not filter by it.
///
/// # Errors
/// [`CoreError::Decode`] if the bytes are not this response's shape;
/// [`CoreError::Rejected`] if OKX answered with a non-`"0"` `code`.
pub fn parse_spot_instruments(body: &[u8]) -> Result<Vec<SpotInstrument>, CoreError> {
    Ok(parse_instruments(body, Market::Spot)?
        .into_iter()
        .filter(|instrument| instrument.status == InstrumentStatus::Trading)
        .map(|instrument| SpotInstrument {
            symbol: instrument.symbol,
            source_symbol: instrument.source_symbol,
            name: instrument.name,
            base: instrument.base,
            quote: instrument.quote,
            price_scale: instrument.price_scale,
            tick_size: instrument.tick_size,
            qty_scale: instrument.qty_scale,
            step_size: instrument.step_size,
        })
        .collect())
}

/// Parses one OKX instrument-catalog document (`GET
/// /api/v5/public/instruments?instType=<SPOT|SWAP|FUTURES|OPTION>`) into
/// [`Instrument`]s of `market`'s own [`InstrumentKind`], skipping (never
/// failing on) a row this parser cannot represent — the same "one bad row
/// is not fatal" discipline `senken_venue::skip` documents for the native
/// side. Unlike [`parse_spot_instruments`], this does not drop a
/// non-tradable row: `status` carries it, for a caller whose own boundary
/// (a domain `Instrument`, or a WIT `instrument` record with a `status`
/// field) can represent it.
///
/// This is the one place either the native adapter or a `wasm32-wasip2`
/// component makes sense of a derivative row — `ctType`/`ctVal`/`expTime`/
/// `optType`/`stk`, the pair-resolution fallback through `uly`/
/// `instFamily`, and the status mapping are each read here once, not
/// reimplemented at both call sites.
///
/// # Errors
/// [`CoreError::Decode`] if the bytes are not this response's shape;
/// [`CoreError::Rejected`] if OKX answered with a non-`"0"` `code`.
pub fn parse_instruments(body: &[u8], market: Market) -> Result<Vec<Instrument>, CoreError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(|error| CoreError::Decode(error.to_string()))?;
    if response.code != "0" {
        return Err(CoreError::Rejected(format!(
            "code {}: {}",
            response.code, response.msg
        )));
    }
    Ok(response
        .data
        .into_iter()
        .filter_map(|raw| to_instrument(raw, market))
        .collect())
}

fn to_instrument(raw: RawInstrument, market: Market) -> Option<Instrument> {
    if raw.inst_id.trim().is_empty() {
        return None;
    }
    let (base, quote) = pair_of(&raw)?;
    let (price_scale, tick_size) = raw.tick_sz.increment()?;
    let (qty_scale, step_size) = raw.lot_sz.increment()?;

    let symbol = normalise_okx_symbol(&raw.inst_id);
    let kind = market.kind();
    let status = map_status(&raw.state);
    let (base, quote) = (base.to_owned(), quote.to_owned());
    let name = name_of(&base, &quote, kind);
    let contract = match kind {
        InstrumentKind::Spot => None,
        InstrumentKind::Future | InstrumentKind::Option | InstrumentKind::Perpetual => {
            Some(contract_of(&raw, &quote)?)
        }
    };

    Some(Instrument {
        symbol,
        source_symbol: raw.inst_id,
        name,
        base,
        quote,
        kind,
        status,
        price_scale,
        tick_size,
        qty_scale,
        step_size,
        contract,
    })
}

/// Base and quote for any instrument type.
///
/// Spot carries them directly; every derivative leaves them empty and puts
/// the pair in `uly` instead.
fn pair_of(raw: &RawInstrument) -> Option<(&str, &str)> {
    if !raw.base_ccy.is_empty() && !raw.quote_ccy.is_empty() {
        return Some((&raw.base_ccy, &raw.quote_ccy));
    }
    // Index-tracking swaps leave `uly` empty and carry the pair only in
    // `instFamily`; one not yet launched — `JP225-USDT-SWAP` in `preopen` —
    // leaves every field empty and names its legs only in the id. Each is
    // tried in turn before giving up.
    raw.uly
        .split_once('-')
        .or_else(|| raw.inst_family.split_once('-'))
        .or_else(|| {
            raw.inst_id
                .trim_end_matches("-SWAP")
                .split_once('-')
                .filter(|(base, quote)| !base.is_empty() && !quote.is_empty())
        })
}

/// Builds the [`Contract`] for a derivative row. `None` only when an
/// option's own `stk` cannot be read as an increment — every other field
/// here has a safe fallback (no expiry, a `1`-unit contract size) rather
/// than a reason to drop the whole row.
fn contract_of(raw: &RawInstrument, quote: &str) -> Option<Contract> {
    let settlement = if raw.ct_type.eq_ignore_ascii_case("inverse") {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = if raw.settle_ccy.is_empty() {
        quote.to_owned()
    } else {
        raw.settle_ccy.clone()
    };
    let mut contract = Contract {
        settle,
        settlement,
        expiry: None,
        size_scale: 0,
        contract_size: 1,
        option: None,
    };
    if let Some(expiry_ms) = raw.exp_time.as_i64().filter(|ms| *ms > 0) {
        contract.expiry = Some(UnixNanos::from_millis(expiry_ms)?);
    }
    if let Some((scale, size)) = raw.ct_val.increment() {
        contract.size_scale = scale;
        contract.contract_size = size;
    }
    if let Some(right) = option_right(&raw.opt_type) {
        let (strike_scale, strike) = raw.stk.increment()?;
        contract.option = Some(OptionTerms {
            right,
            strike_scale,
            strike,
        });
    }
    Some(contract)
}

fn option_right(opt_type: &str) -> Option<OptionRight> {
    match opt_type {
        "C" => Some(OptionRight::Call),
        "P" => Some(OptionRight::Put),
        _ => None,
    }
}

fn name_of(base: &str, quote: &str, kind: InstrumentKind) -> String {
    match kind {
        InstrumentKind::Spot => format!("{base} / {quote}"),
        InstrumentKind::Perpetual => format!("{base} / {quote} perpetual"),
        InstrumentKind::Option => format!("{base} / {quote} option"),
        InstrumentKind::Future => format!("{base} / {quote} future"),
    }
}

/// Maps OKX's own `state` string to [`InstrumentStatus`] — copied verbatim
/// from `plugins/okx/src`'s own (now-removed) private `map_status`, minus
/// the diagnostic log its native-only caller had: this crate logs nothing
/// (see [`SpotInstrument`]'s sibling parser, which never has either),
/// consistent with the rest of this module.
fn map_status(raw: &str) -> InstrumentStatus {
    match raw {
        "live" | "post_only" => InstrumentStatus::Trading,
        "suspend" | "rebase" | "settling" => InstrumentStatus::Halted,
        "preopen" => InstrumentStatus::PreOpen,
        "expired" => InstrumentStatus::Closed,
        "test" => InstrumentStatus::Test,
        _ => InstrumentStatus::Unknown,
    }
}

/// Mirrors `senken_series::BarUnit`'s cases OKX ever maps
/// ([`okx_interval`]), without depending on that crate — the wasm side has
/// no `senken-series` to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarUnit {
    /// One second — never mapped by [`okx_interval`]; carried only so this
    /// type mirrors every unit the two real callers know about.
    Second,
    /// One minute.
    Minute,
    /// One hour.
    Hour,
    /// One calendar day.
    Day,
    /// Seven days.
    Week,
    /// One calendar month.
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

/// The specs OKX's history-candles endpoint maps to a request `bar`
/// string, copied from `plugins/okx/src/bars.rs`'s own `supported_specs`.
/// Only 1-minute has actually been fetched and verified against a real
/// response; the rest follow the same already-shipped native decision,
/// not a new claim about OKX's documentation.
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
            step: 1,
            unit: BarUnit::Day,
        },
        BarSpec {
            step: 1,
            unit: BarUnit::Week,
        },
    ]
}

/// OKX's `bar` request-parameter string for `spec` (`15m`, `4H`, `1Dutc`).
/// `None` when `spec` is not one of [`supported_bar_specs`]. Copied
/// verbatim from `plugins/okx/src/bars.rs`'s own `interval_of`, including
/// always requesting the `utc` variant for Day and above (OKX's plain
/// `1D`/`1W`/`1M` opens at 16:00 UTC, not UTC midnight).
#[must_use]
pub fn okx_interval(spec: BarSpec) -> Option<String> {
    let step = spec.step;
    match spec.unit {
        BarUnit::Minute => Some(format!("{step}m")),
        BarUnit::Hour => Some(format!("{step}H")),
        BarUnit::Day => Some(format!("{step}Dutc")),
        BarUnit::Week => Some(format!("{step}Wutc")),
        BarUnit::Month => Some(format!("{step}Mutc")),
        BarUnit::Second => None,
    }
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
/// `plugins/okx/src/bars.rs`'s own `bars` checks before ever reaching this
/// crate.
#[must_use]
pub fn time_range_from_instants(start_nanos: i64, end_nanos: i64) -> Option<TimeRange> {
    TimeRange::new(
        UnixNanos::from_nanos(start_nanos),
        UnixNanos::from_nanos(end_nanos),
    )
}

/// The query string (no leading `?`) for one `bars()` page against
/// `/market/history-candles` — copied verbatim from
/// `plugins/okx/src/bars.rs`'s own `candles_url`.
///
/// `after=X` returns candles strictly **older** than `X`; `before=X` is
/// the newer direction ("the single most commonly mis-implemented
/// parameter in this API"). This is the one place either caller builds
/// that query, precisely so the direction is never spelled out a second
/// time and allowed to drift.
#[must_use]
pub fn history_candles_query(source_symbol: &str, interval: &str, range: TimeRange) -> String {
    format!(
        "instId={source_symbol}&bar={interval}&limit={HISTORY_CANDLES_MAX_ROWS}&after={}&before={}",
        range.end().as_millis(),
        range.start().as_millis() - 1,
    )
}

/// One OHLCV candle, field-for-field what `wit/senken.wit`'s `bar` record
/// and `senken_series::Bar` both need modulo their own volume/quote-volume
/// wrapping — OKX always reports both, so a caller on either side wraps
/// `volume`/`quote_volume` in whatever "real volume, always present" shape
/// its own boundary expects.
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
    /// Quote-asset volume traded, at `qty_scale`.
    pub quote_volume: i64,
}

/// One row of `GET /api/v5/market/history-candles`: nine positional
/// strings — open time, O, H, L, C, volume, quote volume, a second
/// quote-volume variant (unused, mirroring `plugins/okx/src/bars.rs`'s own
/// `RawCandle`), and `confirm`.
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

/// The smallest common scale that represents every value in `values`
/// without losing precision — the maximum of each value's own
/// [`senken_core::decimal_places`]. Reimplemented from
/// `senken_venue::common_scale`, which this crate cannot depend on without
/// pulling an HTTP stack into a WASM component.
fn common_scale<'a>(values: impl IntoIterator<Item = &'a str>) -> u8 {
    values.into_iter().map(decimal_places).max().unwrap_or(0)
}

/// Parses `GET /api/v5/market/history-candles`'s body into ascending
/// [`CandleBar`]s within `range`, dropping the still-forming newest row —
/// copied verbatim from `plugins/okx/src/bars.rs`'s own `bars` method.
///
/// # Errors
/// [`CoreError::Decode`] if the bytes are not this response's shape, or a
/// row's own fields do not parse at the scale this batch requires;
/// [`CoreError::Rejected`] if OKX answered with a non-`"0"` `code`.
pub fn parse_history_candles(body: &[u8], range: TimeRange) -> Result<Vec<CandleBar>, CoreError> {
    let response: CandlesResponse =
        serde_json::from_slice(body).map_err(|error| CoreError::Decode(error.to_string()))?;
    if response.code != "0" {
        return Err(CoreError::Rejected(format!(
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
            .map_err(|_| CoreError::Decode(format!("{ts:?} is not a valid timestamp")))?;
        let ts_open = UnixNanos::from_millis(ts_ms)
            .ok_or_else(|| CoreError::Decode(format!("open time {ts_ms} overflowed")))?;
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
            quote_volume: scaled(&quote_volume, qty_scale)?,
        });
    }

    // Ascending regardless of what the venue returns — OKX is descending.
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
        BarSpec, BarUnit, InstrumentKind, InstrumentStatus, Market, OptionRight, Settlement,
        common_scale, history_candles_query, map_status, normalise_okx_symbol, okx_interval,
        parse_history_candles, parse_instruments, parse_spot_instruments,
    };
    use senken_core::{TimeRange, UnixNanos};

    const SPOT: &[u8] = include_bytes!("../../tests/fixtures/instruments.json");
    const SWAP: &[u8] = include_bytes!("../../tests/fixtures/swap.json");
    const FUTURES: &[u8] = include_bytes!("../../tests/fixtures/futures.json");
    const OPTION: &[u8] = include_bytes!("../../tests/fixtures/option.json");
    const CANDLES: &[u8] = include_bytes!("../../tests/fixtures/candles_1m.json");

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::EPOCH,
            UnixNanos::from_millis(4_102_444_800_000).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_perpetual_swaps_dash_swap_suffix_and_dash_both_disappear() {
        assert_eq!(normalise_okx_symbol("BTC-USD-SWAP"), "BTCUSD");
        assert_eq!(normalise_okx_symbol("BTC-USDT"), "BTCUSDT");
    }

    #[test]
    fn spot_normalises_to_the_fixed_point_contract() {
        let instruments = parse_spot_instruments(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();
        assert_eq!(btc.source_symbol, "BTC-USDT");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!((btc.price_scale, btc.tick_size), (1, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (8, 1));
        assert_eq!(btc.name, "BTC / USDT");
    }

    #[test]
    fn a_suspended_instrument_is_omitted_not_merely_flagged() {
        let instruments = parse_spot_instruments(SPOT).unwrap();
        assert!(
            instruments.iter().all(|i| i.symbol != "OLDUSDT"),
            "this parser has no status field to mark it Halted on, so it must not appear at all"
        );
    }

    #[test]
    fn a_zero_lot_size_is_skipped_not_fatal_to_the_rest_of_the_catalog() {
        let instruments = parse_spot_instruments(SPOT).unwrap();
        assert_eq!(
            instruments.len(),
            3,
            "BTC, ETH, SCI — OLD suspended, BAD unusable lot size"
        );
        assert!(instruments.iter().all(|i| i.symbol != "BADUSDT"));
    }

    #[test]
    fn scientific_notation_lot_sizes_are_accepted() {
        let instruments = parse_spot_instruments(SPOT).unwrap();
        let sci = instruments.iter().find(|i| i.symbol == "SCIUSDT").unwrap();
        assert_eq!((sci.qty_scale, sci.step_size), (8, 1));
    }

    /// Ported from `plugins/okx/src`'s own (now-removed) unit test of the
    /// same name: [`parse_instruments`], unlike [`parse_spot_instruments`],
    /// does carry a suspended row's status rather than omitting it.
    #[test]
    fn a_suspended_spot_instrument_carries_halted_status_via_the_general_parser() {
        let instruments = parse_instruments(SPOT, Market::Spot).unwrap();
        let old = instruments
            .iter()
            .find(|i| i.symbol == "OLDUSDT")
            .expect("the general parser must not drop a suspended row");
        assert_eq!(old.status, InstrumentStatus::Halted);
    }

    #[test]
    fn swaps_take_their_pair_from_uly_since_base_ccy_is_empty() {
        // The trap: OKX leaves baseCcy/quoteCcy empty on every derivative.
        let instruments = parse_instruments(SWAP, Market::Swap).unwrap();
        let inverse = instruments.iter().find(|i| i.symbol == "BTCUSD").unwrap();

        assert_eq!(inverse.symbol, normalise_okx_symbol(&inverse.source_symbol));
        assert_eq!(inverse.base, "BTC");
        assert_eq!(inverse.quote, "USD");
        assert_eq!(inverse.kind, InstrumentKind::Perpetual);
        assert_eq!(inverse.source_symbol, "BTC-USD-SWAP");

        let contract = inverse.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, "BTC");
        assert_eq!(contract.expiry, None, "a swap never expires");
        assert_eq!((contract.size_scale, contract.contract_size), (0, 100));
    }

    #[test]
    fn dated_futures_carry_their_expiry() {
        let instruments = parse_instruments(FUTURES, Market::Futures).unwrap();
        let dated = instruments.first().unwrap();

        assert_eq!(dated.kind, InstrumentKind::Future);
        assert_eq!(dated.base, "BTC");
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
        assert!(
            dated.symbol.len() > "BTCUSD".len(),
            "the date must stay in the symbol: {}",
            dated.symbol
        );
    }

    #[test]
    fn options_carry_a_strike_and_a_right() {
        let instruments = parse_instruments(OPTION, Market::Option).unwrap();
        let call = instruments
            .iter()
            .find(|i| i.source_symbol.ends_with("-C"))
            .unwrap();

        assert_eq!(call.kind, InstrumentKind::Option);
        let terms = call.contract.as_ref().unwrap().option.as_ref().unwrap();
        assert_eq!(terms.right, OptionRight::Call);
        // The fixture's own recorded `stk` (`"70000"`, a bare integer) and
        // its scale (`0`, since it carries no fractional digits) —
        // asserted exactly rather than `> 0`, which a wrong scale would
        // still satisfy.
        assert_eq!((terms.strike_scale, terms.strike), (0, 70_000));
    }

    #[test]
    fn each_market_maps_to_its_own_instrument_kind() {
        assert_eq!(Market::Spot.kind(), InstrumentKind::Spot);
        assert_eq!(Market::Swap.kind(), InstrumentKind::Perpetual);
        assert_eq!(Market::Futures.kind(), InstrumentKind::Future);
        assert_eq!(Market::Option.kind(), InstrumentKind::Option);
    }

    #[test]
    fn maps_every_documented_state() {
        assert_eq!(map_status("live"), InstrumentStatus::Trading);
        assert_eq!(map_status("post_only"), InstrumentStatus::Trading);
        assert_eq!(map_status("suspend"), InstrumentStatus::Halted);
        assert_eq!(map_status("preopen"), InstrumentStatus::PreOpen);
        assert_eq!(map_status("test"), InstrumentStatus::Test);
        assert_eq!(map_status("something_new"), InstrumentStatus::Unknown);
    }

    #[test]
    fn an_application_error_code_is_a_rejection() {
        let body = br#"{"code":"50011","msg":"Rate limit reached","data":[]}"#;
        assert!(matches!(
            parse_instruments(body, Market::Spot),
            Err(super::CoreError::Rejected(reason)) if reason.contains("50011")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_instruments(b"<html>rate limited</html>", Market::Spot).is_err());
    }

    #[test]
    fn candles_decode_ascending_with_the_unconfirmed_row_dropped() {
        let bars = parse_history_candles(CANDLES, wide_range()).unwrap();
        assert_eq!(bars.len(), 4, "the unconfirmed newest row must be dropped");
        assert!(bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open));
        let first = bars[0];
        assert_eq!(
            first.ts_open,
            UnixNanos::from_millis(1_788_083_040_000).unwrap()
        );
        assert_eq!(first.price_scale, 1);
        assert_eq!(first.open, 780_343);
        assert_eq!(first.high, 780_401);
        assert_eq!(first.low, 780_342);
        assert_eq!(first.close, 780_401);
    }

    #[test]
    fn the_pagination_cursor_walks_backwards_correctly() {
        // `after` must be the range's *end* (strictly older than) and
        // `before` the range's start minus one millisecond (strictly
        // newer than) — getting these swapped silently walks the wrong
        // direction through history while still compiling.
        let range = TimeRange::new(
            UnixNanos::from_millis(1_788_066_600_000).unwrap(),
            UnixNanos::from_millis(1_788_066_720_000).unwrap(),
        )
        .unwrap();
        let query = history_candles_query("BTC-USDT", "1m", range);
        assert!(query.contains("after=1788066720000"), "{query}");
        assert!(query.contains("before=1788066599999"), "{query}");
    }

    #[test]
    fn day_and_above_always_request_the_utc_variant() {
        assert_eq!(
            okx_interval(BarSpec {
                step: 1,
                unit: BarUnit::Day
            })
            .as_deref(),
            Some("1Dutc")
        );
        assert_eq!(
            okx_interval(BarSpec {
                step: 1,
                unit: BarUnit::Week
            })
            .as_deref(),
            Some("1Wutc")
        );
    }

    #[test]
    fn common_scale_of_an_empty_batch_is_zero() {
        assert_eq!(common_scale(std::iter::empty()), 0);
    }
}
