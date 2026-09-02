//! Each test states one MetaTrader 5 mechanic as a sentence and checks it
//! against a number derived from `MetaQuotes`' own published formula, not
//! from running this code and writing down what it printed.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_mt5_hedging::margin::{AccountFigures, CalcMode, SymbolMargin, margin_for};
use senken_plugin_mt5_hedging::ticket::{HedgingBook, Ticket};
use senken_trade::PositionSide;

const CASH: u8 = 8;

fn eurusd() -> InstrumentId {
    InstrumentId::parse("mt5:EURUSD").unwrap()
}

fn lots(hundredths: i64) -> Scaled {
    Scaled::new(2, hundredths)
}

fn ticket(id: u64, side: PositionSide, open: i64) -> Ticket {
    Ticket {
        id,
        instrument: eurusd(),
        side,
        lots: lots(100),
        open_price: Scaled::new(5, open),
        stop_loss: None,
        take_profit: None,
        swap: 0,
        margin: 0,
        opened_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
    }
}

#[test]
fn forex_margin_is_lots_times_contract_size_over_leverage() {
    // 1.00 lot of a 100 000-unit contract at 1:100 is 1 000.00 of margin.
    let charged = margin_for(
        SymbolMargin {
            mode: CalcMode::Forex,
            contract_size: 100_000,
            leverage: 100,
            percentage: 100,
        },
        lots(100),
        Scaled::new(5, 108_500),
    )
    .unwrap();

    assert_eq!(
        charged,
        1_000 * 10_i64.pow(u32::from(CASH)),
        "forex margin does not depend on the price at all — only lots, contract size and leverage"
    );
}

#[test]
fn forex_without_leverage_charges_the_whole_contract() {
    let charged = margin_for(
        SymbolMargin {
            mode: CalcMode::ForexNoLeverage,
            contract_size: 100_000,
            leverage: 100,
            percentage: 100,
        },
        lots(100),
        Scaled::new(5, 108_500),
    )
    .unwrap();

    assert_eq!(
        charged,
        100_000 * 10_i64.pow(u32::from(CASH)),
        "the no-leverage mode ignores the account's leverage rather than applying it"
    );
}

#[test]
fn a_cfd_is_margined_on_its_market_price_and_a_forex_pair_is_not() {
    let terms = SymbolMargin {
        mode: CalcMode::Cfd,
        contract_size: 100,
        leverage: 100,
        percentage: 1,
    };
    let cheap = margin_for(terms, lots(100), Scaled::new(2, 100_000)).unwrap();
    let dear = margin_for(terms, lots(100), Scaled::new(2, 200_000)).unwrap();

    assert_eq!(
        dear,
        cheap * 2,
        "a CFD's margin scales with the market price, which is exactly what separates it from \
         the forex formula"
    );
}

#[test]
fn margin_level_is_equity_over_margin_used_as_a_percentage() {
    let figures = AccountFigures::new(
        10_000 * 10_i64.pow(u32::from(CASH)),
        -2_000 * 10_i64.pow(u32::from(CASH)),
        4_000 * 10_i64.pow(u32::from(CASH)),
    );

    assert_eq!(
        figures.equity,
        8_000 * 10_i64.pow(u32::from(CASH)),
        "equity is balance plus unrealised profit, so a losing book lowers it"
    );
    assert_eq!(
        figures.free_margin,
        4_000 * 10_i64.pow(u32::from(CASH)),
        "free margin is equity less margin held"
    );
    // 100 × 8 000 / 4 000 = 200%.
    assert_eq!(
        figures.margin_level(),
        Some(Scaled::new(2, 20_000)),
        "margin level is the percentage margin call and stop out are both measured against"
    );
}

#[test]
fn an_account_holding_no_margin_has_no_margin_level_rather_than_zero_or_infinity() {
    let figures = AccountFigures::new(10_000, 0, 0);

    assert_eq!(
        figures.margin_level(),
        None,
        "an account with nothing open has no margin level; a zero would read as fully margin \
         called and an infinity is not a number a terminal can show"
    );
}

#[test]
fn stop_out_closes_the_biggest_loser_and_leaves_a_profitable_hedge_alone() {
    let book = HedgingBook {
        tickets: vec![
            ticket(1, PositionSide::Long, 110_000),
            ticket(2, PositionSide::Short, 108_000),
            ticket(3, PositionSide::Long, 109_000),
        ],
        next_ticket: 4,
    };

    // Deliberately *not* the first ticket: ticket 1 is a small loser and
    // ticket 3 the deep one, so "oldest" and "biggest loser" give
    // different answers and the test can tell which rule ran.
    let unrealized = |ticket: &Ticket| {
        Some(match ticket.id {
            1 => -100,
            2 => 300,
            3 => -500,
            _ => 0,
        })
    };

    assert_eq!(
        book.biggest_loser(&unrealized).map(|ticket| ticket.id),
        Some(3),
        "a stop out closes the largest losing position first, not the oldest ticket"
    );
}

#[test]
fn a_stop_out_has_nothing_to_close_when_every_position_is_in_profit() {
    let book = HedgingBook {
        tickets: vec![ticket(1, PositionSide::Long, 110_000)],
        next_ticket: 2,
    };

    assert_eq!(
        book.biggest_loser(&|_| Some(400)).map(|ticket| ticket.id),
        None,
        "however low the margin level has fallen, a stop out closes losing positions and there \
         are none"
    );
}

#[test]
fn one_symbol_holds_a_long_and_a_short_at_once() {
    let book = HedgingBook {
        tickets: vec![
            ticket(1, PositionSide::Long, 110_000),
            ticket(2, PositionSide::Short, 108_000),
        ],
        next_ticket: 3,
    };

    let sides: Vec<_> = book.on(&eurusd()).map(|ticket| ticket.side).collect();
    assert_eq!(
        sides,
        vec![PositionSide::Long, PositionSide::Short],
        "locking one symbol long and short is the whole reason a trader chooses a hedging \
         account, so both must survive as separate tickets"
    );
}
