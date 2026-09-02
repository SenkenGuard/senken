//! The hedging account driven purely through the kernel's seam, with no
//! knowledge of which system is behind it.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_mt5_hedging::account::{Account, StopLevels};
use senken_plugin_mt5_hedging::commission::CommissionModel;
use senken_plugin_mt5_hedging::margin::{CalcMode, SymbolMargin};
use senken_plugin_mt5_hedging::model::Hedging;
use senken_plugin_mt5_hedging::swap::{SwapMode, SwapTerms};
use senken_sim_core::{FillContext, Marks, SettlementModel};
use senken_trade::{OrderSide, PositionSide};

const MONDAY: i64 = 1_767_571_200;
const DAY: i64 = 86_400;

fn at(secs: i64) -> UnixNanos {
    UnixNanos::from_secs(secs).unwrap()
}

fn eurusd() -> InstrumentId {
    InstrumentId::parse("mt5:EURUSD").unwrap()
}

fn model() -> Hedging {
    Hedging {
        margin: SymbolMargin {
            mode: CalcMode::Forex,
            contract_size: 100_000,
            leverage: 100,
            percentage: 100,
        },
        levels: StopLevels {
            margin_call: Some(Scaled::new(2, 10_000)),
            stop_out: Some(Scaled::new(2, 5_000)),
        },
        swap: SwapTerms {
            mode: SwapMode::CurrencyDeposit,
            long_rate: -700,
            short_rate: 200,
            rate_scale: 2,
            rollover3_weekday: Some(2),
            contract_size: 100_000,
        },
        commission: CommissionModel::None,
    }
}

fn marks(price: i64) -> Marks {
    let mut marks = Marks::new();
    marks.insert(eurusd().to_string(), Scaled::new(5, price));
    marks
}

fn fill(side: OrderSide, lots: i64, price: i64) -> FillContext<'static> {
    // Leaked so the fixture can hand out a `'static` instrument without
    // threading a lifetime through every helper in this file.
    let instrument: &'static InstrumentId = Box::leak(Box::new(eurusd()));
    FillContext {
        instrument,
        side,
        quantity: Scaled::new(2, lots),
        price: Scaled::new(5, price),
        now: at(MONDAY),
    }
}

#[test]
fn settling_two_opposite_fills_leaves_two_tickets_not_one_net_position() {
    let mut book = Account {
        cash: 10_000 * 100_000_000,
        book: senken_plugin_mt5_hedging::ticket::HedgingBook {
            tickets: Vec::new(),
            next_ticket: 1,
        },
    };
    let hedging = model();

    hedging
        .settle(&mut book, &fill(OrderSide::Buy, 100, 108_000))
        .unwrap();
    hedging
        .settle(&mut book, &fill(OrderSide::Sell, 100, 108_000))
        .unwrap();

    assert_eq!(
        book.book.tickets.len(),
        2,
        "a netting model settling the same two fills would hold one position, or none — this \
         is the seam carrying a real difference rather than a shared implementation"
    );
    let sides: Vec<_> = book.book.tickets.iter().map(|t| t.side).collect();
    assert_eq!(sides, vec![PositionSide::Long, PositionSide::Short]);
}

#[test]
fn a_fill_that_opens_a_ticket_realises_nothing() {
    let mut book = Account::default();
    book.book.next_ticket = 1;

    let settled = model()
        .settle(&mut book, &fill(OrderSide::Buy, 100, 108_000))
        .unwrap();

    assert_eq!(
        settled.realized, 0,
        "on a hedging account a profit is realised only when a ticket is closed, and no fill \
         ever closes one implicitly"
    );
}

#[test]
fn risk_and_enforcement_reach_the_same_stop_out_through_the_seam() {
    let mut book = Account {
        cash: 2_000 * 100_000_000,
        book: senken_plugin_mt5_hedging::ticket::HedgingBook {
            tickets: Vec::new(),
            next_ticket: 1,
        },
    };
    let hedging = model();
    hedging
        .settle(&mut book, &fill(OrderSide::Buy, 100, 112_000))
        .unwrap();
    hedging
        .settle(&mut book, &fill(OrderSide::Buy, 100, 109_000))
        .unwrap();

    let marks = marks(108_000);
    let risk = hedging.risk(&book, &marks).unwrap();
    assert!(
        risk.breach.is_some(),
        "the account is deep enough under water to have breached something"
    );

    let closed = hedging.enforce(&mut book, &marks, at(MONDAY)).unwrap();
    assert!(
        !closed.is_empty(),
        "and enforcing that breach through the seam closes a position"
    );
    assert_eq!(
        closed[0].position, "1",
        "the biggest loser first, which the seam carries without the caller knowing the rule"
    );
}

#[test]
fn accruing_over_a_week_charges_the_week_and_not_one_night() {
    let mut book = Account::default();
    book.book.next_ticket = 1;
    let hedging = model();
    hedging
        .settle(&mut book, &fill(OrderSide::Buy, 100, 108_000))
        .unwrap();

    let one_night = {
        let mut nightly = book.clone();
        hedging
            .accrue(&mut nightly, &marks(108_000), at(MONDAY), at(MONDAY + DAY))
            .unwrap()
    };
    let a_week = hedging
        .accrue(&mut book, &marks(108_000), at(MONDAY), at(MONDAY + 7 * DAY))
        .unwrap();

    assert!(
        a_week < one_night,
        "swap is charged against a long here, so a week costs more than a night — a book left \
         unread for a week must accrue the week ({a_week} against {one_night})"
    );
    assert_eq!(
        book.book.tickets[0].swap, a_week,
        "and it lands on the ticket that accrued it"
    );
}

#[test]
fn accruing_across_no_rollover_charges_nothing() {
    let mut book = Account::default();
    book.book.next_ticket = 1;
    let hedging = model();
    hedging
        .settle(&mut book, &fill(OrderSide::Buy, 100, 108_000))
        .unwrap();

    let charged = hedging
        .accrue(&mut book, &marks(108_000), at(MONDAY), at(MONDAY + 3_600))
        .unwrap();

    assert_eq!(
        charged, 0,
        "reading an account twice in an afternoon must not charge it swap twice"
    );
}
