//! Orders and the fills they produce.
//!
//! Every price and quantity here is a [`Scaled`] `(scale, value)` pair, not
//! an `f64`. A venue that reports `1e-06`, `0.00000100` and `1E-6` must land
//! on the same integer, and a fill price that has been through a float is
//! no longer the price that was traded.

use senken_core::UnixNanos;
use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use serde::{Deserialize, Serialize};

use crate::id::{ClientOrderId, OrderId, TradeAccountId};

/// Which way an order goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    /// Buy the instrument.
    Buy,
    /// Sell the instrument.
    Sell,
}

impl OrderSide {
    /// The opposite side.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }

    /// `+1` for a buy, `-1` for a sell — the multiplier that turns a price
    /// difference into a signed profit.
    #[must_use]
    pub fn sign(self) -> i64 {
        match self {
            Self::Buy => 1,
            Self::Sell => -1,
        }
    }
}

/// How an order is to be executed, carrying whatever prices that kind
/// needs.
///
/// The prices live **inside** the variants rather than beside them as
/// `Option`s, so a limit order with no limit price, or a market order
/// carrying one, cannot be constructed at all — the same instinct that
/// makes a `SourceSymbol` a distinct type from a normalised one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum OrderKind {
    /// Execute immediately at whatever the venue offers.
    Market,
    /// Rest at `price` until filled or cancelled.
    Limit {
        /// The limit price, at the instrument's own price scale.
        price: Scaled,
    },
    /// Become a market order once the mark reaches `trigger`.
    Stop {
        /// The trigger price, at the instrument's own price scale.
        trigger: Scaled,
    },
    /// Become a limit order at `price` once the mark reaches `trigger`.
    StopLimit {
        /// The trigger price.
        trigger: Scaled,
        /// The limit price the resulting order rests at.
        price: Scaled,
    },
}

impl OrderKind {
    /// The tag naming this kind, without its prices — what an adapter's
    /// [`capabilities`](crate::AdapterCapabilities) enumerates.
    #[must_use]
    pub fn tag(&self) -> OrderKindTag {
        match self {
            Self::Market => OrderKindTag::Market,
            Self::Limit { .. } => OrderKindTag::Limit,
            Self::Stop { .. } => OrderKindTag::Stop,
            Self::StopLimit { .. } => OrderKindTag::StopLimit,
        }
    }

    /// The resting price, for the kinds that have one.
    #[must_use]
    pub fn limit_price(&self) -> Option<Scaled> {
        match self {
            Self::Limit { price } | Self::StopLimit { price, .. } => Some(*price),
            Self::Market | Self::Stop { .. } => None,
        }
    }

    /// The trigger price, for the kinds that have one.
    #[must_use]
    pub fn trigger_price(&self) -> Option<Scaled> {
        match self {
            Self::Stop { trigger } | Self::StopLimit { trigger, .. } => Some(*trigger),
            Self::Market | Self::Limit { .. } => None,
        }
    }

    /// `true` when this kind waits for a trigger before it can execute.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.trigger_price().is_some()
    }
}

/// An [`OrderKind`] with its prices stripped: the unit an adapter advertises
/// support for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OrderKindTag {
    /// [`OrderKind::Market`].
    Market,
    /// [`OrderKind::Limit`].
    Limit,
    /// [`OrderKind::Stop`].
    Stop,
    /// [`OrderKind::StopLimit`].
    StopLimit,
}

/// How long an order stays alive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TimeInForce {
    /// Rest until filled or cancelled. The default: the only one every
    /// venue has.
    #[default]
    Gtc,
    /// Fill what is available immediately, cancel the rest.
    Ioc,
    /// Fill entirely and immediately, or not at all.
    Fok,
    /// Cancel at the end of the venue's trading day.
    Day,
}

/// Where an order has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OrderStatus {
    /// Sent, not yet acknowledged. The one status whose outcome is unknown.
    Pending,
    /// Live at the venue, nothing filled yet.
    Open,
    /// Live at the venue, some quantity filled.
    PartiallyFilled,
    /// Fully filled.
    Filled,
    /// Cancelled before it filled completely.
    Cancelled,
    /// Refused by the venue.
    Rejected,
    /// Expired under its own time in force.
    Expired,
}

impl OrderStatus {
    /// `true` while the order can still fill or be cancelled.
    #[must_use]
    pub fn is_open(self) -> bool {
        matches!(self, Self::Pending | Self::Open | Self::PartiallyFilled)
    }

    /// `true` once the order can never change again.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        !self.is_open()
    }
}

/// What a caller asks an adapter to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderRequest {
    /// The instrument to trade, in this platform's `source:symbol` form.
    /// An adapter maps it to whatever its own venue calls the thing.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: OrderSide,
    /// How to execute, with the prices that kind needs.
    #[serde(flatten)]
    pub kind: OrderKind,
    /// How much, in the adapter's own
    /// [`quantity_unit`](crate::AdapterCapabilities::quantity_unit).
    pub quantity: Scaled,
    /// How long the order lives.
    #[serde(default)]
    pub time_in_force: TimeInForce,
    /// Only allowed to shrink an existing position, never to open or
    /// reverse one.
    #[serde(default)]
    pub reduce_only: bool,
    /// Refuse the order rather than let it take liquidity.
    #[serde(default)]
    pub post_only: bool,
    /// A caller-chosen idempotency key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<ClientOrderId>,
    /// A stop loss to attach to the position this order opens or adds to.
    ///
    /// Every platform Senken simulates lets an order carry its stops at
    /// send time, and the reason is not convenience: a position opened
    /// without one is unprotected for however long it takes a second
    /// request to arrive, which on a fast market is the part that costs
    /// money. `None` leaves whatever the position already has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_loss: Option<Scaled>,
    /// A take profit to attach, on the same terms as
    /// [`stop_loss`](Self::stop_loss).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_profit: Option<Scaled>,
}

impl OrderRequest {
    /// A market order for `quantity` of `instrument`.
    #[must_use]
    pub fn market(instrument: InstrumentId, side: OrderSide, quantity: Scaled) -> Self {
        Self {
            instrument,
            side,
            kind: OrderKind::Market,
            quantity,
            time_in_force: TimeInForce::default(),
            reduce_only: false,
            post_only: false,
            client_order_id: None,
            stop_loss: None,
            take_profit: None,
        }
    }

    /// A limit order resting at `price`.
    #[must_use]
    pub fn limit(
        instrument: InstrumentId,
        side: OrderSide,
        quantity: Scaled,
        price: Scaled,
    ) -> Self {
        Self {
            kind: OrderKind::Limit { price },
            ..Self::market(instrument, side, quantity)
        }
    }

    /// Sets the time in force.
    #[must_use]
    pub fn with_time_in_force(mut self, tif: TimeInForce) -> Self {
        self.time_in_force = tif;
        self
    }

    /// Marks the order reduce-only.
    #[must_use]
    pub fn reduce_only(mut self) -> Self {
        self.reduce_only = true;
        self
    }

    /// Attaches an idempotency key.
    #[must_use]
    pub fn with_client_order_id(mut self, id: ClientOrderId) -> Self {
        self.client_order_id = Some(id);
        self
    }
}

/// An order as the adapter currently sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    /// The venue's own id for it.
    pub id: OrderId,
    /// The idempotency key it was sent with, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_order_id: Option<ClientOrderId>,
    /// The attached account it belongs to.
    pub account_id: TradeAccountId,
    /// The instrument.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: OrderSide,
    /// How it executes, with its prices.
    #[serde(flatten)]
    pub kind: OrderKind,
    /// The quantity originally asked for.
    pub quantity: Scaled,
    /// How much of it has filled.
    pub filled_quantity: Scaled,
    /// The volume-weighted average price of everything filled so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_price: Option<Scaled>,
    /// How long it lives.
    pub time_in_force: TimeInForce,
    /// Where it has got to.
    pub status: OrderStatus,
    /// Whether it may only shrink a position.
    pub reduce_only: bool,
    /// Whether it refuses to take liquidity.
    pub post_only: bool,
    /// When it was submitted.
    pub submitted_at: UnixNanos,
    /// When its state last changed.
    pub updated_at: UnixNanos,
    /// Why it was rejected, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reject_reason: Option<String>,
}

/// Which side of the book a fill took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Liquidity {
    /// Rested on the book and was traded against.
    Maker,
    /// Crossed the spread.
    Taker,
}

/// One execution against an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    /// The venue's own id for this execution.
    pub id: OrderId,
    /// The order it filled.
    pub order_id: OrderId,
    /// The account.
    pub account_id: TradeAccountId,
    /// The instrument.
    pub instrument: InstrumentId,
    /// Buy or sell.
    pub side: OrderSide,
    /// How much traded.
    pub quantity: Scaled,
    /// At what price.
    pub price: Scaled,
    /// The fee charged, in [`fee_currency`](Self::fee_currency).
    pub fee: Scaled,
    /// The asset the fee was charged in — not always the quote currency,
    /// which is why it travels with the amount.
    pub fee_currency: String,
    /// Which side of the book this took.
    pub liquidity: Liquidity,
    /// When the venue reported it.
    pub executed_at: UnixNanos,
}

/// What an amendment changes. Every field is optional; `None` leaves that
/// part of the order alone.
///
/// A price is amended by kind, not by field name: amending the trigger of
/// an order that has no trigger is a caller mistake, and the engine refuses
/// it rather than adding a trigger the order never had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OrderAmendment {
    /// The new size, replacing the order's current one.
    pub quantity: Option<Scaled>,
    /// The new resting price, for an order that has one.
    pub limit_price: Option<Scaled>,
    /// The new trigger price, for an order that has one.
    pub trigger_price: Option<Scaled>,
}

impl OrderAmendment {
    /// `true` when nothing would change — refused rather than sent, since a
    /// no-op amendment still costs a venue round trip and can still lose
    /// queue position.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.quantity.is_none() && self.limit_price.is_none() && self.trigger_price.is_none()
    }
}

/// Which orders a listing should return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum OrderFilter {
    /// Only orders that can still fill or be cancelled.
    #[default]
    Open,
    /// Every order the adapter still remembers, newest first.
    All,
}

#[cfg(test)]
mod tests {
    use super::{OrderAmendment, OrderKind, OrderKindTag, OrderSide, OrderStatus, TimeInForce};
    use senken_core::decimal::Scaled;

    #[test]
    fn a_side_signs_a_price_difference_the_way_profit_runs() {
        assert_eq!(OrderSide::Buy.sign(), 1);
        assert_eq!(OrderSide::Sell.sign(), -1);
        assert_eq!(OrderSide::Buy.opposite(), OrderSide::Sell);
    }

    #[test]
    fn only_the_kinds_that_rest_report_a_limit_price() {
        let price = Scaled::new(2, 10_000);
        assert_eq!(OrderKind::Market.limit_price(), None);
        assert_eq!(OrderKind::Limit { price }.limit_price(), Some(price));
        assert_eq!(OrderKind::Stop { trigger: price }.limit_price(), None);
        assert_eq!(
            OrderKind::StopLimit {
                trigger: price,
                price
            }
            .limit_price(),
            Some(price)
        );
    }

    #[test]
    fn only_the_kinds_that_wait_report_a_trigger() {
        let price = Scaled::new(2, 10_000);
        assert!(!OrderKind::Market.is_triggered());
        assert!(!OrderKind::Limit { price }.is_triggered());
        assert!(OrderKind::Stop { trigger: price }.is_triggered());
        assert!(
            OrderKind::StopLimit {
                trigger: price,
                price
            }
            .is_triggered()
        );
    }

    #[test]
    fn a_kind_tags_itself_without_its_prices() {
        assert_eq!(OrderKind::Market.tag(), OrderKindTag::Market);
        assert_eq!(
            OrderKind::Limit {
                price: Scaled::new(2, 1)
            }
            .tag(),
            OrderKindTag::Limit
        );
    }

    #[test]
    fn open_and_terminal_partition_every_status() {
        for status in [
            OrderStatus::Pending,
            OrderStatus::Open,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
            OrderStatus::Expired,
        ] {
            assert_ne!(
                status.is_open(),
                status.is_terminal(),
                "{status:?} must be exactly one of open or terminal"
            );
        }
    }

    #[test]
    fn a_market_order_serialises_its_kind_without_a_price_field() {
        // The flattened tag is what lets a client discriminate; a market
        // order carrying a `price` key at all would invite one to be read.
        let json = serde_json::to_string(&OrderKind::Market).unwrap();
        assert_eq!(json, r#"{"kind":"market"}"#);
    }

    #[test]
    fn time_in_force_defaults_to_resting_until_cancelled() {
        assert_eq!(TimeInForce::default(), TimeInForce::Gtc);
    }

    #[test]
    fn an_amendment_with_nothing_set_reports_itself_empty() {
        assert!(OrderAmendment::default().is_empty());
        assert!(
            !OrderAmendment {
                quantity: Some(Scaled::new(3, 100)),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
