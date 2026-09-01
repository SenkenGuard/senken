//! The Senken trade engine: the contract a broker, exchange or simulator
//! implements, the vocabulary it speaks, and the registry of adapters and
//! user-attached accounts in front of it.
//!
//! # The shape of the thing
//!
//! ```text
//!   plugin  ──registers──▶  TradeAdapter  ──registered in──▶  TradeEngine
//!                                 ▲                                │
//!                                 │                         validates, routes
//!                                 │                                ▼
//!                       TradeContext (time, catalog,      TradeAccountStore
//!                        mark price) per call             (whose account,
//!                                                          which settings)
//! ```
//!
//! A plugin registers a [`TradeAdapter`] during activation, exactly the way
//! it registers a market data source. The runtime collects them into a
//! [`TradeEngine`]. A user attaches an *account* to an adapter, and that
//! attachment — with the settings the adapter declared a schema for — is
//! what [`TradeAccountStore`] persists.
//!
//! # Three decisions worth knowing before reading further
//!
//! **The adapter owns the money; the engine owns the attachment.** A real
//! broker already holds the authoritative orders, positions and balances
//! for an account. A copy of those in Senken's database could only ever be
//! a copy that disagrees, and it would disagree at exactly the moment
//! someone needed it to be right. So they are read through the adapter on
//! every request and none of them are stored here. What is stored is which
//! adapter, whose account, under what label, with which settings.
//!
//! **The vocabulary is broad; each adapter declares its part of it.** This
//! has to serve a crypto spot exchange, a perpetual-futures venue, an FX
//! broker quoting lots and a paper simulator. A lowest common denominator
//! would be useless for the venues that matter, so instead
//! [`AdapterCapabilities`] and [`InstrumentCoverage`] let each adapter say
//! what it does, the engine refuses what it cannot serve before anything is
//! sent, and the order ticket renders only the controls that mean something
//! for the account in front of the user.
//!
//! **A plugin describes its settings; it never ships user interface.** An
//! adapter's [`settings_schema`](TradeAdapter::settings_schema) and its
//! [`actions`](TradeAdapter::actions) are data. The server validates
//! against them and the client renders a form from them, so neither side
//! carries adapter-specific code — and no plugin author is ever handed the
//! session of a user who opens its settings screen.
//!
//! # Money is exact here, without exception
//!
//! Every price, quantity, balance and fee in this crate is a
//! [`Scaled`](senken_core::decimal::Scaled) `(scale, value)` pair. There is
//! no `f64` in the crate at all. An indicator may produce `68420.1379`; the
//! engine rounds it onto the instrument's tick, as an integer, before it
//! reaches anything that trades.
//!
//! # Cargo features
//!
//! * *(none)* — the contract and its vocabulary. An adapter crate needs
//!   only this: no registry, no database, no identity layer.
//! * `engine` — [`TradeEngine`], the registry and its validation.
//! * `accounts` — [`TradeAccountStore`], the attached accounts as guarded
//!   SQLite queries against `senken-identity`'s own database.

mod adapter;
mod capability;
mod error;
mod id;
mod order;
mod portfolio;
/// The schema an adapter declares for its settings, and the values stored
/// against it.
pub mod settings;

#[cfg(feature = "engine")]
mod engine;
#[cfg(feature = "accounts")]
mod store;

pub use crate::adapter::{
    AccountRef, ActionOutcome, AdapterAction, InstrumentSource, MarkPrice, MarkPriceSource,
    TradeAdapter, TradeContext,
};
pub use crate::capability::{
    AdapterCapabilities, AdapterFeature, AdapterKind, InstrumentCoverage, PositionMode,
    QuantityUnit,
};
pub use crate::error::{BoxError, TradeError};
pub use crate::id::{
    ClientOrderId, ClientOrderIdError, MAX_CLIENT_ORDER_ID_LEN, OrderId, TradeAccountId,
};
pub use crate::order::{
    Fill, Liquidity, Order, OrderFilter, OrderKind, OrderKindTag, OrderRequest, OrderSide,
    OrderStatus, TimeInForce,
};
pub use crate::portfolio::{AccountBalances, AdapterHealth, AssetBalance, Position, PositionSide};
pub use crate::settings::{
    ActionForm, ChoiceOption, FieldKind, SecretString, SettingField, SettingValue, SettingsError,
    SettingsInput, SettingsSchema, SettingsValues,
};

#[cfg(feature = "engine")]
pub use crate::engine::{TradeEngine, rescale_exact};
#[cfg(feature = "accounts")]
pub use crate::store::{TradeAccountStore, TradeAccountSummary};

// Re-exported for convenience: every listing here returns
// `senken_identity::Page<T>`, the same paginated shape `senken-watchlist`
// and `senken-chart` reuse rather than re-declare.
#[cfg(feature = "accounts")]
pub use senken_identity::Page;
