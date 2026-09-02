//! The broker's own numbers, as account settings.
//!
//! MetaTrader fixes the formulas; brokers fix the numbers. Every value
//! here is something a real account reads from its symbol specification or
//! its server configuration — the two stop levels, the contract size, the
//! margin percentage, the swap rates and the triple-swap weekday. None of
//! them has a default this simulator may invent, so each field's default
//! is a *this broker* choice the reader can see and change, not a platform
//! constant hidden in code.

use senken_core::decimal::Scaled;
use senken_trade::{ChoiceOption, FieldKind, SettingField, SettingsSchema, SettingsValues};

use crate::account::StopLevels;
use crate::commission::CommissionModel;
use crate::margin::{CalcMode, SymbolMargin};
use crate::swap::{SwapMode, SwapTerms};
use crate::volume::VolumeLimits;

/// What every figure on the account is denominated in.
pub const KEY_CURRENCY: &str = "currency";
/// What the account opens with.
pub const KEY_STARTING_BALANCE: &str = "starting_balance";
/// The account's leverage.
pub const KEY_LEVERAGE: &str = "leverage";
/// Which margin formula the symbol uses.
pub const KEY_CALC_MODE: &str = "calc_mode";
/// Units of the instrument in one lot.
pub const KEY_CONTRACT_SIZE: &str = "contract_size";
/// The broker's margin requirement percentage.
pub const KEY_MARGIN_PERCENTAGE: &str = "margin_percentage";
/// Margin level below which opening is blocked.
pub const KEY_MARGIN_CALL: &str = "margin_call_level";
/// Margin level below which the server closes positions.
pub const KEY_STOP_OUT: &str = "stop_out_level";
/// How swap is calculated.
pub const KEY_SWAP_MODE: &str = "swap_mode";
/// Swap charged on a long, per lot per night.
pub const KEY_SWAP_LONG: &str = "swap_long";
/// Swap charged on a short, per lot per night.
pub const KEY_SWAP_SHORT: &str = "swap_short";
/// Which weekday carries three days of swap.
pub const KEY_SWAP_ROLLOVER3: &str = "swap_rollover3";
/// How commission is charged.
pub const KEY_COMMISSION_MODE: &str = "commission_mode";
/// The commission amount, in the unit its mode implies.
pub const KEY_COMMISSION_AMOUNT: &str = "commission_amount";
/// Smallest tradable volume.
pub const KEY_VOLUME_MIN: &str = "volume_min";
/// Largest volume in one order.
pub const KEY_VOLUME_MAX: &str = "volume_max";
/// The increment volume must be a multiple of.
pub const KEY_VOLUME_STEP: &str = "volume_step";
/// How far a market order fills from the quoted price.
pub const KEY_DEVIATION_POINTS: &str = "deviation_points";
/// Whether the login may trade or only read.
pub const KEY_ACCESS: &str = "access";

/// The value [`KEY_ACCESS`] takes for an investor login.
pub const ACCESS_READ_ONLY: &str = "read_only";

/// Every setting this adapter reads, as the host renders them.
///
/// Grouped by mechanic rather than listed flat: a reader looking for
/// what governs swap should find the four swap fields together, and a
/// reader adding a mechanic should have one obvious place to put it.
#[must_use]
pub fn schema() -> SettingsSchema {
    SettingsSchema::new(
        account_fields()
            .into_iter()
            .chain(margin_fields())
            .chain(swap_fields())
            .chain(cost_fields())
            .chain(access_fields())
            .collect(),
    )
}

/// The account itself: what it is denominated in, what it opens with, and what it may trade on.
fn account_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_CURRENCY,
            "Deposit currency",
            FieldKind::Choice {
                default: Some("USD".to_owned()),
                options: vec![
                    ChoiceOption::new("USD", "USD"),
                    ChoiceOption::new("EUR", "EUR"),
                    ChoiceOption::new("GBP", "GBP"),
                    ChoiceOption::new("JPY", "JPY"),
                ],
            },
        )
        .with_help("What balance, equity, swap and profit are all reported in."),
        SettingField::new(
            KEY_STARTING_BALANCE,
            "Starting balance",
            FieldKind::Decimal {
                scale: 2,
                default: Some(1_000_000),
                min: 0,
                max: 100_000_000_000,
                unit: String::new(),
            },
        )
        .with_help("What the account opens with."),
        SettingField::new(
            KEY_LEVERAGE,
            "Leverage",
            FieldKind::Number {
                default: Some(100),
                min: 1,
                max: 3_000,
                unit: ":1".to_owned(),
            },
        )
        .with_help("Your broker sets this per account. 1:100 and 1:500 are both common."),
    ]
}

/// How margin is charged, and the two levels the broker acts on.
fn margin_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_CALC_MODE,
            "Margin calculation",
            FieldKind::Choice {
                default: Some("forex".to_owned()),
                options: vec![
                    ChoiceOption::new("forex", "Forex"),
                    ChoiceOption::new("forex_no_leverage", "Forex, no leverage"),
                    ChoiceOption::new("cfd", "CFD"),
                    ChoiceOption::new("cfd_leverage", "CFD with leverage"),
                ],
            },
        )
        .with_help(
            "Read from the symbol specification. Forex margin does not depend on the price; a \
                 CFD's does.",
        ),
        SettingField::new(
            KEY_CONTRACT_SIZE,
            "Contract size",
            FieldKind::Number {
                default: Some(100_000),
                min: 1,
                max: 100_000_000,
                unit: "units/lot".to_owned(),
            },
        )
        .with_help("Units of the instrument in one lot. 100 000 for a standard forex lot."),
        SettingField::new(
            KEY_MARGIN_PERCENTAGE,
            "Margin requirement",
            FieldKind::Number {
                default: Some(100),
                min: 1,
                max: 1_000,
                unit: "%".to_owned(),
            },
        )
        .with_help("The broker's own percentage for this symbol, used by the CFD modes."),
        SettingField::new(
            KEY_MARGIN_CALL,
            "Margin call level",
            FieldKind::Decimal {
                scale: 2,
                default: Some(10_000),
                min: 0,
                max: 100_000,
                unit: "%".to_owned(),
            },
        )
        .with_help(
            "Below this margin level no new position may be opened. Nothing is closed — that \
                 is the stop out's job.",
        ),
        SettingField::new(
            KEY_STOP_OUT,
            "Stop out level",
            FieldKind::Decimal {
                scale: 2,
                default: Some(5_000),
                min: 0,
                max: 100_000,
                unit: "%".to_owned(),
            },
        )
        .with_help(
            "Below this margin level the server closes your biggest losing position, then \
                 looks again, until the level recovers.",
        ),
    ]
}

/// Swap: the mode, the two rates, and the triple-charge night.
fn swap_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_SWAP_MODE,
            "Swap calculation",
            FieldKind::Choice {
                default: Some("currency_deposit".to_owned()),
                options: vec![
                    ChoiceOption::new("disabled", "None"),
                    ChoiceOption::new("points", "Points"),
                    ChoiceOption::new("currency_deposit", "Deposit currency"),
                    ChoiceOption::new("interest_current", "Annual %, current price"),
                    ChoiceOption::new("interest_open", "Annual %, open price"),
                ],
            },
        )
        .with_help("Also read from the symbol specification."),
        SettingField::new(
            KEY_SWAP_LONG,
            "Swap long",
            FieldKind::Decimal {
                scale: 2,
                default: Some(-700),
                min: -1_000_000,
                max: 1_000_000,
                unit: "/lot/night".to_owned(),
            },
        )
        .with_help("Negative is charged to you, positive is paid to you."),
        SettingField::new(
            KEY_SWAP_SHORT,
            "Swap short",
            FieldKind::Decimal {
                scale: 2,
                default: Some(200),
                min: -1_000_000,
                max: 1_000_000,
                unit: "/lot/night".to_owned(),
            },
        )
        .with_help(
            "Almost always a different number from the long rate, and often the opposite sign \
                 — that is the carry.",
        ),
        SettingField::new(
            KEY_SWAP_ROLLOVER3,
            "Triple swap day",
            FieldKind::Choice {
                default: Some("wednesday".to_owned()),
                options: vec![
                    ChoiceOption::new("none", "None"),
                    ChoiceOption::new("monday", "Monday"),
                    ChoiceOption::new("tuesday", "Tuesday"),
                    ChoiceOption::new("wednesday", "Wednesday"),
                    ChoiceOption::new("thursday", "Thursday"),
                    ChoiceOption::new("friday", "Friday"),
                ],
            },
        )
        .with_help(
            "The night three days of swap are charged to cover the weekend value date. Your \
                 broker chooses it per symbol.",
        ),
    ]
}

/// Commission and the volume limits an order is checked against.
fn cost_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_COMMISSION_MODE,
            "Commission",
            FieldKind::Choice {
                default: Some("none".to_owned()),
                options: vec![
                    ChoiceOption::new("none", "None (built into the spread)"),
                    ChoiceOption::new("per_lot", "Per lot"),
                    ChoiceOption::new("notional", "Basis points of notional"),
                ],
            },
        )
        .with_help("Configured by your broker per symbol group; MetaTrader fixes no formula."),
        SettingField::new(
            KEY_COMMISSION_AMOUNT,
            "Commission amount",
            FieldKind::Decimal {
                scale: 2,
                default: Some(0),
                min: 0,
                max: 1_000_000,
                unit: String::new(),
            },
        )
        .with_help("Per lot, or in basis points, according to the mode above."),
        SettingField::new(
            KEY_VOLUME_MIN,
            "Minimum volume",
            FieldKind::Decimal {
                scale: 2,
                default: Some(1),
                min: 1,
                max: 100_000,
                unit: "lots".to_owned(),
            },
        ),
        SettingField::new(
            KEY_VOLUME_MAX,
            "Maximum volume",
            FieldKind::Decimal {
                scale: 2,
                default: Some(10_000),
                min: 1,
                max: 100_000_000,
                unit: "lots".to_owned(),
            },
        ),
        SettingField::new(
            KEY_VOLUME_STEP,
            "Volume step",
            FieldKind::Decimal {
                scale: 2,
                default: Some(1),
                min: 1,
                max: 100_000,
                unit: "lots".to_owned(),
            },
        )
        .with_help("An order off the step is refused, not rounded onto it."),
        SettingField::new(
            KEY_DEVIATION_POINTS,
            "Deviation",
            FieldKind::Number {
                default: Some(10),
                min: 0,
                max: 10_000,
                unit: "points".to_owned(),
            },
        )
        .with_help("How far a market order may fill from the quoted price. Always against you."),
    ]
}

/// Whether this login may trade at all.
fn access_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_ACCESS,
            "Login type",
            FieldKind::Choice {
                default: Some("trade".to_owned()),
                options: vec![
                    ChoiceOption::new("trade", "Trading"),
                    ChoiceOption::new(ACCESS_READ_ONLY, "Investor (read-only)"),
                ],
            },
        )
        .with_help(
            "An investor login reads the account and places nothing — the same shape a real \
                 MT5 investor password has.",
        ),
    ]
}

/// The margin terms one account trades under.
#[must_use]
pub fn margin_of(values: &SettingsValues) -> SymbolMargin {
    SymbolMargin {
        mode: match values.text(KEY_CALC_MODE).unwrap_or("forex") {
            "forex_no_leverage" => CalcMode::ForexNoLeverage,
            "cfd" => CalcMode::Cfd,
            "cfd_leverage" => CalcMode::CfdLeverage,
            _ => CalcMode::Forex,
        },
        contract_size: values.number(KEY_CONTRACT_SIZE).unwrap_or(100_000),
        leverage: values.number(KEY_LEVERAGE).unwrap_or(100).max(1),
        percentage: values.number(KEY_MARGIN_PERCENTAGE).unwrap_or(100),
    }
}

/// The two thresholds this account's broker applies.
#[must_use]
pub fn stop_levels_of(values: &SettingsValues) -> StopLevels {
    StopLevels {
        margin_call: values.decimal(KEY_MARGIN_CALL),
        stop_out: values.decimal(KEY_STOP_OUT),
    }
}

/// The swap configuration for this account's symbol.
#[must_use]
pub fn swap_of(values: &SettingsValues) -> SwapTerms {
    let long = values
        .decimal(KEY_SWAP_LONG)
        .unwrap_or(Scaled::new(2, -700));
    let short = values
        .decimal(KEY_SWAP_SHORT)
        .unwrap_or(Scaled::new(2, 200));
    SwapTerms {
        mode: match values.text(KEY_SWAP_MODE).unwrap_or("currency_deposit") {
            "disabled" => SwapMode::Disabled,
            "points" => SwapMode::Points,
            "interest_current" => SwapMode::InterestCurrent,
            "interest_open" => SwapMode::InterestOpen,
            _ => SwapMode::CurrencyDeposit,
        },
        long_rate: long.value,
        short_rate: short.value,
        rate_scale: long.scale,
        rollover3_weekday: match values.text(KEY_SWAP_ROLLOVER3).unwrap_or("wednesday") {
            "monday" => Some(0),
            "tuesday" => Some(1),
            "wednesday" => Some(2),
            "thursday" => Some(3),
            "friday" => Some(4),
            _ => None,
        },
        contract_size: values.number(KEY_CONTRACT_SIZE).unwrap_or(100_000),
    }
}

/// How this account's broker charges commission.
#[must_use]
pub fn commission_of(values: &SettingsValues) -> CommissionModel {
    let amount = values
        .decimal(KEY_COMMISSION_AMOUNT)
        .unwrap_or(Scaled::new(2, 0));
    match values.text(KEY_COMMISSION_MODE).unwrap_or("none") {
        "per_lot" => CommissionModel::PerLot {
            amount: amount.value,
        },
        "notional" => CommissionModel::Notional {
            bps: amount.value / 100,
        },
        _ => CommissionModel::None,
    }
}

/// The volume limits this account's symbol carries.
#[must_use]
pub fn volume_of(values: &SettingsValues) -> VolumeLimits {
    VolumeLimits {
        min: values.decimal(KEY_VOLUME_MIN).unwrap_or(Scaled::new(2, 1)),
        max: values
            .decimal(KEY_VOLUME_MAX)
            .unwrap_or(Scaled::new(2, 10_000)),
        step: values.decimal(KEY_VOLUME_STEP).unwrap_or(Scaled::new(2, 1)),
        limit: None,
    }
}
