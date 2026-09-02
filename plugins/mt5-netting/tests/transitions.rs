//! The four transitions, with the worked numbers taken from MetaTrader's
//! own published examples rather than from running this code.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_plugin_mt5_netting::book::{NettingBook, Transition};
use senken_plugin_mt5_netting::model::Netting;
use senken_sim_core::{FillContext, SettlementModel};
use senken_trade::{OrderSide, PositionSide};

fn btc() -> &'static InstrumentId {
    Box::leak(Box::new(InstrumentId::parse("mt5:BTCUSD").unwrap()))
}

fn at(secs: i64) -> UnixNanos {
    UnixNanos::from_secs(secs).unwrap()
}

fn fill(side: OrderSide, lots: i64, price: i64, secs: i64) -> FillContext<'static> {
    FillContext {
        instrument: btc(),
        side,
        quantity: Scaled::new(1, lots),
        price: Scaled::new(0, price),
        now: at(secs),
    }
}

fn book() -> NettingBook {
    NettingBook {
        next_ticket: 1,
        ..NettingBook::default()
    }
}

fn model() -> Netting {
    Netting { fee_bps: 0 }
}

#[test]
fn a_first_fill_opens_the_symbols_one_position() {
    let mut held = book();
    let (_, transition) = model()
        .apply(&mut held, &fill(OrderSide::Buy, 5, 104_000, 1_000))
        .unwrap();

    assert_eq!(transition, Transition::Opened);
    assert_eq!(held.positions.len(), 1);
    let position = &held.positions[btc()];
    assert_eq!(
        position.ticket, position.identifier,
        "a fresh position's identifier is its opening ticket"
    );
}

#[test]
fn a_same_direction_fill_averages_the_entry_and_keeps_the_ticket() {
    let mut held = book();
    let netting = model();
    netting
        .apply(&mut held, &fill(OrderSide::Buy, 5, 104_000, 1_000))
        .unwrap();
    let ticket_before = held.positions[btc()].ticket;

    let (_, transition) = netting
        .apply(&mut held, &fill(OrderSide::Buy, 5, 104_100, 2_000))
        .unwrap();

    assert_eq!(transition, Transition::Added);
    let position = &held.positions[btc()];
    // (0.5 × 104 000 + 0.5 × 104 100) / 1.0 = 104 050.
    assert_eq!(
        position.entry,
        Scaled::new(0, 104_050),
        "MetaTrader's own P = (N1×P1 + N2×P2) / (N1 + N2)"
    );
    assert_eq!(position.volume, Scaled::new(1, 10));
    assert_eq!(
        position.ticket, ticket_before,
        "an add keeps the ticket, which is why deal history stays grouped across it"
    );
}

#[test]
fn a_partial_reduce_books_profit_and_leaves_the_entry_alone() {
    let mut held = book();
    let netting = model();
    netting
        .apply(&mut held, &fill(OrderSide::Buy, 10, 104_000, 1_000))
        .unwrap();

    let (settled, transition) = netting
        .apply(&mut held, &fill(OrderSide::Sell, 4, 104_500, 2_000))
        .unwrap();

    assert_eq!(transition, Transition::Reduced);
    assert!(
        settled.realized > 0,
        "closing 0.4 lots into a rise banks profit"
    );
    let position = &held.positions[btc()];
    assert_eq!(position.volume, Scaled::new(1, 6));
    assert_eq!(
        position.entry,
        Scaled::new(0, 104_000),
        "moving the entry to the closing price would silently rewrite the cost basis of \
         everything still open"
    );
}

#[test]
fn an_exactly_opposite_fill_closes_the_position_flat() {
    let mut held = book();
    let netting = model();
    netting
        .apply(&mut held, &fill(OrderSide::Buy, 10, 104_000, 1_000))
        .unwrap();

    let (_, transition) = netting
        .apply(&mut held, &fill(OrderSide::Sell, 10, 104_500, 2_000))
        .unwrap();

    assert_eq!(transition, Transition::Closed);
    assert!(held.positions.is_empty());
    assert!(
        held.realized > 0,
        "and the profit survives the position that earned it being gone"
    );
}

#[test]
fn a_reversal_changes_the_ticket_but_the_identifier_survives_it() {
    // MetaTrader's own worked example: long 1.0 @ 103 690, then sell 2.0
    // @ 103 620.
    let mut held = book();
    let netting = model();
    netting
        .apply(&mut held, &fill(OrderSide::Buy, 10, 103_690, 1_000))
        .unwrap();
    let before = held.positions[btc()].clone();

    let (settled, transition) = netting
        .apply(&mut held, &fill(OrderSide::Sell, 20, 103_620, 2_000))
        .unwrap();

    assert_eq!(transition, Transition::Reversed);
    // (103 620 − 103 690) × 1.0 = −70, booked immediately.
    assert_eq!(
        settled.realized,
        -70 * 10_i64.pow(8),
        "the closed leg's loss is booked at the reversal, not deferred"
    );

    let after = &held.positions[btc()];
    assert_eq!(after.side, PositionSide::Short, "the direction flipped");
    assert_eq!(
        after.volume,
        Scaled::new(1, 10),
        "1.0 lot of the 2.0 is left"
    );
    assert_eq!(
        after.entry,
        Scaled::new(0, 103_620),
        "the new position is priced at the reversing deal's own price, not a weighted average \
         with the leg that just closed"
    );
    assert_ne!(
        after.ticket, before.ticket,
        "POSITION_TICKET becomes the reversing order's own"
    );
    assert_eq!(
        after.identifier, before.identifier,
        "POSITION_IDENTIFIER survives, because every deal that ever belonged to this position \
         is keyed on it"
    );
    assert_eq!(
        after.opened_at,
        at(2_000),
        "and the open time resets, since the exposure that exists now did not exist before"
    );
}

#[test]
fn one_symbol_never_holds_more_than_one_position_however_it_is_traded() {
    let mut held = book();
    let netting = model();
    for (side, lots, price, at) in [
        (OrderSide::Buy, 10, 104_000, 1_000),
        (OrderSide::Sell, 30, 103_000, 2_000),
        (OrderSide::Buy, 5, 103_500, 3_000),
        (OrderSide::Buy, 40, 104_000, 4_000),
    ] {
        netting
            .apply(&mut held, &fill(side, lots, price, at))
            .unwrap();
        assert!(
            held.positions.len() <= 1,
            "netting holds at most one position per symbol, whatever sequence produced it"
        );
    }
}

#[test]
fn the_seam_settles_a_netting_book_without_the_caller_knowing_the_rule() {
    let mut held = book();
    let netting = model();
    // Driven through the shared trait rather than `apply`, which is the
    // whole claim: a second system is a settlement model, not a fork.
    netting
        .settle(&mut held, &fill(OrderSide::Buy, 10, 104_000, 1_000))
        .unwrap();
    netting
        .settle(&mut held, &fill(OrderSide::Sell, 10, 104_000, 2_000))
        .unwrap();

    assert!(
        held.positions.is_empty(),
        "a hedging model settling the same two fills holds two tickets; this one holds none"
    );
}
