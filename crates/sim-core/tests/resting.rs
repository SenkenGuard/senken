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
    AccountBalances, AccountRef, AdapterCapabilities, HistoryBar, InstrumentSource, MarkPrice,
    MarkPriceSource, OrderFilter, OrderKind, OrderKindTag, OrderRequest, OrderSide, OrderStatus,
    Position, PositionMode, PriceHistorySource, QuantityUnit, SettingsSchema, SettingsValues,
    TradeAccountId, TradeAdapter, TradeContext, TradeError,
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

/// Bars the fixture hands back verbatim, so a test can say exactly what
/// the market did between two reads.
#[derive(Clone)]
struct History(Vec<HistoryBar>);

#[async_trait::async_trait]
impl PriceHistorySource for History {
    async fn bars_between(
        &self,
        _instrument: &InstrumentId,
        from: UnixNanos,
        to: UnixNanos,
    ) -> Result<Vec<HistoryBar>, TradeError> {
        Ok(self
            .0
            .iter()
            .filter(|bar| {
                bar.opened_at.as_nanos() > from.as_nanos()
                    && bar.opened_at.as_nanos() <= to.as_nanos()
            })
            .copied()
            .collect())
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
    history: History,
    /// The instant a read happens at. Moved forward so a test can place an
    /// order and then let the market run past it, which is the only way an
    /// order ever fills: bars *after* it was placed.
    now: std::sync::Mutex<i64>,
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
            history: History(Vec::new()),
            now: std::sync::Mutex::new(1_700_000_000),
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
            UnixNanos::from_secs(*self.now.lock().unwrap()).unwrap(),
            &self.mark,
            &self.catalog,
        )
        .with_history(&self.history)
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

fn bar(at: i64, high: i64, low: i64, close: i64) -> HistoryBar {
    HistoryBar {
        opened_at: UnixNanos::from_secs(at).unwrap(),
        high: Scaled::new(0, high),
        low: Scaled::new(0, low),
        close: Scaled::new(0, close),
    }
}

#[tokio::test]
async fn a_stop_fills_on_the_bar_that_reached_it_and_not_at_the_readers_price() {
    let mut fixture = Fixture::open().await;
    // The market dipped to 40 an hour ago and came back to 100. A reader
    // arriving now sees only 100 — but the stop was hit, and it was hit
    // then.
    fixture.history = History(vec![
        bar(1_700_003_600, 105, 95, 100),
        bar(1_700_007_200, 102, 40, 100),
        bar(1_700_010_800, 101, 99, 100),
    ]);

    let mut request = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 1));
    request.kind = OrderKind::Stop {
        trigger: Scaled::new(0, 50),
    };
    fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await
        .unwrap();
    // Three hours pass. The reader arrives back to a mark of 100 and no
    // sign that anything happened.
    *fixture.now.lock().unwrap() = 1_700_014_400;

    let fills = fixture
        .adapter
        .fills(&fixture.ctx(), fixture.account(), 10)
        .await
        .unwrap();
    assert_eq!(
        fills.len(),
        1,
        "the bar's low reached 40, so a stop at 50 was hit — settling on the current mark of \
         100 alone would never have seen it"
    );
    assert_eq!(
        fills[0].executed_at,
        UnixNanos::from_secs(1_700_007_200).unwrap(),
        "and it happened at that bar's own time, not at the instant the reader looked"
    );
}

#[tokio::test]
async fn a_level_no_bar_reached_leaves_the_order_resting() {
    let mut fixture = Fixture::open().await;
    fixture.history = History(vec![bar(1_700_003_600, 105, 95, 100)]);

    let mut request = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 1));
    request.kind = OrderKind::Stop {
        trigger: Scaled::new(0, 50),
    };
    fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await
        .unwrap();
    *fixture.now.lock().unwrap() = 1_700_014_400;

    assert_eq!(
        fixture.open_orders().await.len(),
        1,
        "the bar never traded down to 50, so nothing was triggered"
    );
}

#[tokio::test]
async fn the_earliest_bar_that_reached_the_level_is_the_one_that_fills_it() {
    let mut fixture = Fixture::open().await;
    // Two bars both reach it. The first one is when it happened.
    fixture.history = History(vec![
        bar(1_700_003_600, 102, 45, 100),
        bar(1_700_007_200, 102, 30, 100),
    ]);

    let mut request = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 1));
    request.kind = OrderKind::Stop {
        trigger: Scaled::new(0, 50),
    };
    fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await
        .unwrap();
    *fixture.now.lock().unwrap() = 1_700_014_400;

    let fills = fixture
        .adapter
        .fills(&fixture.ctx(), fixture.account(), 10)
        .await
        .unwrap();
    assert_eq!(
        fills[0].executed_at,
        UnixNanos::from_secs(1_700_003_600).unwrap(),
        "an order cannot survive the first bar that reached it to be filled by a later one"
    );
}

#[tokio::test]
async fn a_buy_stop_is_reached_from_below_and_a_sell_stop_from_above() {
    // One bar that rose to 160 and never fell below 95. A buy stop at 150
    // was hit by the high; a sell stop at 50 was not touched at all. An
    // implementation that read the same extreme for both sides would
    // agree with this on one of them and be wrong on the other.
    let bars = History(vec![bar(1_700_003_600, 160, 95, 100)]);

    let mut buying = Fixture::open().await;
    buying.history = History(bars.0.clone());
    let mut up = OrderRequest::market(pair(), OrderSide::Buy, Scaled::new(0, 1));
    up.kind = OrderKind::Stop {
        trigger: Scaled::new(0, 150),
    };
    buying
        .adapter
        .place_order(&buying.ctx(), buying.account(), up)
        .await
        .unwrap();
    *buying.now.lock().unwrap() = 1_700_014_400;
    assert!(
        buying.open_orders().await.is_empty(),
        "a buy stop triggers on the way up, and this bar's high reached it"
    );

    let mut selling = Fixture::open().await;
    selling.history = History(bars.0.clone());
    let mut down = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 1));
    down.kind = OrderKind::Stop {
        trigger: Scaled::new(0, 50),
    };
    selling
        .adapter
        .place_order(&selling.ctx(), selling.account(), down)
        .await
        .unwrap();
    *selling.now.lock().unwrap() = 1_700_014_400;
    assert_eq!(
        selling.open_orders().await.len(),
        1,
        "and a sell stop triggers on the way down, which this bar's low never did"
    );
}
