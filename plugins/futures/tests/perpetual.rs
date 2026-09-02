//! Liquidation, funding, and the two mode choices that change what both
//! mean.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_futures::bracket::{Bracket, BracketTable};
use senken_plugin_futures::funding::{FundingTerms, funding_for, intervals_crossed};
use senken_plugin_futures::model::{Futures, FuturesBook, PositionMode};
use senken_sim_core::risk::RiskBreach;
use senken_sim_core::{FillContext, Marks, SettlementModel};
use senken_trade::{MarginMode, OrderSide, PositionSide};

const CASH: u32 = 8;
const HOUR: i64 = 3_600 * 1_000_000_000;

fn money(units: i64) -> i64 {
    units * 10_i64.pow(CASH)
}

fn pair() -> &'static InstrumentId {
    Box::leak(Box::new(
        InstrumentId::parse("binance-perp:BTCUSDT").unwrap(),
    ))
}

fn at(hours: i64) -> UnixNanos {
    UnixNanos::from_nanos(hours * HOUR)
}

fn fill(side: OrderSide, qty: i64, price: i64) -> FillContext<'static> {
    FillContext {
        instrument: pair(),
        side,
        quantity: Scaled::new(0, qty),
        price: Scaled::new(0, price),
        now: at(0),
    }
}

fn table() -> BracketTable {
    // A venue's own shape, supplied as data. These are this test's
    // numbers, not a table read from memory of a venue's page.
    BracketTable {
        tiers: vec![
            Bracket {
                notional_cap: money(50_000),
                maintenance_bps: 40,
                maintenance_amount: 0,
                max_leverage: 125,
            },
            Bracket {
                notional_cap: money(250_000),
                maintenance_bps: 50,
                maintenance_amount: money(50),
                max_leverage: 100,
            },
        ],
    }
}

fn futures(mode: PositionMode, brackets: BracketTable) -> Futures {
    Futures {
        position_mode: mode,
        margin_mode: MarginMode::Isolated,
        leverage: 10,
        fee_bps: 0,
        brackets,
        funding: FundingTerms {
            interval_nanos: 8 * HOUR,
            rate_bps: 1,
        },
    }
}

fn marks(price: i64) -> Marks {
    let mut marks = Marks::new();
    marks.insert(pair().to_string(), Scaled::new(0, price));
    marks
}

fn book() -> FuturesBook {
    FuturesBook {
        wallet: money(10_000),
        ..FuturesBook::default()
    }
}

#[test]
fn hedge_mode_holds_a_long_and_a_short_at_once_and_one_way_does_not() {
    let mut hedged = book();
    let hedge = futures(PositionMode::Hedge, table());
    hedge
        .settle(&mut hedged, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    hedge
        .settle(&mut hedged, &fill(OrderSide::Sell, 1, 30_000))
        .unwrap();
    assert_eq!(
        hedged.positions.len(),
        2,
        "hedge mode is configuration, and this is what it configures"
    );

    let mut netted = book();
    let one_way = futures(PositionMode::OneWay, table());
    one_way
        .settle(&mut netted, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    one_way
        .settle(&mut netted, &fill(OrderSide::Sell, 1, 30_000))
        .unwrap();
    assert!(
        netted.positions.is_empty(),
        "the same two fills close each other out in one-way mode"
    );
}

#[test]
fn maintenance_margin_steps_up_with_notional_from_the_supplied_table() {
    let brackets = table();
    let small = brackets.maintenance_margin(money(10_000)).unwrap();
    let large = brackets.maintenance_margin(money(200_000)).unwrap();

    // 10 000 × 0.40% = 40; 200 000 × 0.50% − 50 = 950.
    assert_eq!(small, money(40));
    assert_eq!(large, money(950));
    assert!(
        large > small,
        "a bigger position is held to a higher requirement, which is the whole reason the \
         table is tiered rather than one rate"
    );
}

#[test]
fn an_account_with_no_bracket_table_reports_no_liquidation_price_at_all() {
    let mut held = book();
    let no_table = futures(PositionMode::OneWay, BracketTable::default());
    no_table
        .settle(&mut held, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    let position = held.positions.values().next().unwrap();

    assert_eq!(
        no_table.liquidation_price(position).unwrap(),
        None,
        "a trader shown a liquidation price believes it, so an unknown one is reported as \
         unknown rather than estimated"
    );
}

#[test]
fn a_liquidation_price_is_labelled_as_derived_because_the_formula_is_one() {
    let mut held = book();
    let model = futures(PositionMode::OneWay, table());
    model
        .settle(&mut held, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    let position = held.positions.values().next().unwrap();

    let liquidation = model.liquidation_price(position).unwrap().unwrap();
    assert!(
        liquidation.derived,
        "the formula is derived from the venue's equity-versus-maintenance identity, not \
         transcribed from its published expression, and the label travels with the number"
    );
    assert!(
        liquidation.price.value < 30_000,
        "a long liquidates below its entry: got {}",
        liquidation.price.value
    );
}

#[test]
fn more_margin_moves_a_liquidation_further_from_the_entry() {
    let model = futures(PositionMode::OneWay, table());
    let mut thin = book();
    model
        .settle(&mut thin, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    let lean = model
        .liquidation_price(thin.positions.values().next().unwrap())
        .unwrap()
        .unwrap();

    let mut fat = book();
    let mut ten_x_margin = model.clone();
    ten_x_margin.leverage = 1;
    ten_x_margin
        .settle(&mut fat, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    let padded = ten_x_margin
        .liquidation_price(fat.positions.values().next().unwrap())
        .unwrap()
        .unwrap();

    assert!(
        padded.price.value < lean.price.value,
        "posting ten times the margin must move a long's liquidation further below its entry, \
         not nearer: {} against {}",
        padded.price.value,
        lean.price.value
    );
}

#[test]
fn liquidation_triggers_on_equity_below_maintenance_and_not_on_a_percentage() {
    let model = futures(PositionMode::OneWay, table());
    // Enough to cover the 0.40% maintenance on a 30 000 notional, which
    // is 120 — the account starts sound and is broken by the market, not
    // by being underfunded from the first tick.
    let mut held = FuturesBook {
        wallet: money(3_000),
        ..FuturesBook::default()
    };
    model
        .settle(&mut held, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();

    let sound = model.risk(&held, &marks(30_000)).unwrap();
    assert_eq!(
        sound.breach, None,
        "at entry the position is comfortably covered"
    );

    let collapsed = marks(20_000);
    let broken = model.risk(&held, &collapsed).unwrap();
    assert_eq!(
        broken.breach,
        Some(RiskBreach::ForcedClosure),
        "equity below maintenance margin is the trigger both venues state, and a futures \
         venue has no margin-call step between fine and closed"
    );

    let closed = model.enforce(&mut held.clone(), &collapsed, at(1)).unwrap();
    assert!(!closed.is_empty(), "and enforcing it closes the position");
}

#[test]
fn a_long_pays_funding_and_a_short_receives_it_when_the_rate_is_positive() {
    let terms = FundingTerms {
        interval_nanos: 8 * HOUR,
        rate_bps: 10,
    };
    let long = funding_for(terms, PositionSide::Long, money(30_000), 1).unwrap();
    let short = funding_for(terms, PositionSide::Short, money(30_000), 1).unwrap();

    assert!(long < 0, "a long pays a positive rate");
    assert!(short > 0, "and a short is paid it");
    assert_eq!(
        long, -short,
        "funding is a transfer between the two sides, not a fee to the exchange, so it nets \
         to nothing"
    );
}

#[test]
fn funding_accrues_once_per_interval_crossed_and_not_per_read() {
    let terms = FundingTerms {
        interval_nanos: 8 * HOUR,
        rate_bps: 10,
    };
    assert_eq!(
        intervals_crossed(terms, at(0), at(1)),
        0,
        "reading an hour later crosses no funding instant"
    );
    assert_eq!(intervals_crossed(terms, at(0), at(8)), 1);
    assert_eq!(
        intervals_crossed(terms, at(0), at(24)),
        3,
        "a position left for a day funds three times at an eight-hour interval"
    );
}

#[test]
fn a_shorter_interval_funds_more_often_over_the_same_stretch() {
    let eight_hourly = FundingTerms {
        interval_nanos: 8 * HOUR,
        rate_bps: 10,
    };
    let hourly = FundingTerms {
        interval_nanos: HOUR,
        rate_bps: 10,
    };

    assert_eq!(intervals_crossed(eight_hourly, at(0), at(24)), 3);
    assert_eq!(
        intervals_crossed(hourly, at(0), at(24)),
        24,
        "venues shorten the interval during volatility, so a hard-coded eight hours would \
         silently undercount by eight to one"
    );
}

#[test]
fn funding_settles_against_the_balance_with_no_order_and_no_fill() {
    let model = futures(PositionMode::OneWay, table());
    let mut held = book();
    model
        .settle(&mut held, &fill(OrderSide::Buy, 1, 30_000))
        .unwrap();
    let before = held.wallet;

    let charged = model
        .accrue(&mut held, &marks(30_000), at(0), at(8))
        .unwrap();

    assert!(charged < 0, "the long pays");
    assert_eq!(
        held.wallet,
        before + charged,
        "and it lands straight on the balance"
    );
}
