//! Partial close and close-by, and the invariant that separates a partial
//! close from a close-and-reopen.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_mt5_hedging::close::{close_by, close_partial};
use senken_plugin_mt5_hedging::commission::{CommissionModel, commission_for};
use senken_plugin_mt5_hedging::ticket::{HedgingBook, Ticket};
use senken_trade::PositionSide;

fn eurusd() -> InstrumentId {
    InstrumentId::parse("mt5:EURUSD").unwrap()
}

fn ticket(id: u64, side: PositionSide, lots: i64, open: i64) -> Ticket {
    Ticket {
        id,
        instrument: eurusd(),
        side,
        lots: Scaled::new(2, lots),
        open_price: Scaled::new(5, open),
        stop_loss: None,
        take_profit: None,
        swap: -125,
        margin: 0,
        opened_at: UnixNanos::from_secs(1_700_000_000).unwrap(),
    }
}

fn book(tickets: Vec<Ticket>) -> HedgingBook {
    HedgingBook {
        next_ticket: tickets.len() as u64 + 1,
        tickets,
    }
}

#[test]
fn a_partial_close_keeps_the_tickets_number_open_price_and_accrued_swap() {
    let mut held = book(vec![ticket(1, PositionSide::Long, 100, 108_000)]);

    let result = close_partial(&mut held, 1, Scaled::new(2, 40)).unwrap();

    assert_eq!(result.remaining, Some(Scaled::new(2, 60)));
    let left = &held.tickets[0];
    assert_eq!(left.id, 1, "a partial close is not a close and a re-open");
    assert_eq!(
        left.open_price,
        Scaled::new(5, 108_000),
        "so the entry price does not move to the closing price"
    );
    assert_eq!(
        left.swap, -125,
        "and swap already accrued stays against the position that accrued it"
    );
}

#[test]
fn closing_the_whole_volume_removes_the_ticket() {
    let mut held = book(vec![ticket(1, PositionSide::Long, 100, 108_000)]);

    let result = close_partial(&mut held, 1, Scaled::new(2, 100)).unwrap();

    assert_eq!(result.remaining, None);
    assert!(held.tickets.is_empty());
}

#[test]
fn closing_more_than_the_ticket_holds_is_refused_rather_than_clamped() {
    let mut held = book(vec![ticket(1, PositionSide::Long, 100, 108_000)]);

    assert!(
        close_partial(&mut held, 1, Scaled::new(2, 150)).is_err(),
        "clamping would report a close of 1.00 lot as though 1.50 had been asked for and \
         filled, which is a different trade than the one requested"
    );
    assert_eq!(
        held.tickets[0].lots,
        Scaled::new(2, 100),
        "and the ticket is untouched by the refusal"
    );
}

#[test]
fn close_by_settles_the_smaller_volume_and_leaves_the_rest_open() {
    let mut held = book(vec![
        ticket(1, PositionSide::Long, 100, 108_000),
        ticket(2, PositionSide::Short, 60, 108_500),
    ]);

    let matched = close_by(&mut held, 1, 2).unwrap();

    assert_eq!(
        matched,
        Scaled::new(2, 60),
        "the smaller volume is what closes"
    );
    assert_eq!(
        held.tickets.len(),
        1,
        "the fully matched ticket goes and the larger one stays"
    );
    assert_eq!(held.tickets[0].id, 1);
    assert_eq!(
        held.tickets[0].lots,
        Scaled::new(2, 40),
        "with 0.40 lots left under its own number"
    );
}

#[test]
fn a_position_cannot_be_closed_by_one_on_the_same_side() {
    let mut held = book(vec![
        ticket(1, PositionSide::Long, 100, 108_000),
        ticket(2, PositionSide::Long, 60, 108_500),
    ]);

    assert!(
        close_by(&mut held, 1, 2).is_err(),
        "there is nothing to close a position by except its opposite"
    );
    assert_eq!(held.tickets.len(), 2, "and neither ticket is touched");
}

#[test]
fn commission_is_charged_per_lot_and_is_never_a_credit() {
    let charged = commission_for(
        CommissionModel::PerLot { amount: 700 },
        Scaled::new(2, 250),
        Scaled::new(5, 108_000),
        100_000,
    )
    .unwrap();

    assert_eq!(charged, 1_750, "2.50 lots at 7.00 a lot is 17.50");
    assert!(
        charged >= 0,
        "a negative commission would quietly pay the trader for trading"
    );
}

#[test]
fn a_broker_charging_no_commission_charges_none() {
    assert_eq!(
        commission_for(
            CommissionModel::None,
            Scaled::new(2, 250),
            Scaled::new(5, 108_000),
            100_000
        )
        .unwrap(),
        0,
        "the cost is in the spread instead, which is how many retail accounts are priced"
    );
}
