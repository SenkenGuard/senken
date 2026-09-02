//! What an adapter can actually do, declared as data.
//!
//! # Why this exists
//!
//! The engine has to work for a crypto spot exchange, a perpetual-futures
//! venue, an FX broker quoting lots, and a simulator that is none of them.
//! Two ways of handling that were available. One is a lowest common
//! denominator: only market orders, only base-asset quantities, no leverage
//! — which makes the engine useless for the venues that pay for it. The
//! other is what this crate does: the vocabulary is broad enough to say
//! everything any of those venues means, and each adapter declares which
//! part of it applies to itself.
//!
//! That declaration does real work in three places. The engine refuses a
//! request an adapter cannot serve, before it reaches the venue. The order
//! ticket renders only the controls that mean something for the account the
//! user picked, so there is no leverage box on a spot account and no
//! `reduce_only` switch where positions do not net. And an adapter author
//! implements one trait rather than a trait per asset class.

use std::collections::BTreeSet;

use senken_marketdata::InstrumentId;
use serde::{Deserialize, Serialize};

use crate::order::{OrderKindTag, TimeInForce};

/// What kind of thing is behind an adapter.
///
/// Presentational — it groups adapters in the UI and warns on the ones
/// that move real money. It grants nothing and gates nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdapterKind {
    /// Fills against Senken's own prices. No money leaves anywhere.
    Simulation,
    /// A brokerage: FX, CFD, futures, equities.
    Broker,
    /// A crypto exchange, spot or derivative.
    Exchange,
}

impl AdapterKind {
    /// `true` when orders through this adapter reach a real venue.
    ///
    /// The one fact the UI must not get wrong: a simulated account and a
    /// live one look identical on screen otherwise.
    #[must_use]
    pub fn trades_real_money(self) -> bool {
        !matches!(self, Self::Simulation)
    }
}

/// What the number in [`OrderRequest::quantity`](crate::OrderRequest::quantity)
/// counts.
///
/// The single most venue-specific fact in trading, and the one an engine
/// that assumed crypto spot would get wrong everywhere else: `1` means one
/// bitcoin on a spot exchange, one contract on a futures venue, and 100 000
/// units of the base currency on an FX broker quoting standard lots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QuantityUnit {
    /// Units of the instrument's base asset — `0.5` is half a bitcoin.
    Base,
    /// Contracts, whose size is on the instrument's
    /// [`Contract`](senken_marketdata::Contract).
    Contracts,
    /// Lots, whose size is the venue's own convention.
    Lots,
    /// An amount of the quote currency to spend, rather than a size to buy.
    QuoteNotional,
}

impl QuantityUnit {
    /// The word an order ticket puts beside the size box.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Base => "units",
            Self::Contracts => "contracts",
            Self::Lots => "lots",
            Self::QuoteNotional => "notional",
        }
    }
}

/// How two opposing trades in one instrument combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PositionMode {
    /// Buying while short reduces the short. One position per instrument.
    Netting,
    /// A long and a short in one instrument coexist. Common on FX and on
    /// several crypto derivative venues.
    Hedging,
    /// Positions are asset holdings, not directional exposure: there is
    /// nothing to be short of.
    SpotHoldings,
}

/// Which instruments an adapter will trade.
///
/// Modelled as three cases rather than "the adapter returns a list",
/// because the simulator's answer is genuinely "all of them, including ones
/// added tomorrow" and a list could not express that without being
/// regenerated every time a catalog changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "coverage", rename_all = "snake_case")]
#[non_exhaustive]
pub enum InstrumentCoverage {
    /// Every instrument this installation has a catalog for.
    ///
    /// The simulator's answer, and the reason it is worth having: a user
    /// can paper-trade an instrument from any of the venues Senken knows
    /// about, whether or not they hold an account there.
    Universal,
    /// Every instrument belonging to these market data sources.
    ///
    /// What a real venue's adapter normally says — `binance-spot`'s
    /// trading adapter covers exactly `binance-spot`'s catalog, which stays
    /// correct as that catalog changes.
    Sources {
        /// The market data source ids covered.
        source_ids: Vec<String>,
    },
    /// Exactly these instruments and no others — a broker whose symbol list
    /// does not correspond to any one market data source.
    Instruments {
        /// The instruments covered.
        instruments: Vec<InstrumentId>,
    },
}

impl InstrumentCoverage {
    /// Whether this adapter will trade `instrument`.
    #[must_use]
    pub fn covers(&self, instrument: &InstrumentId) -> bool {
        match self {
            Self::Universal => true,
            Self::Sources { source_ids } => source_ids
                .iter()
                .any(|source| source == instrument.source()),
            Self::Instruments { instruments } => instruments.contains(instrument),
        }
    }

    /// Covering exactly the named market data sources.
    pub fn sources<I, S>(sources: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Sources {
            source_ids: sources.into_iter().map(Into::into).collect(),
        }
    }
}

/// One optional behaviour an adapter either has or does not.
///
/// A set rather than a row of booleans on [`AdapterCapabilities`]: a venue
/// with a behaviour nothing here names yet is a new variant, not a new
/// field on a struct every adapter in the workspace would have to be
/// edited to fill in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdapterFeature {
    /// `reduce_only` is honoured.
    ReduceOnly,
    /// `post_only` is honoured.
    PostOnly,
    /// A resting order can be cancelled.
    CancelOrders,
    /// A resting order's price or size can be amended in place.
    ModifyOrders,
    /// The account has a leverage setting.
    Leverage,
    /// Individual executions are reported, not only orders.
    Fills,
}

/// What an account may do, as distinct from what its adapter can.
///
/// [`AdapterCapabilities`] is the most an adapter can ever do — what an
/// adapter card shows before any account exists. This is that, narrowed to
/// one account: a MetaTrader 5 investor login, an exchange key minted
/// without trade scope, a demo account past its trial all report the same
/// adapter but a different access level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AccessLevel {
    /// Orders may be placed, amended and cancelled.
    Trade,
    /// Balances, positions and orders may be read; nothing may be sent.
    ReadOnly,
}

/// One account's resolved access.
///
/// Carries the whole capability set, not just the level, because an account
/// can be narrower than its adapter in more ways than one — a venue that
/// allows stop orders on futures accounts but not on cash ones, a
/// sub-account with lower leverage. [`AccessLevel::ReadOnly`] is the case
/// that matters most, and this general shape costs nothing extra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAccess {
    /// What this account may do right now.
    pub level: AccessLevel,
    /// The adapter's capabilities, **narrowed to this account**.
    pub capabilities: AdapterCapabilities,
    /// One line of product copy explaining a restriction, shown to the
    /// user. `None` when the account is unrestricted.
    pub note: Option<String>,
}

impl AccountAccess {
    /// Full trading access, with `capabilities` exactly as the adapter
    /// declared them.
    #[must_use]
    pub fn trading(capabilities: AdapterCapabilities) -> Self {
        Self {
            level: AccessLevel::Trade,
            capabilities,
            note: None,
        }
    }

    /// Read-only access. `capabilities` is still carried — an order ticket
    /// narrows its controls to it even though nothing on this account can be
    /// sent — and `note` is the line shown to the user explaining why.
    #[must_use]
    pub fn read_only(capabilities: AdapterCapabilities, note: Option<String>) -> Self {
        Self {
            level: AccessLevel::ReadOnly,
            capabilities,
            note,
        }
    }

    /// `true` only for [`AccessLevel::Trade`], the sole level an order may
    /// be sent under.
    ///
    /// Any other level refuses, including one this build has not been
    /// taught about yet: `AccessLevel` is `#[non_exhaustive]`, and a variant
    /// added later must not become tradable just because nothing here
    /// rejects it by name — matching the single variant that grants access,
    /// rather than listing the ones that do not, is what keeps that true
    /// without this function being revisited.
    #[must_use]
    pub fn is_trading(&self) -> bool {
        matches!(self.level, AccessLevel::Trade)
    }
}

/// Everything an adapter declares about how it trades.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCapabilities {
    /// The order kinds it accepts. An order of any other kind is refused by
    /// the engine before it is sent.
    pub order_kinds: Vec<OrderKindTag>,
    /// The times in force it accepts.
    pub time_in_force: Vec<TimeInForce>,
    /// What a quantity counts.
    pub quantity_unit: QuantityUnit,
    /// How opposing trades combine.
    pub position_mode: PositionMode,
    /// The optional behaviours it has.
    pub features: BTreeSet<AdapterFeature>,
}

impl AdapterCapabilities {
    /// The smallest honest declaration: market orders only, good-til-
    /// cancelled, base-asset quantities, spot holdings, reporting fills.
    ///
    /// A starting point for a builder chain, not a default an adapter
    /// should ship unexamined.
    #[must_use]
    pub fn market_only() -> Self {
        Self {
            order_kinds: vec![OrderKindTag::Market],
            time_in_force: vec![TimeInForce::Gtc],
            quantity_unit: QuantityUnit::Base,
            position_mode: PositionMode::SpotHoldings,
            features: BTreeSet::from([AdapterFeature::Fills]),
        }
    }

    /// Replaces the accepted order kinds.
    #[must_use]
    pub fn with_order_kinds(mut self, kinds: Vec<OrderKindTag>) -> Self {
        self.order_kinds = kinds;
        self
    }

    /// Replaces the accepted times in force.
    #[must_use]
    pub fn with_time_in_force(mut self, tifs: Vec<TimeInForce>) -> Self {
        self.time_in_force = tifs;
        self
    }

    /// Sets what a quantity counts.
    #[must_use]
    pub fn with_quantity_unit(mut self, unit: QuantityUnit) -> Self {
        self.quantity_unit = unit;
        self
    }

    /// Sets how opposing trades combine.
    #[must_use]
    pub fn with_position_mode(mut self, mode: PositionMode) -> Self {
        self.position_mode = mode;
        self
    }

    /// Declares one optional behaviour.
    #[must_use]
    pub fn with_feature(mut self, feature: AdapterFeature) -> Self {
        self.features.insert(feature);
        self
    }

    /// Declares leverage and reduce-only together — one venue property, not
    /// two independent switches: an account with leverage always has
    /// positions to reduce.
    #[must_use]
    pub fn with_margin(self) -> Self {
        self.with_feature(AdapterFeature::Leverage)
            .with_feature(AdapterFeature::ReduceOnly)
    }

    /// Whether this adapter has `feature`.
    #[must_use]
    pub fn has(&self, feature: AdapterFeature) -> bool {
        self.features.contains(&feature)
    }

    /// Whether an order of this kind may be sent at all.
    #[must_use]
    pub fn accepts_kind(&self, kind: OrderKindTag) -> bool {
        self.order_kinds.contains(&kind)
    }

    /// Whether this time in force may be sent at all.
    #[must_use]
    pub fn accepts_time_in_force(&self, tif: TimeInForce) -> bool {
        self.time_in_force.contains(&tif)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccessLevel, AccountAccess, AdapterCapabilities, AdapterFeature, AdapterKind,
        InstrumentCoverage, QuantityUnit,
    };
    use crate::order::{OrderKindTag, TimeInForce};
    use senken_marketdata::InstrumentId;

    fn id(raw: &str) -> InstrumentId {
        InstrumentId::parse(raw).unwrap()
    }

    #[test]
    fn only_a_simulation_is_exempt_from_the_real_money_warning() {
        assert!(!AdapterKind::Simulation.trades_real_money());
        assert!(AdapterKind::Broker.trades_real_money());
        assert!(AdapterKind::Exchange.trades_real_money());
    }

    #[test]
    fn universal_coverage_includes_an_instrument_from_any_source() {
        let coverage = InstrumentCoverage::Universal;
        assert!(coverage.covers(&id("okx-spot:BTCUSDT")));
        assert!(coverage.covers(&id("kraken-spot:ETHEUR")));
    }

    #[test]
    fn source_coverage_follows_the_catalog_rather_than_a_frozen_list() {
        // The point of naming a source rather than listing instruments: an
        // instrument the venue lists tomorrow is covered without this
        // declaration changing.
        let coverage = InstrumentCoverage::sources(["binance-spot"]);
        assert!(coverage.covers(&id("binance-spot:BTCUSDT")));
        assert!(coverage.covers(&id("binance-spot:LISTEDTOMORROW")));
        assert!(!coverage.covers(&id("okx-spot:BTCUSDT")));
    }

    #[test]
    fn explicit_coverage_admits_nothing_it_was_not_given() {
        let coverage = InstrumentCoverage::Instruments {
            instruments: vec![id("oanda-fx:EURUSD")],
        };
        assert!(coverage.covers(&id("oanda-fx:EURUSD")));
        assert!(!coverage.covers(&id("oanda-fx:GBPUSD")));
    }

    #[test]
    fn the_minimal_declaration_admits_only_what_it_names() {
        let caps = AdapterCapabilities::market_only();
        assert!(caps.accepts_kind(OrderKindTag::Market));
        assert!(!caps.accepts_kind(OrderKindTag::Limit));
        assert!(caps.accepts_time_in_force(TimeInForce::Gtc));
        assert!(!caps.accepts_time_in_force(TimeInForce::Ioc));
        assert!(!caps.has(AdapterFeature::Leverage));
    }

    #[test]
    fn declaring_margin_turns_on_leverage_and_reduce_only_together() {
        // They are one venue property, not two independent switches: an
        // account with leverage always has positions to reduce.
        let caps = AdapterCapabilities::market_only().with_margin();
        assert!(caps.has(AdapterFeature::Leverage));
        assert!(caps.has(AdapterFeature::ReduceOnly));
    }

    #[test]
    fn a_quantity_unit_names_itself_for_the_order_ticket() {
        assert_eq!(QuantityUnit::Base.label(), "units");
        assert_eq!(QuantityUnit::Lots.label(), "lots");
        assert_eq!(QuantityUnit::Contracts.label(), "contracts");
    }

    #[test]
    fn trading_access_carries_the_adapters_capabilities_unrestricted_and_no_note() {
        let access = AccountAccess::trading(AdapterCapabilities::market_only());
        assert_eq!(access.level, AccessLevel::Trade);
        assert!(access.is_trading());
        assert_eq!(access.note, None);
        assert_eq!(access.capabilities, AdapterCapabilities::market_only());
    }

    #[test]
    fn read_only_access_carries_its_note_and_still_carries_capabilities() {
        // The order ticket narrows its controls to `capabilities` even
        // though nothing on this account can be sent, so it must still be
        // the real, narrowed set rather than an empty one.
        let narrowed = AdapterCapabilities::market_only().with_order_kinds(vec![]);
        let access = AccountAccess::read_only(
            narrowed.clone(),
            Some("This account was attached read-only.".to_owned()),
        );
        assert_eq!(access.level, AccessLevel::ReadOnly);
        assert!(!access.is_trading());
        assert_eq!(
            access.note.as_deref(),
            Some("This account was attached read-only.")
        );
        assert_eq!(access.capabilities, narrowed);
    }

    #[test]
    fn only_the_trade_level_reports_itself_as_trading() {
        // `is_trading` matches the one variant that grants access rather
        // than listing the ones that do not — the property this test
        // exists to pin down, so a variant `AccessLevel` grows later stays
        // refused by construction rather than by someone remembering to add
        // it to a list here.
        assert!(AccountAccess::trading(AdapterCapabilities::market_only()).is_trading());
        assert!(!AccountAccess::read_only(AdapterCapabilities::market_only(), None).is_trading());
    }

    #[test]
    fn an_access_level_this_build_does_not_recognise_fails_to_deserialise_rather_than_being_guessed_at()
     {
        // `AccessLevel` is `#[non_exhaustive]`; a variant a newer build
        // could write (and this one has not been taught) must not silently
        // parse into something — least of all into `Trade`, which is the
        // one failure mode that would let a restricted account trade.
        let error = serde_json::from_str::<AccessLevel>(r#""margin_call""#).unwrap_err();
        assert!(error.to_string().contains("unknown variant"), "got {error}");
    }
}
