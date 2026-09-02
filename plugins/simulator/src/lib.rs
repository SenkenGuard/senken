//! Senken's built-in paper-trading adapter.
//!
//! It registers one [`TradeAdapter`] that will trade **every instrument
//! this installation has a catalog for**, whichever venue the instrument
//! came from. That is the point of it: a user can test a strategy on a
//! Kraken pair and a Deribit option without holding an account at either,
//! and a new venue plugin becomes paper-tradable the moment it is
//! installed, with nothing here to update.
//!
//! It is also the reference implementation of the trade contract. An author
//! writing a real venue's adapter has, in this crate, a complete worked
//! example of a settings schema, custom actions, capability declaration and
//! order handling, in about the space a real one takes.
//!
//! # What it is honest about
//!
//! * It is **cash-settled** against the account currency and does not
//!   custody base assets — see [`book`]'s own docs for why one settlement
//!   model covers spot, perpetuals and FX at once, and what that costs.
//! * Resting orders are matched **against the mark, when the account is
//!   read**, not against an order book. There is no depth to match into.
//! * A market order fills at the mark plus a fixed slippage in basis
//!   points. That is the whole model.
//! * It reaches no network. Prices come from whatever
//!   [`MarkPriceSource`](senken_trade::MarkPriceSource) the engine was
//!   assembled with.
//!
//! The books live in one atomic JSON snapshot per installation under
//! `trade/simulator/books.json`, written through `senken-storage` like
//! every other piece of on-disk state.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use senken_core::decimal::Scaled;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_storage::{Snapshot, Storage};
use senken_trade::{
    AccountAccess, AccountBalances, AccountRef, ActionOutcome, AdapterAction, AdapterCapabilities,
    AdapterFeature, AdapterHealth, AdapterKind, ChoiceOption, FieldKind, Fill, InstrumentCoverage,
    Liquidity, MarginMode, MarginTerms, Order, OrderAmendment, OrderFilter, OrderId, OrderKindTag,
    OrderRequest, OrderStatus, Position, PositionBasis, PositionId, PositionMode, PositionSide,
    QuantityUnit, SettingField, SettingsSchema, SettingsValues, TimeInForce, TradeAccountId,
    TradeAdapter, TradeContext, TradeError,
};

/// The simulated books and the rules that move between them.
pub mod book;
/// The netting rule this adapter settles fills by.
pub mod netting;

use crate::book::{Book, BookOrder};
use senken_sim_core::money::{CASH_SCALE, notional, rescale};
use senken_sim_core::pricing::{Terms, apply_amendment, is_triggered, market_fill_price};

/// The adapter's id, and the plugin's permission namespace.
pub const ADAPTER_ID: &str = "simulator";

/// Where the books are kept, relative to the data directory.
const BOOKS_PATH: &str = "trade/simulator/books.json";

/// Layout version of the books snapshot. Bump when [`Book`] changes
/// incompatibly; an older snapshot is then refused rather than misread.
const BOOKS_SCHEMA_VERSION: u32 = 1;

/// Settings keys. Named constants because the schema, the reader and the
/// tests all have to agree on them, and a typo in one of three string
/// literals is a setting that silently takes its default.
const KEY_CURRENCY: &str = "currency";
const KEY_STARTING_BALANCE: &str = "starting_balance";
const KEY_LEVERAGE: &str = "leverage";
const KEY_FEE_BPS: &str = "fee_bps";
const KEY_SLIPPAGE_BPS: &str = "slippage_bps";
const KEY_ACCESS: &str = "access";

/// [`KEY_ACCESS`]'s read-only option value — a paper account shared with
/// someone who should see it and not trade it, the same shape a broker's
/// investor login has.
const ACCESS_READ_ONLY: &str = "read_only";

/// Action ids.
const ACTION_DEPOSIT: &str = "deposit";
const ACTION_RESET: &str = "reset";
const ACTION_AMOUNT: &str = "amount";

/// The paper broker.
#[derive(Debug)]
pub struct SimulatorAdapter {
    storage: Storage,
    books: Mutex<BTreeMap<TradeAccountId, Book>>,
}

impl SimulatorAdapter {
    /// Builds the adapter over `storage`, loading whatever books are
    /// already on disk.
    ///
    /// A snapshot that cannot be read is logged and treated as absent
    /// rather than failing startup: refusing to boot the whole application
    /// over an unreadable *simulated* ledger would be the wrong trade.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        let books = match storage
            .read_snapshot::<BTreeMap<TradeAccountId, Book>>(BOOKS_PATH, BOOKS_SCHEMA_VERSION)
        {
            Ok(Some(snapshot)) => snapshot.data,
            Ok(None) => BTreeMap::new(),
            Err(error) => {
                tracing::warn!(%error, "simulator books could not be read; starting empty");
                BTreeMap::new()
            }
        };
        Self {
            storage,
            books: Mutex::new(books),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<TradeAccountId, Book>> {
        self.books.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the books out. Failure is logged, not returned: the trade the
    /// user just made has already happened in memory, and reporting it as a
    /// failure would be a lie about a fill that exists.
    fn persist(&self, books: &BTreeMap<TradeAccountId, Book>) {
        let snapshot = Snapshot::new(BOOKS_SCHEMA_VERSION, books);
        if let Err(error) = self.storage.write_snapshot(BOOKS_PATH, &snapshot) {
            tracing::error!(%error, "simulator books could not be written");
        }
    }
}

/// Reads one account's terms out of its settings.
///
/// Every value has a schema default, so a setting that is somehow absent
/// takes the same value the form would have shown — there is no branch here
/// that invents a different one.
fn terms_of(settings: &SettingsValues) -> Terms {
    Terms {
        fee_bps: settings.number(KEY_FEE_BPS).unwrap_or(4),
        slippage_bps: settings.number(KEY_SLIPPAGE_BPS).unwrap_or(2),
        leverage: settings.number(KEY_LEVERAGE).unwrap_or(1).max(1),
    }
}

fn currency_of(settings: &SettingsValues) -> String {
    settings.text(KEY_CURRENCY).unwrap_or("USD").to_owned()
}

/// The starting balance, at [`CASH_SCALE`].
fn starting_cash(settings: &SettingsValues) -> Result<i64, TradeError> {
    let declared = settings
        .decimal(KEY_STARTING_BALANCE)
        .unwrap_or_else(|| Scaled::new(2, 10_000_000));
    rescale(i128::from(declared.value), declared.scale, CASH_SCALE)
}

impl SimulatorAdapter {
    /// Fills every resting order whose condition the current mark meets.
    ///
    /// Run before any read of an account, which is what makes "resting"
    /// mean anything without a background task: the books are always
    /// brought up to date with the market before anyone looks at them.
    async fn settle(
        &self,
        ctx: &TradeContext<'_>,
        account: TradeAccountId,
        terms: Terms,
    ) -> Result<(), TradeError> {
        let pending: Vec<(usize, senken_marketdata::InstrumentId)> = {
            let books = self.lock();
            let Some(book) = books.get(&account) else {
                return Ok(());
            };
            book.orders
                .iter()
                .enumerate()
                .filter(|(_, order)| order.status.is_open())
                .map(|(index, order)| (index, order.instrument.clone()))
                .collect()
        };
        if pending.is_empty() {
            return Ok(());
        }

        // Marks are fetched outside the lock: the source may reach a disk
        // or a network, and holding a mutex across an await would serialise
        // every account in the process behind the slowest one.
        let mut marks = BTreeMap::new();
        for (_, instrument) in &pending {
            if !marks.contains_key(instrument) {
                marks.insert(instrument.clone(), ctx.try_mark_price(instrument).await?);
            }
        }

        let mut books = self.lock();
        let Some(book) = books.get_mut(&account) else {
            return Ok(());
        };
        let mut filled = false;
        for (index, instrument) in pending {
            let Some(Some(mark)) = marks.get(&instrument) else {
                continue;
            };
            // Re-read rather than trusting the index's contents: the vector
            // has not been mutated, but the order may have been filled by
            // an earlier iteration of this same loop.
            let Some(order) = book.orders.get(index) else {
                continue;
            };
            if !order.status.is_open() || !is_triggered(order.kind, order.side, mark.price) {
                continue;
            }
            let mut order = order.clone();
            // A limit fills at its own price; a stop becomes a market order
            // and takes the mark.
            let price = order.kind.limit_price().unwrap_or(mark.price);
            let liquidity = if order.kind.limit_price().is_some() {
                Liquidity::Maker
            } else {
                Liquidity::Taker
            };
            crate::book::execute(
                book,
                account,
                &mut order,
                price,
                terms,
                liquidity,
                ctx.now(),
            )?;
            book.orders[index] = order;
            filled = true;
        }
        if filled {
            book.trim_history();
            let snapshot = books.clone();
            drop(books);
            self.persist(&snapshot);
        }
        Ok(())
    }

    /// Margin currently held against open positions, at [`CASH_SCALE`].
    async fn margin_used(
        ctx: &TradeContext<'_>,
        book: &Book,
        terms: Terms,
    ) -> Result<i64, TradeError> {
        let mut total: i64 = 0;
        for (instrument, position) in &book.positions {
            let mark = ctx
                .try_mark_price(instrument)
                .await?
                .map_or(position.average_entry, |mark| mark.price);
            total = total.saturating_add(notional(mark, position.quantity)? / terms.leverage);
        }
        Ok(total)
    }
}

#[async_trait]
impl TradeAdapter for SimulatorAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn name(&self) -> &'static str {
        "Senken Simulator"
    }

    fn kind(&self) -> AdapterKind {
        AdapterKind::Simulation
    }

    fn description(&self) -> &'static str {
        "Paper trading against Senken's own prices. Every instrument on the platform is tradable, \
         and no order leaves this machine."
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            .with_order_kinds(vec![
                OrderKindTag::Market,
                OrderKindTag::Limit,
                OrderKindTag::Stop,
            ])
            .with_time_in_force(vec![TimeInForce::Gtc])
            .with_quantity_unit(QuantityUnit::Base)
            .with_position_mode(PositionMode::Netting)
            .with_margin()
            .with_feature(AdapterFeature::CancelOrders)
            .with_feature(AdapterFeature::ModifyOrders)
    }

    fn coverage(&self) -> InstrumentCoverage {
        // The whole reason this adapter is worth shipping: a strategy can
        // be paper-traded on any instrument this installation catalogs,
        // including one a venue plugin added after this line was written.
        InstrumentCoverage::Universal
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::new(vec![
            SettingField::new(
                KEY_CURRENCY,
                "Account currency",
                FieldKind::Choice {
                    default: Some("USD".to_owned()),
                    options: vec![
                        ChoiceOption::new("USD", "USD"),
                        ChoiceOption::new("USDT", "USDT"),
                        ChoiceOption::new("EUR", "EUR"),
                        ChoiceOption::new("IDR", "IDR"),
                    ],
                },
            )
            .with_help("What every balance and profit figure on this account is denominated in."),
            SettingField::new(
                KEY_STARTING_BALANCE,
                "Starting balance",
                FieldKind::Decimal {
                    scale: 2,
                    default: Some(10_000_000),
                    min: 0,
                    max: 100_000_000_000,
                    unit: String::new(),
                },
            )
            .with_help("What the account opens with. Resetting the account returns it to this."),
            SettingField::new(
                KEY_LEVERAGE,
                "Leverage",
                FieldKind::Number {
                    default: Some(1),
                    min: 1,
                    max: 125,
                    unit: "x".to_owned(),
                },
            )
            .with_help("How much notional this account may hold per unit of margin."),
            SettingField::new(
                KEY_FEE_BPS,
                "Fee",
                FieldKind::Number {
                    default: Some(4),
                    min: 0,
                    max: 1_000,
                    unit: "bps".to_owned(),
                },
            )
            .with_help("Charged on every fill, as basis points of the traded notional."),
            SettingField::new(
                KEY_SLIPPAGE_BPS,
                "Slippage",
                FieldKind::Number {
                    default: Some(2),
                    min: 0,
                    max: 1_000,
                    unit: "bps".to_owned(),
                },
            )
            .with_help(
                "How far a market order fills from the mark. Always against you, never for you.",
            ),
            SettingField::new(
                KEY_ACCESS,
                "Access",
                FieldKind::Choice {
                    default: Some("trade".to_owned()),
                    options: vec![
                        ChoiceOption::new("trade", "Trading"),
                        ChoiceOption::new(ACCESS_READ_ONLY, "Read-only (investor)"),
                    ],
                },
            )
            .with_help(
                "A read-only account can be opened and read but takes no orders — the same \
                 shape a broker's investor login has.",
            ),
        ])
    }

    fn actions(&self) -> Vec<AdapterAction> {
        vec![
            AdapterAction::new(ACTION_DEPOSIT, "Deposit funds")
                .with_description("Adds to this account's cash balance.")
                .with_form(SettingsSchema::new(vec![SettingField::new(
                    ACTION_AMOUNT,
                    "Amount",
                    FieldKind::Decimal {
                        scale: 2,
                        default: Some(1_000_000),
                        min: 1,
                        max: 100_000_000_000,
                        unit: String::new(),
                    },
                )])),
            AdapterAction::new(ACTION_RESET, "Reset account")
                .with_description(
                    "Closes every position, cancels every order and returns the balance to what \
                     the account started with.",
                )
                .destructive(),
        ]
    }

    async fn open_account(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<(), TradeError> {
        let cash = starting_cash(account.settings)?;
        let currency = currency_of(account.settings);
        let mut books = self.lock();
        // Idempotent: this runs again on every settings edit, and resetting
        // the books here would wipe a user's positions the moment they
        // changed the fee.
        let entry = books
            .entry(account.id)
            .or_insert_with(|| Book::new(currency.clone(), cash));
        entry.currency = currency;
        let snapshot = books.clone();
        drop(books);
        self.persist(&snapshot);
        let _ = ctx;
        Ok(())
    }

    async fn account_access(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountAccess, TradeError> {
        if account.settings.text(KEY_ACCESS) == Some(ACCESS_READ_ONLY) {
            return Ok(AccountAccess::read_only(
                self.capabilities(),
                Some("This account was attached read-only.".to_owned()),
            ));
        }
        Ok(AccountAccess::trading(self.capabilities()))
    }

    async fn close_account(&self, account: AccountRef<'_>) -> Result<(), TradeError> {
        let mut books = self.lock();
        books.remove(&account.id);
        let snapshot = books.clone();
        drop(books);
        self.persist(&snapshot);
        Ok(())
    }

    async fn health(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AdapterHealth, TradeError> {
        if self.lock().contains_key(&account.id) {
            Ok(AdapterHealth::Connected)
        } else {
            // The attachment exists but the books do not — the snapshot was
            // lost, or this is a fresh install restoring an old accounts
            // database. Said rather than papered over with an empty book.
            Ok(AdapterHealth::degraded(
                "this account has no simulated books yet; save its settings to create them",
            ))
        }
    }

    async fn balances(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountBalances, TradeError> {
        let terms = terms_of(account.settings);
        self.settle(ctx, account.id, terms).await?;

        let book = self
            .lock()
            .get(&account.id)
            .cloned()
            .ok_or(TradeError::UnknownAccount)?;

        let mut unrealized: i64 = 0;
        for (instrument, position) in &book.positions {
            if let Some(mark) = ctx.try_mark_price(instrument).await? {
                unrealized = unrealized.saturating_add(crate::book::close_pnl(
                    position.side,
                    position.average_entry,
                    mark.price,
                    position.quantity,
                )?);
            }
        }
        let margin_used = Self::margin_used(ctx, &book, terms).await?;

        Ok(AccountBalances {
            account_id: account.id,
            currency: book.currency,
            balance: Scaled::new(CASH_SCALE, book.cash),
            equity: Scaled::new(CASH_SCALE, book.cash.saturating_add(unrealized)),
            unrealized_pnl: Scaled::new(CASH_SCALE, unrealized),
            realized_pnl: Scaled::new(CASH_SCALE, book.realized_total),
            margin_used: Some(Scaled::new(CASH_SCALE, margin_used)),
            margin_available: Some(Scaled::new(
                CASH_SCALE,
                book.cash
                    .saturating_add(unrealized)
                    .saturating_sub(margin_used)
                    .max(0),
            )),
            // Equity over margin held, as a percentage — the figure a
            // margin call and a stop out are thresholds on. `None` with no
            // margin held: the ratio has no denominator there, and
            // reporting an enormous number instead would make an idle
            // account look like its most leveraged moment.
            margin_level: (margin_used > 0).then(|| {
                Scaled::new(
                    2,
                    book.cash.saturating_add(unrealized).saturating_mul(10_000) / margin_used,
                )
            }),
            // No per-asset rows: this adapter is cash-settled and holds no
            // base assets to report. An empty list is the honest answer,
            // not a missing feature.
            assets: Vec::new(),
        })
    }

    async fn positions(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<Vec<Position>, TradeError> {
        let terms = terms_of(account.settings);
        self.settle(ctx, account.id, terms).await?;

        let book = self
            .lock()
            .get(&account.id)
            .cloned()
            .ok_or(TradeError::UnknownAccount)?;

        let mut out = Vec::with_capacity(book.positions.len());
        for (instrument, position) in &book.positions {
            let mark = ctx.try_mark_price(instrument).await?.map(|mark| mark.price);
            // No mark means no profit figure, reported as absent rather
            // than as a flat zero — a real position showing exactly no
            // profit is a claim, and a wrong one.
            let unrealized = match mark {
                Some(mark) => Some(Scaled::new(
                    CASH_SCALE,
                    crate::book::close_pnl(
                        position.side,
                        position.average_entry,
                        mark,
                        position.quantity,
                    )?,
                )),
                None => None,
            };
            out.push(Position {
                // One position per instrument on a netting book, so the
                // instrument names it uniquely and the id stays stable
                // across reads — a client holding a row from one poll can
                // still act on it after the next.
                id: PositionId::new(instrument.to_string()),
                account_id: account.id,
                instrument: instrument.clone(),
                side: position.side,
                quantity: position.quantity,
                average_entry: position.average_entry,
                mark_price: mark,
                unrealized_pnl: unrealized,
                realized_pnl: Scaled::new(CASH_SCALE, position.realized),
                // This adapter attaches no stops and has no liquidation of
                // its own: `None` says so, rather than a zero that would
                // read as a stop set at nothing.
                stop_loss: None,
                take_profit: None,
                basis: PositionBasis::Margined(MarginTerms {
                    margin: Scaled::new(
                        CASH_SCALE,
                        notional(mark.unwrap_or(position.average_entry), position.quantity)?
                            / terms.leverage,
                    ),
                    leverage: Scaled::new(0, terms.leverage),
                    mode: MarginMode::Cross,
                    liquidation_price: None,
                }),
                opened_at: position.opened_at,
            });
        }
        Ok(out)
    }

    async fn orders(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        filter: OrderFilter,
    ) -> Result<Vec<Order>, TradeError> {
        self.settle(ctx, account.id, terms_of(account.settings))
            .await?;

        let book = self
            .lock()
            .get(&account.id)
            .cloned()
            .ok_or(TradeError::UnknownAccount)?;

        let mut orders: Vec<Order> = book
            .orders
            .iter()
            .filter(|order| match filter {
                OrderFilter::All => true,
                // `OrderFilter` is `#[non_exhaustive]`: a variant this
                // adapter has not been taught falls back to the narrower
                // answer rather than dumping every historical order at a
                // caller that asked for something else.
                _ => order.status.is_open(),
            })
            .map(|order| crate::book::to_order(order, account.id))
            .collect();
        orders.reverse();
        Ok(orders)
    }

    async fn fills(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        limit: usize,
    ) -> Result<Vec<Fill>, TradeError> {
        self.settle(ctx, account.id, terms_of(account.settings))
            .await?;

        let book = self
            .lock()
            .get(&account.id)
            .cloned()
            .ok_or(TradeError::UnknownAccount)?;

        let mut fills: Vec<Fill> = book.fills.iter().rev().take(limit).cloned().collect();
        fills.shrink_to_fit();
        Ok(fills)
    }

    async fn place_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError> {
        let terms = terms_of(account.settings);
        self.settle(ctx, account.id, terms).await?;

        // A market order needs a price to fill at; a resting one only needs
        // one when it is checked. Asking for a mark up front for a limit
        // order would refuse to accept an order on an instrument whose
        // history has not been loaded yet, which is not a real reason.
        let immediate_price = if request.kind.tag() == OrderKindTag::Market {
            Some(market_fill_price(
                ctx.mark_price(&request.instrument).await?.price,
                request.side,
                terms,
            )?)
        } else {
            None
        };

        let mut books = self.lock();
        let book = books
            .get_mut(&account.id)
            .ok_or(TradeError::UnknownAccount)?;

        if request.reduce_only {
            let held = book
                .positions
                .get(&request.instrument)
                .filter(|position| position.side != side_of(request.side))
                .map_or(0, |position| position.quantity.value);
            if held < request.quantity.value {
                return Err(TradeError::invalid(
                    "a reduce-only order cannot be larger than the position it is closing",
                ));
            }
        }

        if let Some(price) = immediate_price {
            let required = notional(price, request.quantity)? / terms.leverage;
            if required > book.cash.max(0) {
                return Err(TradeError::InsufficientBalance(format!(
                    "this order needs more margin than the account's {} balance covers",
                    book.currency
                )));
            }
        }

        let id = OrderId::new(uuid::Uuid::new_v4().to_string());
        let mut stored = BookOrder {
            id,
            client_order_id: request
                .client_order_id
                .as_ref()
                .map(|id| id.as_str().to_owned()),
            instrument: request.instrument.clone(),
            side: request.side,
            kind: request.kind,
            quantity: request.quantity,
            filled: Scaled::new(request.quantity.scale, 0),
            average_price: None,
            time_in_force: request.time_in_force,
            status: OrderStatus::Open,
            reduce_only: request.reduce_only,
            submitted_at: ctx.now(),
            updated_at: ctx.now(),
            reject_reason: None,
        };

        if let Some(price) = immediate_price {
            crate::book::execute(
                book,
                account.id,
                &mut stored,
                price,
                terms,
                Liquidity::Taker,
                ctx.now(),
            )?;
        }

        let reported = crate::book::to_order(&stored, account.id);
        book.orders.push(stored);
        book.trim_history();
        let snapshot = books.clone();
        drop(books);
        self.persist(&snapshot);
        Ok(reported)
    }

    async fn cancel_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
    ) -> Result<Order, TradeError> {
        self.settle(ctx, account.id, terms_of(account.settings))
            .await?;

        let mut books = self.lock();
        let book = books
            .get_mut(&account.id)
            .ok_or(TradeError::UnknownAccount)?;
        let order = book
            .orders
            .iter_mut()
            .find(|order| order.id == *order_id)
            .ok_or(TradeError::UnknownOrder)?;
        if !order.status.is_open() {
            return Err(TradeError::OrderNotOpen);
        }
        order.status = OrderStatus::Cancelled;
        order.updated_at = ctx.now();
        let reported = crate::book::to_order(order, account.id);
        let snapshot = books.clone();
        drop(books);
        self.persist(&snapshot);
        Ok(reported)
    }

    async fn modify_order(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
        amendment: OrderAmendment,
    ) -> Result<Order, TradeError> {
        let terms = terms_of(account.settings);
        self.settle(ctx, account.id, terms).await?;

        {
            let mut books = self.lock();
            let book = books
                .get_mut(&account.id)
                .ok_or(TradeError::UnknownAccount)?;
            let order = book
                .orders
                .iter_mut()
                .find(|order| order.id == *order_id)
                .ok_or(TradeError::UnknownOrder)?;
            if !order.status.is_open() {
                return Err(TradeError::OrderNotOpen);
            }
            if let Some(quantity) = amendment.quantity {
                order.quantity = quantity;
            }
            order.kind = apply_amendment(order.kind, amendment);
            order.updated_at = ctx.now();
            let snapshot = books.clone();
            drop(books);
            self.persist(&snapshot);
        }

        // A limit or stop the current mark already satisfies must fill
        // right away rather than sit there until the account is next read
        // for some other reason — `settle` otherwise only ever runs on a
        // read, and an amendment is itself the event that can make a
        // resting order tradable.
        self.settle(ctx, account.id, terms).await?;

        let books = self.lock();
        let book = books.get(&account.id).ok_or(TradeError::UnknownAccount)?;
        let order = book
            .orders
            .iter()
            .find(|order| order.id == *order_id)
            .ok_or(TradeError::UnknownOrder)?;
        Ok(crate::book::to_order(order, account.id))
    }

    async fn run_action(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        action_id: &str,
        params: SettingsValues,
    ) -> Result<ActionOutcome, TradeError> {
        let _ = ctx;
        let mut books = self.lock();
        let book = books
            .get_mut(&account.id)
            .ok_or(TradeError::UnknownAccount)?;

        let outcome = match action_id {
            ACTION_DEPOSIT => {
                let declared = params
                    .decimal(ACTION_AMOUNT)
                    .ok_or_else(|| TradeError::invalid("a deposit needs an amount"))?;
                let amount = rescale(i128::from(declared.value), declared.scale, CASH_SCALE)?;
                book.deposit(amount);
                ActionOutcome::new(format!(
                    "Deposited {} {}.",
                    senken_core::decimal::format_scaled(declared.value, declared.scale),
                    book.currency
                ))
            }
            ACTION_RESET => {
                book.reset();
                ActionOutcome::new(format!(
                    "Account reset to {} {}.",
                    senken_core::decimal::format_scaled(book.initial_cash, CASH_SCALE),
                    book.currency
                ))
            }
            other => {
                return Err(TradeError::unsupported(
                    ADAPTER_ID,
                    format!("the action `{other}`"),
                ));
            }
        };

        let snapshot = books.clone();
        drop(books);
        self.persist(&snapshot);
        Ok(outcome)
    }
}

fn side_of(side: senken_trade::OrderSide) -> PositionSide {
    match side {
        senken_trade::OrderSide::Buy => PositionSide::Long,
        senken_trade::OrderSide::Sell => PositionSide::Short,
    }
}

/// The plugin that registers [`SimulatorAdapter`].
#[derive(Debug)]
pub struct SimulatorPlugin {
    adapter: Arc<SimulatorAdapter>,
}

impl SimulatorPlugin {
    /// Builds the plugin over `storage`, which is where its books live.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self {
            adapter: Arc::new(SimulatorAdapter::new(storage)),
        }
    }
}

impl Plugin for SimulatorPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ADAPTER_ID.to_owned(),
            name: "Senken Simulator".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Paper trading against Senken's own prices, on every instrument"
                .to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate_without_io(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        context.register_trade_adapter(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>);
        Ok(())
    }
}
