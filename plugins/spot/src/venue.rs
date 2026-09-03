//! The spot account as a registrable adapter.
//!
//! Everything a `TradeAdapter` needs that is not specific to spot — the
//! storage, the lock, settling before a read, the investor-login refusal,
//! recording a fill — comes from `senken_sim_core`'s shared adapter. What
//! is here is only what spot itself decides.

use senken_core::decimal::Scaled;
use senken_marketdata::InstrumentId;
use senken_sim_core::money::{CASH_SCALE, rescale};
use senken_sim_core::{Marks, Reservation, SimAdapter, SimulatedVenue};
use senken_trade::{
    AccountBalances, AdapterCapabilities, AssetBalance, ChoiceOption, FieldKind, OrderKindTag,
    Position, PositionMode, QuantityUnit, SettingField, SettingsSchema, SettingsValues,
    TimeInForce, TradeAccountId, TradeError,
};

use crate::balances::SpotBook;
use crate::model::{FeeAsset, Spot};

/// The id this adapter registers under.
pub const ADAPTER_ID: &str = "spot";

const KEY_BASE: &str = "base_asset";
const KEY_QUOTE: &str = "quote_asset";
const KEY_STARTING_QUOTE: &str = "starting_quote";
const KEY_FEE_BPS: &str = "fee_bps";
const KEY_FEE_ASSET: &str = "fee_asset";
const KEY_ASSET_SCALE: &str = "asset_scale";

/// A simulated spot exchange.
#[derive(Debug, Default)]
pub struct SpotVenue;

impl SimulatedVenue for SpotVenue {
    type Book = SpotBook;
    type Model = Spot;

    fn id(&self) -> &'static str {
        ADAPTER_ID
    }

    fn name(&self) -> &'static str {
        "Spot exchange"
    }

    fn description(&self) -> &'static str {
        "A simulated spot account: asset balances with free and locked, no leverage and no \
         short — you cannot sell what you do not hold"
    }

    fn capabilities(&self) -> AdapterCapabilities {
        AdapterCapabilities::market_only()
            // Spot venues carry stop-loss and stop-loss-limit too; the
            // shared adapter rests all three the same way.
            .with_order_kinds(vec![
                OrderKindTag::Market,
                OrderKindTag::Limit,
                OrderKindTag::Stop,
                OrderKindTag::StopLimit,
            ])
            .with_time_in_force(vec![TimeInForce::Gtc])
            .with_quantity_unit(QuantityUnit::Base)
            // Holdings, not direction. The engine refuses `close_position`
            // on the strength of this, because there is no side to close.
            .with_position_mode(PositionMode::SpotHoldings)
    }

    fn settings_schema(&self) -> SettingsSchema {
        SettingsSchema::new(vec![
            SettingField::new(
                KEY_BASE,
                "Base asset",
                FieldKind::Text {
                    default: Some("BTC".to_owned()),
                    placeholder: String::new(),
                    max_len: 12,
                },
            )
            .with_help("What you are buying. A buy moves quote into this."),
            SettingField::new(
                KEY_QUOTE,
                "Quote asset",
                FieldKind::Text {
                    default: Some("USDT".to_owned()),
                    placeholder: String::new(),
                    max_len: 12,
                },
            )
            .with_help("What you are paying with."),
            SettingField::new(
                KEY_STARTING_QUOTE,
                "Starting quote balance",
                FieldKind::Decimal {
                    scale: 2,
                    default: Some(1_000_000),
                    min: 0,
                    max: 100_000_000_000,
                    unit: String::new(),
                },
            )
            .with_help("What the account opens holding."),
            SettingField::new(
                KEY_FEE_BPS,
                "Fee",
                FieldKind::Number {
                    default: Some(10),
                    min: 0,
                    max: 1_000,
                    unit: "bps".to_owned(),
                },
            )
            .with_help("0.10% is the standard spot rate on this family of venues."),
            SettingField::new(
                KEY_FEE_ASSET,
                "Fee charged in",
                FieldKind::Choice {
                    default: Some("produced".to_owned()),
                    options: vec![
                        ChoiceOption::new("produced", "The asset the trade produces"),
                        ChoiceOption::new("bnb", "BNB (discount)"),
                        ChoiceOption::new("okb", "OKB (discount)"),
                        ChoiceOption::new("bgb", "BGB (discount)"),
                    ],
                },
            )
            .with_help(
                "By default the fee comes out of what the trade produces — base on a buy, \
                 quote on a sell. A native-token discount takes it from that token instead.",
            ),
            SettingField::new(
                KEY_ASSET_SCALE,
                "Asset precision",
                FieldKind::Number {
                    default: Some(8),
                    min: 0,
                    max: 18,
                    unit: "decimals".to_owned(),
                },
            )
            .with_help("Decimal places every balance on this account is kept at."),
        ])
    }

    fn model_for(&self, settings: &SettingsValues) -> Result<Self::Model, TradeError> {
        let scale = u8::try_from(settings.number(KEY_ASSET_SCALE).unwrap_or(8)).map_err(|_| {
            TradeError::InvalidRequest("asset precision is out of range".to_owned())
        })?;
        Ok(Spot {
            base: settings.text(KEY_BASE).unwrap_or("BTC").to_owned(),
            quote: settings.text(KEY_QUOTE).unwrap_or("USDT").to_owned(),
            fee_bps: settings.number(KEY_FEE_BPS).unwrap_or(10),
            fee_asset: match settings.text(KEY_FEE_ASSET).unwrap_or("produced") {
                "bnb" => FeeAsset::Discount {
                    asset: "BNB".to_owned(),
                },
                "okb" => FeeAsset::Discount {
                    asset: "OKB".to_owned(),
                },
                "bgb" => FeeAsset::Discount {
                    asset: "BGB".to_owned(),
                },
                _ => FeeAsset::Produced,
            },
            asset_scale: scale,
        })
    }

    fn open_book(&self, settings: &SettingsValues) -> Result<Self::Book, TradeError> {
        let model = self.model_for(settings)?;
        let declared = settings
            .decimal(KEY_STARTING_QUOTE)
            .unwrap_or_else(|| Scaled::new(2, 1_000_000));
        let opening = rescale(
            i128::from(declared.value),
            declared.scale,
            model.asset_scale,
        )?;
        let mut book = SpotBook::default();
        book.credit(&model.quote, opening);
        Ok(book)
    }

    fn positions(
        &self,
        _book: &Self::Book,
        _marks: &Marks,
        _account: TradeAccountId,
    ) -> Result<Vec<Position>, TradeError> {
        // None, ever. A spot holding is an asset balance, and returning a
        // fabricated position for one is the shape that looks right and
        // means nothing.
        Ok(Vec::new())
    }

    fn balances(
        &self,
        book: &Self::Book,
        _marks: &Marks,
        account: TradeAccountId,
        settings: &SettingsValues,
    ) -> Result<AccountBalances, TradeError> {
        let model = self.model_for(settings)?;
        let quote = book.get(&model.quote);
        let to_cash = |amount: i64| -> Result<Scaled, TradeError> {
            Ok(Scaled::new(
                CASH_SCALE,
                rescale(i128::from(amount), model.asset_scale, CASH_SCALE)?,
            ))
        };
        Ok(AccountBalances {
            account_id: account,
            currency: model.quote.clone(),
            // The quote balance, not a blended portfolio value: there is
            // no rate anywhere in this system, and inventing one to fill a
            // single number is the error this project already removed from
            // the top bar once.
            balance: to_cash(quote.total())?,
            equity: to_cash(quote.total())?,
            unrealized_pnl: Scaled::new(CASH_SCALE, 0),
            realized_pnl: Scaled::new(CASH_SCALE, 0),
            // Nothing is borrowed, so there is no margin to report and no
            // level to measure. `None` says that; a zero would not.
            margin_used: None,
            margin_available: None,
            margin_level: None,
            assets: book
                .assets
                .iter()
                .map(|(asset, held)| {
                    Ok(AssetBalance {
                        asset: asset.clone(),
                        total: to_cash(held.total())?,
                        available: to_cash(held.free)?,
                        reserved: to_cash(held.locked)?,
                    })
                })
                .collect::<Result<Vec<_>, TradeError>>()?,
        })
    }

    /// A buy holds quote, a sell holds base — exactly what the order
    /// could consume if it filled completely.
    ///
    /// This is what stops one balance being promised to two orders at
    /// once, and it is the reason a spot account can refuse a second
    /// order that a glance at the total would say it could afford.
    fn reserve(
        &self,
        book: &mut Self::Book,
        order: &Reservation<'_>,
        settings: &SettingsValues,
    ) -> Result<(), TradeError> {
        let model = self.model_for(settings)?;
        let (asset, amount) = held_by(&model, order)?;
        book.lock(&asset, amount)
    }

    fn release(
        &self,
        book: &mut Self::Book,
        order: &Reservation<'_>,
        settings: &SettingsValues,
    ) -> Result<(), TradeError> {
        let model = self.model_for(settings)?;
        let (asset, amount) = held_by(&model, order)?;
        book.release(&asset, amount)
    }

    fn slippage_bps(&self, _settings: &SettingsValues) -> i64 {
        0
    }

    /// A spot fee is charged in the asset the trade produces, so it is a
    /// different currency on a buy than on a sell — which is exactly why
    /// this is asked per fill rather than read off the account.
    fn fee_currency(&self, settings: &SettingsValues, side: senken_trade::OrderSide) -> String {
        self.model_for(settings).map_or_else(
            |_| "USDT".to_owned(),
            |model| model.fee_currency(side).to_owned(),
        )
    }
}

/// What one resting order holds, and in which asset.
///
/// A buy is held in quote at the price it waits at; a sell in base at its
/// own size. A kind carrying no price — a plain stop, whose fill price is
/// not known until it triggers — holds nothing, because reserving against
/// a price nobody has yet would be inventing the number.
fn held_by(model: &Spot, order: &Reservation<'_>) -> Result<(String, i64), TradeError> {
    let price = match order.kind {
        senken_trade::OrderKind::Limit { price }
        | senken_trade::OrderKind::StopLimit { price, .. } => Some(price),
        _ => None,
    };
    Ok(match order.side {
        senken_trade::OrderSide::Buy => {
            let Some(price) = price else {
                return Ok((model.quote.clone(), 0));
            };
            (
                model.quote.clone(),
                rescale(
                    i128::from(price.value) * i128::from(order.quantity.value),
                    price.scale.saturating_add(order.quantity.scale),
                    model.asset_scale,
                )?,
            )
        }
        senken_trade::OrderSide::Sell => (
            model.base.clone(),
            rescale(
                i128::from(order.quantity.value),
                order.quantity.scale,
                model.asset_scale,
            )?,
        ),
    })
}

/// The adapter, ready to register.
pub type SpotAdapter = SimAdapter<SpotVenue>;

/// The instrument a spot account's pair names, for a caller that wants it.
#[must_use]
pub fn pair_of(settings: &SettingsValues) -> Option<InstrumentId> {
    let base = settings.text(KEY_BASE)?;
    let quote = settings.text(KEY_QUOTE)?;
    InstrumentId::parse(&format!("binance-spot:{base}{quote}")).ok()
}
