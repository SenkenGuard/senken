//! A resting spot order holds what it could consume, so one balance is
//! never promised to two orders.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::{Instrument, InstrumentId};
use senken_plugin_spot::venue::SpotVenue;
use senken_sim_core::SimAdapter;
use senken_storage::Storage;
use senken_trade::{
    AccountRef, InstrumentSource, MarkPrice, MarkPriceSource, OrderKind, OrderRequest, OrderSide,
    SettingsInput, SettingsValues, TradeAccountId, TradeAdapter, TradeContext, TradeError,
};
use tempfile::TempDir;

struct Mark;

#[async_trait::async_trait]
impl MarkPriceSource for Mark {
    async fn mark_price(&self, _id: &InstrumentId) -> Result<Option<MarkPrice>, TradeError> {
        Ok(Some(MarkPrice {
            price: Scaled::new(0, 100),
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
    InstrumentId::parse("binance-spot:BTCUSDT").unwrap()
}

struct Fixture {
    _dir: TempDir,
    adapter: SimAdapter<SpotVenue>,
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
        let adapter = SimAdapter::new(SpotVenue, storage);
        let settings = adapter
            .settings_schema()
            .validate(
                &SettingsInput::new()
                    .with("base_asset", "BTC".into())
                    .with("quote_asset", "USDT".into())
                    // 1 000.00 of quote, kept at whole units so the
                    // arithmetic in the assertions is the venue's own.
                    .with("starting_quote", "1000.00".into())
                    .with("fee_bps", 0.into())
                    .with("asset_scale", 0.into()),
            )
            .unwrap();
        let fixture = Self {
            _dir: dir,
            adapter,
            mark: Mark,
            catalog: Catalog,
            id: TradeAccountId::new(),
            settings,
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
            label: "Spot",
            settings: &self.settings,
        }
    }

    /// A buy limit far below the market, so it rests.
    async fn resting_buy(&self, price: i64, qty: i64) -> Result<senken_trade::Order, TradeError> {
        let mut request = OrderRequest::market(pair(), OrderSide::Buy, Scaled::new(0, qty));
        request.kind = OrderKind::Limit {
            price: Scaled::new(0, price),
        };
        self.adapter
            .place_order(&self.ctx(), self.account(), request)
            .await
    }

    /// Free and locked for one asset, in whole units.
    ///
    /// The account reports balances at the shared cash scale, which is
    /// what a client renders; these tests are about *how much*, so they
    /// read it back in the units the settings declared.
    async fn held(&self, asset: &str) -> (i64, i64) {
        let balances = self
            .adapter
            .balances(&self.ctx(), self.account())
            .await
            .unwrap();
        let row = balances
            .assets
            .iter()
            .find(|held| held.asset == asset)
            .cloned()
            .unwrap();
        let whole =
            |value: senken_core::decimal::Scaled| value.value / 10_i64.pow(u32::from(value.scale));
        (whole(row.available), whole(row.reserved))
    }

    async fn quote(&self) -> (i64, i64) {
        self.held("USDT").await
    }
}

#[tokio::test]
async fn a_resting_buy_moves_quote_out_of_free_and_into_locked() {
    let fixture = Fixture::open().await;

    // 10 at 50 could consume 500 of the 1 000 held.
    fixture.resting_buy(50, 10).await.unwrap();

    let (free, locked) = fixture.quote().await;
    assert_eq!(locked, 500, "the order holds exactly what it could consume");
    assert_eq!(free, 500, "and that much can no longer be spent");
}

#[tokio::test]
async fn the_same_balance_cannot_be_promised_to_two_orders() {
    let fixture = Fixture::open().await;

    fixture.resting_buy(50, 10).await.unwrap();
    // A glance at the 1 000 total says this is affordable. It is not:
    // half of it is already spoken for.
    let second = fixture.resting_buy(50, 15).await;

    assert!(
        second.is_err(),
        "without a lock the account would have two orders resting against one balance, and \
         whichever filled second would overdraw it"
    );
    let (free, locked) = fixture.quote().await;
    assert_eq!(
        locked, 500,
        "and the refusal left the first order's hold alone"
    );
    assert_eq!(free, 500);
}

#[tokio::test]
async fn cancelling_gives_the_whole_hold_back() {
    let fixture = Fixture::open().await;
    let order = fixture.resting_buy(50, 10).await.unwrap();

    fixture
        .adapter
        .cancel_order(&fixture.ctx(), fixture.account(), &order.id)
        .await
        .unwrap();

    let (free, locked) = fixture.quote().await;
    assert_eq!(locked, 0);
    assert_eq!(free, 1_000, "every unit of it, spendable again");
}

#[tokio::test]
async fn a_fill_spends_the_balance_once_and_not_twice() {
    let fixture = Fixture::open().await;

    // At the mark of 100, a buy limit of 150 is already reachable, so this
    // rests and then fills on the next read.
    fixture.resting_buy(150, 2).await.unwrap();

    let (free, locked) = fixture.quote().await;
    assert_eq!(locked, 0, "the hold went when the order did");
    assert_eq!(
        free, 700,
        "2 at 150 is 300, charged once — leaving the hold in place as well would have taken \
         600 for one order"
    );
}

#[tokio::test]
async fn a_resting_sell_holds_base_rather_than_quote() {
    let fixture = Fixture::open().await;
    // Buy some base at the market first, so there is something to sell.
    fixture
        .adapter
        .place_order(
            &fixture.ctx(),
            fixture.account(),
            OrderRequest::market(pair(), OrderSide::Buy, Scaled::new(0, 5)),
        )
        .await
        .unwrap();

    // Above the mark of 100, so the market has not come up to it and the
    // order rests. Below it, a limit sell is immediately reachable and
    // fills instead — which is a different test.
    let mut request = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 3));
    request.kind = OrderKind::Limit {
        price: Scaled::new(0, 150),
    };
    fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await
        .unwrap();

    let (free, locked) = fixture.held("BTC").await;
    assert_eq!(
        locked, 3,
        "a sell holds the base it would deliver, not the quote it would receive"
    );
    assert_eq!(free, 2);
}

#[tokio::test]
async fn a_sell_of_base_the_account_does_not_hold_is_refused_before_it_rests() {
    let fixture = Fixture::open().await;

    let mut request = OrderRequest::market(pair(), OrderSide::Sell, Scaled::new(0, 3));
    request.kind = OrderKind::Limit {
        price: Scaled::new(0, 150),
    };
    let refused = fixture
        .adapter
        .place_order(&fixture.ctx(), fixture.account(), request)
        .await;

    assert!(
        refused.is_err(),
        "an order that could not be covered must not be allowed to wait against balance that \
         is not there"
    );
    assert!(
        fixture
            .adapter
            .orders(
                &fixture.ctx(),
                fixture.account(),
                senken_trade::OrderFilter::Open
            )
            .await
            .unwrap()
            .is_empty(),
        "and the refusal leaves no half-placed order behind"
    );
}
