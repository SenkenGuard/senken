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

use crate::money::{CASH_SCALE, basis_points, notional, slip, weighted_average};

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

/// What one account's terms are, read out of its settings once per call.
#[derive(Debug, Clone, Copy)]
pub struct Terms {
    /// Fee charged on every fill, in basis points of notional.
    pub fee_bps: i64,
    /// How far a market order fills from the mark, in basis points.
    pub slippage_bps: i64,
    /// Notional multiple of margin an account may hold.
    pub leverage: i64,
}

/// Applying a fill to a book, as one value so the caller cannot use half of
/// it.
struct Applied {
    fill_price: Scaled,
    fee: i64,
    realized: i64,
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
    let applied = apply_to_position(
        book,
        &order.instrument,
        order.side,
        quantity,
        price,
        terms,
        now,
    )?;

    book.cash = book
        .cash
        .saturating_add(applied.realized)
        .saturating_sub(applied.fee);

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

/// Folds one execution into the instrument's position, returning the fee
/// charged and the profit realised by whatever it closed.
fn apply_to_position(
    book: &mut Book,
    instrument: &InstrumentId,
    side: OrderSide,
    quantity: Scaled,
    price: Scaled,
    terms: Terms,
    now: UnixNanos,
) -> Result<Applied, TradeError> {
    let fee = basis_points(notional(price, quantity)?, terms.fee_bps)?;
    let opening = match side {
        OrderSide::Buy => PositionSide::Long,
        OrderSide::Sell => PositionSide::Short,
    };

    let Some(existing) = book.positions.get(instrument).cloned() else {
        book.positions.insert(
            instrument.clone(),
            BookPosition {
                side: opening,
                quantity,
                average_entry: price,
                realized: 0,
                opened_at: now,
            },
        );
        return Ok(Applied {
            fill_price: price,
            fee,
            realized: 0,
        });
    };

    if existing.side == opening {
        let merged = BookPosition {
            side: opening,
            quantity: Scaled::new(
                existing.quantity.scale,
                existing.quantity.value.saturating_add(quantity.value),
            ),
            average_entry: weighted_average(
                existing.average_entry,
                existing.quantity,
                price,
                quantity,
            )?,
            ..existing
        };
        book.positions.insert(instrument.clone(), merged);
        return Ok(Applied {
            fill_price: price,
            fee,
            realized: 0,
        });
    }

    // Opposing: close what overlaps, then open the remainder the other way.
    let closed = quantity.value.min(existing.quantity.value);
    let realized = close_pnl(
        existing.side,
        existing.average_entry,
        price,
        Scaled::new(quantity.scale, closed),
    )?;
    let remaining = quantity.value - closed;

    if remaining > 0 {
        book.positions.insert(
            instrument.clone(),
            BookPosition {
                side: opening,
                quantity: Scaled::new(quantity.scale, remaining),
                average_entry: price,
                realized: existing.realized.saturating_add(realized),
                opened_at: now,
            },
        );
    } else if existing.quantity.value == closed {
        book.positions.remove(instrument);
    } else {
        book.positions.insert(
            instrument.clone(),
            BookPosition {
                quantity: Scaled::new(existing.quantity.scale, existing.quantity.value - closed),
                realized: existing.realized.saturating_add(realized),
                ..existing
            },
        );
    }

    Ok(Applied {
        fill_price: price,
        fee,
        realized,
    })
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

/// The price a market order fills at, given the mark.
///
/// # Errors
/// [`TradeError`] when the arithmetic does not fit.
pub fn market_fill_price(
    mark: Scaled,
    side: OrderSide,
    terms: Terms,
) -> Result<Scaled, TradeError> {
    slip(mark, terms.slippage_bps, side.sign())
}

/// Whether a resting order's condition is met by `mark`.
///
/// A limit buy fills when the market comes down to it, a limit sell when it
/// comes up. A stop is the mirror image: it exists to be triggered by the
/// market moving *against* the position, so a stop buy triggers on the way
/// up.
#[must_use]
pub fn is_triggered(kind: OrderKind, side: OrderSide, mark: Scaled) -> bool {
    let Some(reference) = (match kind {
        OrderKind::Limit { price } => Some((price, true)),
        OrderKind::Stop { trigger } | OrderKind::StopLimit { trigger, .. } => {
            Some((trigger, false))
        }
        // `Market` has nothing to wait for, and `OrderKind` is
        // `#[non_exhaustive]`: a kind this adapter has not been taught
        // never fills rather than filling on a guessed condition.
        _ => None,
    }) else {
        return false;
    };
    let (level, is_limit) = reference;
    let Some(mark) = mark.rescale(level.scale) else {
        return false;
    };
    match (side, is_limit) {
        (OrderSide::Buy, true) | (OrderSide::Sell, false) => mark.value <= level.value,
        (OrderSide::Sell, true) | (OrderSide::Buy, false) => mark.value >= level.value,
    }
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
