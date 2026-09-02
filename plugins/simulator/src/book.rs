//! The simulated books: one account's cash, positions, orders and fills,
//! and the rules that move between them.
//!
//! # The settlement model, stated plainly
//!
//! The simulator is **cash-settled against the account currency**. It does
//! not custody base assets: buying 0.25 BTC does not remove 17 105 USD from
//! the balance and add bitcoin to it. Opening a position costs the fee and
//! reserves margin; closing it moves the profit or loss into cash.
//!
//! That is a deliberate choice and it is what makes one simulator cover
//! spot, perpetuals and FX at once — the alternative is three settlement
//! models and three sets of rules about what an account may hold. The cost
//! is that a spot account here behaves like a margin account at 1×, which
//! is close enough for judging a strategy and is said out loud on the
//! adapter card rather than left to be discovered.
//!
//! # Resting orders are matched against the mark, on read
//!
//! There is no order book to rest in. A limit or stop order is checked
//! against the current mark every time the account is looked at, and fills
//! at its own price when the mark reaches it. This is optimistic — a real
//! book might not have had size at that price — and it is the honest
//! approximation available without depth data, not an attempt to look like
//! one.

use std::collections::BTreeMap;

use senken_core::UnixNanos;
use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use senken_trade::{
    Fill, Liquidity, Order, OrderId, OrderKind, OrderSide, OrderStatus, PositionSide, TimeInForce,
    TradeAccountId, TradeError,
};
use serde::{Deserialize, Serialize};

use senken_sim_core::money::{CASH_SCALE, notional};
use senken_sim_core::pricing::Terms;
use senken_sim_core::{FillContext, SettlementModel};

use crate::netting::Netting;

/// One open position in the simulated books.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookPosition {
    /// Long or short.
    pub side: PositionSide,
    /// Size held, at the instrument's own quantity scale.
    pub quantity: Scaled,
    /// Volume-weighted entry, at the instrument's own price scale.
    pub average_entry: Scaled,
    /// Profit already banked on this instrument, at [`CASH_SCALE`].
    pub realized: i64,
    /// When the position was first opened.
    pub opened_at: UnixNanos,
}

/// One order in the simulated books, resting or finished.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookOrder {
    /// The id the simulator minted.
    pub id: OrderId,
    /// The idempotency key it was sent with.
    pub client_order_id: Option<String>,
    /// The instrument.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: OrderSide,
    /// How it executes.
    pub kind: OrderKind,
    /// The size asked for.
    pub quantity: Scaled,
    /// How much has filled.
    pub filled: Scaled,
    /// The price it filled at, once it has.
    pub average_price: Option<Scaled>,
    /// How long it lives.
    pub time_in_force: TimeInForce,
    /// Where it has got to.
    pub status: OrderStatus,
    /// Whether it may only shrink a position.
    pub reduce_only: bool,
    /// When it was submitted.
    pub submitted_at: UnixNanos,
    /// When it last changed.
    pub updated_at: UnixNanos,
    /// Why it was rejected, when it was.
    pub reject_reason: Option<String>,
}

/// The whole state of one simulated account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Book {
    /// The currency every cash figure below is in.
    pub currency: String,
    /// Cash, at [`CASH_SCALE`], excluding unrealised profit.
    pub cash: i64,
    /// What the account started with, so a reset knows where to go back to.
    pub initial_cash: i64,
    /// Open positions, keyed by instrument.
    pub positions: BTreeMap<InstrumentId, BookPosition>,
    /// Every order, newest last.
    pub orders: Vec<BookOrder>,
    /// Every execution, newest last.
    pub fills: Vec<Fill>,
    /// Profit banked on this account, across every instrument and every
    /// position it has ever held, at [`CASH_SCALE`].
    ///
    /// An account total rather than a field on a position, because a
    /// position that has closed is not somewhere a completed fact can
    /// live. Keeping it per-position meant a profit survived a reversal —
    /// which carried the old figure forward — and vanished on a clean
    /// close, which removed the position and the number with it.
    #[serde(default)]
    pub realized_total: i64,
}

/// How many finished orders and fills one account keeps.
///
/// A paper account left running for months would otherwise grow its
/// snapshot without bound, and the snapshot is rewritten whole on every
/// order.
pub const HISTORY_LIMIT: usize = 500;

impl Book {
    /// A fresh account holding `cash` of `currency`.
    #[must_use]
    pub fn new(currency: String, cash: i64) -> Self {
        Self {
            currency,
            cash,
            initial_cash: cash,
            positions: BTreeMap::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            realized_total: 0,
        }
    }

    /// Every order still able to fill.
    pub fn open_orders(&self) -> impl Iterator<Item = &BookOrder> {
        self.orders.iter().filter(|order| order.status.is_open())
    }

    /// Adds `amount` to cash — the deposit action, and the way a reset puts
    /// the starting balance back.
    pub fn deposit(&mut self, amount: i64) {
        self.cash = self.cash.saturating_add(amount);
    }

    /// Drops everything but the currency and the starting balance.
    pub fn reset(&mut self) {
        self.cash = self.initial_cash;
        self.positions.clear();
        self.orders.clear();
        self.fills.clear();
        self.realized_total = 0;
    }

    /// Trims history back to [`HISTORY_LIMIT`], keeping the newest.
    ///
    /// Open orders are never trimmed however old they are: an order that
    /// can still fill is live state, not history.
    pub fn trim_history(&mut self) {
        if self.fills.len() > HISTORY_LIMIT {
            self.fills.drain(..self.fills.len() - HISTORY_LIMIT);
        }
        let finished = self
            .orders
            .iter()
            .filter(|order| order.status.is_terminal())
            .count();
        if finished > HISTORY_LIMIT {
            let mut to_drop = finished - HISTORY_LIMIT;
            self.orders.retain(|order| {
                if to_drop > 0 && order.status.is_terminal() {
                    to_drop -= 1;
                    return false;
                }
                true
            });
        }
    }
}

/// Executes `quantity` of `order` at `price`, updating the position, the
/// cash and the fill log.
///
/// # Errors
/// [`TradeError`] when the arithmetic overflows or the scales cannot be
/// reconciled.
pub fn execute(
    book: &mut Book,
    account_id: TradeAccountId,
    order: &mut BookOrder,
    price: Scaled,
    terms: Terms,
    liquidity: Liquidity,
    now: UnixNanos,
) -> Result<(), TradeError> {
    let quantity = order.quantity;
    let applied = Netting { terms }.settle(
        &mut book.positions,
        &FillContext {
            instrument: &order.instrument,
            side: order.side,
            quantity,
            price,
            now,
        },
    )?;

    book.cash = book
        .cash
        .saturating_add(applied.realized)
        .saturating_sub(applied.fee);
    book.realized_total = book.realized_total.saturating_add(applied.realized);

    order.filled = quantity;
    order.average_price = Some(applied.fill_price);
    order.status = OrderStatus::Filled;
    order.updated_at = now;

    book.fills.push(Fill {
        id: OrderId::new(format!("{}-f", order.id)),
        order_id: order.id.clone(),
        account_id,
        instrument: order.instrument.clone(),
        side: order.side,
        quantity,
        price: applied.fill_price,
        fee: Scaled::new(CASH_SCALE, applied.fee),
        fee_currency: book.currency.clone(),
        liquidity,
        executed_at: now,
    });
    Ok(())
}

/// Profit from closing `quantity` of a position opened at `entry` at
/// `exit`, at [`CASH_SCALE`].
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn close_pnl(
    side: PositionSide,
    entry: Scaled,
    exit: Scaled,
    quantity: Scaled,
) -> Result<i64, TradeError> {
    let difference = Scaled::new(entry.scale, exit.value.saturating_sub(entry.value));
    let gross = notional(difference, quantity)?;
    Ok(gross.saturating_mul(side.sign()))
}

/// Turns a stored order into the shape the engine reports.
#[must_use]
pub fn to_order(order: &BookOrder, account_id: TradeAccountId) -> Order {
    Order {
        id: order.id.clone(),
        client_order_id: order
            .client_order_id
            .as_deref()
            .and_then(|raw| senken_trade::ClientOrderId::new(raw).ok()),
        account_id,
        instrument: order.instrument.clone(),
        side: order.side,
        kind: order.kind,
        quantity: order.quantity,
        filled_quantity: order.filled,
        average_price: order.average_price,
        time_in_force: order.time_in_force,
        status: order.status,
        reduce_only: order.reduce_only,
        post_only: false,
        submitted_at: order.submitted_at,
        updated_at: order.updated_at,
        reject_reason: order.reject_reason.clone(),
    }
}
