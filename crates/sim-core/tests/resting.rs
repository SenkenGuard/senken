//! Pending orders: they rest, they fill at their own price, and cancelling
//! one takes it out of the book.
//!
//! Driven through a minimal venue, so what is exercised is the shared
//! adapter rather than any one trading system's rules.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::{Instrument, InstrumentId};
use senken_sim_core::{FillContext, Marks, Settled, SettlementModel, SimAdapter, SimulatedVenue};
use senken_storage::Storage;
use senken_trade::{
    AccountBalances, AccountRef, AdapterCapabilities, InstrumentSource, MarkPrice, MarkPriceSource,
    OrderFilter, OrderKind, OrderKindTag, OrderRequest, OrderSide, OrderStatus, Position,
    PositionMode, QuantityUnit, SettingsSchema, SettingsValues, TradeAccountId, TradeAdapter,
    TradeContext, TradeError,
};
use tempfile::TempDir;

/// A book that only records what was filled into it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Tape {
    filled: Vec<(String, i64)>,
}

#[derive(Debug, Clone, Copy)]
struct TapeModel;

impl SettlementModel for TapeModel {
    type Book = Tape;

    fn settle(&self, book: &mut Tape, fill: &FillContext<'_>) -> Result<Settled, TradeError> {
        book.filled
            .push((fill.instrument.to_string(), fill.price.value));
        Ok(Settled {
            fill_price: fill.price,
            fee: 0,
            realized: 0,
        })
    }
}

#[derive(Debug, Default)]
struct TapeVenue;

impl SimulatedVenue for TapeVenue {
    type Book = Tape;
    type Model = TapeModel;

    fn id(&self) -> &'static str {
        "tape"
    }
    fn name(&self) -> &'static str {
        "Tape"
    }
    fn description(&self) -> &'static str {
        "A venue that records its fills and nothing else"
    }
    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            .with_order_kinds(vec![OrderKindTag::Market, OrderKindTag::Limit])
            .with_quantity_unit(QuantityUnit::Base)
            .with_position_mode(PositionMode::Netting)
    }
    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::default()
    }
    fn model_for(&self, _settings: &SettingsValues) -> Result<TapeModel, TradeError> {
        Ok(TapeModel)
    }
    fn open_book(&self, _settings: &SettingsValues) -> Result<Tape, TradeError> {
        Ok(Tape::default())
    }
    fn positions(
        &self,
        _book: &Tape,
        _marks: &Marks,
        _account: TradeAccountId,
    ) -> Result<Vec<Position>, TradeError> {
        Ok(Vec::new())
    }
    fn balances(
        &self,
        _book: &Tape,
        _marks: &Marks,
        account: TradeAccountId,
        _settings: &SettingsValues,
    ) -> Result<AccountBalances, TradeError> {
        Ok(AccountBalances {
            account_id: account,
            currency: "USD".to_owned(),
            balance: Scaled::new(2, 0),
            equity: Scaled::new(2, 0),
            unrealized_pnl: Scaled::new(2, 0),
            realized_pnl: Scaled::new(2, 0),
            margin_used: None,
            margin_available: None,
            margin_level: None,
            assets: Vec::new(),
        })
    }
}

struct Mark(std::sync::Mutex<i64>);

#[async_trait::async_trait]
impl MarkPriceSource for Mark {
    async fn mark_price(&self, _id: &InstrumentId) -> Result<Option<MarkPrice>, TradeError> {
        Ok(Some(MarkPrice {
            price: Scaled::new(0, *self.0.lock().unwrap()),
            as_of: UnixNanos::from_secs(1_700_000_000).unwrap(),
        }))
    }
}

struct Catalog;

#[async_trait::async_trait]
impl InstrumentSource for Catalog {
    async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError> {
        Ok(Some(
            Instrument::spot(id.symbol(), id.symbol(), "BTC", "USDT")
                .with_price_increment((0, 1))
                .with_qty_increment((0, 1)),
        ))
    }
}

fn pair() -> InstrumentId {
    InstrumentId::parse("okx-spot:BTCUSDT").unwrap()
}

struct Fixture {
    _dir: TempDir,
    adapter: SimAdapter<TapeVenue>,
    mark: Mark,
    catalog: Catalog,
    id: TradeAccountId,
    settings: SettingsValues,
}

impl Fixture {
    async fn open() -> Self {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path());
        storage.init().unwrap();
        let fixture = Self {
            _dir: dir,
            adapter: SimAdapter::new(TapeVenue, storage),
            mark: Mark(std::sync::Mutex::new(100)),
            catalog: Catalog,
            id: TradeAccountId::new(),
            settings: SettingsValues::default(),
        };
        fixture
            .adapter
            .open_account(&fixture.ctx(), fixture.account())
            .await
            .unwrap();
        fixture
    }

    fn ctx(&self) -> TradeContext<'_> {
        TradeContext::new(
            UnixNanos::from_secs(1_700_000_000).unwrap(),
            &self.mark,
            &self.catalog,
        )
    }

    fn account(&self) -> AccountRef<'_> {
        AccountRef {
            id: self.id,
            label: "Tape",
            settings: &self.settings,
        }
    }

    async fn limit(&self, side: OrderSide, price: i64) -> senken_trade::Order {
        let mut request = OrderRequest::market(pair(), side, Scaled::new(0, 1));
        request.kind = OrderKind::Limit {
            price: Scaled::new(0, price),
        };
        self.adapter
            .place_order(&self.ctx(), self.account(), request)
            .await
            .unwrap()
    }

    async fn open_orders(&self) -> Vec<senken_trade::Order> {
        self.adapter
            .orders(&self.ctx(), self.account(), OrderFilter::Open)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn a_limit_the_market_has_not_reached_rests_instead_of_filling() {
    let fixture = Fixture::open().await;

    // Mark 100, buy limit at 50: the market has not come down to it.
    let order = fixture.limit(OrderSide::Buy, 50).await;

    assert_eq!(
        order.status,
        OrderStatus::Open,
        "filling this at the mark would hand the trader a price they could not have had"
    );
    assert_eq!(order.average_price, None, "and nothing filled");
    assert_eq!(fixture.open_orders().await.len(), 1);
}

#[tokio::test]
async fn a_limit_the_market_has_already_reached_fills_on_the_next_read() {
    let fixture = Fixture::open().await;

    // Mark 100, buy limit at 150: the market is already below it.
    fixture.limit(OrderSide::Buy, 150).await;

    assert!(
        fixture.open_orders().await.is_empty(),
        "a read settles the book first, so an order the market has reached is gone from the \
         resting list"
    );
    let fills = fixture
        .adapter
        .fills(&fixture.ctx(), fixture.account(), 10)
        .await
        .unwrap();
    assert_eq!(fills.len(), 1);
    assert_eq!(
        fills[0].price,
        Scaled::new(0, 150),
        "it fills at its own limit, not at the mark — the market reaching a level is not the \
         same as the market crossing it"
    );
}

#[tokio::test]
async fn cancelling_a_resting_order_takes_it_out_of_the_book() {
    let fixture = Fixture::open().await;
    let order = fixture.limit(OrderSide::Buy, 50).await;

    let cancelled = fixture
        .adapter
        .cancel_order(&fixture.ctx(), fixture.account(), &order.id)
        .await
        .unwrap();

    assert_eq!(cancelled.status, OrderStatus::Cancelled);
    assert!(fixture.open_orders().await.is_empty());
}

#[tokio::test]
async fn cancelling_an_order_that_is_not_there_is_refused_rather_than_silently_ignored() {
    let fixture = Fixture::open().await;

    let error = fixture
        .adapter
        .cancel_order(
            &fixture.ctx(),
            fixture.account(),
            &senken_trade::OrderId::new("never-existed"),
        )
        .await
        .unwrap_err();

    assert!(
        matches!(error, TradeError::UnknownOrder),
        "a cancel that quietly succeeds tells the trader an order is gone that is still live: \
         got {error:?}"
    );
}

#[tokio::test]
async fn amending_a_resting_order_keeps_its_identity() {
    let fixture = Fixture::open().await;
    let order = fixture.limit(OrderSide::Buy, 50).await;

    let amended = fixture
        .adapter
        .modify_order(
            &fixture.ctx(),
            fixture.account(),
            &order.id,
            senken_trade::OrderAmendment {
                limit_price: Some(Scaled::new(0, 60)),
                ..senken_trade::OrderAmendment::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        amended.id, order.id,
        "an amendment is the same order at a new price, not a cancel and a replace — so its \
         id and its place in the queue both survive"
    );
    assert_eq!(
        amended.kind,
        OrderKind::Limit {
            price: Scaled::new(0, 60)
        }
    );
    assert_eq!(
        fixture.open_orders().await.len(),
        1,
        "and still just the one"
    );
}

#[tokio::test]
async fn the_adapter_declares_the_cancelling_and_amending_it_actually_does() {
    let fixture = Fixture::open().await;
    let declared = TradeAdapter::capabilities(&fixture.adapter);
    let resolved = fixture
        .adapter
        .account_access(&fixture.ctx(), fixture.account())
        .await
        .unwrap();

    for feature in [
        senken_trade::AdapterFeature::CancelOrders,
        senken_trade::AdapterFeature::ModifyOrders,
    ] {
        assert!(declared.has(feature), "the adapter says it can {feature:?}");
        assert!(
            resolved.capabilities.has(feature),
            "and so does the account resolved from it — the engine validates against this \
             answer, so a venue list that disagreed would refuse what the adapter can do"
        );
    }
}

#[tokio::test]
async fn a_resting_order_on_an_instrument_with_no_position_still_gets_a_price() {
    let fixture = Fixture::open().await;

    // Nothing is open, so the only instrument needing a mark is the one
    // this order is waiting on.
    fixture.limit(OrderSide::Buy, 150).await;

    // Reading positions settles the book, and comes back empty — this
    // account holds none. The order still had to be priced.
    let positions = fixture
        .adapter
        .positions(&fixture.ctx(), fixture.account())
        .await
        .unwrap();
    assert!(positions.is_empty(), "the account holds no position at all");

    let tape = fixture
        .adapter
        .fills(&fixture.ctx(), fixture.account(), 10)
        .await
        .unwrap();
    assert_eq!(
        tape.len(),
        1,
        "gathering marks only for open positions would leave this order waiting for ever, \
         however far the market moved through it"
    );
}
