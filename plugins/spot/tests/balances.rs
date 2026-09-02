//! Spot: two balances move, nothing is a position, and you cannot sell
//! what you do not hold.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_spot::balances::SpotBook;
use senken_plugin_spot::model::{FeeAsset, Spot};
use senken_sim_core::{FillContext, SettlementModel};
use senken_trade::OrderSide;

fn pair() -> &'static InstrumentId {
    Box::leak(Box::new(
        InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
    ))
}

fn fill(side: OrderSide, qty: i64, price: i64) -> FillContext<'static> {
    FillContext {
        instrument: pair(),
        side,
        quantity: Scaled::new(0, qty),
        price: Scaled::new(0, price),
        now: UnixNanos::from_secs(1_700_000_000).unwrap(),
    }
}

fn spot(fee_bps: i64, fee_asset: FeeAsset) -> Spot {
    Spot {
        base: "BTC".to_owned(),
        quote: "USDT".to_owned(),
        fee_bps,
        fee_asset,
        asset_scale: 0,
    }
}

fn funded(usdt: i64, btc: i64) -> SpotBook {
    let mut book = SpotBook::default();
    book.credit("USDT", usdt);
    book.credit("BTC", btc);
    book
}

#[test]
fn a_buy_moves_quote_into_base_and_opens_no_position() {
    let mut book = funded(100_000, 0);
    let settled = spot(0, FeeAsset::Produced)
        .settle(&mut book, &fill(OrderSide::Buy, 2, 30_000))
        .unwrap();

    assert_eq!(book.get("USDT").free, 40_000, "60 000 of quote was spent");
    assert_eq!(book.get("BTC").free, 2, "and is held as base now");
    assert_eq!(
        settled.realized, 0,
        "the account holds different assets, which is not the same event as closing an \
         exposure — there was never a position to close"
    );
}

#[test]
fn selling_more_base_than_the_account_holds_is_refused_not_shorted() {
    let mut book = funded(0, 1);

    let refused = spot(0, FeeAsset::Produced).settle(&mut book, &fill(OrderSide::Sell, 3, 30_000));

    assert!(
        refused.is_err(),
        "this is the rule that separates spot from every leveraged system: there is nothing to \
         be short of"
    );
    assert_eq!(book.get("BTC").free, 1, "and the balance is untouched");
    assert_eq!(book.get("USDT").free, 0, "nothing was credited either");
}

#[test]
fn the_fee_comes_out_of_the_asset_the_trade_produces() {
    let model = spot(100, FeeAsset::Produced);

    assert_eq!(
        model.fee_currency(OrderSide::Buy),
        "BTC",
        "a buy produces base, so the fee is charged in base"
    );
    assert_eq!(
        model.fee_currency(OrderSide::Sell),
        "USDT",
        "and a sell produces quote"
    );

    let mut book = funded(100_000, 0);
    model
        .settle(&mut book, &fill(OrderSide::Buy, 100, 1_000))
        .unwrap();
    assert_eq!(
        book.get("BTC").free,
        99,
        "1% of the 100 base units bought is taken out of the base received"
    );
}

#[test]
fn a_discount_asset_absorbs_the_fee_instead_of_either_leg() {
    let model = spot(
        100,
        FeeAsset::Discount {
            asset: "BNB".to_owned(),
        },
    );
    let mut book = funded(100_000, 0);
    book.credit("BNB", 50);

    model
        .settle(&mut book, &fill(OrderSide::Buy, 100, 1_000))
        .unwrap();

    assert_eq!(
        book.get("BTC").free,
        100,
        "the base received is untouched by the fee while a discount asset is on — an adapter \
         assuming 'base on a buy' would misreport every fill here"
    );
    assert_eq!(book.get("BNB").free, 49, "the discount asset absorbed it");
}

#[test]
fn a_resting_order_locks_what_it_could_consume_and_cancelling_releases_it() {
    let mut book = funded(100_000, 0);

    book.lock("USDT", 60_000).unwrap();
    assert_eq!(
        book.get("USDT").free,
        40_000,
        "locked balance cannot be spent twice"
    );
    assert_eq!(book.get("USDT").locked, 60_000);
    assert_eq!(
        book.get("USDT").total(),
        100_000,
        "and none of it left the account"
    );

    book.release("USDT", 60_000).unwrap();
    assert_eq!(
        book.get("USDT").free,
        100_000,
        "cancelling releases the whole remaining lock"
    );
    assert_eq!(book.get("USDT").locked, 0);
}

#[test]
fn a_partial_fill_releases_only_the_slice_it_consumed() {
    let mut book = funded(100_000, 0);
    book.lock("USDT", 60_000).unwrap();

    book.spend_locked("USDT", 20_000).unwrap();

    assert_eq!(
        book.get("USDT").locked,
        40_000,
        "the rest stays locked against the order's remaining quantity"
    );
    assert_eq!(
        book.get("USDT").free,
        40_000,
        "and none of it came back as spendable"
    );
}

#[test]
fn locking_more_than_is_free_is_refused() {
    let mut book = funded(100, 0);
    assert!(
        book.lock("USDT", 200).is_err(),
        "an order that could not be covered must not be allowed to rest against balance that \
         is not there"
    );
}

#[test]
fn a_spot_account_reports_no_risk_no_forced_closure_and_no_accrual() {
    let model = spot(0, FeeAsset::Produced);
    let mut book = funded(100_000, 1);
    let marks = senken_sim_core::Marks::new();
    let now = UnixNanos::from_secs(1_700_000_000).unwrap();
    let week = UnixNanos::from_secs(1_700_604_800).unwrap();

    let risk = model.risk(&book, &marks).unwrap();
    assert_eq!(
        risk.margin_level, None,
        "nothing is borrowed, so there is no margin level"
    );
    assert_eq!(risk.breach, None);
    assert!(
        model.enforce(&mut book, &marks, now).unwrap().is_empty(),
        "no venue closes a spot holding on the account's behalf"
    );
    assert_eq!(
        model.accrue(&mut book, &marks, now, week).unwrap(),
        0,
        "and holding an asset for a week costs nothing"
    );
}
