//! The contract a broker or exchange integration implements, and the
//! context the engine hands it on every call.
//!
//! # The adapter owns the money, the engine owns the attachment
//!
//! This is the division the whole design rests on. A real broker already
//! holds the authoritative order book, position list and balance for an
//! account; whatever Senken stored beside it could only ever be a second
//! copy that disagrees. So a [`TradeAdapter`] is asked for those on every
//! request and nothing about them is persisted here.
//!
//! What Senken does own is the *attachment*: which adapter, whose account,
//! under what label, with which settings — that is
//! [`TradeAccountStore`](crate::TradeAccountStore), and it is where
//! ownership and authorisation live. An adapter is never told which user is
//! behind an account, because by the time it is called the question has
//! already been settled.
//!
//! # Everything an adapter needs arrives with the call
//!
//! [`TradeContext`] carries the current time, the instrument catalog, and a
//! price to mark against. Passing them per call rather than handing them to
//! the adapter at construction is what keeps the dependency pointing one
//! way: the engine is assembled after the market data and bar loaders
//! exist, so it can supply them; an adapter built during plugin activation
//! could not have taken them then, and would have needed a lazily-filled
//! handle to work around the ordering.
//!
//! It also means an adapter is trivially testable: a `TradeContext` over a
//! fixed clock and a fixed price is three lines.

use async_trait::async_trait;
use senken_core::UnixNanos;
use senken_core::decimal::Scaled;
use senken_marketdata::{Instrument, InstrumentId};

use crate::capability::{AdapterCapabilities, AdapterKind, InstrumentCoverage};
use crate::error::TradeError;
use crate::id::{OrderId, TradeAccountId};
use crate::order::{Fill, Order, OrderFilter, OrderRequest};
use crate::portfolio::{AccountBalances, AdapterHealth, Position};
use crate::settings::{ActionForm, SettingsSchema, SettingsValues};

/// A price to mark a position, or fill a simulated order, against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkPrice {
    /// The price, at the instrument's own price scale.
    pub price: Scaled,
    /// When this price was current. Carried so a caller can see how stale a
    /// mark is rather than assuming it is live — the difference between a
    /// tick from a second ago and the close of a bar from last Friday.
    pub as_of: UnixNanos,
}

/// Where a mark price comes from.
///
/// A port, implemented by whatever layer actually has prices — a live feed,
/// a bar loader, a fixed value in a test. This crate deliberately has no
/// implementation of its own: it would have to pick one source, and the
/// right source differs between a live server, a backtest and a test.
#[async_trait]
pub trait MarkPriceSource: Send + Sync {
    /// The most recent price known for `instrument`, or `None` when none
    /// is.
    ///
    /// `None` is a normal answer, not a failure: an instrument nobody has
    /// ever loaded history for genuinely has no price here.
    ///
    /// # Errors
    /// [`TradeError`] only when the lookup itself failed — a decode error,
    /// an unreachable store — never for a simple absence.
    async fn mark_price(&self, instrument: &InstrumentId) -> Result<Option<MarkPrice>, TradeError>;
}

/// Where an instrument's own trading rules come from.
///
/// An adapter needs the tick size to round a price to, the step size to
/// round a quantity to, and the contract terms to size a derivative. Those
/// live in the market data catalog, and this is the port that reaches it
/// without this crate depending on the catalog's registry half.
#[async_trait]
pub trait InstrumentSource: Send + Sync {
    /// The catalogued instrument, or `None` when no source has it.
    ///
    /// # Errors
    /// [`TradeError`] when the lookup itself failed.
    async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError>;
}

/// Everything an adapter is given for one call.
pub struct TradeContext<'a> {
    now: UnixNanos,
    marks: &'a dyn MarkPriceSource,
    instruments: &'a dyn InstrumentSource,
}

impl std::fmt::Debug for TradeContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TradeContext")
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

impl<'a> TradeContext<'a> {
    /// Builds a context for one call.
    #[must_use]
    pub fn new(
        now: UnixNanos,
        marks: &'a dyn MarkPriceSource,
        instruments: &'a dyn InstrumentSource,
    ) -> Self {
        Self {
            now,
            marks,
            instruments,
        }
    }

    /// The instant this call is happening at.
    ///
    /// One value for the whole call, so every timestamp an adapter stamps
    /// during it agrees — an order and the fill it produced cannot end up
    /// microseconds apart in the wrong direction.
    #[must_use]
    pub fn now(&self) -> UnixNanos {
        self.now
    }

    /// The most recent price for `instrument`.
    ///
    /// # Errors
    /// [`TradeError::NoMarkPrice`] when nothing is known — the absence the
    /// port reports as `None` becomes an error here, because an adapter
    /// asking this question always needs an answer.
    pub async fn mark_price(&self, instrument: &InstrumentId) -> Result<MarkPrice, TradeError> {
        self.marks
            .mark_price(instrument)
            .await?
            .ok_or_else(|| TradeError::NoMarkPrice(instrument.clone()))
    }

    /// The most recent price, or `None` — for the callers that can carry on
    /// without one, such as listing positions that simply show no profit
    /// figure.
    ///
    /// # Errors
    /// [`TradeError`] when the lookup itself failed.
    pub async fn try_mark_price(
        &self,
        instrument: &InstrumentId,
    ) -> Result<Option<MarkPrice>, TradeError> {
        self.marks.mark_price(instrument).await
    }

    /// The instrument's catalogued trading rules.
    ///
    /// # Errors
    /// [`TradeError::UnknownInstrument`] when no loaded catalog has it —
    /// trading something whose tick size is unknown is refused rather than
    /// guessed at.
    pub async fn instrument(&self, id: &InstrumentId) -> Result<Instrument, TradeError> {
        self.instruments
            .instrument(id)
            .await?
            .ok_or_else(|| TradeError::UnknownInstrument(id.clone()))
    }
}

/// The account an adapter is acting for.
///
/// Deliberately carries **no owner**. By the time an adapter sees this, the
/// account store has already decided that this caller may act on this
/// account; handing the user's identity through as well would create a
/// second place the rule could be re-implemented, and re-implemented
/// differently.
#[derive(Debug, Clone, Copy)]
pub struct AccountRef<'a> {
    /// The attachment's id. An adapter keys its own state by this.
    pub id: TradeAccountId,
    /// The label the user gave the account.
    pub label: &'a str,
    /// The validated settings for it, including any credentials.
    pub settings: &'a SettingsValues,
}

/// A named operation an adapter offers beyond placing and cancelling
/// orders.
///
/// The extension point for everything that is specific to one integration:
/// the simulator's "reset this account" and "deposit funds", an exchange's
/// "test the API key", a broker's "re-run the OAuth handshake".
///
/// A plugin describes the action and its parameters as data, exactly as it
/// describes its settings, and the client renders a form from that. **A
/// plugin never ships user interface code.** Letting one inject markup or
/// script into the app would hand every plugin author the session of every
/// user who opens its settings screen, and no amount of review makes that
/// safe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdapterAction {
    /// Stable id the client sends back to invoke it.
    pub id: String,
    /// The button's label.
    pub label: String,
    /// One line explaining what it does. Product copy — a user reads it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Whether the client must confirm before invoking.
    pub confirm: bool,
    /// Whether it destroys state, so the client can style it as such.
    pub destructive: bool,
    /// The parameters it takes, as a form. Empty for an action that takes
    /// none.
    #[serde(default)]
    pub form: ActionForm,
}

impl AdapterAction {
    /// An action taking no parameters and needing no confirmation.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: String::new(),
            confirm: false,
            destructive: false,
            form: ActionForm::default(),
        }
    }

    /// Adds the line shown under the button.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Gives the action a parameter form.
    #[must_use]
    pub fn with_form(mut self, form: ActionForm) -> Self {
        self.form = form;
        self
    }

    /// Marks the action destructive, which also makes it require
    /// confirmation — there is no case for destroying an account's state
    /// on a single stray click.
    #[must_use]
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self.confirm = true;
        self
    }
}

/// What an action did, for the client to show.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActionOutcome {
    /// One line of product copy describing what happened.
    pub message: String,
}

impl ActionOutcome {
    /// An outcome carrying just a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A broker, exchange or simulator Senken can trade through.
///
/// Registered by a plugin at activation, exactly as a
/// [`MarketDataSource`](senken_marketdata::MarketDataSource) is, and used
/// through [`TradeEngine`](crate::TradeEngine).
///
/// Implementors must be cheap to clone-by-`Arc` and hold no per-request
/// state: one instance serves every account attached to it, and calls for
/// different accounts run concurrently.
#[async_trait]
pub trait TradeAdapter: Send + Sync {
    /// Stable, unique, lowercase `[a-z0-9-]` id such as `simulator` or
    /// `binance-spot`.
    fn id(&self) -> &str;

    /// Human-readable name such as `Senken Simulator`.
    fn name(&self) -> &str;

    /// What kind of thing is behind it.
    fn kind(&self) -> AdapterKind;

    /// One line describing it, shown on the adapter card.
    fn description(&self) -> &'static str {
        ""
    }

    /// What it can do. Read on every request the engine validates.
    fn capabilities(&self) -> AdapterCapabilities;

    /// Which instruments it will trade.
    fn coverage(&self) -> InstrumentCoverage;

    /// The settings an account on this adapter needs.
    fn settings_schema(&self) -> SettingsSchema;

    /// Custom operations it offers per account. None by default.
    fn actions(&self) -> Vec<AdapterAction> {
        Vec::new()
    }

    /// Prepares an account for use: validate credentials, create whatever
    /// state the adapter keeps for it.
    ///
    /// Called when the account is attached and again whenever its settings
    /// change, so it must be idempotent — an adapter that reset an
    /// account's state here would wipe it on every settings edit.
    ///
    /// # Errors
    /// [`TradeError::Rejected`] for credentials the venue refuses,
    /// [`TradeError::Transport`] when the venue cannot be reached.
    async fn open_account(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<(), TradeError>;

    /// Releases whatever [`open_account`](Self::open_account) set up.
    /// Called when the attachment is deleted.
    ///
    /// # Errors
    /// [`TradeError`] as the adapter needs; the engine logs it and deletes
    /// the attachment regardless — a user removing an account must not be
    /// blocked by a venue that is down.
    async fn close_account(&self, account: AccountRef<'_>) -> Result<(), TradeError> {
        let _ = account;
        Ok(())
    }

    /// Whether this account can be used right now.
    ///
    /// # Errors
    /// [`TradeError`] only when the check could not be performed at all;
    /// an adapter that is simply unreachable reports
    /// [`AdapterHealth::Disconnected`], which is an answer rather than a
    /// failure.
    async fn health(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AdapterHealth, TradeError>;

    /// The account's money.
    ///
    /// # Errors
    /// [`TradeError`] as the venue call fails.
    async fn balances(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountBalances, TradeError>;

    /// The account's open positions.
    ///
    /// # Errors
    /// [`TradeError`] as the venue call fails.
    async fn positions(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<Vec<Position>, TradeError>;

    /// The account's orders, newest first.
    ///
    /// # Errors
    /// [`TradeError`] as the venue call fails.
    async fn orders(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        filter: OrderFilter,
    ) -> Result<Vec<Order>, TradeError>;

    /// The account's most recent executions, newest first, at most `limit`
    /// of them.
    ///
    /// The default returns nothing, for an adapter whose venue reports
    /// orders but not individual fills — which it declares by leaving
    /// [`AdapterFeature::Fills`](crate::AdapterFeature) out of its
    /// capabilities.
    ///
    /// # Errors
    /// [`TradeError`] as the venue call fails.
    async fn fills(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        limit: usize,
    ) -> Result<Vec<Fill>, TradeError> {
        let _ = (ctx, account, limit);
        Ok(Vec::new())
    }

    /// Sends an order.
    ///
    /// The engine has already checked that the adapter covers the
    /// instrument and accepts the order's kind and time in force; what is
    /// left for the adapter is everything only it can know — balance,
    /// venue-side limits, credentials.
    ///
    /// # Errors
    /// [`TradeError::Rejected`] when the venue refuses,
    /// [`TradeError::InsufficientBalance`] when the account cannot cover
    /// it, [`TradeError::Transport`] when the request's fate is unknown.
    async fn place_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError>;

    /// Cancels a resting order, returning it in its final state.
    ///
    /// The default refuses, for an adapter that did not declare
    /// [`AdapterFeature::CancelOrders`](crate::AdapterFeature).
    ///
    /// # Errors
    /// [`TradeError::UnknownOrder`], [`TradeError::OrderNotOpen`], or the
    /// venue's own failure.
    async fn cancel_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
    ) -> Result<Order, TradeError> {
        let _ = (ctx, account, order_id);
        Err(TradeError::unsupported(self.id(), "cancelling orders"))
    }

    /// Runs one of [`actions`](Self::actions).
    ///
    /// The default refuses every id, which is correct for an adapter that
    /// declared no actions.
    ///
    /// # Errors
    /// [`TradeError::Unsupported`] for an id this adapter does not offer,
    /// [`TradeError::Settings`] when the parameters do not fit the action's
    /// own form, or the venue's own failure.
    async fn run_action(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        action_id: &str,
        params: SettingsValues,
    ) -> Result<ActionOutcome, TradeError> {
        let _ = (ctx, account, params);
        Err(TradeError::unsupported(
            self.id(),
            format!("the action `{action_id}`"),
        ))
    }
}

impl std::fmt::Debug for dyn TradeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TradeAdapter")
            .field("id", &self.id())
            .field("name", &self.name())
            .field("kind", &self.kind())
            .finish()
    }
}
