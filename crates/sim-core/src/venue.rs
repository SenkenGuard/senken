//! One simulated venue, and the adapter that drives any of them.
//!
//! Four simulators need the same twelve `TradeAdapter` methods, and eleven
//! of them do not vary: reading books off disk, taking the lock, settling
//! before every read, refusing an order on an investor login, recording a
//! fill, writing the books back. Writing those four times is how a fee
//! rounding gets fixed in one simulator and not the others.
//!
//! So they are written once here, and a system supplies only what is
//! genuinely its own through [`SimulatedVenue`]: its settings, its
//! capabilities, how to build its model out of an account's settings, and
//! how to report its book as positions and balances.
//!
//! The measure of whether this is the right shape is what a fifth system
//! costs. The target is one file: a [`SettlementModel`] and a
//! [`SimulatedVenue`], with no adapter boilerplate and no edit here.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use senken_storage::{Snapshot, Storage};
use senken_trade::{
    AccountAccess, AccountBalances, AccountRef, AdapterCapabilities, AdapterHealth, AdapterKind,
    Fill, InstrumentCoverage, Liquidity, Order, OrderAmendment, OrderFilter, OrderId, OrderKind,
    OrderRequest, OrderStatus, Position, SettingsSchema, SettingsValues, TimeInForce,
    TradeAccountId, TradeAdapter, TradeContext, TradeError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::money::CASH_SCALE;
use crate::pricing::{Terms, apply_amendment, is_triggered, market_fill_price};
use crate::settlement::{FillContext, Marks, SettlementModel};

/// One trading system, as the shared adapter needs to know it.
///
/// Everything here is something the system genuinely decides. Anything a
/// system would answer the same way as the other three is not here — it is
/// in [`SimAdapter`], written once.
pub trait SimulatedVenue: Send + Sync + 'static {
    /// The book this system keeps.
    type Book: Default + Clone + Serialize + DeserializeOwned + Send + 'static;
    /// The settlement rules, built per account from its settings.
    type Model: SettlementModel<Book = Self::Book> + Send + Sync;

    /// The id this adapter registers under.
    fn id(&self) -> &'static str;
    /// What a user sees it called.
    fn name(&self) -> &'static str;
    /// One line describing what it simulates.
    fn description(&self) -> &'static str;
    /// What it can do.
    fn capabilities(&self) -> AdapterCapabilities;
    /// The broker or venue numbers it reads per account.
    fn settings_schema(&self) -> SettingsSchema;

    /// Builds the settlement rules for one account.
    ///
    /// # Errors
    /// [`TradeError`] when the settings cannot produce a usable model.
    fn model_for(&self, settings: &SettingsValues) -> Result<Self::Model, TradeError>;

    /// The book a freshly opened account starts with.
    ///
    /// # Errors
    /// [`TradeError`] when the settings cannot produce one.
    fn open_book(&self, settings: &SettingsValues) -> Result<Self::Book, TradeError>;

    /// How this system reports its book as positions.
    ///
    /// A spot venue answers with an empty list, because it holds none —
    /// which is a different statement from having no book.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    fn positions(
        &self,
        book: &Self::Book,
        marks: &Marks,
        account: TradeAccountId,
    ) -> Result<Vec<Position>, TradeError>;

    /// How this system reports its account-level figures.
    ///
    /// # Errors
    /// [`TradeError`] when the arithmetic does not fit.
    fn balances(
        &self,
        book: &Self::Book,
        marks: &Marks,
        account: TradeAccountId,
        settings: &SettingsValues,
    ) -> Result<AccountBalances, TradeError>;

    /// Whether this account's login may place orders.
    ///
    /// Defaults to yes. A system with an investor-login concept overrides
    /// it; one without does not have to say so.
    fn is_read_only(&self, settings: &SettingsValues) -> bool {
        let _ = settings;
        false
    }

    /// How far a market order fills from the mark, in basis points.
    fn slippage_bps(&self, settings: &SettingsValues) -> i64 {
        let _ = settings;
        0
    }

    /// What currency a fill's fee is reported in.
    ///
    /// Its own method rather than read back off `balances`, because on a
    /// spot venue the fee currency changes per side and is not the
    /// account's headline currency at all.
    fn fee_currency(&self, settings: &SettingsValues, side: senken_trade::OrderSide) -> String {
        let _ = (settings, side);
        "USD".to_owned()
    }

    /// The instruments this system will trade.
    fn coverage(&self) -> InstrumentCoverage {
        InstrumentCoverage::Universal
    }
}

/// One account's stored state, plus the instant it was settled through.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(bound = "B: Serialize + DeserializeOwned")]
struct StoredAccount<B> {
    book: B,
    /// So a second read on the same day accrues nothing rather than
    /// accruing again.
    settled_through: i64,
    fills: Vec<StoredFill>,
    /// Orders waiting for the market to reach them.
    #[serde(default)]
    resting: Vec<RestingOrder>,
}

impl<B: Default> Default for StoredAccount<B> {
    fn default() -> Self {
        Self {
            book: B::default(),
            settled_through: 0,
            fills: Vec::new(),
            resting: Vec::new(),
        }
    }
}

/// An order waiting for the market to reach it.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct RestingOrder {
    id: String,
    client_order_id: Option<String>,
    instrument: String,
    side: senken_trade::OrderSide,
    kind: OrderKind,
    quantity: Scaled,
    time_in_force: TimeInForce,
    reduce_only: bool,
    submitted_at: i64,
    updated_at: i64,
}

impl RestingOrder {
    fn to_order(&self, account: TradeAccountId) -> Option<Order> {
        Some(Order {
            id: OrderId::new(self.id.clone()),
            client_order_id: self
                .client_order_id
                .as_deref()
                .and_then(|id| senken_trade::ClientOrderId::new(id).ok()),
            account_id: account,
            instrument: InstrumentId::parse(&self.instrument).ok()?,
            side: self.side,
            kind: self.kind,
            quantity: self.quantity,
            filled_quantity: Scaled::new(self.quantity.scale, 0),
            average_price: None,
            time_in_force: self.time_in_force,
            status: OrderStatus::Open,
            reduce_only: self.reduce_only,
            post_only: false,
            submitted_at: senken_core::time::UnixNanos::from_nanos(self.submitted_at),
            updated_at: senken_core::time::UnixNanos::from_nanos(self.updated_at),
            reject_reason: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct StoredFill {
    order_id: String,
    instrument: String,
    side: senken_trade::OrderSide,
    quantity: Scaled,
    price: Scaled,
    fee: i64,
    fee_currency: String,
    at: i64,
}

/// The `TradeAdapter` every simulated system shares.
#[derive(Debug)]
pub struct SimAdapter<V: SimulatedVenue> {
    venue: V,
    storage: Storage,
    path: String,
    accounts: Mutex<BTreeMap<TradeAccountId, StoredAccount<V::Book>>>,
}

impl<V: SimulatedVenue> SimAdapter<V> {
    /// Builds the adapter for `venue`, keeping its books under `storage`.
    #[must_use]
    pub fn new(venue: V, storage: Storage) -> Self {
        let path = format!("trade/{}/books.json", venue.id());
        let accounts = match storage
            .read_snapshot::<BTreeMap<TradeAccountId, StoredAccount<V::Book>>>(&path, 1)
        {
            Ok(Some(snapshot)) => snapshot.data,
            _ => BTreeMap::new(),
        };
        Self {
            venue,
            storage,
            path,
            accounts: Mutex::new(accounts),
        }
    }

    /// The system behind this adapter.
    pub fn venue(&self) -> &V {
        &self.venue
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<TradeAccountId, StoredAccount<V::Book>>> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the books out. Failure is logged by the storage layer rather
    /// than returned: the trade has already happened in memory, and
    /// reporting it as a failure would be a lie about a fill that exists.
    fn persist(&self, accounts: &BTreeMap<TradeAccountId, StoredAccount<V::Book>>) {
        drop(
            self.storage
                .write_snapshot(&self.path, &Snapshot::new(1, accounts)),
        );
    }

    /// Puts a non-market order into the book to wait.
    ///
    /// Filling a limit at the mark would make the capability a lie: a
    /// trader who places a limit below the market and sees it fill
    /// immediately learned nothing true about their strategy.
    fn rest_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError> {
        // A non-market order rests. Filling a limit at the mark would make
        // the capability a lie: a trader who places a limit below the
        // market and sees it fill immediately learned nothing true about
        // their strategy.
        let order_id = OrderId::new(format!("{}-{}", self.venue.id(), uuid_like(ctx)));
        let snapshot = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account.id) else {
                return Err(TradeError::UnknownAccount);
            };
            stored.resting.push(RestingOrder {
                id: order_id.to_string(),
                client_order_id: request.client_order_id.as_ref().map(ToString::to_string),
                instrument: request.instrument.to_string(),
                side: request.side,
                kind: request.kind,
                quantity: request.quantity,
                time_in_force: request.time_in_force,
                reduce_only: request.reduce_only,
                submitted_at: ctx.now().as_nanos(),
                updated_at: ctx.now().as_nanos(),
            });
            accounts.clone()
        };
        self.persist(&snapshot);
        Ok(Order {
            id: order_id,
            client_order_id: request.client_order_id.clone(),
            account_id: account.id,
            instrument: request.instrument,
            side: request.side,
            kind: request.kind,
            quantity: request.quantity,
            filled_quantity: Scaled::new(request.quantity.scale, 0),
            average_price: None,
            time_in_force: request.time_in_force,
            status: OrderStatus::Open,
            reduce_only: request.reduce_only,
            post_only: false,
            submitted_at: ctx.now(),
            updated_at: ctx.now(),
            reject_reason: None,
        })
    }

    /// Fills every resting order the market has reached.
    ///
    /// Evaluated against the current mark. The bar-accurate form —
    /// replaying the bars between `settled_through` and now, so a level is
    /// reached by the bar whose extreme actually reached it — lives in
    /// `crate::replay` and is what this becomes once the adapter is handed
    /// a bar source. Until then the honest description is: a level is
    /// reached when a *read* sees it reached, which is coarser than a real
    /// venue and is stated rather than implied.
    fn fill_resting(
        venue: &V,
        settings: &SettingsValues,
        stored: &mut StoredAccount<V::Book>,
        model: &V::Model,
        marks: &Marks,
        now: senken_core::time::UnixNanos,
    ) -> Result<(), TradeError> {
        let mut still_resting = Vec::with_capacity(stored.resting.len());
        // Taken in the order they were placed, so two orders the same read
        // triggers fill in the order the trader placed them rather than in
        // whatever order a map happened to hold them.
        for order in std::mem::take(&mut stored.resting) {
            let Some(mark) = marks.get(&order.instrument).copied() else {
                still_resting.push(order);
                continue;
            };
            if !is_triggered(order.kind, order.side, mark) {
                still_resting.push(order);
                continue;
            }
            let Ok(instrument) = InstrumentId::parse(&order.instrument) else {
                continue;
            };
            // A limit fills at its own price, never better: the market
            // reaching a level is not the same as the market crossing it,
            // and paying the mark would hand the trader a fill they could
            // not have had.
            let price = match order.kind {
                OrderKind::Limit { price } | OrderKind::StopLimit { price, .. } => price,
                _ => mark,
            };
            let settled = model.settle(
                &mut stored.book,
                &FillContext {
                    instrument: &instrument,
                    side: order.side,
                    quantity: order.quantity,
                    price,
                    now,
                },
            )?;
            stored.fills.push(StoredFill {
                order_id: order.id.clone(),
                instrument: order.instrument.clone(),
                side: order.side,
                quantity: order.quantity,
                price: settled.fill_price,
                fee: settled.fee,
                fee_currency: venue.fee_currency(settings, order.side),
                at: now.as_nanos(),
            });
        }
        stored.resting = still_resting;
        Ok(())
    }

    /// Every mark the account's positions need, gathered once.
    async fn marks_for(
        &self,
        ctx: &TradeContext<'_>,
        account: TradeAccountId,
    ) -> Result<Marks, TradeError> {
        let instruments: std::collections::BTreeSet<String> = {
            let accounts = self.lock();
            let Some(stored) = accounts.get(&account) else {
                return Ok(Marks::new());
            };
            // Both the open positions and anything resting: an order
            // waiting on an instrument the account holds no position in
            // still needs a price, and without one it would rest for ever
            // however far the market moved through it.
            self.venue
                .positions(&stored.book, &Marks::new(), account)?
                .into_iter()
                .map(|position| position.instrument.to_string())
                .chain(stored.resting.iter().map(|order| order.instrument.clone()))
                .collect()
        };
        let mut marks = Marks::new();
        for raw in instruments {
            let Ok(id) = InstrumentId::parse(&raw) else {
                continue;
            };
            if let Some(mark) = ctx.try_mark_price(&id).await? {
                marks.insert(raw, mark.price);
            }
        }
        Ok(marks)
    }

    /// Brings one account up to date: accrue what time cost, then enforce
    /// whatever the system closes on the account's behalf.
    ///
    /// The order is deliberate. Accrual is a cost already incurred, so it
    /// lands before equity is measured; enforcement then runs on what is
    /// actually left.
    async fn settle(
        &self,
        ctx: &TradeContext<'_>,
        account: TradeAccountId,
        settings: &SettingsValues,
    ) -> Result<(), TradeError> {
        let marks = self.marks_for(ctx, account).await?;
        let model = self.venue.model_for(settings)?;
        let now = ctx.now();
        let snapshot = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account) else {
                return Ok(());
            };
            let from = senken_core::time::UnixNanos::from_nanos(stored.settled_through);
            model.accrue(&mut stored.book, &marks, from, now)?;
            stored.settled_through = now.as_nanos();
            // Resting orders fill before risk is enforced: an order the
            // market reached changed the book, and enforcing against the
            // book as it stood before would liquidate a position the fill
            // had already rescued.
            Self::fill_resting(&self.venue, settings, stored, &model, &marks, now)?;
            model.enforce(&mut stored.book, &marks, now)?;
            accounts.clone()
        };
        self.persist(&snapshot);
        Ok(())
    }
}

#[async_trait::async_trait]
impl<V: SimulatedVenue> TradeAdapter for SimAdapter<V> {
    fn id(&self) -> &'static str {
        self.venue.id()
    }

    fn name(&self) -> &'static str {
        self.venue.name()
    }

    fn kind(&self) -> AdapterKind {
        AdapterKind::Simulation
    }

    fn description(&self) -> &'static str {
        self.venue.description()
    }

    fn capabilities(&self) -> AdapterCapabilities {
        // Cancelling and amending are the shared adapter's, not each
        // venue's, so they are added here rather than remembered four
        // times — a venue that forgot would advertise less than it can do.
        self.venue
            .capabilities()
            .with_feature(senken_trade::AdapterFeature::CancelOrders)
            .with_feature(senken_trade::AdapterFeature::ModifyOrders)
    }

    fn coverage(&self) -> InstrumentCoverage {
        self.venue.coverage()
    }

    fn settings_schema(&self) -> SettingsSchema {
        self.venue.settings_schema()
    }

    async fn open_account(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<(), TradeError> {
        let book = self.venue.open_book(account.settings)?;
        let snapshot = {
            let mut accounts = self.lock();
            accounts.entry(account.id).or_insert_with(|| StoredAccount {
                book,
                settled_through: 0,
                fills: Vec::new(),
                resting: Vec::new(),
            });
            accounts.clone()
        };
        self.persist(&snapshot);
        Ok(())
    }

    async fn account_access(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountAccess, TradeError> {
        // The adapter's own, not the venue's: cancelling and amending are
        // added by this layer, and an account resolved from the venue's
        // list alone would be refused the two things the adapter can
        // actually do. The engine validates against *this* answer.
        let capabilities = TradeAdapter::capabilities(self);
        if self.venue.is_read_only(account.settings) {
            return Ok(AccountAccess::read_only(
                capabilities,
                Some("This login reads the account and places nothing.".to_owned()),
            ));
        }
        Ok(AccountAccess::trading(capabilities))
    }

    async fn close_account(&self, account: AccountRef<'_>) -> Result<(), TradeError> {
        let snapshot = {
            let mut accounts = self.lock();
            accounts.remove(&account.id);
            accounts.clone()
        };
        self.persist(&snapshot);
        Ok(())
    }

    async fn health(
        &self,
        _ctx: &TradeContext<'_>,
        _account: AccountRef<'_>,
    ) -> Result<AdapterHealth, TradeError> {
        Ok(AdapterHealth::Connected)
    }

    async fn balances(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountBalances, TradeError> {
        self.settle(ctx, account.id, account.settings).await?;
        let marks = self.marks_for(ctx, account.id).await?;
        let accounts = self.lock();
        let Some(stored) = accounts.get(&account.id) else {
            return Err(TradeError::UnknownAccount);
        };
        self.venue
            .balances(&stored.book, &marks, account.id, account.settings)
    }

    async fn positions(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<Vec<Position>, TradeError> {
        self.settle(ctx, account.id, account.settings).await?;
        let marks = self.marks_for(ctx, account.id).await?;
        let accounts = self.lock();
        let Some(stored) = accounts.get(&account.id) else {
            return Ok(Vec::new());
        };
        self.venue.positions(&stored.book, &marks, account.id)
    }

    async fn orders(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        filter: OrderFilter,
    ) -> Result<Vec<Order>, TradeError> {
        // Settled first, so an order the market has already reached is
        // reported as filled rather than as still waiting.
        self.settle(ctx, account.id, account.settings).await?;
        let accounts = self.lock();
        let Some(stored) = accounts.get(&account.id) else {
            return Ok(Vec::new());
        };
        Ok(stored
            .resting
            .iter()
            .filter_map(|order| order.to_order(account.id))
            // Everything still here is open by definition — a resting
            // order that filled or was cancelled is no longer in this
            // list — so `All` and `Open` agree, and this build keeps no
            // history of the closed ones to widen `All` with.
            .filter(|_| matches!(filter, OrderFilter::Open | OrderFilter::All))
            .collect())
    }

    async fn cancel_order(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
    ) -> Result<Order, TradeError> {
        if self.venue.is_read_only(account.settings) {
            return Err(TradeError::ReadOnly {
                adapter: self.venue.id().to_owned(),
                note: Some("this login cancels no orders".to_owned()),
            });
        }
        let (cancelled, snapshot) = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account.id) else {
                return Err(TradeError::UnknownAccount);
            };
            let Some(index) = stored
                .resting
                .iter()
                .position(|order| order.id == order_id.as_str())
            else {
                return Err(TradeError::UnknownOrder);
            };
            (stored.resting.remove(index), accounts.clone())
        };
        self.persist(&snapshot);
        let mut order = cancelled
            .to_order(account.id)
            .ok_or(TradeError::UnknownOrder)?;
        order.status = OrderStatus::Cancelled;
        Ok(order)
    }

    async fn modify_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
        amendment: OrderAmendment,
    ) -> Result<Order, TradeError> {
        if self.venue.is_read_only(account.settings) {
            return Err(TradeError::ReadOnly {
                adapter: self.venue.id().to_owned(),
                note: Some("this login amends no orders".to_owned()),
            });
        }
        let (amended, snapshot) = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account.id) else {
                return Err(TradeError::UnknownAccount);
            };
            let Some(order) = stored
                .resting
                .iter_mut()
                .find(|order| order.id == order_id.as_str())
            else {
                return Err(TradeError::UnknownOrder);
            };
            // An amendment keeps the order's identity: it is the same
            // order at a new price, not a cancel and a replace, so its id
            // and its place in the queue both survive.
            order.kind = apply_amendment(order.kind, amendment);
            if let Some(quantity) = amendment.quantity {
                order.quantity = quantity;
            }
            order.updated_at = ctx.now().as_nanos();
            (order.clone(), accounts.clone())
        };
        self.persist(&snapshot);
        amended.to_order(account.id).ok_or(TradeError::UnknownOrder)
    }

    async fn fills(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        limit: usize,
    ) -> Result<Vec<Fill>, TradeError> {
        let accounts = self.lock();
        let Some(stored) = accounts.get(&account.id) else {
            return Ok(Vec::new());
        };
        Ok(stored
            .fills
            .iter()
            .rev()
            .take(limit)
            .filter_map(|fill| {
                Some(Fill {
                    id: OrderId::new(format!("{}-d", fill.order_id)),
                    order_id: OrderId::new(fill.order_id.clone()),
                    account_id: account.id,
                    instrument: InstrumentId::parse(&fill.instrument).ok()?,
                    side: fill.side,
                    quantity: fill.quantity,
                    price: fill.price,
                    fee: Scaled::new(CASH_SCALE, fill.fee),
                    fee_currency: fill.fee_currency.clone(),
                    liquidity: Liquidity::Taker,
                    executed_at: senken_core::time::UnixNanos::from_nanos(fill.at),
                })
            })
            .collect())
    }

    async fn place_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError> {
        if self.venue.is_read_only(account.settings) {
            return Err(TradeError::ReadOnly {
                adapter: self.venue.id().to_owned(),
                note: Some("this login places no orders".to_owned()),
            });
        }
        self.settle(ctx, account.id, account.settings).await?;

        if !matches!(request.kind, OrderKind::Market) {
            return self.rest_order(ctx, account, request);
        }

        let mark = ctx.mark_price(&request.instrument).await?.price;
        let terms = Terms {
            fee_bps: 0,
            slippage_bps: self.venue.slippage_bps(account.settings),
            leverage: 1,
        };
        let fill_price = market_fill_price(mark, request.side, terms)?;
        let model = self.venue.model_for(account.settings)?;
        let currency = self.venue.fee_currency(account.settings, request.side);

        let order_id = OrderId::new(format!("{}-{}", self.venue.id(), uuid_like(ctx)));
        let snapshot = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account.id) else {
                return Err(TradeError::UnknownAccount);
            };
            let settled = model.settle(
                &mut stored.book,
                &FillContext {
                    instrument: &request.instrument,
                    side: request.side,
                    quantity: request.quantity,
                    price: fill_price,
                    now: ctx.now(),
                },
            )?;
            stored.fills.push(StoredFill {
                order_id: order_id.to_string(),
                instrument: request.instrument.to_string(),
                side: request.side,
                quantity: request.quantity,
                price: settled.fill_price,
                fee: settled.fee,
                fee_currency: currency,
                at: ctx.now().as_nanos(),
            });
            accounts.clone()
        };
        self.persist(&snapshot);

        Ok(Order {
            id: order_id,
            client_order_id: request.client_order_id.clone(),
            account_id: account.id,
            instrument: request.instrument,
            side: request.side,
            kind: OrderKind::Market,
            quantity: request.quantity,
            filled_quantity: request.quantity,
            average_price: Some(fill_price),
            time_in_force: TimeInForce::Gtc,
            status: OrderStatus::Filled,
            reduce_only: request.reduce_only,
            post_only: false,
            submitted_at: ctx.now(),
            updated_at: ctx.now(),
            reject_reason: None,
        })
    }
}

/// A per-call identifier stable within one context.
///
/// The context's own instant, which is one value for the whole call, so an
/// order and the fill it produced cannot end up microseconds apart in the
/// wrong direction.
fn uuid_like(ctx: &TradeContext<'_>) -> i64 {
    ctx.now().as_nanos()
}
