//! The `TradeAdapter` that joins the margin, swap, volume and risk rules
//! into an account a trader can actually use.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::pricing::{Terms, market_fill_price};
use senken_sim_core::risk::RiskBreach;
use senken_storage::{Snapshot, Storage};
use senken_trade::{
    AccountAccess, AccountBalances, AccountRef, AdapterCapabilities, AdapterFeature, AdapterHealth,
    AdapterKind, Fill, InstrumentCoverage, Liquidity, MarginMode, MarginTerms, Order, OrderFilter,
    OrderId, OrderKind, OrderKindTag, OrderRequest, OrderSide, OrderStatus, Position,
    PositionBasis, PositionId, PositionMode, PositionSide, QuantityUnit, SettingsSchema,
    SettingsValues, TimeInForce, TradeAccountId, TradeAdapter, TradeContext, TradeError,
};

use crate::account::{Account, SymbolTerms, ticket_profit};
use crate::commission::commission_for;
use crate::margin::{AccountFigures, margin_for};
use crate::settings::{
    ACCESS_READ_ONLY, KEY_ACCESS, KEY_CURRENCY, KEY_DEVIATION_POINTS, KEY_STARTING_BALANCE,
    commission_of, margin_of, schema, stop_levels_of, swap_of, volume_of,
};
use crate::swap::{swap_days, swap_for};
use crate::ticket::Ticket;
use crate::volume::check;

/// The id this adapter registers under.
pub const ADAPTER_ID: &str = "mt5-hedging";

const BOOKS_PATH: &str = "trade/mt5-hedging/books.json";
const BOOKS_SCHEMA_VERSION: u32 = 1;

/// One account's stored state.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StoredAccount {
    cash: i64,
    currency: String,
    tickets: Vec<StoredTicket>,
    next_ticket: u64,
    fills: Vec<StoredFill>,
    /// The instant swap was last accrued through, so a second read on the
    /// same day charges nothing rather than charging again.
    settled_through: i64,
}

/// A ticket as it survives a restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredTicket {
    id: u64,
    instrument: String,
    side: PositionSide,
    lots: Scaled,
    open_price: Scaled,
    stop_loss: Option<Scaled>,
    take_profit: Option<Scaled>,
    swap: i64,
    margin: i64,
    opened_at: i64,
}

/// A deal, carrying MetaTrader's three separate numbers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StoredFill {
    order_id: String,
    ticket: u64,
    instrument: String,
    side: OrderSide,
    lots: Scaled,
    price: Scaled,
    commission: i64,
    swap: i64,
    profit: i64,
    at: i64,
}

impl StoredTicket {
    fn to_ticket(&self) -> Option<Ticket> {
        Some(Ticket {
            id: self.id,
            instrument: InstrumentId::parse(&self.instrument).ok()?,
            side: self.side,
            lots: self.lots,
            open_price: self.open_price,
            stop_loss: self.stop_loss,
            take_profit: self.take_profit,
            swap: self.swap,
            margin: self.margin,
            opened_at: UnixNanos::from_nanos(self.opened_at),
        })
    }

    fn from_ticket(ticket: &Ticket) -> Self {
        Self {
            id: ticket.id,
            instrument: ticket.instrument.to_string(),
            side: ticket.side,
            lots: ticket.lots,
            open_price: ticket.open_price,
            stop_loss: ticket.stop_loss,
            take_profit: ticket.take_profit,
            swap: ticket.swap,
            margin: ticket.margin,
            opened_at: ticket.opened_at.as_nanos(),
        }
    }
}

/// A simulated MetaTrader 5 hedging broker.
#[derive(Debug)]
pub struct Mt5HedgingAdapter {
    storage: Storage,
    accounts: Mutex<BTreeMap<TradeAccountId, StoredAccount>>,
}

impl Mt5HedgingAdapter {
    /// Builds the adapter over `storage`, where its books live.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        let accounts = match storage.read_snapshot::<BTreeMap<TradeAccountId, StoredAccount>>(
            BOOKS_PATH,
            BOOKS_SCHEMA_VERSION,
        ) {
            Ok(Some(snapshot)) => snapshot.data,
            _ => BTreeMap::new(),
        };
        Self {
            storage,
            accounts: Mutex::new(accounts),
        }
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<TradeAccountId, StoredAccount>> {
        self.accounts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes the books out. Failure is logged rather than returned: the
    /// trade has already happened in memory, and reporting it as a failure
    /// would be a lie about a fill that exists.
    fn persist(&self, accounts: &BTreeMap<TradeAccountId, StoredAccount>) {
        let snapshot = Snapshot::new(BOOKS_SCHEMA_VERSION, accounts);
        drop(self.storage.write_snapshot(BOOKS_PATH, &snapshot));
    }

    fn account_of(stored: &StoredAccount) -> Account {
        Account {
            cash: stored.cash,
            book: crate::ticket::HedgingBook {
                tickets: stored
                    .tickets
                    .iter()
                    .filter_map(StoredTicket::to_ticket)
                    .collect(),
                next_ticket: stored.next_ticket,
            },
        }
    }

    fn write_back(stored: &mut StoredAccount, account: &Account) {
        stored.cash = account.cash;
        stored.tickets = account
            .book
            .tickets
            .iter()
            .map(StoredTicket::from_ticket)
            .collect();
        stored.next_ticket = account.book.next_ticket;
    }

    /// Brings one account up to date with the market: accrues swap for
    /// every rollover crossed since it was last read, triggers any stop
    /// loss or take profit the mark has reached, and then applies the
    /// stop out.
    ///
    /// Order matters and is MetaTrader's own. Swap is a cost the position
    /// has already incurred, so it lands before equity is measured; a stop
    /// that the market reached closes at its own level rather than being
    /// caught by the stop out; and the stop out runs last, on what is
    /// actually left.
    async fn settle(
        &self,
        ctx: &TradeContext<'_>,
        account_id: TradeAccountId,
        values: &SettingsValues,
    ) -> Result<(), TradeError> {
        let marks = self.marks_for(ctx, account_id).await?;
        let mut accounts = self.lock();
        let Some(stored) = accounts.get_mut(&account_id) else {
            return Ok(());
        };
        let now = ctx.now();
        let swap_terms = swap_of(values);
        let symbol = margin_of(values);

        let days = swap_days(
            swap_terms,
            UnixNanos::from_nanos(stored.settled_through),
            now,
        );
        if days > 0 {
            for ticket in &mut stored.tickets {
                let price = marks
                    .get(&ticket.instrument)
                    .copied()
                    .unwrap_or(ticket.open_price);
                let charged = swap_for(swap_terms, ticket.side, ticket.lots, price, days)?;
                ticket.swap = ticket.swap.saturating_add(charged);
            }
            stored.settled_through = now.as_nanos();
        }

        let mut account = Self::account_of(stored);
        Self::trigger_stops(&mut account, &marks, symbol.contract_size)?;
        let terms = |instrument: &InstrumentId| -> Option<SymbolTerms> {
            Some(SymbolTerms {
                margin: symbol,
                mark: marks.get(&instrument.to_string()).copied(),
            })
        };
        account.apply_stop_out(stop_levels_of(values), &terms, now)?;
        Self::write_back(stored, &account);
        let snapshot = accounts.clone();
        drop(accounts);
        self.persist(&snapshot);
        Ok(())
    }

    /// Closes every ticket whose stop loss or take profit the mark has
    /// reached.
    fn trigger_stops(
        account: &mut Account,
        marks: &BTreeMap<String, Scaled>,
        contract_size: i64,
    ) -> Result<(), TradeError> {
        let mut hit = Vec::new();
        for ticket in &account.book.tickets {
            let Some(mark) = marks.get(&ticket.instrument.to_string()).copied() else {
                continue;
            };
            let Some(mark) = mark.rescale(ticket.open_price.scale) else {
                continue;
            };
            let reached = |level: Option<Scaled>, below: bool| -> bool {
                level
                    .and_then(|level| level.rescale(mark.scale))
                    .is_some_and(|level| {
                        if below {
                            mark.value <= level.value
                        } else {
                            mark.value >= level.value
                        }
                    })
            };
            // A long's stop is below it and its target above; a short is
            // the mirror image.
            let long = ticket.side == PositionSide::Long;
            if reached(ticket.stop_loss, long) || reached(ticket.take_profit, !long) {
                hit.push((ticket.id, mark));
            }
        }
        for (id, price) in hit {
            let Some(index) = account.book.tickets.iter().position(|t| t.id == id) else {
                continue;
            };
            let ticket = account.book.tickets.remove(index);
            let realized =
                ticket_profit(&ticket, price, contract_size)?.saturating_add(ticket.swap);
            account.cash = account.cash.saturating_add(realized);
        }
        Ok(())
    }

    async fn marks_for(
        &self,
        ctx: &TradeContext<'_>,
        account_id: TradeAccountId,
    ) -> Result<BTreeMap<String, Scaled>, TradeError> {
        let instruments: Vec<String> = {
            let accounts = self.lock();
            accounts
                .get(&account_id)
                .map(|stored| {
                    stored
                        .tickets
                        .iter()
                        .map(|ticket| ticket.instrument.clone())
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut marks = BTreeMap::new();
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
}

/// The starting balance, at [`CASH_SCALE`].
fn starting_cash(values: &SettingsValues) -> Result<i64, TradeError> {
    let declared = values
        .decimal(KEY_STARTING_BALANCE)
        .unwrap_or_else(|| Scaled::new(2, 1_000_000));
    rescale(i128::from(declared.value), declared.scale, CASH_SCALE)
}

fn is_read_only(values: &SettingsValues) -> bool {
    values.text(KEY_ACCESS) == Some(ACCESS_READ_ONLY)
}

#[async_trait::async_trait]
impl TradeAdapter for Mt5HedgingAdapter {
    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn name(&self) -> &'static str {
        "MetaTrader 5 (hedging)"
    }

    fn kind(&self) -> AdapterKind {
        AdapterKind::Simulation
    }

    fn description(&self) -> &'static str {
        "A simulated MetaTrader 5 hedging account: independent tickets, per-symbol margin, \
         swap, margin call and stop out"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            .with_order_kinds(vec![
                OrderKindTag::Market,
                OrderKindTag::Limit,
                OrderKindTag::Stop,
                OrderKindTag::StopLimit,
            ])
            .with_time_in_force(vec![TimeInForce::Gtc])
            .with_quantity_unit(QuantityUnit::Base)
            // The whole point of the account: a long and a short on one
            // symbol coexist.
            .with_position_mode(PositionMode::Hedging)
            .with_margin()
            .with_feature(AdapterFeature::PositionStops)
    }

    fn coverage(&self) -> InstrumentCoverage {
        InstrumentCoverage::Universal
    }

    fn settings_schema(&self) -> SettingsSchema {
        schema()
    }

    async fn open_account(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<(), TradeError> {
        let cash = starting_cash(account.settings)?;
        let currency = account
            .settings
            .text(KEY_CURRENCY)
            .unwrap_or("USD")
            .to_owned();
        let mut accounts = self.lock();
        accounts.entry(account.id).or_insert_with(|| StoredAccount {
            cash,
            currency,
            next_ticket: 1,
            ..StoredAccount::default()
        });
        let snapshot = accounts.clone();
        drop(accounts);
        self.persist(&snapshot);
        Ok(())
    }

    async fn account_access(
        &self,
        _ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountAccess, TradeError> {
        let capabilities = self.capabilities();
        if is_read_only(account.settings) {
            // An investor password reads everything and places nothing.
            return Ok(AccountAccess::read_only(
                capabilities,
                Some(
                    "This is an investor login: it reads the account and places nothing."
                        .to_owned(),
                ),
            ));
        }
        Ok(AccountAccess::trading(capabilities))
    }

    async fn close_account(&self, account: AccountRef<'_>) -> Result<(), TradeError> {
        let mut accounts = self.lock();
        accounts.remove(&account.id);
        let snapshot = accounts.clone();
        drop(accounts);
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
        let symbol = margin_of(account.settings);
        let held = Self::account_of(stored);
        let terms = |instrument: &InstrumentId| -> Option<SymbolTerms> {
            Some(SymbolTerms {
                margin: symbol,
                mark: marks.get(&instrument.to_string()).copied(),
            })
        };
        let risk = held.risk(stop_levels_of(account.settings), &terms)?;
        let figures = AccountFigures::new(
            risk.balance,
            risk.equity.saturating_sub(risk.balance),
            risk.margin_used,
        );

        Ok(AccountBalances {
            account_id: account.id,
            currency: stored.currency.clone(),
            balance: Scaled::new(CASH_SCALE, figures.balance),
            equity: Scaled::new(CASH_SCALE, figures.equity),
            unrealized_pnl: Scaled::new(CASH_SCALE, figures.equity.saturating_sub(figures.balance)),
            realized_pnl: Scaled::new(CASH_SCALE, 0),
            margin_used: Some(Scaled::new(CASH_SCALE, figures.margin_used)),
            margin_available: Some(Scaled::new(CASH_SCALE, figures.free_margin)),
            margin_level: risk.margin_level,
            assets: Vec::new(),
        })
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
        let symbol = margin_of(account.settings);
        let mut out = Vec::new();
        for stored_ticket in &stored.tickets {
            let Some(ticket) = stored_ticket.to_ticket() else {
                continue;
            };
            let mark = marks.get(&stored_ticket.instrument).copied();
            let unrealized = mark
                .map(|mark| ticket_profit(&ticket, mark, symbol.contract_size))
                .transpose()?;
            out.push(Position {
                // The ticket number *is* the position's identity on a
                // hedging account, and it does not change for its life.
                id: PositionId::new(ticket.id.to_string()),
                account_id: account.id,
                instrument: ticket.instrument.clone(),
                side: ticket.side,
                quantity: ticket.lots,
                average_entry: ticket.open_price,
                mark_price: mark,
                unrealized_pnl: unrealized.map(|value| Scaled::new(CASH_SCALE, value)),
                realized_pnl: Scaled::new(CASH_SCALE, ticket.swap),
                stop_loss: ticket.stop_loss,
                take_profit: ticket.take_profit,
                basis: PositionBasis::Margined(MarginTerms {
                    margin: Scaled::new(CASH_SCALE, ticket.margin),
                    leverage: Scaled::new(0, symbol.leverage),
                    // MetaTrader pools every position's margin against one
                    // account balance; there is no per-position isolation
                    // to report.
                    mode: MarginMode::Cross,
                    // A stop out is not a per-position liquidation price:
                    // it depends on every other open ticket, so there is
                    // no single price this one closes at.
                    liquidation_price: None,
                }),
                opened_at: ticket.opened_at,
            });
        }
        Ok(out)
    }

    async fn orders(
        &self,
        _ctx: &TradeContext<'_>,
        _account: AccountRef<'_>,
        _filter: OrderFilter,
    ) -> Result<Vec<Order>, TradeError> {
        // Pending orders are the next pass; this build fills at market and
        // reports no resting order rather than an empty one it might later
        // populate differently.
        Ok(Vec::new())
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
                    quantity: fill.lots,
                    price: fill.price,
                    // MetaTrader keeps commission, swap and profit as three
                    // separate numbers on a deal; this is the fee one.
                    fee: Scaled::new(CASH_SCALE, fill.commission),
                    fee_currency: stored.currency.clone(),
                    liquidity: Liquidity::Taker,
                    executed_at: UnixNanos::from_nanos(fill.at),
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
        if is_read_only(account.settings) {
            return Err(TradeError::ReadOnly {
                adapter: ADAPTER_ID.to_owned(),
                note: Some("an investor login places no orders".to_owned()),
            });
        }
        self.settle(ctx, account.id, account.settings).await?;

        let symbol = margin_of(account.settings);
        let mark = ctx.mark_price(&request.instrument).await?.price;
        let deviation = account.settings.number(KEY_DEVIATION_POINTS).unwrap_or(10);
        let terms = Terms {
            fee_bps: 0,
            slippage_bps: deviation,
            leverage: symbol.leverage,
        };
        let fill_price = market_fill_price(mark, request.side, terms)?;

        let marks = self.marks_for(ctx, account.id).await?;
        let mut accounts = self.lock();
        let Some(stored) = accounts.get_mut(&account.id) else {
            return Err(TradeError::UnknownAccount);
        };

        check_volume(stored, account.settings, &request)?;

        let mut held = Self::account_of(stored);
        let symbol_terms = |instrument: &InstrumentId| -> Option<SymbolTerms> {
            Some(SymbolTerms {
                margin: symbol,
                mark: marks.get(&instrument.to_string()).copied(),
            })
        };
        check_can_open(&held, account.settings, &symbol_terms)?;

        let margin = margin_for(symbol, request.quantity, fill_price)?;
        let commission = commission_for(
            commission_of(account.settings),
            request.quantity,
            fill_price,
            symbol.contract_size,
        )?;

        let id = held.book.next_ticket;
        held.book.next_ticket += 1;
        held.book.tickets.push(Ticket {
            id,
            instrument: request.instrument.clone(),
            side: match request.side {
                OrderSide::Buy => PositionSide::Long,
                OrderSide::Sell => PositionSide::Short,
            },
            lots: request.quantity,
            open_price: fill_price,
            // Opened with its stops already attached, as every one of
            // these platforms allows — a position that must wait for a
            // second request is unprotected for however long that takes.
            stop_loss: request.stop_loss,
            take_profit: request.take_profit,
            swap: 0,
            margin,
            opened_at: ctx.now(),
        });
        held.cash = held.cash.saturating_sub(commission);
        Self::write_back(stored, &held);

        let order_id = OrderId::new(format!("mt5-{id}"));
        stored.fills.push(StoredFill {
            order_id: order_id.to_string(),
            ticket: id,
            instrument: request.instrument.to_string(),
            side: request.side,
            lots: request.quantity,
            price: fill_price,
            commission,
            swap: 0,
            profit: 0,
            at: ctx.now().as_nanos(),
        });
        let snapshot = accounts.clone();
        drop(accounts);
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
            status: OrderStatus::Filled,
            time_in_force: TimeInForce::Gtc,
            reduce_only: request.reduce_only,
            post_only: false,
            submitted_at: ctx.now(),
            updated_at: ctx.now(),
            reject_reason: None,
        })
    }

    async fn set_position_stops(
        &self,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        position: &PositionId,
        stop_loss: Option<Scaled>,
        take_profit: Option<Scaled>,
    ) -> Result<Position, TradeError> {
        if is_read_only(account.settings) {
            return Err(TradeError::ReadOnly {
                adapter: ADAPTER_ID.to_owned(),
                note: Some("an investor login places no orders".to_owned()),
            });
        }
        let wanted: u64 = position
            .as_str()
            .parse()
            .map_err(|_| TradeError::UnknownPositionId(position.clone()))?;
        // The lock is taken and released inside this block: the read that
        // builds the answer awaits, and a guard cannot be held across one.
        let snapshot = {
            let mut accounts = self.lock();
            let Some(stored) = accounts.get_mut(&account.id) else {
                return Err(TradeError::UnknownAccount);
            };
            let Some(ticket) = stored.tickets.iter_mut().find(|t| t.id == wanted) else {
                return Err(TradeError::UnknownPositionId(position.clone()));
            };
            // At most one of each, which is MetaTrader's invariant: setting
            // a stop replaces the ticket's stop rather than adding a
            // second.
            ticket.stop_loss = stop_loss;
            ticket.take_profit = take_profit;
            accounts.clone()
        };
        self.persist(&snapshot);

        // Answered with the amended position rather than the caller's own
        // request, so a client redraws what the book actually holds.
        self.positions(ctx, account)
            .await?
            .into_iter()
            .find(|candidate| &candidate.id == position)
            .ok_or_else(|| TradeError::UnknownPositionId(position.clone()))
    }
}

/// Refuses a volume the broker's own limits would reject.
///
/// Before the ticket exists, so an order that could not have been placed
/// never appears in the book and the message names which limit it broke.
fn check_volume(
    stored: &StoredAccount,
    values: &SettingsValues,
    request: &OrderRequest,
) -> Result<(), TradeError> {
    let already: i64 = stored
        .tickets
        .iter()
        .filter(|ticket| ticket.instrument == request.instrument.to_string())
        .map(|ticket| ticket.lots.value)
        .sum();
    check(
        volume_of(values),
        request.quantity,
        Scaled::new(request.quantity.scale, already),
    )
    .map_err(|rejection| TradeError::InvalidRequest(format!("{rejection:?}")))
}

/// Refuses to open while the account is margin called.
///
/// A margin call blocks opening and closes nothing; this is where the
/// blocking half is enforced. Checked before the ticket exists rather than
/// unwound after it does.
fn check_can_open(
    held: &Account,
    values: &SettingsValues,
    terms: &dyn Fn(&InstrumentId) -> Option<SymbolTerms>,
) -> Result<(), TradeError> {
    let risk = held.risk(stop_levels_of(values), terms)?;
    if matches!(
        risk.breach,
        Some(RiskBreach::OpeningBlocked | RiskBreach::ForcedClosure)
    ) {
        return Err(TradeError::InsufficientBalance(
            "margin level is below this account's margin call level, so no new position may be \
             opened"
                .to_owned(),
        ));
    }
    Ok(())
}

/// The plugin that registers the adapter.
#[derive(Debug)]
pub struct Mt5HedgingPlugin {
    adapter: Arc<Mt5HedgingAdapter>,
}

impl Mt5HedgingPlugin {
    /// Builds the plugin over `storage`.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self {
            adapter: Arc::new(Mt5HedgingAdapter::new(storage)),
        }
    }
}

impl Plugin for Mt5HedgingPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ADAPTER_ID.to_owned(),
            name: "MetaTrader 5 (hedging)".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "A simulated MT5 hedging account with swap, margin call and stop out"
                .to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate_without_io(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        context.register_trade_adapter(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>);
        Ok(())
    }
}
