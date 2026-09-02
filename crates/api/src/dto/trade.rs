//! Request and response bodies for the trade engine.
//!
//! # Scaled integers cross the wire as `(scale, value)`, not as numbers
//!
//! A price, a quantity, a balance and a fee all travel as a
//! [`ScaledDto`] — the integer and the number of fractional digits it is
//! expressed at. A JSON number would put every one of them through a
//! double on the way through the browser, and `0.1` does not survive that
//! exactly. The client formats from the pair; it never adds two of them.
//!
//! # An adapter's settings schema is served as-is
//!
//! [`AdapterDto::settings_schema`] is `senken_trade::SettingsSchema`'s own
//! serialisation, not a re-declaration of it. Copying that shape into a DTO
//! would create two definitions of one document that could drift, and the
//! client builds its form from whichever one it is served.

use senken_core::decimal::Scaled;
use senken_trade::{
    AccountBalances, AdapterAction, AdapterCapabilities, AdapterHealth, AdapterKind, Fill,
    InstrumentCoverage, Order, Position, SettingsInput, SettingsSchema, TradeAccountSummary,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// An `i64` that crosses the wire as a decimal string.
///
/// A JSON number would go through a double in the browser, and a quantity
/// at scale 8 can exceed what a double holds exactly. A size that changes
/// in its last digit on the way to a venue is the class of bug the whole
/// scaled-integer contract exists to prevent, so the digits travel as text.
#[derive(Debug, Clone, Copy, ToSchema)]
#[schema(value_type = String, example = "150")]
pub(crate) struct WireInt(pub i64);

impl Serialize for WireInt {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WireInt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// A plain number is accepted too: a hand-written request, or a
        /// client that has not been updated, should not fail on a value
        /// that is unambiguous anyway.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Text(String),
            Number(i64),
        }
        Ok(Self(match Either::deserialize(deserializer)? {
            Either::Text(text) => text.trim().parse().map_err(serde::de::Error::custom)?,
            Either::Number(value) => value,
        }))
    }
}

/// A fixed-point number: `value × 10^-scale`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct ScaledDto {
    /// How many of `value`'s digits are fractional.
    pub scale: u8,
    /// The integer itself, as a decimal string — see [`WireInt`].
    pub value: WireInt,
}

impl From<Scaled> for ScaledDto {
    fn from(scaled: Scaled) -> Self {
        Self {
            scale: scaled.scale,
            value: WireInt(scaled.value),
        }
    }
}

impl From<ScaledDto> for Scaled {
    fn from(dto: ScaledDto) -> Self {
        Self::new(dto.scale, dto.value.0)
    }
}

/// One registered adapter, with everything a client needs to render its
/// card, its settings form and its order ticket.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AdapterDto {
    /// The adapter's id.
    pub id: String,
    /// Its display name.
    pub name: String,
    /// Simulation, broker or exchange.
    #[schema(value_type = String)]
    pub kind: AdapterKind,
    /// One line for its card.
    pub description: String,
    /// `false` only for the simulator: whether orders through it reach a
    /// real venue. The one fact a client must not get wrong.
    pub trades_real_money: bool,
    /// What it can do.
    #[schema(value_type = Object)]
    pub capabilities: AdapterCapabilities,
    /// Which instruments it trades.
    #[schema(value_type = Object)]
    pub coverage: InstrumentCoverage,
    /// The form an account on it is configured through.
    #[schema(value_type = Object)]
    pub settings_schema: SettingsSchema,
    /// The custom operations it offers per account.
    #[schema(value_type = Object)]
    pub actions: Vec<AdapterAction>,
}

/// `GET /api/trade/adapters` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AdaptersResponse {
    /// Every registered adapter, in id order.
    pub adapters: Vec<AdapterDto>,
}

/// One attached account.
///
/// **Carries no settings.** A listing that included them would put every
/// user's API keys in a response an operator can request; reading settings
/// is its own endpoint, for the account's own owner.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TradeAccountDto {
    /// The account's id.
    pub id: String,
    /// Who attached it.
    pub owner_id: String,
    /// The adapter it trades through.
    pub adapter_id: String,
    /// The label its owner gave it.
    pub label: String,
    /// Whether it may be used.
    pub enabled: bool,
    /// `true` when this is the caller's own account — the only ones whose
    /// settings they can read or trade with.
    pub owned: bool,
    /// Unix timestamp of attachment.
    pub created_at: i64,
    /// Unix timestamp of the last change.
    pub updated_at: i64,
}

impl TradeAccountDto {
    pub(crate) fn from_summary(
        summary: TradeAccountSummary,
        caller: senken_identity::UserId,
    ) -> Self {
        Self {
            owned: summary.owner_id == caller,
            id: summary.id.to_string(),
            owner_id: summary.owner_id.to_string(),
            adapter_id: summary.adapter_id,
            label: summary.label,
            enabled: summary.enabled,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/trade/accounts` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TradeAccountsPage {
    /// The rows for this page.
    pub rows: Vec<TradeAccountDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/trade/accounts` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateTradeAccountRequest {
    /// Which adapter to attach to.
    pub adapter_id: String,
    /// What to call it.
    pub label: String,
    /// The settings form's values, validated server-side against the
    /// adapter's own schema.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub settings: SettingsInput,
}

/// `PATCH /api/trade/accounts/{account_id}` request body: whichever fields
/// are present are changed, the rest are left alone.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateTradeAccountRequest {
    /// A new label.
    #[serde(default)]
    pub label: Option<String>,
    /// Whether the account may be used.
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// `GET`/`PUT /api/trade/accounts/{account_id}/settings` response body.
///
/// Secret fields come back as `null` — that is
/// [`senken_trade::SecretString`]'s own serialisation and not something
/// this layer strips — with [`secrets_set`](Self::secrets_set) saying which
/// of them actually hold a credential, so a form can show "configured"
/// rather than an empty box that looks like the key was lost.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TradeAccountSettingsDto {
    /// The account.
    pub account: TradeAccountDto,
    /// The stored values, credentials redacted to `null`.
    #[schema(value_type = Object)]
    pub settings: senken_trade::SettingsValues,
    /// Which secret fields hold a credential.
    #[schema(value_type = Object)]
    pub secrets_set: std::collections::BTreeMap<String, bool>,
}

/// `PUT /api/trade/accounts/{account_id}/settings` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReplaceSettingsRequest {
    /// The form's values. A secret left absent or blank keeps whatever is
    /// stored.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub settings: SettingsInput,
}

/// One asset's balance.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AssetBalanceDto {
    /// The asset's ticker.
    pub asset: String,
    /// Everything held.
    pub total: ScaledDto,
    /// The part that can be spent.
    pub available: ScaledDto,
    /// The part held against orders and margin.
    pub reserved: ScaledDto,
}

/// `GET /api/trade/accounts/{account_id}/balances` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BalancesDto {
    /// The currency the figures below are in.
    pub currency: String,
    /// Cash, excluding unrealised profit.
    pub balance: ScaledDto,
    /// Cash plus unrealised profit.
    pub equity: ScaledDto,
    /// Unrealised profit across every open position.
    pub unrealized_pnl: ScaledDto,
    /// Margin held against positions and orders.
    pub margin_used: Option<ScaledDto>,
    /// Margin still available.
    pub margin_available: Option<ScaledDto>,
    /// Per-asset rows, for venues that have them.
    pub assets: Vec<AssetBalanceDto>,
}

impl From<AccountBalances> for BalancesDto {
    fn from(balances: AccountBalances) -> Self {
        Self {
            currency: balances.currency,
            balance: balances.balance.into(),
            equity: balances.equity.into(),
            unrealized_pnl: balances.unrealized_pnl.into(),
            margin_used: balances.margin_used.map(Into::into),
            margin_available: balances.margin_available.map(Into::into),
            assets: balances
                .assets
                .into_iter()
                .map(|asset| AssetBalanceDto {
                    asset: asset.asset,
                    total: asset.total.into(),
                    available: asset.available.into(),
                    reserved: asset.reserved.into(),
                })
                .collect(),
        }
    }
}

/// One open position.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PositionDto {
    /// The account holding it.
    pub account_id: String,
    /// The instrument, as `source:symbol`.
    pub instrument: String,
    /// `long` or `short`.
    #[schema(value_type = String)]
    pub side: senken_trade::PositionSide,
    /// Size held.
    pub quantity: ScaledDto,
    /// Volume-weighted entry.
    pub average_entry: ScaledDto,
    /// The price it is marked at, when one is available.
    pub mark_price: Option<ScaledDto>,
    /// Profit if closed at the mark. Absent whenever the mark is — never a
    /// zero standing in for "unknown".
    pub unrealized_pnl: Option<ScaledDto>,
    /// Profit already banked on this instrument.
    pub realized_pnl: ScaledDto,
    /// Margin held against it.
    pub margin: Option<ScaledDto>,
    /// Leverage applied.
    pub leverage: Option<ScaledDto>,
    /// When it was opened, as Unix nanoseconds.
    pub opened_at: i64,
}

impl From<Position> for PositionDto {
    fn from(position: Position) -> Self {
        Self {
            account_id: position.account_id.to_string(),
            instrument: position.instrument.to_string(),
            side: position.side,
            quantity: position.quantity.into(),
            average_entry: position.average_entry.into(),
            mark_price: position.mark_price.map(Into::into),
            unrealized_pnl: position.unrealized_pnl.map(Into::into),
            realized_pnl: position.realized_pnl.into(),
            margin: position.margin.map(Into::into),
            leverage: position.leverage.map(Into::into),
            opened_at: position.opened_at.as_nanos(),
        }
    }
}

/// One order.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct OrderDto {
    /// The venue's own id.
    pub id: String,
    /// The idempotency key it was sent with.
    pub client_order_id: Option<String>,
    /// The account.
    pub account_id: String,
    /// The instrument, as `source:symbol`.
    pub instrument: String,
    /// `buy` or `sell`.
    #[schema(value_type = String)]
    pub side: senken_trade::OrderSide,
    /// `market`, `limit`, `stop` or `stop_limit`.
    #[schema(value_type = String)]
    pub kind: senken_trade::OrderKindTag,
    /// The resting price, for the kinds that have one.
    pub limit_price: Option<ScaledDto>,
    /// The trigger price, for the kinds that have one.
    pub trigger_price: Option<ScaledDto>,
    /// The size asked for.
    pub quantity: ScaledDto,
    /// How much has filled.
    pub filled_quantity: ScaledDto,
    /// The average price of everything filled.
    pub average_price: Option<ScaledDto>,
    /// How long it lives.
    #[schema(value_type = String)]
    pub time_in_force: senken_trade::TimeInForce,
    /// Where it has got to.
    #[schema(value_type = String)]
    pub status: senken_trade::OrderStatus,
    /// Whether it may only shrink a position.
    pub reduce_only: bool,
    /// When it was submitted, as Unix nanoseconds.
    pub submitted_at: i64,
    /// When it last changed, as Unix nanoseconds.
    pub updated_at: i64,
    /// Why it was rejected, when it was.
    pub reject_reason: Option<String>,
}

impl From<Order> for OrderDto {
    fn from(order: Order) -> Self {
        Self {
            id: order.id.to_string(),
            client_order_id: order.client_order_id.map(|id| id.as_str().to_owned()),
            account_id: order.account_id.to_string(),
            instrument: order.instrument.to_string(),
            side: order.side,
            kind: order.kind.tag(),
            limit_price: order.kind.limit_price().map(Into::into),
            trigger_price: order.kind.trigger_price().map(Into::into),
            quantity: order.quantity.into(),
            filled_quantity: order.filled_quantity.into(),
            average_price: order.average_price.map(Into::into),
            time_in_force: order.time_in_force,
            status: order.status,
            reduce_only: order.reduce_only,
            submitted_at: order.submitted_at.as_nanos(),
            updated_at: order.updated_at.as_nanos(),
            reject_reason: order.reject_reason,
        }
    }
}

/// One execution.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct FillDto {
    /// The execution's own id.
    pub id: String,
    /// The order it filled.
    pub order_id: String,
    /// The account.
    pub account_id: String,
    /// The instrument, as `source:symbol`.
    pub instrument: String,
    /// `buy` or `sell`.
    #[schema(value_type = String)]
    pub side: senken_trade::OrderSide,
    /// How much traded.
    pub quantity: ScaledDto,
    /// At what price.
    pub price: ScaledDto,
    /// The fee charged.
    pub fee: ScaledDto,
    /// The asset the fee was charged in.
    pub fee_currency: String,
    /// `maker` or `taker`.
    #[schema(value_type = String)]
    pub liquidity: senken_trade::Liquidity,
    /// When it executed, as Unix nanoseconds.
    pub executed_at: i64,
}

impl From<Fill> for FillDto {
    fn from(fill: Fill) -> Self {
        Self {
            id: fill.id.to_string(),
            order_id: fill.order_id.to_string(),
            account_id: fill.account_id.to_string(),
            instrument: fill.instrument.to_string(),
            side: fill.side,
            quantity: fill.quantity.into(),
            price: fill.price.into(),
            fee: fill.fee.into(),
            fee_currency: fill.fee_currency,
            liquidity: fill.liquidity,
            executed_at: fill.executed_at.as_nanos(),
        }
    }
}

/// `POST /api/trade/accounts/{account_id}/orders` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PlaceOrderRequest {
    /// The instrument to trade, as `source:symbol`.
    pub instrument: String,
    /// `buy` or `sell`.
    #[schema(value_type = String)]
    pub side: senken_trade::OrderSide,
    /// `market`, `limit`, `stop` or `stop_limit`.
    #[schema(value_type = String)]
    pub kind: senken_trade::OrderKindTag,
    /// How much, in the adapter's own quantity unit.
    pub quantity: ScaledDto,
    /// Required for `limit` and `stop_limit`, refused otherwise.
    #[serde(default)]
    pub limit_price: Option<ScaledDto>,
    /// Required for `stop` and `stop_limit`, refused otherwise.
    #[serde(default)]
    pub trigger_price: Option<ScaledDto>,
    /// How long the order lives.
    #[serde(default)]
    #[schema(value_type = String)]
    pub time_in_force: senken_trade::TimeInForce,
    /// Only allowed to shrink an existing position.
    #[serde(default)]
    pub reduce_only: bool,
    /// Refuse the order rather than let it take liquidity.
    #[serde(default)]
    pub post_only: bool,
    /// A caller-chosen idempotency key.
    #[serde(default)]
    pub client_order_id: Option<String>,
}

/// `GET /api/trade/accounts/{account_id}/health` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct HealthDto {
    /// `connected`, `degraded` or `disconnected`, with a reason for the
    /// latter two.
    #[schema(value_type = Object)]
    pub health: AdapterHealth,
}

/// `POST /api/trade/accounts/{account_id}/actions/{action_id}` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RunActionRequest {
    /// The action form's values, validated server-side against the action's
    /// own form.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub params: SettingsInput,
}

/// `POST /api/trade/accounts/{account_id}/actions/{action_id}` response
/// body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActionOutcomeDto {
    /// One line of product copy describing what happened.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::ScaledDto;
    use senken_core::decimal::Scaled;

    #[test]
    fn a_scaled_value_crosses_the_wire_as_a_string_not_a_number() {
        // A quantity at scale 8 can exceed what a double holds exactly; a
        // JSON number would let the browser round it.
        let json =
            serde_json::to_string(&ScaledDto::from(Scaled::new(8, 9_007_199_254_740_993))).unwrap();
        assert_eq!(json, r#"{"scale":8,"value":"9007199254740993"}"#);
    }

    #[test]
    fn a_scaled_value_round_trips_through_json_without_losing_a_digit() {
        let original = Scaled::new(8, 9_007_199_254_740_993);
        let json = serde_json::to_string(&ScaledDto::from(original)).unwrap();
        let parsed: ScaledDto = serde_json::from_str(&json).unwrap();
        assert_eq!(Scaled::from(parsed), original);
    }

    #[test]
    fn a_plain_json_number_is_still_accepted_on_the_way_in() {
        let parsed: ScaledDto = serde_json::from_str(r#"{"scale":2,"value":150}"#).unwrap();
        assert_eq!(Scaled::from(parsed), Scaled::new(2, 150));
    }
}
