//! Swap and volume, each stated as the sentence MetaTrader's own
//! documentation states it in.

use senken_core::decimal::Scaled;
use senken_core::time::UnixNanos;
use senken_plugin_mt5_hedging::swap::{SwapMode, SwapTerms, swap_days, swap_for, weekday};
use senken_plugin_mt5_hedging::volume::{VolumeLimits, VolumeRejection, check};
use senken_trade::PositionSide;

/// 2026-01-05 was a Monday.
const MONDAY: i64 = 1_767_571_200;
const DAY: i64 = 86_400;

fn at(secs: i64) -> UnixNanos {
    UnixNanos::from_secs(secs).unwrap()
}

fn terms(rollover3: Option<u8>) -> SwapTerms {
    SwapTerms {
        mode: SwapMode::CurrencyDeposit,
        long_rate: -7,
        short_rate: 2,
        rate_scale: 2,
        rollover3_weekday: rollover3,
        contract_size: 100_000,
    }
}

fn lots(hundredths: i64) -> Scaled {
    Scaled::new(2, hundredths)
}

#[test]
fn the_fixture_monday_really_is_a_monday() {
    assert_eq!(
        weekday(at(MONDAY)),
        0,
        "every swap test below counts weekdays from this instant, so if it is not a Monday the \
         rest of the file proves nothing"
    );
}

#[test]
fn a_position_held_within_one_day_crosses_no_rollover_and_accrues_nothing() {
    assert_eq!(
        swap_days(terms(None), at(MONDAY), at(MONDAY + 3_600)),
        0,
        "reading an account twice in an afternoon must not charge it swap twice"
    );
}

#[test]
fn each_rollover_crossed_charges_one_day() {
    assert_eq!(
        swap_days(terms(None), at(MONDAY), at(MONDAY + 3 * DAY)),
        3,
        "swap is charged once per trading day the position is held through rollover"
    );
}

#[test]
fn the_brokers_configured_weekday_charges_three_days_and_the_others_charge_one() {
    // Wednesday is weekday 2. Monday → Thursday crosses Tue, Wed, Thu.
    let with_wednesday = swap_days(terms(Some(2)), at(MONDAY), at(MONDAY + 3 * DAY));
    let without = swap_days(terms(None), at(MONDAY), at(MONDAY + 3 * DAY));

    assert_eq!(without, 3, "three rollovers, one day each");
    assert_eq!(
        with_wednesday, 5,
        "the triple day counts for three, so the same three rollovers come to five days"
    );
}

#[test]
fn the_triple_day_is_read_from_the_broker_and_is_not_always_wednesday() {
    // Wednesday → Saturday crosses Thu, Fri and Sat, and no Wednesday at
    // all. A broker charging triple on Friday must see it; an
    // implementation that assumed Wednesday would see none, so the two
    // answers cannot coincide.
    let wednesday = at(MONDAY + 2 * DAY);
    let saturday = at(MONDAY + 5 * DAY);

    assert_eq!(
        swap_days(terms(Some(4)), wednesday, saturday),
        5,
        "three rollovers with the broker's Friday counting triple is five days"
    );
    assert_eq!(
        swap_days(terms(Some(2)), wednesday, saturday),
        3,
        "the same three rollovers for a Wednesday-triple broker are three days, because this \
         range crosses no Wednesday — which is what a hard-coded Wednesday could not tell apart"
    );
}

#[test]
fn a_long_and_a_short_on_one_symbol_are_charged_different_swap() {
    let long = swap_for(
        terms(None),
        PositionSide::Long,
        lots(100),
        Scaled::new(5, 108_500),
        1,
    )
    .unwrap();
    let short = swap_for(
        terms(None),
        PositionSide::Short,
        lots(100),
        Scaled::new(5, 108_500),
        1,
    )
    .unwrap();

    assert!(
        long < 0 && short > 0,
        "the carry mechanic: one side pays and the other is paid, which a single rate could \
         not express — long {long}, short {short}"
    );
}

#[test]
fn a_disabled_symbol_is_charged_no_swap_at_all() {
    let mut disabled = terms(None);
    disabled.mode = SwapMode::Disabled;

    assert_eq!(
        swap_for(
            disabled,
            PositionSide::Long,
            lots(100),
            Scaled::new(5, 108_500),
            10
        )
        .unwrap(),
        0,
        "a symbol with swap disabled accrues none however long it is held"
    );
}

fn limits() -> VolumeLimits {
    VolumeLimits {
        min: Scaled::new(2, 1),
        max: Scaled::new(2, 10_000),
        step: Scaled::new(2, 1),
        limit: Some(Scaled::new(2, 20_000)),
    }
}

#[test]
fn a_volume_below_the_symbols_minimum_is_refused() {
    assert_eq!(
        check(limits(), Scaled::new(3, 5), Scaled::new(2, 0)),
        Err(VolumeRejection::BelowMinimum),
        "0.005 lots is under a 0.01 minimum"
    );
}

#[test]
fn a_volume_off_the_step_is_refused_rather_than_rounded_to_it() {
    assert_eq!(
        check(limits(), Scaled::new(3, 15), Scaled::new(2, 0)),
        Err(VolumeRejection::OffStep),
        "rounding 0.015 to 0.01 or 0.02 would fill the trader at a size they did not ask for, \
         which on a leveraged account is a different risk than the one they sized"
    );
}

#[test]
fn a_volume_past_the_per_order_maximum_is_refused() {
    assert_eq!(
        check(limits(), Scaled::new(2, 20_000), Scaled::new(2, 0)),
        Err(VolumeRejection::AboveMaximum),
        "200 lots is past a 100-lot per-order cap"
    );
}

#[test]
fn the_symbol_limit_counts_what_is_already_open_and_not_just_this_order() {
    assert_eq!(
        check(limits(), Scaled::new(2, 5_000), Scaled::new(2, 0)),
        Ok(()),
        "50 lots alone is inside the 200-lot symbol limit"
    );
    assert_eq!(
        check(limits(), Scaled::new(2, 5_000), Scaled::new(2, 18_000)),
        Err(VolumeRejection::OverSymbolLimit),
        "the same 50 lots on top of 180 already open is not, which is the whole point of a \
         total limit as against a per-order one"
    );
}
