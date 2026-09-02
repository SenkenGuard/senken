//! [`TradeEngine`]: the registry of adapters, and the checks every request
//! passes before it reaches one.
//!
//! Mirrors [`MarketData`](senken_marketdata::MarketData) deliberately —
//! plugins register into it at activation, ids are unique and slug-shaped,
//! and it is read through an `Arc` for the process's lifetime.
//!
//! # Why validation lives here and not in each adapter
//!
//! Tick and step rounding, coverage, order kind and time in force are the
//! same rules for every venue, and they are the rules that decide whether
//! an order is *representable*. Leaving them to each adapter means twenty
//! implementations of one rule, of which some are wrong — and the way they
//! are wrong is that an order goes to a venue slightly malformed and comes
//! back rejected, at a price that has moved.
//!
//! What stays with the adapter is everything only it can know: the
//! balance, the venue's own limits, the credentials.

use std::collections::BTreeMap;
use std::sync::Arc;

use senken_core::decimal::checked_rescale;
use senken_marketdata::{Instrument, InstrumentId};

use crate::adapter::{AccountRef, ActionOutcome, TradeAdapter, TradeContext};
use crate::capability::{AccountAccess, AdapterCapabilities, AdapterFeature};
use crate::error::TradeError;
use crate::id::OrderId;
use crate::order::{Order, OrderAmendment, OrderFilter, OrderKind, OrderRequest, OrderSide};
use crate::portfolio::PositionSide;
use crate::settings::SettingsValues;

/// Every registered [`TradeAdapter`], keyed by id.
#[derive(Debug, Default)]
pub struct TradeEngine {
    adapters: BTreeMap<String, Arc<dyn TradeAdapter>>,
}

impl TradeEngine {
    /// An engine with no adapters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `adapter`.
    ///
    /// # Errors
    /// [`TradeError::InvalidAdapterId`] when the id is not a lowercase
    /// `[a-z0-9-]` slug, [`TradeError::DuplicateAdapter`] when one is
    /// already registered under it.
    pub fn register(&mut self, adapter: Arc<dyn TradeAdapter>) -> Result<(), TradeError> {
        let id = adapter.id().to_owned();
        if !is_valid_adapter_id(&id) {
            return Err(TradeError::InvalidAdapterId(id));
        }
        if self.adapters.contains_key(&id) {
            return Err(TradeError::DuplicateAdapter(id));
        }
        self.adapters.insert(id, adapter);
        Ok(())
    }

    /// The adapter registered under `id`.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`] when none is.
    pub fn adapter(&self, id: &str) -> Result<&Arc<dyn TradeAdapter>, TradeError> {
        self.adapters
            .get(id)
            .ok_or_else(|| TradeError::UnknownAdapter(id.to_owned()))
    }

    /// Every registered adapter, in id order.
    pub fn adapters(&self) -> impl Iterator<Item = &Arc<dyn TradeAdapter>> {
        self.adapters.values()
    }

    /// How many adapters are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    /// `true` when nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// Validates a request against the account's own access and the
    /// instrument, then sends it.
    ///
    /// The order that reaches the adapter has had its prices rounded to the
    /// instrument's tick and its quantity to the instrument's step — as
    /// scaled integers, never through a float. An indicator-derived price
    /// arriving here as `68420.1379` becomes `68420.13` before anything
    /// trades on it, which is the boundary this project draws between a
    /// display value and a money value.
    ///
    /// Validated against the **account's** resolved capabilities, not the
    /// adapter's own — an order kind the adapter supports in general but
    /// this particular account may not (a read-only login narrows to
    /// nothing; a real venue could narrow to less) is refused here rather
    /// than reaching the adapter and being taken at its word.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::ReadOnly`] when the
    /// account's access is not [`AccessLevel::Trade`](crate::AccessLevel::Trade),
    /// [`TradeError::InstrumentNotTradable`], [`TradeError::Unsupported`]
    /// for an order kind or time in force the account does not accept,
    /// [`TradeError::UnknownInstrument`] when the instrument is not
    /// catalogued, [`TradeError::InvalidRequest`] for a non-positive
    /// quantity or a price that cannot be expressed at the instrument's
    /// scale, and whatever the adapter itself returns.
    pub async fn place_order(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        let access = self.require_trading_access(adapter, ctx, account).await?;
        let request = self
            .prepare_order(adapter, &access.capabilities, ctx, request)
            .await?;
        adapter.place_order(ctx, account, request).await
    }

    /// Cancels a resting order, after the same read-only check every
    /// mutation goes through.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::ReadOnly`], or
    /// whatever the adapter itself returns.
    pub async fn cancel_order(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
    ) -> Result<Order, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        self.require_trading_access(adapter, ctx, account).await?;
        adapter.cancel_order(ctx, account, order_id).await
    }

    /// Amends a resting order's price or size in place.
    ///
    /// Refused, in order, before the adapter is ever asked: a read-only
    /// account (the same check every mutation goes through); an account
    /// whose resolved capabilities do not include
    /// [`AdapterFeature::ModifyOrders`]; an amendment that changes nothing;
    /// and an amendment naming a price the order's own kind does not carry
    /// (a limit price on a market order, a trigger on a plain limit). Once
    /// past those, every price and the quantity are rounded to the
    /// instrument's tick and step exactly as [`place_order`](Self::place_order)
    /// rounds a new order.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::ReadOnly`],
    /// [`TradeError::Unsupported`] when the account cannot amend orders at
    /// all, [`TradeError::InvalidRequest`] for an empty amendment or one
    /// naming a price the order does not have, [`TradeError::UnknownOrder`]
    /// when no open order on this account matches `order_id`, or whatever
    /// the adapter itself returns.
    pub async fn modify_order(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        order_id: &OrderId,
        amendment: OrderAmendment,
    ) -> Result<Order, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        let access = self.require_trading_access(adapter, ctx, account).await?;
        if !access.capabilities.has(AdapterFeature::ModifyOrders) {
            return Err(TradeError::unsupported(adapter.id(), "amending orders"));
        }
        if amendment.is_empty() {
            return Err(TradeError::invalid(
                "an amendment must change at least one of quantity, limit price or trigger price",
            ));
        }

        let order = adapter
            .orders(ctx, account, OrderFilter::Open)
            .await?
            .into_iter()
            .find(|order| order.id == *order_id)
            .ok_or(TradeError::UnknownOrder)?;
        if amendment.limit_price.is_some() && order.kind.limit_price().is_none() {
            return Err(TradeError::invalid(
                "this order has no limit price to amend",
            ));
        }
        if amendment.trigger_price.is_some() && order.kind.trigger_price().is_none() {
            return Err(TradeError::invalid(
                "this order has no trigger price to amend",
            ));
        }

        let instrument = ctx.instrument(&order.instrument).await?;
        let amendment = OrderAmendment {
            quantity: amendment
                .quantity
                .map(|quantity| {
                    round_to_increment(
                        quantity,
                        instrument.qty_scale,
                        instrument.step_size,
                        "quantity",
                    )
                })
                .transpose()?,
            limit_price: amendment
                .limit_price
                .map(|price| {
                    round_to_increment(price, instrument.price_scale, instrument.tick_size, "price")
                })
                .transpose()?,
            trigger_price: amendment
                .trigger_price
                .map(|price| {
                    round_to_increment(price, instrument.price_scale, instrument.tick_size, "price")
                })
                .transpose()?,
        };
        adapter
            .modify_order(ctx, account, order_id, amendment)
            .await
    }

    /// Closes an open position by sending an opposite market order for
    /// exactly the size held.
    ///
    /// The size comes from the adapter's own current answer to
    /// [`positions`](crate::TradeAdapter::positions), never from a number a
    /// screen was holding: a position that moved between the table being
    /// drawn and the button being pressed closes at what it is now, not at
    /// what it was — this project's most expensive bug class, in miniature.
    ///
    /// `reduce_only` is set on the closing order only when the account's own
    /// resolved capabilities include [`AdapterFeature::ReduceOnly`]; a
    /// spot-holdings adapter has no such flag and must not be sent one. The
    /// request otherwise goes through the same validation
    /// [`place_order`](Self::place_order) does, so the size is rounded to
    /// the instrument's step exactly as any other order.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::ReadOnly`],
    /// [`TradeError::UnknownPosition`] when the account holds nothing on
    /// `instrument`, or whatever [`place_order`](Self::place_order)'s own
    /// validation and the adapter itself return.
    pub async fn close_position(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        instrument: &InstrumentId,
    ) -> Result<Order, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        let access = self.require_trading_access(adapter, ctx, account).await?;

        let position = adapter
            .positions(ctx, account)
            .await?
            .into_iter()
            .find(|position| &position.instrument == instrument)
            .ok_or_else(|| TradeError::UnknownPosition(instrument.clone()))?;

        let mut request = OrderRequest::market(
            instrument.clone(),
            closing_side(position.side),
            position.quantity,
        );
        if access.capabilities.has(AdapterFeature::ReduceOnly) {
            request = request.reduce_only();
        }
        let request = self
            .prepare_order(adapter, &access.capabilities, ctx, request)
            .await?;
        adapter.place_order(ctx, account, request).await
    }

    /// Runs one of the adapter's own actions, after the same read-only
    /// check every mutation goes through.
    ///
    /// An adapter action is by definition something that changes state —
    /// the simulator's own "deposit funds" moves money — so there is no
    /// action a read-only account may run; letting one through on a guess
    /// would be the wrong default for a screen that can move money.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::ReadOnly`], or
    /// whatever the adapter itself returns.
    pub async fn run_action(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        action_id: &str,
        params: SettingsValues,
    ) -> Result<ActionOutcome, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        self.require_trading_access(adapter, ctx, account).await?;
        adapter.run_action(ctx, account, action_id, params).await
    }

    /// Resolves `account`'s access on `adapter` and refuses anything short
    /// of full trading rights.
    ///
    /// The one place every mutation — [`place_order`](Self::place_order),
    /// [`cancel_order`](Self::cancel_order), [`run_action`](Self::run_action),
    /// [`modify_order`](Self::modify_order) — passes through before the
    /// adapter is called at all.
    ///
    /// # Errors
    /// [`TradeError::ReadOnly`] when the account's resolved level is not
    /// [`AccessLevel::Trade`](crate::AccessLevel::Trade) — including a level
    /// this build does not recognise, which refuses by the same rule rather
    /// than being treated as tradable — or whatever
    /// [`TradeAdapter::account_access`] itself fails with.
    async fn require_trading_access(
        &self,
        adapter: &Arc<dyn TradeAdapter>,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
    ) -> Result<AccountAccess, TradeError> {
        let access = adapter.account_access(ctx, account).await?;
        if !access.is_trading() {
            return Err(TradeError::ReadOnly {
                adapter: adapter.id().to_owned(),
                note: access.note,
            });
        }
        Ok(access)
    }

    /// The validation half of [`place_order`](Self::place_order), split out
    /// so it can be exercised on its own.
    async fn prepare_order(
        &self,
        adapter: &Arc<dyn TradeAdapter>,
        capabilities: &AdapterCapabilities,
        ctx: &TradeContext<'_>,
        mut request: OrderRequest,
    ) -> Result<OrderRequest, TradeError> {
        if !adapter.coverage().covers(&request.instrument) {
            return Err(TradeError::InstrumentNotTradable {
                adapter: adapter.id().to_owned(),
                instrument: request.instrument.clone(),
            });
        }

        if !capabilities.accepts_kind(request.kind.tag()) {
            return Err(TradeError::unsupported(
                adapter.id(),
                format!("{:?} orders", request.kind.tag()),
            ));
        }
        if !capabilities.accepts_time_in_force(request.time_in_force) {
            return Err(TradeError::unsupported(
                adapter.id(),
                format!("{:?} orders", request.time_in_force),
            ));
        }
        if request.reduce_only && !capabilities.has(AdapterFeature::ReduceOnly) {
            return Err(TradeError::unsupported(adapter.id(), "reduce-only orders"));
        }
        if request.post_only && !capabilities.has(AdapterFeature::PostOnly) {
            return Err(TradeError::unsupported(adapter.id(), "post-only orders"));
        }

        let instrument = ctx.instrument(&request.instrument).await?;
        request.quantity = round_to_increment(
            request.quantity,
            instrument.qty_scale,
            instrument.step_size,
            "quantity",
        )?;
        if request.quantity.value <= 0 {
            return Err(TradeError::invalid(
                "quantity must be greater than zero once rounded to the instrument's step size",
            ));
        }
        request.kind = round_kind_prices(request.kind, &instrument)?;
        Ok(request)
    }
}

/// The order side that closes a position held on `side` — a long is closed
/// by selling, a short by buying.
fn closing_side(side: PositionSide) -> OrderSide {
    match side {
        PositionSide::Long => OrderSide::Sell,
        PositionSide::Short => OrderSide::Buy,
    }
}

/// Rounds every price a kind carries down onto the instrument's tick.
fn round_kind_prices(kind: OrderKind, instrument: &Instrument) -> Result<OrderKind, TradeError> {
    let tick =
        |price| round_to_increment(price, instrument.price_scale, instrument.tick_size, "price");
    Ok(match kind {
        OrderKind::Market => OrderKind::Market,
        OrderKind::Limit { price } => OrderKind::Limit {
            price: tick(price)?,
        },
        OrderKind::Stop { trigger } => OrderKind::Stop {
            trigger: tick(trigger)?,
        },
        OrderKind::StopLimit { trigger, price } => OrderKind::StopLimit {
            trigger: tick(trigger)?,
            price: tick(price)?,
        },
    })
}

/// Re-expresses `value` at `scale` and rounds it down to a whole multiple
/// of `increment`.
///
/// Rounding **down** rather than to nearest, on purpose: the result of
/// rounding a size up is an order slightly larger than the one the user
/// asked for, and the result of rounding a limit price up on a buy is
/// paying more than the price shown. Down is the direction that cannot
/// surprise anyone in the expensive direction.
///
/// A value that rounds away to nothing is not caught here — the caller
/// checks for a non-positive quantity afterwards, because a *price* of zero
/// after rounding is a different problem from a *size* of zero and they do
/// not share an error message.
fn round_to_increment(
    value: senken_core::decimal::Scaled,
    scale: u8,
    increment: i64,
    what: &str,
) -> Result<senken_core::decimal::Scaled, TradeError> {
    let at_scale = if scale >= value.scale {
        // Widening cannot lose a digit, only overflow.
        checked_rescale(value.value, value.scale, scale).ok_or_else(|| {
            TradeError::invalid(format!(
                "{what} {} is too large to express at this instrument's {scale} decimal places",
                senken_core::decimal::format_scaled(value.value, value.scale)
            ))
        })?
    } else {
        // Narrowing is the whole point of this function: the caller
        // supplied more precision than the instrument has, and the extra
        // digits are dropped downward. `checked_rescale` deliberately
        // refuses this — it exists to stop a silent truncation somewhere
        // that did not mean one — so the flooring is written out here,
        // where it is the intended behaviour.
        let factor = 10_i64
            .checked_pow(u32::from(value.scale - scale))
            .ok_or_else(|| TradeError::invalid(format!("{what} has an unusable scale")))?;
        value.value.div_euclid(factor)
    };
    if increment <= 0 {
        // A venue that reports no usable increment: nothing to round to, so
        // the value passes through rather than being divided by zero.
        return Ok(senken_core::decimal::Scaled::new(scale, at_scale));
    }
    Ok(senken_core::decimal::Scaled::new(
        scale,
        at_scale - at_scale.rem_euclid(increment),
    ))
}

/// Whether an adapter id is a lowercase `[a-z0-9-]` slug — the same shape
/// [`InstrumentId`](senken_marketdata::InstrumentId) requires of a source
/// id, so the two families of ids stay spellable in the same places.
fn is_valid_adapter_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Rescales `value` from `from` to `to`, saturating rather than failing.
///
/// Used where a value has already been validated and only its presentation
/// scale differs. Exposed because adapters need it as often as this crate
/// does.
///
/// # Errors
/// [`TradeError::InvalidRequest`] when the conversion would lose non-zero
/// digits or overflow.
pub fn rescale_exact(value: i64, from: u8, to: u8) -> Result<i64, TradeError> {
    checked_rescale(value, from, to).ok_or_else(|| {
        TradeError::invalid(format!(
            "{} cannot be expressed at {to} decimal places",
            senken_core::decimal::format_scaled(value, from)
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use senken_core::UnixNanos;
    use senken_core::decimal::Scaled;
    use senken_marketdata::{Instrument, InstrumentId};

    use super::TradeEngine;
    use crate::adapter::ActionOutcome;
    use crate::adapter::{
        AccountRef, InstrumentSource, MarkPrice, MarkPriceSource, TradeAdapter, TradeContext,
    };
    use crate::capability::{
        AccountAccess, AdapterCapabilities, AdapterFeature, AdapterKind, InstrumentCoverage,
        PositionMode, QuantityUnit,
    };
    use crate::error::TradeError;
    use crate::id::{OrderId, TradeAccountId};
    use crate::order::{
        Order, OrderAmendment, OrderFilter, OrderKind, OrderKindTag, OrderRequest, OrderSide,
        OrderStatus, TimeInForce,
    };
    use crate::portfolio::{AccountBalances, AdapterHealth, Position, PositionSide};
    use crate::settings::{SettingsSchema, SettingsValues};

    fn id(raw: &str) -> InstrumentId {
        InstrumentId::parse(raw).unwrap()
    }

    /// A catalog holding one instrument with a `0.01` tick and a `0.001`
    /// step — the shape of a real spot pair.
    struct OneInstrument;

    #[async_trait]
    impl InstrumentSource for OneInstrument {
        async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError> {
            if id.symbol() != "BTCUSDT" {
                return Ok(None);
            }
            Ok(Some(
                Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT")
                    .with_price_increment((2, 1))
                    .with_qty_increment((3, 1)),
            ))
        }
    }

    struct FixedPrice;

    #[async_trait]
    impl MarkPriceSource for FixedPrice {
        async fn mark_price(
            &self,
            _instrument: &InstrumentId,
        ) -> Result<Option<MarkPrice>, TradeError> {
            Ok(Some(MarkPrice {
                price: Scaled::new(2, 6_842_000),
                as_of: UnixNanos::from_secs(1_700_000_000).unwrap(),
            }))
        }
    }

    /// Records the request it was handed, so a test can assert on what the
    /// engine passed through rather than on what it was given.
    struct Recording {
        id: &'static str,
        capabilities: AdapterCapabilities,
        coverage: InstrumentCoverage,
        /// What [`TradeAdapter::account_access`] answers. `None` falls back
        /// to full trading access at `capabilities` — the shape every test
        /// but the access-level ones wants, and the same fallback the trait
        /// default gives a real adapter that never overrides the method.
        access_override: Option<AccountAccess>,
        seen: std::sync::Mutex<Option<OrderRequest>>,
        cancel_called: std::sync::Mutex<bool>,
        action_called: std::sync::Mutex<bool>,
        /// What [`TradeAdapter::positions`] answers — a test sets this to
        /// put a position in front of `close_position`, and can mutate it
        /// again between setup and the call under test to simulate a fill
        /// landing in between.
        positions: std::sync::Mutex<Vec<Position>>,
        /// What [`TradeAdapter::orders`] answers, for `modify_order` to find
        /// its target in.
        orders: std::sync::Mutex<Vec<Order>>,
        /// `true` once [`TradeAdapter::orders`] has been called — lets a
        /// test prove the adapter was never reached at all, not merely that
        /// its answer was ignored.
        orders_called: std::sync::Mutex<bool>,
        /// The `(order_id, amendment)` pair, rounded, that reached
        /// [`TradeAdapter::modify_order`].
        modify_seen: std::sync::Mutex<Option<(OrderId, OrderAmendment)>>,
    }

    impl Recording {
        fn new() -> Self {
            Self {
                id: "recorder",
                capabilities: AdapterCapabilities::market_only()
                    .with_order_kinds(vec![OrderKindTag::Market, OrderKindTag::Limit])
                    .with_quantity_unit(QuantityUnit::Base)
                    .with_position_mode(PositionMode::SpotHoldings),
                coverage: InstrumentCoverage::Universal,
                access_override: None,
                seen: std::sync::Mutex::new(None),
                cancel_called: std::sync::Mutex::new(false),
                action_called: std::sync::Mutex::new(false),
                positions: std::sync::Mutex::new(Vec::new()),
                orders: std::sync::Mutex::new(Vec::new()),
                orders_called: std::sync::Mutex::new(false),
                modify_seen: std::sync::Mutex::new(None),
            }
        }

        fn taken(&self) -> OrderRequest {
            self.seen
                .lock()
                .unwrap()
                .clone()
                .expect("no order reached the adapter")
        }

        fn set_positions(&self, positions: Vec<Position>) {
            *self.positions.lock().unwrap() = positions;
        }

        fn set_orders(&self, orders: Vec<Order>) {
            *self.orders.lock().unwrap() = orders;
        }

        fn modified(&self) -> (OrderId, OrderAmendment) {
            self.modify_seen
                .lock()
                .unwrap()
                .clone()
                .expect("no amendment reached the adapter")
        }
    }

    #[async_trait]
    impl TradeAdapter for Recording {
        fn id(&self) -> &str {
            self.id
        }
        fn name(&self) -> &'static str {
            "Recorder"
        }
        fn kind(&self) -> AdapterKind {
            AdapterKind::Simulation
        }
        fn capabilities(&self) -> AdapterCapabilities {
            self.capabilities.clone()
        }
        fn coverage(&self) -> InstrumentCoverage {
            self.coverage.clone()
        }
        fn settings_schema(&self) -> SettingsSchema {
            SettingsSchema::default()
        }
        async fn open_account(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
        ) -> Result<(), TradeError> {
            Ok(())
        }
        async fn account_access(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
        ) -> Result<AccountAccess, TradeError> {
            Ok(self
                .access_override
                .clone()
                .unwrap_or_else(|| AccountAccess::trading(self.capabilities.clone())))
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
            _ctx: &TradeContext<'_>,
            account: AccountRef<'_>,
        ) -> Result<AccountBalances, TradeError> {
            Ok(AccountBalances {
                account_id: account.id,
                currency: "USDT".to_owned(),
                balance: Scaled::new(2, 0),
                equity: Scaled::new(2, 0),
                unrealized_pnl: Scaled::new(2, 0),
                margin_used: None,
                margin_available: None,
                assets: Vec::new(),
            })
        }
        async fn positions(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
        ) -> Result<Vec<Position>, TradeError> {
            Ok(self.positions.lock().unwrap().clone())
        }
        async fn orders(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
            _filter: OrderFilter,
        ) -> Result<Vec<Order>, TradeError> {
            *self.orders_called.lock().unwrap() = true;
            Ok(self.orders.lock().unwrap().clone())
        }
        async fn place_order(
            &self,
            ctx: &TradeContext<'_>,
            account: AccountRef<'_>,
            request: OrderRequest,
        ) -> Result<Order, TradeError> {
            *self.seen.lock().unwrap() = Some(request.clone());
            Ok(Order {
                id: OrderId::new("1"),
                client_order_id: None,
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
                post_only: request.post_only,
                submitted_at: ctx.now(),
                updated_at: ctx.now(),
                reject_reason: None,
            })
        }
        async fn cancel_order(
            &self,
            ctx: &TradeContext<'_>,
            account: AccountRef<'_>,
            order_id: &OrderId,
        ) -> Result<Order, TradeError> {
            *self.cancel_called.lock().unwrap() = true;
            Ok(Order {
                id: order_id.clone(),
                client_order_id: None,
                account_id: account.id,
                instrument: id("okx-spot:BTCUSDT"),
                side: OrderSide::Buy,
                kind: OrderKind::Market,
                quantity: Scaled::new(3, 250),
                filled_quantity: Scaled::new(3, 0),
                average_price: None,
                time_in_force: TimeInForce::Gtc,
                status: OrderStatus::Cancelled,
                reduce_only: false,
                post_only: false,
                submitted_at: ctx.now(),
                updated_at: ctx.now(),
                reject_reason: None,
            })
        }
        async fn run_action(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
            _action_id: &str,
            _params: SettingsValues,
        ) -> Result<ActionOutcome, TradeError> {
            *self.action_called.lock().unwrap() = true;
            Ok(ActionOutcome::new("done"))
        }
        async fn modify_order(
            &self,
            ctx: &TradeContext<'_>,
            account: AccountRef<'_>,
            order_id: &OrderId,
            amendment: OrderAmendment,
        ) -> Result<Order, TradeError> {
            *self.modify_seen.lock().unwrap() = Some((order_id.clone(), amendment));
            let existing = self
                .orders
                .lock()
                .unwrap()
                .iter()
                .find(|order| order.id == *order_id)
                .cloned()
                .expect("test set up an order to amend");
            Ok(Order {
                quantity: amendment.quantity.unwrap_or(existing.quantity),
                kind: match existing.kind {
                    OrderKind::Limit { .. } => OrderKind::Limit {
                        price: amendment
                            .limit_price
                            .unwrap_or_else(|| existing.kind.limit_price().unwrap()),
                    },
                    other => other,
                },
                updated_at: ctx.now(),
                account_id: account.id,
                ..existing
            })
        }
    }

    fn engine_with(adapter: Arc<Recording>) -> TradeEngine {
        let mut engine = TradeEngine::new();
        engine.register(adapter).unwrap();
        engine
    }

    async fn place(engine: &TradeEngine, request: OrderRequest) -> Result<Order, TradeError> {
        let marks = FixedPrice;
        let instruments = OneInstrument;
        let ctx = TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &marks,
            &instruments,
        );
        let settings = SettingsValues::new();
        let account = AccountRef {
            id: TradeAccountId::new(),
            label: "Main",
            settings: &settings,
        };
        engine.place_order("recorder", &ctx, account, request).await
    }

    async fn cancel(engine: &TradeEngine) -> Result<Order, TradeError> {
        let marks = FixedPrice;
        let instruments = OneInstrument;
        let ctx = TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &marks,
            &instruments,
        );
        let settings = SettingsValues::new();
        let account = AccountRef {
            id: TradeAccountId::new(),
            label: "Main",
            settings: &settings,
        };
        engine
            .cancel_order("recorder", &ctx, account, &OrderId::new("1"))
            .await
    }

    async fn run(engine: &TradeEngine) -> Result<ActionOutcome, TradeError> {
        let marks = FixedPrice;
        let instruments = OneInstrument;
        let ctx = TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &marks,
            &instruments,
        );
        let settings = SettingsValues::new();
        let account = AccountRef {
            id: TradeAccountId::new(),
            label: "Main",
            settings: &settings,
        };
        engine
            .run_action("recorder", &ctx, account, "deposit", SettingsValues::new())
            .await
    }

    async fn close(engine: &TradeEngine, instrument: &InstrumentId) -> Result<Order, TradeError> {
        let marks = FixedPrice;
        let instruments = OneInstrument;
        let ctx = TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &marks,
            &instruments,
        );
        let settings = SettingsValues::new();
        let account = AccountRef {
            id: TradeAccountId::new(),
            label: "Main",
            settings: &settings,
        };
        engine
            .close_position("recorder", &ctx, account, instrument)
            .await
    }

    async fn modify(
        engine: &TradeEngine,
        order_id: &OrderId,
        amendment: OrderAmendment,
    ) -> Result<Order, TradeError> {
        let marks = FixedPrice;
        let instruments = OneInstrument;
        let ctx = TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &marks,
            &instruments,
        );
        let settings = SettingsValues::new();
        let account = AccountRef {
            id: TradeAccountId::new(),
            label: "Main",
            settings: &settings,
        };
        engine
            .modify_order("recorder", &ctx, account, order_id, amendment)
            .await
    }

    /// An open long position of `quantity` on `okx-spot:BTCUSDT`, the shape
    /// `close_position` reads.
    fn open_position(quantity: Scaled) -> Position {
        Position {
            account_id: TradeAccountId::new(),
            instrument: id("okx-spot:BTCUSDT"),
            side: PositionSide::Long,
            quantity,
            average_entry: Scaled::new(2, 6_800_000),
            mark_price: None,
            unrealized_pnl: None,
            realized_pnl: Scaled::new(2, 0),
            margin: None,
            leverage: None,
            opened_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
        }
    }

    /// A resting limit order for `okx-spot:BTCUSDT`, the shape `modify_order`
    /// reads to check what kind of prices it may amend.
    fn resting_limit_order(price: Scaled) -> Order {
        Order {
            id: OrderId::new("amend-me"),
            client_order_id: None,
            account_id: TradeAccountId::new(),
            instrument: id("okx-spot:BTCUSDT"),
            side: OrderSide::Buy,
            kind: OrderKind::Limit { price },
            quantity: Scaled::new(3, 250),
            filled_quantity: Scaled::new(3, 0),
            average_price: None,
            time_in_force: TimeInForce::Gtc,
            status: OrderStatus::Open,
            reduce_only: false,
            post_only: false,
            submitted_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
            updated_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
            reject_reason: None,
        }
    }

    #[test]
    fn an_adapter_id_that_is_not_a_slug_is_refused() {
        let mut engine = TradeEngine::new();
        // Only the id matters here, so the rest is the recorder's.
        let adapter = Arc::new(Recording {
            id: "Binance Spot",
            ..Recording::new()
        });
        assert!(matches!(
            engine.register(adapter).unwrap_err(),
            TradeError::InvalidAdapterId(_)
        ));
    }

    #[test]
    fn registering_the_same_id_twice_is_refused() {
        let mut engine = TradeEngine::new();
        engine.register(Arc::new(Recording::new())).unwrap();
        assert!(matches!(
            engine.register(Arc::new(Recording::new())).unwrap_err(),
            TradeError::DuplicateAdapter(_)
        ));
    }

    #[tokio::test]
    async fn a_limit_price_finer_than_the_tick_is_rounded_down_before_it_trades() {
        // The boundary this project draws: an indicator may produce
        // 68420.1379, but what reaches a venue is a scaled integer on the
        // tick.
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        place(
            &engine,
            OrderRequest::limit(
                id("okx-spot:BTCUSDT"),
                OrderSide::Buy,
                Scaled::new(3, 250),
                Scaled::new(4, 684_201_379),
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            adapter.taken().kind,
            OrderKind::Limit {
                price: Scaled::new(2, 6_842_013)
            },
            "the price must arrive on the tick, rounded down"
        );
    }

    #[tokio::test]
    async fn a_quantity_finer_than_the_step_is_rounded_down_before_it_trades() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        place(
            &engine,
            OrderRequest::market(
                id("okx-spot:BTCUSDT"),
                OrderSide::Buy,
                Scaled::new(6, 250_999),
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            adapter.taken().quantity,
            Scaled::new(3, 250),
            "0.250999 must become 0.250, never 0.251"
        );
    }

    #[tokio::test]
    async fn a_quantity_that_rounds_away_to_nothing_is_refused_rather_than_sent_as_zero() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(id("okx-spot:BTCUSDT"), OrderSide::Buy, Scaled::new(6, 400)),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::InvalidRequest(_)),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_order_kind_the_adapter_does_not_accept_never_reaches_it() {
        let adapter = Arc::new(Recording {
            capabilities: AdapterCapabilities::market_only(),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::limit(
                id("okx-spot:BTCUSDT"),
                OrderSide::Buy,
                Scaled::new(3, 250),
                Scaled::new(2, 6_800_000),
            ),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::Unsupported { .. }),
            "got {error:?}"
        );
        assert!(
            adapter.seen.lock().unwrap().is_none(),
            "the adapter must not be called at all for a request it cannot serve"
        );
    }

    #[tokio::test]
    async fn a_time_in_force_the_adapter_does_not_accept_never_reaches_it() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(id("okx-spot:BTCUSDT"), OrderSide::Buy, Scaled::new(3, 250))
                .with_time_in_force(TimeInForce::Fok),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::Unsupported { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_instrument_outside_the_adapters_coverage_never_reaches_it() {
        let adapter = Arc::new(Recording {
            coverage: InstrumentCoverage::sources(["binance-spot"]),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(id("okx-spot:BTCUSDT"), OrderSide::Buy, Scaled::new(3, 250)),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::InstrumentNotTradable { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_uncatalogued_instrument_is_refused_rather_than_traded_without_a_tick_size() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(
                id("okx-spot:NOTLISTED"),
                OrderSide::Buy,
                Scaled::new(3, 250),
            ),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::UnknownInstrument(_)),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_reduce_only_order_to_an_adapter_without_positions_is_refused() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(id("okx-spot:BTCUSDT"), OrderSide::Buy, Scaled::new(3, 250))
                .reduce_only(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::Unsupported { .. }),
            "got {error:?}"
        );
    }

    #[tokio::test]
    async fn an_order_to_a_read_only_account_is_refused_before_the_adapter_is_called() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::read_only(
                AdapterCapabilities::market_only(),
                Some("This account was attached read-only.".to_owned()),
            )),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::market(id("okx-spot:BTCUSDT"), OrderSide::Buy, Scaled::new(3, 250)),
        )
        .await
        .unwrap_err();

        match error {
            TradeError::ReadOnly { note, .. } => {
                assert_eq!(
                    note.as_deref(),
                    Some("This account was attached read-only.")
                );
            }
            other => panic!("got {other:?}"),
        }
        assert!(
            adapter.seen.lock().unwrap().is_none(),
            "a read-only account's order must never reach the adapter"
        );
    }

    #[tokio::test]
    async fn an_order_kind_the_account_lacks_is_refused_even_when_the_adapter_has_it() {
        // The adapter itself accepts limit orders (`Recording::new`'s own
        // capabilities); this account's *resolved* access narrows to market
        // only, the way a venue that allows fewer order kinds on one
        // account type would. The narrowing must actually be what the
        // engine validates against.
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::trading(
                AdapterCapabilities::market_only().with_order_kinds(vec![OrderKindTag::Market]),
            )),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        let error = place(
            &engine,
            OrderRequest::limit(
                id("okx-spot:BTCUSDT"),
                OrderSide::Buy,
                Scaled::new(3, 250),
                Scaled::new(2, 6_800_000),
            ),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::Unsupported { .. }),
            "got {error:?}"
        );
        assert!(
            adapter.seen.lock().unwrap().is_none(),
            "the account's own narrower capabilities must be what is checked"
        );
    }

    #[tokio::test]
    async fn cancelling_and_running_an_action_on_a_read_only_account_are_both_refused() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::read_only(
                AdapterCapabilities::market_only(),
                None,
            )),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        assert!(
            matches!(
                cancel(&engine).await.unwrap_err(),
                TradeError::ReadOnly { .. }
            ),
            "a resting order must not be cancellable on a read-only account through the engine"
        );
        assert!(
            !*adapter.cancel_called.lock().unwrap(),
            "the adapter must not be asked to cancel at all"
        );

        assert!(
            matches!(run(&engine).await.unwrap_err(), TradeError::ReadOnly { .. }),
            "an adapter action must not run on a read-only account through the engine"
        );
        assert!(
            !*adapter.action_called.lock().unwrap(),
            "the adapter must not be asked to run the action at all"
        );
    }

    #[tokio::test]
    async fn a_trading_account_may_still_cancel_and_run_an_action() {
        // The counterpart to the read-only test above: the same two calls
        // must actually reach the adapter for an ordinary trading account,
        // proving the read-only check refuses only what it should.
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        cancel(&engine).await.unwrap();
        assert!(*adapter.cancel_called.lock().unwrap());

        run(&engine).await.unwrap();
        assert!(*adapter.action_called.lock().unwrap());
    }

    #[tokio::test]
    async fn closing_a_position_sends_an_opposite_market_order_for_its_current_size() {
        let adapter = Arc::new(Recording::new());
        adapter.set_positions(vec![open_position(Scaled::new(3, 100))]);
        let engine = engine_with(Arc::clone(&adapter));

        // The size the adapter reports changes between the table being
        // drawn and the button being pressed — a fill landed in between.
        // `close_position` must read it fresh at call time, never from a
        // value captured earlier (there is no such parameter to capture:
        // the size is not something a caller can pass in at all).
        adapter.set_positions(vec![open_position(Scaled::new(3, 500))]);

        let order = close(&engine, &id("okx-spot:BTCUSDT")).await.unwrap();

        assert_eq!(order.side, OrderSide::Sell, "closing a long sells");
        assert_eq!(order.kind, OrderKind::Market);
        assert_eq!(
            adapter.taken().quantity,
            Scaled::new(3, 500),
            "the close must use the size the adapter reports now, not a stale one"
        );
    }

    #[tokio::test]
    async fn closing_a_position_that_is_not_open_is_refused() {
        let adapter = Arc::new(Recording::new());
        let engine = engine_with(Arc::clone(&adapter));

        let error = close(&engine, &id("okx-spot:BTCUSDT")).await.unwrap_err();

        assert!(
            matches!(error, TradeError::UnknownPosition(_)),
            "got {error:?}"
        );
        assert!(
            adapter.seen.lock().unwrap().is_none(),
            "nothing to close must never reach the adapter as an order"
        );
    }

    #[tokio::test]
    async fn closing_sets_reduce_only_only_when_the_accounts_capabilities_have_it() {
        let adapter = Arc::new(Recording::new());
        adapter.set_positions(vec![open_position(Scaled::new(3, 250))]);
        let engine = engine_with(Arc::clone(&adapter));

        close(&engine, &id("okx-spot:BTCUSDT")).await.unwrap();
        assert!(
            !adapter.taken().reduce_only,
            "the recorder's own default capabilities have no reduce-only feature"
        );

        let margined = Arc::new(Recording {
            access_override: Some(AccountAccess::trading(
                AdapterCapabilities::market_only().with_margin(),
            )),
            ..Recording::new()
        });
        margined.set_positions(vec![open_position(Scaled::new(3, 250))]);
        let engine = engine_with(Arc::clone(&margined));

        close(&engine, &id("okx-spot:BTCUSDT")).await.unwrap();
        assert!(
            margined.taken().reduce_only,
            "an account whose resolved capabilities include reduce-only must have it set on \
             the closing order"
        );
    }

    #[tokio::test]
    async fn closing_on_a_read_only_account_is_refused_before_the_adapter_is_called() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::read_only(
                AdapterCapabilities::market_only(),
                None,
            )),
            ..Recording::new()
        });
        adapter.set_positions(vec![open_position(Scaled::new(3, 250))]);
        let engine = engine_with(Arc::clone(&adapter));

        assert!(matches!(
            close(&engine, &id("okx-spot:BTCUSDT")).await.unwrap_err(),
            TradeError::ReadOnly { .. }
        ));
        assert!(adapter.seen.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn amending_with_nothing_set_is_refused() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::trading(
                AdapterCapabilities::market_only().with_feature(AdapterFeature::ModifyOrders),
            )),
            ..Recording::new()
        });
        let engine = engine_with(Arc::clone(&adapter));

        let error = modify(
            &engine,
            &OrderId::new("amend-me"),
            OrderAmendment::default(),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::InvalidRequest(_)),
            "got {error:?}"
        );
        assert!(
            !*adapter.orders_called.lock().unwrap(),
            "an empty amendment must be refused before the adapter's own orders are even read"
        );
    }

    #[tokio::test]
    async fn amending_a_limit_price_on_a_market_order_is_refused() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::trading(
                AdapterCapabilities::market_only().with_feature(AdapterFeature::ModifyOrders),
            )),
            ..Recording::new()
        });
        adapter.set_orders(vec![Order {
            kind: OrderKind::Market,
            ..resting_limit_order(Scaled::new(2, 6_800_000))
        }]);
        let engine = engine_with(Arc::clone(&adapter));

        let error = modify(
            &engine,
            &OrderId::new("amend-me"),
            OrderAmendment {
                limit_price: Some(Scaled::new(2, 6_900_000)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::InvalidRequest(_)),
            "got {error:?}"
        );
        assert!(
            adapter.modify_seen.lock().unwrap().is_none(),
            "a market order has no limit price to amend, and the adapter must never be asked to"
        );
    }

    #[tokio::test]
    async fn amending_on_an_adapter_without_modify_orders_is_refused_before_it_is_called() {
        let adapter = Arc::new(Recording::new());
        adapter.set_orders(vec![resting_limit_order(Scaled::new(2, 6_800_000))]);
        let engine = engine_with(Arc::clone(&adapter));

        let error = modify(
            &engine,
            &OrderId::new("amend-me"),
            OrderAmendment {
                quantity: Some(Scaled::new(3, 100)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TradeError::Unsupported { .. }),
            "got {error:?}"
        );
        assert!(
            !*adapter.orders_called.lock().unwrap(),
            "the adapter's own orders must not even be read when it cannot amend at all"
        );
    }

    #[tokio::test]
    async fn amended_prices_arrive_rounded_to_the_instruments_tick() {
        let adapter = Arc::new(Recording {
            access_override: Some(AccountAccess::trading(
                AdapterCapabilities::market_only().with_feature(AdapterFeature::ModifyOrders),
            )),
            ..Recording::new()
        });
        adapter.set_orders(vec![resting_limit_order(Scaled::new(2, 6_800_000))]);
        let engine = engine_with(Arc::clone(&adapter));

        modify(
            &engine,
            &OrderId::new("amend-me"),
            OrderAmendment {
                limit_price: Some(Scaled::new(4, 684_201_379)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (_, amendment) = adapter.modified();
        assert_eq!(
            amendment.limit_price,
            Some(Scaled::new(2, 6_842_013)),
            "the amended price must arrive on the tick, rounded down, exactly as a new order's does"
        );
    }
}
