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
    AccessLevel, AccountAccess, AccountBalances, AdapterAction, AdapterCapabilities, AdapterHealth,
    AdapterKind, Fill, InstrumentCoverage, Order, Position, SettingsInput, SettingsSchema,
    TradeAccountSummary,
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

/// One account's resolved access — `senken_trade::AccountAccess` over the
/// wire, narrower than [`AdapterDto::capabilities`] when the venue
/// distinguishes a restricted login (MetaTrader 5's investor password, an
/// exchange key minted without trade scope).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AccountAccessDto {
    /// `trade` or `read_only`.
    #[schema(value_type = String)]
    pub level: AccessLevel,
    /// The adapter's capabilities, narrowed to this account.
    #[schema(value_type = Object)]
    pub capabilities: AdapterCapabilities,
    /// Product copy explaining a restriction, shown to the user. Absent
    /// when the account is unrestricted.
    pub note: Option<String>,
}

impl From<AccountAccess> for AccountAccessDto {
    fn from(access: AccountAccess) -> Self {
        Self {
            level: access.level,
            capabilities: access.capabilities,
            note: access.note,
        }
    }
}

/// `GET /api/trade/accounts/{account_id}` response body: the account, its
/// resolved access and its health, in the one round trip a screen needs —
/// replacing three a client previously had to make.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct TradeAccountStateDto {
    /// The account.
    pub account: TradeAccountDto,
    /// What this account may do right now.
    pub access: AccountAccessDto,
    /// Whether the account can be reached right now.
    #[schema(value_type = Object)]
    pub health: AdapterHealth,
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

/// One stored setting on its way out.
///
/// Mirrors [`senken_trade::SettingValue`]'s untagged shape so the wire
/// form is unchanged, with one correction: a decimal travels as
/// [`ScaledDto`], whose digits are a string. Serialising the domain type
/// directly — which this endpoint used to do — sent money as a JSON
/// *number* while every other endpoint sent a string, so a value past
/// 2^53 would have been rounded by the browser on the way in. See
/// [`WireInt`] for why that is the one thing this API may never do.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub(crate) enum SettingValueDto {
    /// A credential. Carries nothing at all, so it serialises as `null`
    /// and there is no field a future edit could accidentally fill with
    /// the real value.
    Secret,
    /// An on/off switch.
    Toggle(bool),
    /// A whole number, small by construction (a count, not money).
    Number(i64),
    /// A fixed-point decimal — digits as a string.
    Decimal(ScaledDto),
    /// Text, or the value of a chosen option.
    Text(String),
}

impl From<&senken_trade::SettingValue> for SettingValueDto {
    fn from(value: &senken_trade::SettingValue) -> Self {
        use senken_trade::SettingValue as V;
        match value {
            V::Secret(_) => Self::Secret,
            V::Toggle(flag) => Self::Toggle(*flag),
            V::Number(number) => Self::Number(*number),
            V::Decimal(scaled) => Self::Decimal((*scaled).into()),
            V::Text(text) => Self::Text(text.clone()),
            // `SettingValue` is `#[non_exhaustive]`: a variant added
            // upstream reaches here as text rather than silently vanishing
            // from a settings form.
            other => Self::Text(format!("{other:?}")),
        }
    }
}

/// Every stored value, converted for the wire.
///
/// `keys`/`get` rather than an `IntoIterator`, because
/// [`senken_trade::SettingsValues`] deliberately exposes no direct access
/// to its map.
pub(crate) fn settings_for_wire(
    values: &senken_trade::SettingsValues,
) -> std::collections::BTreeMap<String, SettingValueDto> {
    values
        .keys()
        .filter_map(|key| {
            values
                .get(key)
                .map(|value| (key.to_owned(), SettingValueDto::from(value)))
        })
        .collect()
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
    /// The stored values, credentials redacted to `null`, every decimal
    /// carrying its digits as a string.
    pub settings: std::collections::BTreeMap<String, SettingValueDto>,
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
    /// A stop loss to attach to the position this order opens, sent with
    /// the order rather than after it: a position that has to wait for a
    /// second request is unprotected for exactly as long as that takes.
    #[serde(default)]
    pub stop_loss: Option<ScaledDto>,
    /// A take profit to attach, on the same terms as
    /// [`stop_loss`](Self::stop_loss).
    #[serde(default)]
    pub take_profit: Option<ScaledDto>,
}

/// `POST /api/trade/accounts/{account_id}/close` request body.
///
/// The position travels in the body rather than the path: an id is opaque
/// venue text that may hold any character, and path-encoding it invites
/// exactly the double-decoding mistakes that make one endpoint disagree
/// with another about the same thing.
///
/// It names a **position**, not an instrument, because a hedging account
/// holds several on one instrument at once and "close BTCUSDT" has no
/// answer there. A client always has the position row in hand when it
/// offers a close, so it always has the id.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CloseRequest {
    /// The position to close, as the adapter reported its id.
    pub position_id: String,
}

/// `PATCH /api/trade/accounts/{account_id}/orders/{order_id}` request body.
///
/// Every field is optional; a field left absent leaves that part of the
/// order alone, exactly as [`senken_trade::OrderAmendment`] does — this is
/// that type's own shape over the wire, not a re-declaration of it.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AmendOrderRequest {
    /// The new size, replacing the order's current one.
    #[serde(default)]
    pub quantity: Option<ScaledDto>,
    /// The new resting price, for an order that has one.
    #[serde(default)]
    pub limit_price: Option<ScaledDto>,
    /// The new trigger price, for an order that has one.
    #[serde(default)]
    pub trigger_price: Option<ScaledDto>,
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
    use super::{ScaledDto, settings_for_wire};
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

    /// A settings decimal is money, and money leaves this API as digits in
    /// a string. It used to leave as a JSON number, because the response
    /// serialised the domain type directly — which put every stored
    /// balance through the browser's `double` on the way in.
    #[test]
    fn a_settings_decimal_leaves_as_a_string_like_every_other_money_field() {
        let mut values = senken_trade::SettingsValues::new();
        values.set(
            "starting_balance",
            senken_trade::SettingValue::Decimal(Scaled::new(2, 1_234_567_890)),
        );

        let wire = serde_json::to_value(settings_for_wire(&values)).unwrap();

        assert_eq!(
            wire["starting_balance"]["value"],
            serde_json::json!("1234567890")
        );
        assert!(
            wire["starting_balance"]["value"].is_string(),
            "a JSON number here is the defect this test exists for: {wire}"
        );
    }

    /// The reason the string matters, stated as an amount rather than as a
    /// principle: 2^53 + 1 is the first integer a `double` cannot tell
    /// from its neighbour, and a satoshi-scale balance passes it easily.
    #[test]
    fn a_settings_decimal_past_the_double_boundary_keeps_its_last_digit() {
        const PAST_EXACT_DOUBLE: i64 = 9_007_199_254_740_993; // 2^53 + 1

        let mut values = senken_trade::SettingsValues::new();
        values.set(
            "starting_balance",
            senken_trade::SettingValue::Decimal(Scaled::new(8, PAST_EXACT_DOUBLE)),
        );

        let wire = serde_json::to_value(settings_for_wire(&values)).unwrap();

        assert_eq!(
            wire["starting_balance"]["value"],
            serde_json::json!("9007199254740993")
        );
        // The same digits through a double, which is what a JSON number
        // would have become: the last one changes.
        #[expect(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            reason = "demonstrating the loss this test exists to prevent"
        )]
        let through_a_double = PAST_EXACT_DOUBLE as f64 as i64;
        assert_ne!(
            through_a_double, PAST_EXACT_DOUBLE,
            "if this ever passes, the boundary moved and this test needs a bigger number"
        );
    }

    /// A secret has no field to leak from: the wire variant carries no
    /// payload at all, so `null` is not a policy this layer applies, it is
    /// the only thing the type can produce.
    #[test]
    fn a_settings_secret_leaves_as_null() {
        let mut values = senken_trade::SettingsValues::new();
        values.set(
            "api_key",
            senken_trade::SettingValue::Secret(senken_trade::SecretString::new(
                "super-secret".to_owned(),
            )),
        );

        let wire = serde_json::to_value(settings_for_wire(&values)).unwrap();

        assert_eq!(wire["api_key"], serde_json::Value::Null);
        assert!(
            !serde_json::to_string(&wire)
                .unwrap()
                .contains("super-secret")
        );
    }
}
