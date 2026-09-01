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
use senken_marketdata::Instrument;

use crate::adapter::{AccountRef, TradeAdapter, TradeContext};
use crate::capability::AdapterFeature;
use crate::error::TradeError;
use crate::order::{Order, OrderKind, OrderRequest};

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

    /// Validates a request against the adapter and the instrument, then
    /// sends it.
    ///
    /// The order that reaches the adapter has had its prices rounded to the
    /// instrument's tick and its quantity to the instrument's step — as
    /// scaled integers, never through a float. An indicator-derived price
    /// arriving here as `68420.1379` becomes `68420.13` before anything
    /// trades on it, which is the boundary this project draws between a
    /// display value and a money value.
    ///
    /// # Errors
    /// [`TradeError::UnknownAdapter`], [`TradeError::InstrumentNotTradable`],
    /// [`TradeError::Unsupported`] for an order kind or time in force the
    /// adapter does not accept, [`TradeError::UnknownInstrument`] when the
    /// instrument is not catalogued, [`TradeError::InvalidRequest`] for a
    /// non-positive quantity or a price that cannot be expressed at the
    /// instrument's scale, and whatever the adapter itself returns.
    pub async fn place_order(
        &self,
        adapter_id: &str,
        ctx: &TradeContext<'_>,
        account: AccountRef<'_>,
        request: OrderRequest,
    ) -> Result<Order, TradeError> {
        let adapter = self.adapter(adapter_id)?;
        let request = self.prepare_order(adapter, ctx, request).await?;
        adapter.place_order(ctx, account, request).await
    }

    /// The validation half of [`place_order`](Self::place_order), split out
    /// so it can be exercised on its own.
    async fn prepare_order(
        &self,
        adapter: &Arc<dyn TradeAdapter>,
        ctx: &TradeContext<'_>,
        mut request: OrderRequest,
    ) -> Result<OrderRequest, TradeError> {
        if !adapter.coverage().covers(&request.instrument) {
            return Err(TradeError::InstrumentNotTradable {
                adapter: adapter.id().to_owned(),
                instrument: request.instrument.clone(),
            });
        }

        let capabilities = adapter.capabilities();
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
    use crate::adapter::{
        AccountRef, InstrumentSource, MarkPrice, MarkPriceSource, TradeAdapter, TradeContext,
    };
    use crate::capability::{
        AdapterCapabilities, AdapterKind, InstrumentCoverage, PositionMode, QuantityUnit,
    };
    use crate::error::TradeError;
    use crate::id::{OrderId, TradeAccountId};
    use crate::order::{
        Order, OrderFilter, OrderKind, OrderKindTag, OrderRequest, OrderSide, OrderStatus,
        TimeInForce,
    };
    use crate::portfolio::{AccountBalances, AdapterHealth, Position};
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
        seen: std::sync::Mutex<Option<OrderRequest>>,
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
                seen: std::sync::Mutex::new(None),
            }
        }

        fn taken(&self) -> OrderRequest {
            self.seen
                .lock()
                .unwrap()
                .clone()
                .expect("no order reached the adapter")
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
            Ok(Vec::new())
        }
        async fn orders(
            &self,
            _ctx: &TradeContext<'_>,
            _account: AccountRef<'_>,
            _filter: OrderFilter,
        ) -> Result<Vec<Order>, TradeError> {
            Ok(Vec::new())
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
}
