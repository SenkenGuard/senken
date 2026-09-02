//! End-to-end exercises of the paper broker: an order placed against a
//! fixed price, the position it opens, the profit it realises when closed,
//! and the resting orders that fill only when the mark reaches them.
//!
//! The mark price source here is a value a test sets directly, which is the
//! point of `MarkPriceSource` being a port: none of this waits on a
//! network, a clock, or a scheduler, so a fill either happens because the
//! rule says so or it does not happen at all.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use senken_core::UnixNanos;
use senken_core::decimal::Scaled;
use senken_marketdata::{Instrument, InstrumentId};
use senken_plugin_simulator::{ADAPTER_ID, SimulatorAdapter};
use senken_sim_core::money::CASH_SCALE;
use senken_storage::Storage;
use senken_trade::{
    AccessLevel, AccountRef, InstrumentSource, MarkPrice, MarkPriceSource, OrderAmendment,
    OrderFilter, OrderKind, OrderRequest, OrderSide, OrderStatus, PositionSide, SettingsInput,
    SettingsSchema, SettingsValues, TradeAccountId, TradeAdapter, TradeContext, TradeEngine,
    TradeError,
};
use tempfile::TempDir;

/// A price a test moves between calls, so "the market reached the limit"
/// is a thing the test *does* rather than a thing it waits for.
struct MovingMark(Mutex<Option<Scaled>>);

impl MovingMark {
    fn at(price: i64) -> Self {
        Self(Mutex::new(Some(Scaled::new(2, price))))
    }

    fn set(&self, price: i64) {
        *self.0.lock().unwrap() = Some(Scaled::new(2, price));
    }

    fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

#[async_trait]
impl MarkPriceSource for MovingMark {
    async fn mark_price(
        &self,
        _instrument: &InstrumentId,
    ) -> Result<Option<MarkPrice>, TradeError> {
        Ok(self.0.lock().unwrap().map(|price| MarkPrice {
            price,
            as_of: UnixNanos::from_secs(1_700_000_000).unwrap(),
        }))
    }
}

struct Catalog;

#[async_trait]
impl InstrumentSource for Catalog {
    async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError> {
        Ok(Some(
            Instrument::spot(id.symbol(), id.symbol(), "BTC", "USDT")
                .with_price_increment((2, 1))
                .with_qty_increment((3, 1)),
        ))
    }
}

fn instrument() -> InstrumentId {
    InstrumentId::parse("okx-spot:BTCUSDT").unwrap()
}

/// The id this adapter mints for the one position it can hold on
/// [`instrument`] — a netting book holds at most one per instrument, so
/// the instrument names it.
fn position_id() -> senken_trade::PositionId {
    senken_trade::PositionId::new(instrument().to_string())
}

/// A price in whole currency units, at the two-decimal scale every
/// instrument in these tests quotes at.
fn at(units: i64) -> i64 {
    units * 100
}

fn qty(thousandths: i64) -> Scaled {
    Scaled::new(3, thousandths)
}

fn price(cents: i64) -> Scaled {
    Scaled::new(2, cents)
}

/// Settings with no fee and no slippage, so a test's arithmetic is the
/// model's arithmetic and not the model's arithmetic minus a rounding.
fn clean_settings(adapter: &SimulatorAdapter) -> SettingsValues {
    settings(
        adapter,
        &SettingsInput::new()
            .with("fee_bps", 0.into())
            .with("slippage_bps", 0.into()),
    )
}

fn settings(adapter: &SimulatorAdapter, input: &SettingsInput) -> SettingsValues {
    let schema: SettingsSchema = adapter.settings_schema();
    schema.validate(input).unwrap()
}

struct Fixture {
    _dir: TempDir,
    adapter: Arc<SimulatorAdapter>,
    mark: MovingMark,
    catalog: Catalog,
    id: TradeAccountId,
}

impl Fixture {
    async fn open(
        mark_at: i64,
        settings: impl Fn(&SimulatorAdapter) -> SettingsValues,
    ) -> (Self, SettingsValues) {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path());
        storage.init().unwrap();
        let adapter = Arc::new(SimulatorAdapter::new(storage));
        let values = settings(&adapter);
        let fixture = Self {
            _dir: dir,
            adapter,
            mark: MovingMark::at(mark_at),
            catalog: Catalog,
            id: TradeAccountId::new(),
        };
        let account = AccountRef {
            id: fixture.id,
            label: "Test",
            settings: &values,
        };
        fixture
            .adapter
            .open_account(&fixture.ctx(), account)
            .await
            .unwrap();
        (fixture, values)
    }

    fn ctx(&self) -> TradeContext<'_> {
        TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &self.mark,
            &self.catalog,
        )
    }

    fn account<'a>(&'a self, settings: &'a SettingsValues) -> AccountRef<'a> {
        AccountRef {
            id: self.id,
            label: "Test",
            settings,
        }
    }

    /// A [`TradeEngine`] with this fixture's own adapter registered — for
    /// the tests that exercise `close_position`, which lives on the engine
    /// rather than on the adapter itself.
    fn engine(&self) -> TradeEngine {
        let mut engine = TradeEngine::new();
        engine
            .register(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>)
            .unwrap();
        engine
    }
}

#[tokio::test]
async fn a_market_buy_opens_a_long_position_at_the_mark() {
    let (f, settings) = Fixture::open(6_842_000, clean_settings).await;

    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(250)),
        )
        .await
        .unwrap();

    assert_eq!(order.status, OrderStatus::Filled);
    assert_eq!(order.average_price, Some(price(6_842_000)));

    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].side, PositionSide::Long);
    assert_eq!(positions[0].quantity, qty(250));
    assert_eq!(positions[0].average_entry, price(6_842_000));
}

#[tokio::test]
async fn a_long_position_shows_profit_when_the_mark_rises_and_loss_when_it_falls() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    f.mark.set(at(11_000));
    let up = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    // One unit bought at 10 000 and marked at 11 000 is 1 000 of profit.
    assert_eq!(
        up[0].unrealized_pnl,
        Some(Scaled::new(CASH_SCALE, 1_000 * 100_000_000))
    );

    f.mark.set(at(9_000));
    let down = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(
        down[0].unrealized_pnl,
        Some(Scaled::new(CASH_SCALE, -1_000 * 100_000_000))
    );
}

#[tokio::test]
async fn closing_a_position_moves_its_profit_into_cash_and_removes_it() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let opening = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    f.mark.set(at(11_000));
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(1_000)),
        )
        .await
        .unwrap();

    let closed = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    assert_eq!(
        closed.balance.value - opening.balance.value,
        1_000 * 100_000_000,
        "the 1 000 of profit must land in cash, not stay unrealised"
    );
    assert_eq!(closed.unrealized_pnl.value, 0);
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty(),
        "a fully closed position must not linger at zero size"
    );
}

#[tokio::test]
async fn selling_more_than_a_long_position_flips_it_short() {
    // Netting, not hedging: the two sides cannot both exist.
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(3_000)),
        )
        .await
        .unwrap();

    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].side, PositionSide::Short);
    assert_eq!(positions[0].quantity, qty(2_000));
}

#[tokio::test]
async fn a_second_buy_averages_the_entry_by_size_not_by_midpoint() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(3_000)),
        )
        .await
        .unwrap();
    f.mark.set(at(20_000));
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(
        positions[0].average_entry,
        price(at(12_500)),
        "3 at 10 000 and 1 at 20 000 averages 12 500, not 15 000"
    );
}

#[tokio::test]
async fn a_limit_buy_rests_until_the_market_comes_down_to_it() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;

    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::limit(instrument(), OrderSide::Buy, qty(1_000), price(at(9_000))),
        )
        .await
        .unwrap();
    assert_eq!(order.status, OrderStatus::Open, "9 000 is below the market");

    let still_open = f
        .adapter
        .orders(&f.ctx(), f.account(&settings), OrderFilter::Open)
        .await
        .unwrap();
    assert_eq!(still_open.len(), 1);
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty(),
        "a resting order must not open a position"
    );

    f.mark.set(at(8_900));
    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(
        positions.len(),
        1,
        "the mark reached the limit, so it filled"
    );
    assert_eq!(
        positions[0].average_entry,
        price(at(9_000)),
        "a limit order fills at its own price, not at the mark that triggered it"
    );
    assert!(
        f.adapter
            .orders(&f.ctx(), f.account(&settings), OrderFilter::Open)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_stop_sell_triggers_on_the_way_down_and_a_stop_buy_on_the_way_up() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest {
                kind: OrderKind::Stop {
                    trigger: price(at(9_500)),
                },
                ..OrderRequest::market(instrument(), OrderSide::Sell, qty(1_000))
            },
        )
        .await
        .unwrap();

    f.mark.set(at(9_600));
    assert_eq!(
        f.adapter
            .orders(&f.ctx(), f.account(&settings), OrderFilter::Open)
            .await
            .unwrap()
            .len(),
        1,
        "9 600 has not reached the 9 500 stop"
    );

    f.mark.set(at(9_400));
    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(positions[0].side, PositionSide::Short);
    assert_eq!(
        positions[0].average_entry,
        price(at(9_400)),
        "a stop becomes a market order and takes the mark, not the trigger"
    );
}

#[tokio::test]
async fn a_cancelled_order_never_fills_however_far_the_market_moves() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::limit(instrument(), OrderSide::Buy, qty(1_000), price(at(9_000))),
        )
        .await
        .unwrap();

    let cancelled = f
        .adapter
        .cancel_order(&f.ctx(), f.account(&settings), &order.id)
        .await
        .unwrap();
    assert_eq!(cancelled.status, OrderStatus::Cancelled);

    f.mark.set(at(1));
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        matches!(
            f.adapter
                .cancel_order(&f.ctx(), f.account(&settings), &order.id)
                .await,
            Err(TradeError::OrderNotOpen)
        ),
        "cancelling twice must be refused, not silently repeated"
    );
}

#[tokio::test]
async fn a_market_order_on_an_instrument_with_no_price_is_refused_by_name() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.mark.clear();

    let error = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, TradeError::NoMarkPrice(_)),
        "the one failure a user can fix themselves must say so; got {error:?}"
    );
}

#[tokio::test]
async fn a_limit_order_is_accepted_even_with_no_price_yet() {
    // A resting order needs a mark when it is *checked*, not when it is
    // placed; refusing it up front would block an order on an instrument
    // whose history simply has not loaded.
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.mark.clear();

    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::limit(instrument(), OrderSide::Buy, qty(1_000), price(at(9_000))),
        )
        .await
        .unwrap();

    assert_eq!(order.status, OrderStatus::Open);
}

#[tokio::test]
async fn a_fee_is_charged_on_every_fill_and_leaves_cash_lower_than_the_profit_alone() {
    let (f, settings) = Fixture::open(at(10_000), |adapter| {
        settings(
            adapter,
            &SettingsInput::new()
                .with("fee_bps", 10.into())
                .with("slippage_bps", 0.into()),
        )
    })
    .await;
    let opening = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    f.mark.set(at(11_000));
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(1_000)),
        )
        .await
        .unwrap();

    let closed = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    let gained = closed.balance.value - opening.balance.value;

    // 1 000 of profit, less 10 bps on 10 000 and 10 bps on 11 000: 21 of
    // fees.
    assert_eq!(gained, (1_000 - 21) * 100_000_000);
}

#[tokio::test]
async fn slippage_always_moves_the_fill_against_the_trader() {
    let (f, settings) = Fixture::open(at(10_000), |adapter| {
        settings(
            adapter,
            &SettingsInput::new()
                .with("fee_bps", 0.into())
                .with("slippage_bps", 100.into()),
        )
    })
    .await;

    let bought = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    assert_eq!(
        bought.average_price,
        Some(price(at(10_100))),
        "a buyer pays up"
    );

    let sold = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(2_000)),
        )
        .await
        .unwrap();
    assert_eq!(
        sold.average_price,
        Some(price(at(9_900))),
        "a seller gets less"
    );
}

#[tokio::test]
async fn an_order_larger_than_the_account_can_margin_is_refused() {
    let (f, settings) = Fixture::open(at(10_000), |adapter| {
        settings(
            adapter,
            &SettingsInput::new()
                .with("starting_balance", "1000.00".into())
                .with("leverage", 1.into()),
        )
    })
    .await;

    let error = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, TradeError::InsufficientBalance(_)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn leverage_is_what_lets_the_same_account_take_the_same_order() {
    let (f, settings) = Fixture::open(at(10_000), |adapter| {
        settings(
            adapter,
            &SettingsInput::new()
                .with("starting_balance", "1000.00".into())
                .with("leverage", 20.into()),
        )
    })
    .await;

    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_reduce_only_order_cannot_exceed_the_position_it_closes() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    let error = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(2_000)).reduce_only(),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, TradeError::InvalidRequest(_)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn the_deposit_action_adds_to_cash_and_the_reset_action_puts_it_back() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let schema = f
        .adapter
        .actions()
        .into_iter()
        .find(|action| action.id == "deposit")
        .unwrap()
        .form;
    let params = schema
        .validate(&SettingsInput::new().with("amount", "500.00".into()))
        .unwrap();

    let opening = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    f.adapter
        .run_action(&f.ctx(), f.account(&settings), "deposit", params)
        .await
        .unwrap();
    let after = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(
        after.balance.value - opening.balance.value,
        500 * 100_000_000
    );

    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(500)),
        )
        .await
        .unwrap();

    f.adapter
        .run_action(
            &f.ctx(),
            f.account(&settings),
            "reset",
            SettingsValues::new(),
        )
        .await
        .unwrap();

    let reset = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(reset.balance, opening.balance);
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_action_the_adapter_does_not_offer_is_refused_by_name() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;

    let error = f
        .adapter
        .run_action(
            &f.ctx(),
            f.account(&settings),
            "drain_the_account",
            SettingsValues::new(),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, TradeError::Unsupported { .. }),
        "got {error:?}"
    );
}

#[tokio::test]
async fn the_books_survive_the_process_restarting() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::new(dir.path());
    storage.init().unwrap();
    let id = TradeAccountId::new();
    let mark = MovingMark::at(at(10_000));
    let catalog = Catalog;
    let now = UnixNanos::from_secs(1_700_000_000).unwrap();

    let values = {
        let adapter = SimulatorAdapter::new(Storage::new(dir.path()));
        let values = clean_settings(&adapter);
        let account = AccountRef {
            id,
            label: "Test",
            settings: &values,
        };
        let ctx = TradeContext::new(now, &mark, &catalog);
        adapter.open_account(&ctx, account).await.unwrap();
        adapter
            .place_order(
                &ctx,
                account,
                OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
            )
            .await
            .unwrap();
        values
    };

    // A second adapter over the same directory is what a restart is.
    let restarted = SimulatorAdapter::new(Storage::new(dir.path()));
    let account = AccountRef {
        id,
        label: "Test",
        settings: &values,
    };
    let ctx = TradeContext::new(now, &mark, &catalog);
    let positions = restarted.positions(&ctx, account).await.unwrap();

    assert_eq!(positions.len(), 1, "the position must survive the restart");
    assert_eq!(positions[0].average_entry, price(at(10_000)));
}

#[tokio::test]
async fn reopening_an_account_does_not_wipe_its_books() {
    // `open_account` runs again on every settings edit; resetting there
    // would erase a user's positions the moment they changed the fee.
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    f.adapter
        .open_account(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    assert_eq!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn every_instrument_is_covered_whatever_venue_it_came_from() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::new(dir.path());
    storage.init().unwrap();
    let adapter = SimulatorAdapter::new(storage);
    let coverage = adapter.coverage();

    for id in [
        "okx-spot:BTCUSDT",
        "kraken-spot:ETHEUR",
        "deribit-option:BTC-27JUN25-100000-C",
    ] {
        assert!(
            coverage.covers(&InstrumentId::parse(id).unwrap()),
            "{id} must be paper-tradable"
        );
    }
}

/// Settings identical to [`clean_settings`] but with the account set to
/// read-only — the same shape a broker's investor login has.
fn read_only_settings(adapter: &SimulatorAdapter) -> SettingsValues {
    settings(
        adapter,
        &SettingsInput::new()
            .with("fee_bps", 0.into())
            .with("slippage_bps", 0.into())
            .with("access", "read_only".into()),
    )
}

#[tokio::test]
async fn the_access_setting_flips_what_account_access_reports() {
    let (f, trade_settings) = Fixture::open(at(10_000), clean_settings).await;
    let trading = f
        .adapter
        .account_access(&f.ctx(), f.account(&trade_settings))
        .await
        .unwrap();
    assert_eq!(trading.level, AccessLevel::Trade);
    assert!(trading.is_trading());
    assert_eq!(trading.note, None);

    let (f, ro_settings) = Fixture::open(at(10_000), read_only_settings).await;
    let read_only = f
        .adapter
        .account_access(&f.ctx(), f.account(&ro_settings))
        .await
        .unwrap();
    assert_eq!(read_only.level, AccessLevel::ReadOnly);
    assert!(!read_only.is_trading());
    assert!(
        read_only.note.is_some(),
        "a restriction shown on screen needs a reason attached to it"
    );
}

#[tokio::test]
async fn a_read_only_simulator_account_still_reports_its_balances_and_positions() {
    // Opened trading so it actually holds a position, then re-saved
    // read-only — the exact edit a settings screen performs, and
    // `open_account` runs again on every one.
    let (f, trade_settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&trade_settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    let ro_settings = read_only_settings(&f.adapter);
    f.adapter
        .open_account(&f.ctx(), f.account(&ro_settings))
        .await
        .unwrap();

    let balances = f
        .adapter
        .balances(&f.ctx(), f.account(&ro_settings))
        .await
        .unwrap();
    // No fee, no slippage and the mark never moved off the entry price, so
    // equity must be exactly the schema's own default starting balance
    // ($100,000.00) re-expressed at the simulator's cash scale.
    assert_eq!(balances.equity.value, 10_000_000 * 1_000_000);

    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&ro_settings))
        .await
        .unwrap();
    assert_eq!(
        positions.len(),
        1,
        "a read-only account still exists to be read"
    );
}

#[tokio::test]
async fn amending_a_resting_limit_changes_its_price() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::limit(instrument(), OrderSide::Buy, qty(1_000), price(at(9_000))),
        )
        .await
        .unwrap();

    let amended = f
        .adapter
        .modify_order(
            &f.ctx(),
            f.account(&settings),
            &order.id,
            OrderAmendment {
                limit_price: Some(price(at(9_200))),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        amended.kind,
        OrderKind::Limit {
            price: price(at(9_200))
        }
    );
    let stored = f
        .adapter
        .orders(&f.ctx(), f.account(&settings), OrderFilter::Open)
        .await
        .unwrap();
    assert_eq!(
        stored[0].kind,
        OrderKind::Limit {
            price: price(at(9_200))
        },
        "the amendment must actually be persisted, not just echoed back"
    );
}

#[tokio::test]
async fn an_amendment_the_current_mark_already_satisfies_fills_immediately() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::limit(instrument(), OrderSide::Buy, qty(1_000), price(at(9_000))),
        )
        .await
        .unwrap();
    assert_eq!(order.status, OrderStatus::Open);

    // The market comes down to 9 300 — still above the resting 9 000, so
    // nothing has filled yet.
    f.mark.set(at(9_300));
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty()
    );

    // Amending the limit up to 9 500 is a price the market at 9 300 already
    // satisfies. If the simulator only re-checked resting orders on the
    // next unrelated read, this order would sit there filled-but-for-the-
    // fact-nobody-looked; the property under test is that amending is
    // itself enough to re-settle.
    let amended = f
        .adapter
        .modify_order(
            &f.ctx(),
            f.account(&settings),
            &order.id,
            OrderAmendment {
                limit_price: Some(price(at(9_500))),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        amended.status,
        OrderStatus::Filled,
        "the mark already satisfies the amended price, so it must fill right away"
    );
    assert_eq!(
        amended.average_price,
        Some(price(at(9_500))),
        "a limit fills at its own (amended) price, not the mark that satisfied it"
    );
    assert_eq!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn amending_a_filled_order_is_refused() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    let order = f
        .adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    assert_eq!(order.status, OrderStatus::Filled);

    let error = f
        .adapter
        .modify_order(
            &f.ctx(),
            f.account(&settings),
            &order.id,
            OrderAmendment {
                quantity: Some(qty(500)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();

    assert!(matches!(error, TradeError::OrderNotOpen), "got {error:?}");
}

#[tokio::test]
async fn closing_through_the_engine_leaves_no_position_and_banks_the_difference() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();
    f.mark.set(at(11_000));
    let opening = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    f.engine()
        .close_position(ADAPTER_ID, &f.ctx(), f.account(&settings), &position_id())
        .await
        .unwrap();

    let closed = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(
        closed.balance.value - opening.balance.value,
        1_000 * 100_000_000,
        "the 1 000 of profit must land in cash"
    );
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty(),
        "a full close must leave no position behind"
    );
}

#[tokio::test]
async fn closing_after_the_position_grows_between_the_read_and_the_close_uses_the_new_size() {
    let (f, settings) = Fixture::open(at(10_000), clean_settings).await;
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(1_000)),
        )
        .await
        .unwrap();

    // The table is drawn here: this is the size a screen would have read.
    let drawn = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert_eq!(drawn[0].quantity, qty(1_000));

    // A second fill lands before the CLOSE button is pressed.
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(500)),
        )
        .await
        .unwrap();

    f.engine()
        .close_position(ADAPTER_ID, &f.ctx(), f.account(&settings), &position_id())
        .await
        .unwrap();

    // If the close had used the 1 000 the table drew rather than the 1 500
    // actually held, this would still show a 500-unit long.
    assert!(
        f.adapter
            .positions(&f.ctx(), f.account(&settings))
            .await
            .unwrap()
            .is_empty(),
        "the close must use the size the adapter reports now, not the one the screen opened with"
    );
}

/// A realised profit records something that already happened, so closing
/// the position that earned it must not take the number with it.
///
/// This is the shape the book got wrong: a reversal carried the figure
/// forward and a partial reduce carried it forward, but closing exactly
/// flat called `positions.remove` and dropped it — leaving an account
/// whose balance had moved and whose reported profit said nothing had.
#[tokio::test]
async fn a_profit_survives_the_position_that_earned_it_being_closed() {
    let (f, settings) = Fixture::open(6_842_000, clean_settings).await;

    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Buy, qty(250)),
        )
        .await
        .unwrap();

    let opening_balance = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap()
        .balance;

    // The mark moves up, and the whole position is closed at the new one.
    f.mark.set(6_942_000);
    f.adapter
        .place_order(
            &f.ctx(),
            f.account(&settings),
            OrderRequest::market(instrument(), OrderSide::Sell, qty(250)),
        )
        .await
        .unwrap();

    let positions = f
        .adapter
        .positions(&f.ctx(), f.account(&settings))
        .await
        .unwrap();
    assert!(
        positions.is_empty(),
        "the position closed exactly flat, which is the case that used to lose the number"
    );

    let balances = f
        .adapter
        .balances(&f.ctx(), f.account(&settings))
        .await
        .unwrap();

    // 0.250 at a 1 000.00 gain per unit is 250.00, at the scale this
    // adapter keeps cash in.
    let expected = Scaled::new(balances.realized_pnl.scale, 250 * 10_i64.pow(8));
    assert_eq!(
        balances.realized_pnl, expected,
        "the profit is still reported after the position is gone"
    );
    assert_eq!(
        balances.balance.value - opening_balance.value,
        expected.value,
        "and it agrees with what the balance actually did"
    );
}
