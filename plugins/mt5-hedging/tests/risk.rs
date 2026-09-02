//! Margin call and stop out: two different events, with the numbers taken
//! from MetaTrader's own definitions rather than from running this code.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_mt5_hedging::account::{Account, StopLevels, SymbolTerms, ticket_profit};
use senken_plugin_mt5_hedging::margin::{CalcMode, SymbolMargin};
use senken_plugin_mt5_hedging::ticket::{HedgingBook, Ticket};
use senken_sim_core::risk::RiskBreach;
use senken_trade::PositionSide;

const CASH: u32 = 8;

fn money(units: i64) -> i64 {
    units * 10_i64.pow(CASH)
}

fn eurusd() -> InstrumentId {
    InstrumentId::parse("mt5:EURUSD").unwrap()
}

fn levels() -> StopLevels {
    StopLevels {
        // A common retail pair, but read from the broker rather than fixed
        // here: these are this test's broker, not MetaTrader's rule.
        margin_call: Some(Scaled::new(2, 10_000)),
        stop_out: Some(Scaled::new(2, 5_000)),
    }
}

fn symbol(mark: i64) -> SymbolTerms {
    SymbolTerms {
        margin: SymbolMargin {
            mode: CalcMode::Forex,
            contract_size: 100_000,
            leverage: 100,
            percentage: 100,
        },
        mark: Some(Scaled::new(5, mark)),
    }
}

fn ticket(id: u64, side: PositionSide, open: i64, margin: i64) -> Ticket {
    Ticket {
        id,
        instrument: eurusd(),
        side,
        lots: Scaled::new(2, 100),
        open_price: Scaled::new(5, open),
        stop_loss: None,
        take_profit: None,
        swap: 0,
        margin,
        opened_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
    }
}

fn account(cash: i64, tickets: Vec<Ticket>) -> Account {
    Account {
        cash,
        book: HedgingBook {
            next_ticket: tickets.len() as u64 + 1,
            tickets,
        },
    }
}

#[test]
fn profit_is_the_price_difference_times_contract_size_times_lots() {
    let long = ticket(1, PositionSide::Long, 108_000, money(1_000));
    // 1.00 lot of 100 000 units moving 0.00500 is 500.00.
    let profit = ticket_profit(&long, Scaled::new(5, 108_500), 100_000).unwrap();

    assert_eq!(profit, money(500), "MetaTrader's own profit formula");

    let short = ticket(2, PositionSide::Short, 108_000, money(1_000));
    assert_eq!(
        ticket_profit(&short, Scaled::new(5, 108_500), 100_000).unwrap(),
        money(-500),
        "the same move is the mirror image for a short, which is what makes a locked hedge flat"
    );
}

#[test]
fn a_margin_call_blocks_opening_and_closes_nothing() {
    // 1 000 margin, equity 900 → level 90%, under the 100% call and above
    // the 50% stop out.
    let mut held = account(
        money(1_000),
        vec![ticket(1, PositionSide::Long, 108_000, money(1_000))],
    );
    let terms = |_: &InstrumentId| Some(symbol(107_900));

    let risk = held.risk(levels(), &terms).unwrap();
    assert_eq!(
        risk.breach,
        Some(RiskBreach::OpeningBlocked),
        "under the call threshold and above the stop out is a margin call"
    );

    let closed = held
        .apply_stop_out(
            levels(),
            &terms,
            UnixNanos::from_secs(1_700_000_000).unwrap(),
        )
        .unwrap();
    assert!(
        closed.is_empty(),
        "a margin call closes nothing — that is the whole difference between it and a stop out"
    );
    assert_eq!(
        held.book.tickets.len(),
        1,
        "and the position is still open afterwards"
    );
}

#[test]
fn a_stop_out_closes_the_biggest_loser_and_stops_once_the_level_recovers() {
    // Two losing tickets and one winning one. Equity is far under the
    // stop-out threshold until the deepest loser goes.
    let mut held = account(
        money(2_000),
        vec![
            ticket(1, PositionSide::Long, 109_000, money(1_000)),
            ticket(2, PositionSide::Long, 112_000, money(1_000)),
            ticket(3, PositionSide::Short, 112_000, money(1_000)),
        ],
    );
    let terms = |_: &InstrumentId| Some(symbol(108_000));

    let before = held.risk(levels(), &terms).unwrap();
    assert_eq!(
        before.breach,
        Some(RiskBreach::ForcedClosure),
        "the account starts under the stop-out threshold"
    );

    let closed = held
        .apply_stop_out(
            levels(),
            &terms,
            UnixNanos::from_secs(1_700_000_000).unwrap(),
        )
        .unwrap();

    assert_eq!(
        closed.first().map(|forced| forced.position.as_str()),
        Some("2"),
        "ticket 2 is 4 000 down against ticket 1's 1 000, so it goes first"
    );
    let after = held.risk(levels(), &terms).unwrap();
    assert_ne!(
        after.breach,
        Some(RiskBreach::ForcedClosure),
        "closing stops as soon as the level recovers"
    );
    assert_eq!(
        closed.len(),
        1,
        "and it stops *there*: one close was enough, so liquidating the rest would have taken \
         positions the account no longer needed to give up"
    );
    let left: Vec<_> = held.book.tickets.iter().map(|ticket| ticket.id).collect();
    assert_eq!(
        left,
        vec![1, 3],
        "the smaller loser and the profitable hedge both survive"
    );
}

#[test]
fn a_stop_out_closes_nothing_when_every_position_is_in_profit() {
    let mut held = account(
        money(1),
        vec![ticket(1, PositionSide::Long, 108_000, money(1_000))],
    );
    let terms = |_: &InstrumentId| Some(symbol(108_500));

    let closed = held
        .apply_stop_out(
            levels(),
            &terms,
            UnixNanos::from_secs(1_700_000_000).unwrap(),
        )
        .unwrap();

    assert!(
        closed.is_empty(),
        "a stop out closes losing positions, and there are none however low the level is"
    );
}

#[test]
fn an_account_with_nothing_open_is_in_no_breach_at_all() {
    let flat = account(money(10_000), Vec::new());
    let risk = flat
        .risk(levels(), &|_: &InstrumentId| Some(symbol(108_000)))
        .unwrap();

    assert_eq!(
        risk.margin_level, None,
        "no margin held, so no margin level"
    );
    assert_eq!(
        risk.breach, None,
        "and an account holding nothing is neither margin called nor stopped out, however the \
         thresholds are set"
    );
}
