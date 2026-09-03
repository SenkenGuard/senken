//! The netting account as a registrable adapter.
//!
//! One file, because everything a `TradeAdapter` needs that is not
//! specific to netting comes from `senken_sim_core`'s shared adapter.

use std::sync::Arc;

use senken_core::decimal::Scaled;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::{Marks, SimAdapter, SimulatedVenue};
use senken_storage::Storage;
use senken_trade::{
    AccountBalances, AdapterCapabilities, ChoiceOption, FieldKind, MarginMode, MarginTerms,
    OrderKindTag, Position, PositionBasis, PositionId, PositionMode, QuantityUnit, SettingField,
    SettingsSchema, SettingsValues, TimeInForce, TradeAccountId, TradeAdapter, TradeError,
};

use crate::book::NettingBook;
use crate::model::Netting;

/// The id this adapter registers under.
pub const ADAPTER_ID: &str = "mt5-netting";

const KEY_CURRENCY: &str = "currency";
const KEY_STARTING_BALANCE: &str = "starting_balance";
const KEY_LEVERAGE: &str = "leverage";
const KEY_FEE_BPS: &str = "fee_bps";
const KEY_SLIPPAGE_BPS: &str = "slippage_bps";
const KEY_ACCESS: &str = "access";
const ACCESS_READ_ONLY: &str = "read_only";

/// A simulated MetaTrader 5 netting account.
#[derive(Debug, Default)]
pub struct NettingVenue;

impl SimulatedVenue for NettingVenue {
    type Book = NettingBook;
    type Model = Netting;

    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn name(&self) -> &'static str {
        "MetaTrader 5 (netting)"
    }

    fn description(&self) -> &'static str {
        "A simulated MT5 netting account: one position per symbol, folded by weighted average, \
         reduced, closed or reversed"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            .with_order_kinds(vec![OrderKindTag::Market, OrderKindTag::Limit])
            .with_time_in_force(vec![TimeInForce::Gtc])
            .with_quantity_unit(QuantityUnit::Base)
            // One position per symbol: an opposite fill meets the existing
            // one rather than sitting beside it.
            .with_position_mode(PositionMode::Netting)
            .with_margin()
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::new(vec![
            SettingField::new(
                KEY_CURRENCY,
                "Deposit currency",
                FieldKind::Choice {
                    default: Some("USD".to_owned()),
                    options: vec![
                        ChoiceOption::new("USD", "USD"),
                        ChoiceOption::new("EUR", "EUR"),
                        ChoiceOption::new("GBP", "GBP"),
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
                    default: Some(100),
                    min: 1,
                    max: 3_000,
                    unit: ":1".to_owned(),
                },
            ),
            SettingField::new(
                KEY_FEE_BPS,
                "Commission",
                FieldKind::Number {
                    default: Some(0),
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
            )
            .with_help("Always against you, never for you."),
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
            ),
        ])
    }

    fn model_for(&self, settings: &SettingsValues) -> Result<Self::Model, TradeError> {
        Ok(Netting {
            fee_bps: settings.number(KEY_FEE_BPS).unwrap_or(0),
        })
    }

    fn open_book(&self, _settings: &SettingsValues) -> Result<Self::Book, TradeError> {
        Ok(NettingBook {
            next_ticket: 1,
            ..NettingBook::default()
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
            .map(|(instrument, position)| {
                let mark = marks.get(&instrument.to_string()).copied();
                Ok(Position {
                    // The identifier, not the ticket: it survives a
                    // reversal, so a client holding this row can still act
                    // on the position after one.
                    id: PositionId::new(position.identifier.to_string()),
                    account_id: account,
                    instrument: instrument.clone(),
                    side: position.side,
                    quantity: position.volume,
                    average_entry: position.entry,
                    mark_price: mark,
                    unrealized_pnl: None,
                    realized_pnl: Scaled::new(CASH_SCALE, book.realized),
                    stop_loss: None,
                    take_profit: None,
                    basis: PositionBasis::Margined(MarginTerms {
                        margin: Scaled::new(CASH_SCALE, 0),
                        leverage: Scaled::new(0, 1),
                        mode: MarginMode::Cross,
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
        _marks: &Marks,
        account: TradeAccountId,
        settings: &SettingsValues,
    ) -> Result<AccountBalances, TradeError> {
        let declared = settings
            .decimal(KEY_STARTING_BALANCE)
            .unwrap_or_else(|| Scaled::new(2, 1_000_000));
        let opening = rescale(i128::from(declared.value), declared.scale, CASH_SCALE)?;
        let balance = opening.saturating_add(book.realized);
        Ok(AccountBalances {
            account_id: account,
            currency: settings.text(KEY_CURRENCY).unwrap_or("USD").to_owned(),
            balance: Scaled::new(CASH_SCALE, balance),
            equity: Scaled::new(CASH_SCALE, balance),
            unrealized_pnl: Scaled::new(CASH_SCALE, 0),
            // Survives every transition, including a position closing —
            // which is the ledger rule this workspace already fixed once.
            realized_pnl: Scaled::new(CASH_SCALE, book.realized),
            margin_used: None,
            margin_available: None,
            margin_level: None,
            assets: Vec::new(),
        })
    }

    fn is_read_only(&self, settings: &SettingsValues) -> bool {
        settings.text(KEY_ACCESS) == Some(ACCESS_READ_ONLY)
    }

    fn slippage_bps(&self, settings: &SettingsValues) -> i64 {
        settings.number(KEY_SLIPPAGE_BPS).unwrap_or(2)
    }

    fn fee_currency(&self, settings: &SettingsValues, _side: senken_trade::OrderSide) -> String {
        settings.text(KEY_CURRENCY).unwrap_or("USD").to_owned()
    }
}

/// The plugin that registers the netting adapter.
#[derive(Debug)]
pub struct NettingPlugin {
    adapter: Arc<SimAdapter<NettingVenue>>,
}

impl NettingPlugin {
    /// Builds the plugin over `storage`, where its books live.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self {
            adapter: Arc::new(SimAdapter::new(NettingVenue, storage)),
        }
    }
}

impl Plugin for NettingPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ADAPTER_ID.to_owned(),
            name: "MetaTrader 5 (netting)".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "A simulated MT5 netting account".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate_without_io(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        context.register_trade_adapter(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>);
        Ok(())
    }
}
