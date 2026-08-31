//! The normalised instrument record every source produces.

use senken_core::UnixNanos;
use serde::{Deserialize, Serialize};

/// What kind of contract an instrument is.
///
/// A plain tag, kept `Copy` so it is cheap to filter and display. The terms
/// that only a derivative carries — how it settles, when it expires — live
/// in [`Contract`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
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

impl InstrumentKind {
    /// `true` for everything that is not [`Spot`](Self::Spot), i.e. every
    /// kind that should carry a [`Contract`].
    #[must_use]
    pub fn is_derivative(self) -> bool {
        !matches!(self, Self::Spot)
    }
}

/// What a derivative is collateralised and settled in.
///
/// This is the difference between the venue's `BTCUSDT` perpetual and its
/// `BTCUSD` one: same underlying, different money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Settlement {
    /// Settled in the quote currency (`USDT`-margined). Profit is linear in
    /// the price.
    Linear,
    /// Settled in the base currency (coin-margined). Profit is non-linear in
    /// the price, since the collateral is the thing being priced.
    Inverse,
    /// Settled in a third currency that is neither leg of the pair — an
    /// `ETH/USD` contract margined in Bitcoin, say. The settlement currency
    /// floats against both, so the payoff carries its own exchange risk.
    Quanto,
}

/// Which side an option confers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
pub enum OptionRight {
    /// The right to buy.
    Call,
    /// The right to sell.
    Put,
}

/// The strike of an option, as fixed-point at its own scale.
///
/// The strike needs a scale of its own: a venue may quote an option's
/// premium in one currency and its strike in another, so `price_scale` on
/// the instrument does not describe it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OptionTerms {
    /// Whether the option is a call or a put.
    pub right: OptionRight,
    /// Decimal places in `strike`.
    pub strike_scale: u8,
    /// Strike price at `strike_scale`.
    pub strike: i64,
}

/// The terms only a derivative carries.
///
/// Present on every instrument whose [`InstrumentKind::is_derivative`] is
/// `true`, absent on spot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Contract {
    /// Currency positions settle in: `USDT` for a linear contract, `BTC`
    /// for an inverse one.
    pub settle: String,
    /// Linear or inverse.
    pub settlement: Settlement,
    /// Expiry instant, UTC. `None` for a perpetual, which never expires —
    /// never a sentinel far-future date, which some venues report.
    pub expiry: Option<UnixNanos>,
    /// Decimal places in `contract_size`.
    pub size_scale: u8,
    /// Units of the underlying one contract represents, at `size_scale`.
    /// `1` at scale `0` when the venue quotes in the underlying directly.
    pub contract_size: i64,
    /// Strike and right, for an option.
    pub option: Option<OptionTerms>,
}

impl Contract {
    /// A perpetual or dated contract settling in `settle`, one unit of the
    /// underlying per contract. Refine it with the `with_*` methods.
    #[must_use]
    pub fn new(settle: impl Into<String>, settlement: Settlement) -> Self {
        Self {
            settle: settle.into(),
            settlement,
            expiry: None,
            size_scale: 0,
            contract_size: 1,
            option: None,
        }
    }

    /// Sets the expiry instant.
    #[must_use]
    pub fn with_expiry(mut self, expiry: UnixNanos) -> Self {
        self.expiry = Some(expiry);
        self
    }

    /// Sets how much of the underlying one contract represents.
    #[must_use]
    pub fn with_contract_size(mut self, size_scale: u8, contract_size: i64) -> Self {
        self.size_scale = size_scale;
        self.contract_size = contract_size;
        self
    }

    /// Sets the option terms.
    #[must_use]
    pub fn with_option(mut self, right: OptionRight, strike_scale: u8, strike: i64) -> Self {
        self.option = Some(OptionTerms {
            right,
            strike_scale,
            strike,
        });
        self
    }
}

/// Whether the venue currently accepts orders for an instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
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
    /// The venue reported a status this crate does not recognise.
    Unknown,
}

impl InstrumentStatus {
    /// `true` when orders can be placed right now.
    #[must_use]
    pub fn is_tradable(self) -> bool {
        matches!(self, Self::Trading)
    }

    /// `true` when the instrument should appear in search results.
    ///
    /// Halted and pre-open instruments are shown because a user may be
    /// looking for them on purpose; closed, test and unknown ones are not.
    #[must_use]
    pub fn is_searchable(self) -> bool {
        matches!(self, Self::Trading | Self::Halted | Self::PreOpen)
    }
}

/// An instrument's venue-native identifier — `Instrument`'s `source_symbol`
/// field, wrapped so a fetch call cannot be handed the normalised `symbol`
/// field by mistake.
///
/// `normalise_symbol` throws away separator *position* when it builds the
/// normalised symbol, so there is no general way back from it to this one.
/// Passing the wrong string to a venue's bars endpoint fails outright on a
/// separator-using venue (OKX's `BTC-USDT`) but **silently succeeds**
/// wherever the two forms happen to coincide (Binance's own wire format
/// already equals its normalised symbol) — which makes the mistake look
/// venue-specific and is miserable to diagnose from a bug report alone.
///
/// The ordinary way to obtain one is [`Instrument::source_symbol()`] — the
/// *method*, not the field of the same name; Rust resolves `i.source_symbol`
/// (no parens) to the field and `i.source_symbol()` to this method without
/// ambiguity — so a caller that reaches for the normalised `symbol` field at
/// a `BarSource::bars` call site gets a compile error instead of a
/// hard-to-diagnose runtime one. [`Self::assume`] is the one documented
/// exception, for a boundary that already receives a string it cannot
/// upgrade into an [`Instrument`] — see its own docs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSymbol(Box<str>);

impl SourceSymbol {
    /// The venue-native identifier, verbatim — e.g. OKX's `BTC-USDT`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps `raw` as a [`SourceSymbol`] **without** going through
    /// [`Instrument::source_symbol()`] — the caller is asserting, not
    /// proving, that `raw` is already venue-native.
    ///
    /// This exists for exactly one kind of caller: a generic, symbol-agnostic
    /// boundary that only ever sees a bare string and has no `Instrument` to
    /// derive one from. Concretely, `senken_loader::PluginBarSource` bridges
    /// a real `senken_plugin::BarSource` onto `senken-loader`'s own fetch
    /// port, whose `bars(symbol: &str, ..)` signature
    /// predates this type and carries whatever string a
    /// `senken_series::SeriesKey` was built with. Inside *this* workspace
    /// that string is documented as the **normalised** symbol
    /// (`senken_series::SeriesKey::symbol`), so calling this from
    /// `senken-runtime` would reintroduce exactly the mistake this type
    /// exists to make unrepresentable — which is
    /// why `senken-runtime`'s own wiring resolves a real [`SourceSymbol`]
    /// from the instrument catalog first and does not route through
    /// `PluginBarSource` at all (see the M8.1/M8.2 executor's report). A
    /// different, standalone consumer of `senken-loader` that keys its own
    /// `SeriesKey`s by venue-native symbol in the first place would use this
    /// safely; calling it with a normalised symbol anywhere is the bug F7
    /// describes.
    #[must_use]
    pub fn assume(raw: impl Into<Box<str>>) -> Self {
        Self(raw.into())
    }
}

impl std::fmt::Display for SourceSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SourceSymbol {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// One tradable instrument, normalised across venues.
///
/// # Fixed-point contract
///
/// Prices and quantities are integers at a fixed number of decimal places,
/// never floats. Every source **must** populate the four numeric fields so
/// that:
///
/// * `price_scale` is the number of decimal places in a price, and
///   `tick_size` is the venue's minimum price increment expressed at that
///   scale. A quoted price `p` is represented as `p × 10^price_scale`.
/// * `qty_scale` and `step_size` are the same for quantities.
/// * Both scales are the *minimal* scale that represents every valid value:
///   derive them from the tick/step with [`decimal_places`], not from a
///   venue's "asset precision" field. A `0.01` tick means `price_scale = 2`
///   and `tick_size = 1`, on every venue.
///
/// That last rule is what makes instruments comparable across sources: two
/// venues quoting the same tick produce identical `(price_scale, tick_size)`
/// pairs.
///
/// [`decimal_places`]: crate::decimal::decimal_places
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Instrument {
    /// Normalised symbol, `{base}{quote}` with no separator (`BTCUSDT`).
    /// Unique within a source; used as the symbol half of an [`InstrumentId`].
    ///
    /// [`InstrumentId`]: crate::id::InstrumentId
    pub symbol: String,
    /// The venue's own identifier, verbatim (`BTC-USDT` on OKX).
    pub source_symbol: String,
    /// Human-readable name (`BTC / USDT`).
    pub name: String,
    /// Base asset code.
    pub base: String,
    /// Quote asset code.
    pub quote: String,
    /// Contract type.
    pub kind: InstrumentKind,
    /// Current venue status.
    pub status: InstrumentStatus,
    /// Decimal places in a price. See the fixed-point contract above.
    pub price_scale: u8,
    /// Minimum price increment at `price_scale`. Always `>= 1`.
    pub tick_size: i64,
    /// Decimal places in a quantity.
    pub qty_scale: u8,
    /// Minimum quantity increment at `qty_scale`. Always `>= 1`.
    pub step_size: i64,
    /// Derivative terms. `Some` exactly when [`kind`](Self::kind) is a
    /// derivative; `None` for spot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<Contract>,
}

impl Instrument {
    /// A spot instrument. Prices and quantities still follow the
    /// fixed-point contract above.
    #[must_use]
    pub fn spot(
        symbol: impl Into<String>,
        source_symbol: impl Into<String>,
        base: impl Into<String>,
        quote: impl Into<String>,
    ) -> Self {
        let (base, quote) = (base.into(), quote.into());
        Self {
            symbol: symbol.into(),
            source_symbol: source_symbol.into(),
            name: format!("{base} / {quote}"),
            base,
            quote,
            kind: InstrumentKind::Spot,
            status: InstrumentStatus::Unknown,
            price_scale: 0,
            tick_size: 1,
            qty_scale: 0,
            step_size: 1,
            contract: None,
        }
    }

    /// A derivative on `base`/`quote` with the given terms.
    #[must_use]
    pub fn derivative(
        symbol: impl Into<String>,
        source_symbol: impl Into<String>,
        base: impl Into<String>,
        quote: impl Into<String>,
        kind: InstrumentKind,
        contract: Contract,
    ) -> Self {
        Self {
            kind,
            contract: Some(contract),
            ..Self::spot(symbol, source_symbol, base, quote)
        }
    }

    /// Sets the display name, which otherwise defaults to `BASE / QUOTE`.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Sets the venue status.
    #[must_use]
    pub fn with_status(mut self, status: InstrumentStatus) -> Self {
        self.status = status;
        self
    }

    /// Sets the price scale and tick, as returned by
    /// [`parse_increment`](crate::decimal::parse_increment).
    #[must_use]
    pub fn with_price_increment(mut self, (scale, tick): (u8, i64)) -> Self {
        self.price_scale = scale;
        self.tick_size = tick;
        self
    }

    /// Sets the quantity scale and step, as returned by
    /// [`parse_increment`](crate::decimal::parse_increment).
    #[must_use]
    pub fn with_qty_increment(mut self, (scale, step): (u8, i64)) -> Self {
        self.qty_scale = scale;
        self.step_size = step;
        self
    }

    /// The settlement currency for a derivative; `None` for spot.
    #[must_use]
    pub fn settle(&self) -> Option<&str> {
        self.contract.as_ref().map(|c| c.settle.as_str())
    }

    /// `true` when this instrument expires and that expiry has passed.
    /// Spot and perpetuals are never expired.
    #[must_use]
    pub fn is_expired_at(&self, now: UnixNanos) -> bool {
        self.contract
            .as_ref()
            .and_then(|c| c.expiry)
            .is_some_and(|expiry| expiry <= now)
    }

    /// This instrument's venue-native identifier, typed as [`SourceSymbol`]
    /// so it cannot be confused with the normalised `symbol` field at a
    /// `BarSource::bars`-style call site — see
    /// [`SourceSymbol`]'s own docs for why the mix-up is dangerous enough to
    /// need a distinct type rather than a doc comment.
    #[must_use]
    pub fn source_symbol(&self) -> SourceSymbol {
        SourceSymbol(self.source_symbol.as_str().into())
    }
}

#[cfg(test)]
mod tests {
    use super::{Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight, Settlement};
    use senken_core::UnixNanos;

    fn spot() -> Instrument {
        Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")
            .with_status(InstrumentStatus::Trading)
            .with_price_increment((2, 1))
            .with_qty_increment((8, 1))
    }

    /// The serialised shape below is what cached snapshots hold on disk.
    /// When this assertion breaks the layout changed: bump
    /// `INSTRUMENTS_SCHEMA_VERSION` (or restore compatibility) before
    /// updating the expected string.
    #[test]
    fn serialised_shape_matches_the_snapshot_schema() {
        let json = serde_json::to_string(&spot()).unwrap();
        assert_eq!(
            json,
            r#"{"symbol":"BTCUSDT","source_symbol":"BTC-USDT","name":"BTC / USDT","base":"BTC","quote":"USDT","kind":"Spot","status":"Trading","price_scale":2,"tick_size":1,"qty_scale":8,"step_size":1}"#
        );
        assert_eq!(serde_json::from_str::<Instrument>(&json).unwrap(), spot());
    }

    #[test]
    fn a_derivative_carries_its_contract_through_json() {
        let inverse = Instrument::derivative(
            "BTCUSD",
            "BTC-USD-SWAP",
            "BTC",
            "USD",
            InstrumentKind::Perpetual,
            Contract::new("BTC", Settlement::Inverse).with_contract_size(0, 100),
        );

        let json = serde_json::to_string(&inverse).unwrap();
        assert!(json.contains(r#""settlement":"Inverse""#));
        assert_eq!(serde_json::from_str::<Instrument>(&json).unwrap(), inverse);
    }

    #[test]
    fn spot_carries_no_contract_and_stays_byte_compatible() {
        // `contract` is skipped when absent, so a spot instrument
        // serialises exactly as it did before derivatives existed.
        assert!(!serde_json::to_string(&spot()).unwrap().contains("contract"));
        assert!(spot().contract.is_none());
        assert!(spot().settle().is_none());
    }

    #[test]
    fn option_terms_survive_a_round_trip() {
        let call = Instrument::derivative(
            "BTCUSD260830C70000",
            "BTC-USD-260830-70000-C",
            "BTC",
            "USD",
            InstrumentKind::Option,
            Contract::new("BTC", Settlement::Inverse)
                .with_expiry(UnixNanos::from_millis(1_788_076_800_000).unwrap())
                .with_option(OptionRight::Call, 0, 70_000),
        );

        let back: Instrument =
            serde_json::from_str(&serde_json::to_string(&call).unwrap()).unwrap();
        let terms = back.contract.unwrap().option.unwrap();
        assert_eq!(terms.right, OptionRight::Call);
        assert_eq!(terms.strike, 70_000);
    }

    #[test]
    fn only_dated_contracts_can_expire() {
        let now = UnixNanos::from_millis(1_800_000_000_000).unwrap();
        assert!(!spot().is_expired_at(now), "spot never expires");

        let perpetual = Instrument::derivative(
            "BTCUSDT",
            "BTC-USDT-SWAP",
            "BTC",
            "USDT",
            InstrumentKind::Perpetual,
            Contract::new("USDT", Settlement::Linear),
        );
        assert!(!perpetual.is_expired_at(now), "a perpetual never expires");

        let dated = Instrument::derivative(
            "BTCUSD260904",
            "BTC-USD-260904",
            "BTC",
            "USD",
            InstrumentKind::Future,
            Contract::new("BTC", Settlement::Inverse)
                .with_expiry(UnixNanos::from_millis(1_788_508_800_000).unwrap()),
        );
        assert!(dated.is_expired_at(now));
        assert!(!dated.is_expired_at(UnixNanos::from_millis(1_700_000_000_000).unwrap()));
    }

    #[test]
    fn kinds_know_whether_they_are_derivatives() {
        assert!(!InstrumentKind::Spot.is_derivative());
        assert!(InstrumentKind::Perpetual.is_derivative());
        assert!(InstrumentKind::Future.is_derivative());
        assert!(InstrumentKind::Option.is_derivative());
    }
}
