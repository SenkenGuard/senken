//! The perpetual futures account as a registrable adapter.

use std::sync::Arc;

use senken_core::decimal::Scaled;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::{Marks, SettlementModel, SimAdapter, SimulatedVenue};
use senken_storage::Storage;
use senken_trade::{
    AccountBalances, AdapterCapabilities, ChoiceOption, FieldKind, MarginMode, MarginTerms,
    OrderKindTag, Position, PositionBasis, PositionId, QuantityUnit, SettingField, SettingsSchema,
    SettingsValues, TimeInForce, TradeAccountId, TradeAdapter, TradeError,
};

use crate::bracket::{Bracket, BracketTable};
use crate::funding::FundingTerms;
use crate::model::{Futures, FuturesBook, PositionMode as PerpMode};

/// The id this adapter registers under.
pub const ADAPTER_ID: &str = "futures";

const KEY_CURRENCY: &str = "currency";
const KEY_STARTING_BALANCE: &str = "starting_balance";
const KEY_LEVERAGE: &str = "leverage";
const KEY_POSITION_MODE: &str = "position_mode";
const KEY_MARGIN_MODE: &str = "margin_mode";
const KEY_FEE_BPS: &str = "fee_bps";
const KEY_SLIPPAGE_BPS: &str = "slippage_bps";
const KEY_FUNDING_HOURS: &str = "funding_hours";
const KEY_FUNDING_BPS: &str = "funding_bps";
const KEY_MAINTENANCE_BPS: &str = "maintenance_bps";
const KEY_NOTIONAL_CAP: &str = "bracket_notional_cap";

const HOUR_NANOS: i64 = 3_600 * 1_000_000_000;

/// A simulated crypto perpetual futures venue.
#[derive(Debug, Default)]
pub struct FuturesVenue;

impl SimulatedVenue for FuturesVenue {
    type Book = FuturesBook;
    type Model = Futures;

    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn name(&self) -> &'static str {
        "Perpetual futures"
    }

    fn description(&self) -> &'static str {
        "A simulated USDT-margined perpetual account: one-way or hedge, isolated or cross, with \
         funding and liquidation"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            .with_order_kinds(vec![
                OrderKindTag::Market,
                OrderKindTag::Limit,
                OrderKindTag::Stop,
                OrderKindTag::StopLimit,
            ])
            .with_time_in_force(vec![TimeInForce::Gtc])
            .with_quantity_unit(QuantityUnit::Base)
            .with_position_mode(senken_trade::PositionMode::Netting)
            .with_margin()
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::new(
            account_fields()
                .into_iter()
                .chain(mode_fields())
                .chain(cost_fields())
                .chain(funding_fields())
                .chain(liquidation_fields())
                .collect(),
        )
    }
    fn model_for(&self, settings: &SettingsValues) -> Result<Self::Model, TradeError> {
        let maintenance_bps = settings.number(KEY_MAINTENANCE_BPS).unwrap_or(0);
        let cap = settings
            .decimal(KEY_NOTIONAL_CAP)
            .unwrap_or_else(|| Scaled::new(2, 100_000_000));
        // No rate configured means no table, which means no liquidation
        // price — the honest answer rather than a plausible invention.
        let brackets = if maintenance_bps > 0 {
            BracketTable {
                tiers: vec![Bracket {
                    notional_cap: rescale(i128::from(cap.value), cap.scale, CASH_SCALE)?,
                    maintenance_bps,
                    maintenance_amount: 0,
                    max_leverage: settings.number(KEY_LEVERAGE).unwrap_or(10),
                }],
            }
        } else {
            BracketTable::default()
        };
        Ok(Futures {
            position_mode: match settings.text(KEY_POSITION_MODE).unwrap_or("one_way") {
                "hedge" => PerpMode::Hedge,
                _ => PerpMode::OneWay,
            },
            margin_mode: match settings.text(KEY_MARGIN_MODE).unwrap_or("isolated") {
                "cross" => MarginMode::Cross,
                _ => MarginMode::Isolated,
            },
            leverage: settings.number(KEY_LEVERAGE).unwrap_or(10).max(1),
            fee_bps: settings.number(KEY_FEE_BPS).unwrap_or(5),
            brackets,
            funding: FundingTerms {
                interval_nanos: settings.number(KEY_FUNDING_HOURS).unwrap_or(8) * HOUR_NANOS,
                rate_bps: settings.number(KEY_FUNDING_BPS).unwrap_or(1),
            },
        })
    }

    fn open_book(&self, settings: &SettingsValues) -> Result<Self::Book, TradeError> {
        let declared = settings
            .decimal(KEY_STARTING_BALANCE)
            .unwrap_or_else(|| Scaled::new(2, 1_000_000));
        Ok(FuturesBook {
            wallet: rescale(i128::from(declared.value), declared.scale, CASH_SCALE)?,
            ..FuturesBook::default()
        })
    }

    fn positions(
        &self,
        book: &Self::Book,
        marks: &Marks,
        account: TradeAccountId,
    ) -> Result<Vec<Position>, TradeError> {
        book.positions
            .iter()
            .map(|(key, position)| {
                let mark = marks.get(&position.instrument.to_string()).copied();
                Ok(Position {
                    id: PositionId::new(key.clone()),
                    account_id: account,
                    instrument: position.instrument.clone(),
                    side: position.side,
                    quantity: position.quantity,
                    average_entry: position.entry,
                    mark_price: mark,
                    unrealized_pnl: None,
                    realized_pnl: Scaled::new(CASH_SCALE, position.funding),
                    stop_loss: None,
                    take_profit: None,
                    basis: PositionBasis::Margined(MarginTerms {
                        margin: Scaled::new(CASH_SCALE, position.margin),
                        leverage: Scaled::new(0, 1),
                        mode: position.margin_mode,
                        // Filled in by `positions_with` below when a
                        // bracket table exists; absent here rather than
                        // estimated.
                        liquidation_price: None,
                    }),
                    opened_at: position.opened_at,
                })
            })
            .collect()
    }

    fn balances(
        &self,
        book: &Self::Book,
        marks: &Marks,
        account: TradeAccountId,
        settings: &SettingsValues,
    ) -> Result<AccountBalances, TradeError> {
        let model = self.model_for(settings)?;
        let risk = model.risk(book, marks)?;
        Ok(AccountBalances {
            account_id: account,
            currency: settings.text(KEY_CURRENCY).unwrap_or("USDT").to_owned(),
            balance: Scaled::new(CASH_SCALE, risk.balance),
            equity: Scaled::new(CASH_SCALE, risk.equity),
            unrealized_pnl: Scaled::new(CASH_SCALE, risk.equity.saturating_sub(risk.balance)),
            realized_pnl: Scaled::new(CASH_SCALE, 0),
            margin_used: Some(Scaled::new(CASH_SCALE, risk.margin_used)),
            margin_available: Some(Scaled::new(
                CASH_SCALE,
                risk.equity.saturating_sub(risk.margin_used),
            )),
            margin_level: risk.margin_level,
            assets: Vec::new(),
        })
    }

    fn slippage_bps(&self, settings: &SettingsValues) -> i64 {
        settings.number(KEY_SLIPPAGE_BPS).unwrap_or(2)
    }

    fn fee_currency(&self, settings: &SettingsValues, _side: senken_trade::OrderSide) -> String {
        settings.text(KEY_CURRENCY).unwrap_or("USDT").to_owned()
    }
}

/// The account: what it is margined in, what it opens with, and at what leverage.
fn account_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_CURRENCY,
            "Margin currency",
            FieldKind::Choice {
                default: Some("USDT".to_owned()),
                options: vec![
                    ChoiceOption::new("USDT", "USDT"),
                    ChoiceOption::new("USDC", "USDC"),
                ],
            },
        ),
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
        ),
        SettingField::new(
            KEY_LEVERAGE,
            "Leverage",
            FieldKind::Number {
                default: Some(10),
                min: 1,
                max: 125,
                unit: "x".to_owned(),
            },
        ),
    ]
}

/// The two mode choices that change what liquidation means.
fn mode_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_POSITION_MODE,
            "Position mode",
            FieldKind::Choice {
                default: Some("one_way".to_owned()),
                options: vec![
                    ChoiceOption::new("one_way", "One-way"),
                    ChoiceOption::new("hedge", "Hedge"),
                ],
            },
        )
        .with_help("Hedge mode lets a long and a short on one symbol coexist."),
        SettingField::new(
            KEY_MARGIN_MODE,
            "Margin mode",
            FieldKind::Choice {
                default: Some("isolated".to_owned()),
                options: vec![
                    ChoiceOption::new("isolated", "Isolated"),
                    ChoiceOption::new("cross", "Cross"),
                ],
            },
        )
        .with_help("Isolated margin can be liquidated without touching your other positions."),
    ]
}

/// Taker fee and slippage.
fn cost_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_FEE_BPS,
            "Taker fee",
            FieldKind::Number {
                default: Some(5),
                min: 0,
                max: 1_000,
                unit: "bps".to_owned(),
            },
        ),
        SettingField::new(
            KEY_SLIPPAGE_BPS,
            "Slippage",
            FieldKind::Number {
                default: Some(2),
                min: 0,
                max: 1_000,
                unit: "bps".to_owned(),
            },
        ),
    ]
}

/// Funding: how often, and at what rate.
fn funding_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_FUNDING_HOURS,
            "Funding interval",
            FieldKind::Number {
                default: Some(8),
                min: 1,
                max: 24,
                unit: "hours".to_owned(),
            },
        )
        .with_help(
            "Eight hours is the usual default, but venues shorten it during extreme \
             volatility — so it is read here rather than assumed.",
        ),
        SettingField::new(
            KEY_FUNDING_BPS,
            "Funding rate",
            FieldKind::Number {
                default: Some(1),
                min: -1_000,
                max: 1_000,
                unit: "bps".to_owned(),
            },
        )
        .with_help("Positive means longs pay shorts. It is a transfer, not a fee."),
    ]
}

/// The bracket table, whose absence is an honest answer.
fn liquidation_fields() -> Vec<SettingField> {
    vec![
        SettingField::new(
            KEY_MAINTENANCE_BPS,
            "Maintenance margin rate",
            FieldKind::Number {
                default: Some(0),
                min: 0,
                max: 10_000,
                unit: "bps".to_owned(),
            },
        )
        .with_help(
            "Leave at zero and no liquidation price is reported at all. A real bracket \
             table comes from the venue; a made-up one is worse than none, because a \
             liquidation price you are shown is one you will believe.",
        ),
        SettingField::new(
            KEY_NOTIONAL_CAP,
            "Bracket covers up to",
            FieldKind::Decimal {
                scale: 2,
                default: Some(100_000_000),
                min: 0,
                max: 1_000_000_000_000,
                unit: String::new(),
            },
        )
        .with_help("The notional this single tier covers. Past it, no price is reported."),
    ]
}

/// The plugin that registers the futures adapter.
#[derive(Debug)]
pub struct FuturesPlugin {
    adapter: Arc<SimAdapter<FuturesVenue>>,
}

impl FuturesPlugin {
    /// Builds the plugin over `storage`, where its books live.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self {
            adapter: Arc::new(SimAdapter::new(FuturesVenue, storage)),
        }
    }
}

impl Plugin for FuturesPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ADAPTER_ID.to_owned(),
            name: "Perpetual futures".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "A simulated perpetual futures account with funding and liquidation"
                .to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate_without_io(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        context.register_trade_adapter(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>);
        Ok(())
    }
}
