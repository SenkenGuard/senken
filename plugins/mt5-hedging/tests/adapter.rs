//! The adapter driven as a real account: open, trade both sides of one
//! symbol, and read back what a terminal would show.

use std::sync::Arc;

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::{Instrument, InstrumentId};
use senken_plugin_mt5_hedging::adapter::Mt5HedgingAdapter;
use senken_storage::Storage;
use senken_trade::{
    AccountRef, InstrumentSource, MarkPrice, MarkPriceSource, OrderRequest, OrderSide,
    PositionMode, PositionSide, SettingsInput, SettingsValues, TradeAccountId, TradeAdapter,
    TradeContext, TradeError,
};
use tempfile::TempDir;

struct Mark(std::sync::Mutex<i64>);

#[async_trait::async_trait]
impl MarkPriceSource for Mark {
    async fn mark_price(&self, _id: &InstrumentId) -> Result<Option<MarkPrice>, TradeError> {
        Ok(Some(MarkPrice {
            price: Scaled::new(5, *self.0.lock().unwrap()),
            as_of: UnixNanos::from_secs(1_767_571_200).unwrap(),
        }))
    }
}

struct Catalog;

#[async_trait::async_trait]
impl InstrumentSource for Catalog {
    async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError> {
        Ok(Some(
            Instrument::spot(id.symbol(), id.symbol(), "EUR", "USD")
                .with_price_increment((5, 1))
                .with_qty_increment((2, 1)),
        ))
    }
}

fn eurusd() -> InstrumentId {
    InstrumentId::parse("mt5:EURUSD").unwrap()
}

struct Fixture {
    _dir: TempDir,
    adapter: Arc<Mt5HedgingAdapter>,
    mark: Mark,
    catalog: Catalog,
    id: TradeAccountId,
    values: SettingsValues,
}

impl Fixture {
    async fn open() -> Self {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path());
        storage.init().unwrap();
        let adapter = Arc::new(Mt5HedgingAdapter::new(storage));
        let values = adapter
            .settings_schema()
            .validate(
                &SettingsInput::new()
                    .with("starting_balance", "10000.00".into())
                    .with("commission_mode", "none".into())
                    .with("deviation_points", 0.into()),
            )
            .unwrap();
        let fixture = Self {
            _dir: dir,
            adapter,
            mark: Mark(std::sync::Mutex::new(108_000)),
            catalog: Catalog,
            id: TradeAccountId::new(),
            values,
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
            UnixNanos::from_secs(1_767_571_200).unwrap(),
            &self.mark,
            &self.catalog,
        )
    }

    fn account(&self) -> AccountRef<'_> {
        AccountRef {
            id: self.id,
            label: "MT5 demo",
            settings: &self.values,
        }
    }

    async fn buy(&self, lots: i64) {
        self.adapter
            .place_order(
                &self.ctx(),
                self.account(),
                OrderRequest::market(eurusd(), OrderSide::Buy, Scaled::new(2, lots)),
            )
            .await
            .unwrap();
    }

    async fn sell(&self, lots: i64) {
        self.adapter
            .place_order(
                &self.ctx(),
                self.account(),
                OrderRequest::market(eurusd(), OrderSide::Sell, Scaled::new(2, lots)),
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn the_adapter_declares_a_hedging_account() {
    let fixture = Fixture::open().await;
    assert_eq!(
        fixture.adapter.capabilities().position_mode,
        PositionMode::Hedging,
        "everything else about this adapter follows from it being a hedging account"
    );
}

#[tokio::test]
async fn a_buy_and_a_sell_on_one_symbol_are_two_separate_tickets() {
    let fixture = Fixture::open().await;
    fixture.buy(100).await;
    fixture.sell(100).await;

    let positions = fixture
        .adapter
        .positions(&fixture.ctx(), fixture.account())
        .await
        .unwrap();

    assert_eq!(
        positions.len(),
        2,
        "a netting book would have shown one flat position, or none — this is the whole \
         difference a trader picks a hedging account for"
    );
    let sides: Vec<_> = positions.iter().map(|position| position.side).collect();
    assert!(sides.contains(&PositionSide::Long) && sides.contains(&PositionSide::Short));
    assert_ne!(
        positions[0].id, positions[1].id,
        "and each carries its own ticket number, so a close can name one"
    );
}

#[tokio::test]
async fn balances_report_margin_used_free_margin_and_the_margin_level() {
    let fixture = Fixture::open().await;
    fixture.buy(100).await;

    let balances = fixture
        .adapter
        .balances(&fixture.ctx(), fixture.account())
        .await
        .unwrap();

    // 1.00 lot of 100 000 at 1:100 is 1 000.00 of margin.
    assert_eq!(
        balances.margin_used,
        Some(Scaled::new(8, 1_000 * 100_000_000)),
        "forex margin is lots × contract size ÷ leverage"
    );
    assert!(
        balances.margin_level.is_some(),
        "a terminal shows the margin level most prominently of all, so an account holding \
         margin must report one"
    );
    assert_eq!(
        balances.margin_available,
        Some(Scaled::new(8, 9_000 * 100_000_000)),
        "free margin is equity less margin held"
    );
}

#[tokio::test]
async fn a_position_opens_with_its_stops_already_attached() {
    let fixture = Fixture::open().await;
    let mut request = OrderRequest::market(eurusd(), OrderSide::Buy, Scaled::new(2, 100));
    request.stop_loss = Some(Scaled::new(5, 107_000));
    request.take_profit = Some(Scaled::new(5, 110_000));
    fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await
        .unwrap();

    let positions = fixture
        .adapter
        .positions(&fixture.ctx(), fixture.account())
        .await
        .unwrap();

    assert_eq!(
        positions[0].stop_loss,
        Some(Scaled::new(5, 107_000)),
        "a position that must wait for a second request to be protected is unprotected for \
         however long that takes"
    );
    assert_eq!(positions[0].take_profit, Some(Scaled::new(5, 110_000)));
}

#[tokio::test]
async fn setting_a_stop_replaces_the_tickets_stop_rather_than_adding_a_second() {
    let fixture = Fixture::open().await;
    fixture.buy(100).await;
    let id = fixture
        .adapter
        .positions(&fixture.ctx(), fixture.account())
        .await
        .unwrap()[0]
        .id
        .clone();

    fixture
        .adapter
        .set_position_stops(
            &fixture.ctx(),
            fixture.account(),
            &id,
            Some(Scaled::new(5, 107_000)),
            None,
        )
        .await
        .unwrap();
    let amended = fixture
        .adapter
        .set_position_stops(
            &fixture.ctx(),
            fixture.account(),
            &id,
            Some(Scaled::new(5, 106_000)),
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        amended.stop_loss,
        Some(Scaled::new(5, 106_000)),
        "MetaTrader allows one stop loss per position, so the second sets rather than adds"
    );
}

#[tokio::test]
async fn an_investor_login_reads_the_account_and_places_nothing() {
    let mut fixture = Fixture::open().await;
    fixture.values = fixture
        .adapter
        .settings_schema()
        .validate(
            &SettingsInput::new()
                .with("starting_balance", "10000.00".into())
                .with("access", "read_only".into()),
        )
        .unwrap();

    let refused = fixture
        .adapter
        .place_order(
            &fixture.ctx(),
            fixture.account(),
            OrderRequest::market(eurusd(), OrderSide::Buy, Scaled::new(2, 100)),
        )
        .await;

    assert!(
        matches!(refused, Err(TradeError::ReadOnly { .. })),
        "an investor password is exactly this: full sight, no orders"
    );
    assert!(
        fixture
            .adapter
            .balances(&fixture.ctx(), fixture.account())
            .await
            .is_ok(),
        "and it can still read"
    );
}

#[tokio::test]
async fn a_volume_off_the_brokers_step_is_refused_by_the_adapter_itself() {
    let fixture = Fixture::open().await;

    let refused = fixture
        .adapter
        .place_order(
            &fixture.ctx(),
            fixture.account(),
            OrderRequest::market(eurusd(), OrderSide::Buy, Scaled::new(3, 15)),
        )
        .await;

    assert!(
        refused.is_err(),
        "0.015 lots is off a 0.01 step, and the broker would reject it rather than round it"
    );
    assert!(
        fixture
            .adapter
            .positions(&fixture.ctx(), fixture.account())
            .await
            .unwrap()
            .is_empty(),
        "and nothing reached the book"
    );
}
