//! The trade engine over HTTP.
//!
//! Every handler here follows the same three steps, and the order matters:
//!
//! 1. `senken_trade::TradeAccountStore` resolves the account, performing
//!    its own guarded check. Reading a portfolio goes through
//!    `account`/`settings_for`; anything that moves money goes through
//!    `account_for_trading`, which is owner-only whatever grants the caller
//!    holds.
//! 2. `senken_trade::TradeEngine` validates the request against the
//!    adapter's declared capabilities and the instrument's tick and step.
//! 3. The adapter is called.
//!
//! No handler here re-implements a permission rule. The store is where
//! authorisation lives, so a headless caller — a backtest, a strategy
//! runner — gets the same answer with no HTTP layer to inherit it from.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use senken_marketdata::InstrumentId;
use senken_series::Clock;
use senken_trade::{
    AccountRef, ClientOrderId, OrderAmendment, OrderFilter, OrderId, OrderKind, OrderKindTag,
    OrderRequest, SettingsValues, TradeAccountId, TradeAdapter, TradeContext, TradeError,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    ActionOutcomeDto, AdapterDto, AdaptersResponse, AmendOrderRequest, BalancesDto, CloseRequest,
    CreateTradeAccountRequest, FillDto, HealthDto, IdResponse, OrderDto, PlaceOrderRequest,
    PositionDto, ReplaceSettingsRequest, RunActionRequest, TradeAccountDto,
    TradeAccountSettingsDto, TradeAccountStateDto, TradeAccountsPage, UpdateTradeAccountRequest,
};
use crate::pagination::{PaginationQuery, normalize_pagination};
use crate::trade_context::{CatalogInstruments, StoredMarkPrice};

/// How many executions a fills listing returns.
const FILLS_LIMIT: usize = 200;

fn parse_account_id(raw: &str) -> Result<TradeAccountId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid account id".to_owned()))
}

/// Builds the per-call context an adapter is given.
///
/// One `now` for the whole request, so every timestamp an adapter stamps
/// during it agrees.
fn context<'a>(
    marks: &'a StoredMarkPrice,
    instruments: &'a CatalogInstruments,
) -> TradeContext<'a> {
    // The same value answers for marks and for bars, so an adapter
    // settling a book through time and one asking the current price are
    // reading the same installation's data.
    TradeContext::new(senken_loader::SystemClock.now(), marks, instruments).with_history(marks)
}

/// The account and the adapter behind it, for a **read**.
///
/// Uses `settings_for`, so a caller who does not own the account is told it
/// does not exist rather than shown someone else's portfolio.
fn resolve_for_read(
    state: &AppState,
    ctx: &crate::auth::AuthContext,
    id: TradeAccountId,
) -> Result<
    (
        Arc<dyn TradeAdapter>,
        senken_trade::TradeAccountSummary,
        SettingsValues,
    ),
    HandlerError,
> {
    let (account, settings) = state.trade_accounts.settings_for(&ctx.user, id)?;
    let adapter = state
        .runtime
        .trade()
        .adapter(&account.adapter_id)
        .map_err(HandlerError::from)?
        .clone();
    Ok((adapter, account, settings))
}

/// The account and the adapter behind it, for something that **moves
/// money**.
///
/// Uses `account_for_trading`: owner-only, and refuses a disabled account.
fn resolve_for_trading(
    state: &AppState,
    ctx: &crate::auth::AuthContext,
    id: TradeAccountId,
) -> Result<
    (
        Arc<dyn TradeAdapter>,
        senken_trade::TradeAccountSummary,
        SettingsValues,
    ),
    HandlerError,
> {
    let (account, settings) = state.trade_accounts.account_for_trading(&ctx.user, id)?;
    let adapter = state
        .runtime
        .trade()
        .adapter(&account.adapter_id)
        .map_err(HandlerError::from)?
        .clone();
    Ok((adapter, account, settings))
}

/// `GET /api/trade/adapters`: every registered adapter, with its settings
/// schema, actions and capabilities.
///
/// Needs a session but no grant: this is the catalogue of what *could* be
/// attached, carries no account data at all, and a client has to render the
/// "attach an account" screen before it can know whether the user may use
/// one.
#[utoipa::path(
    get,
    path = "/api/trade/adapters",
    responses(
        (status = 200, body = AdaptersResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_adapters(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
) -> Json<AdaptersResponse> {
    let adapters = state
        .runtime
        .trade()
        .adapters()
        .map(|adapter| AdapterDto {
            id: adapter.id().to_owned(),
            name: adapter.name().to_owned(),
            kind: adapter.kind(),
            description: adapter.description().to_owned(),
            trades_real_money: adapter.kind().trades_real_money(),
            capabilities: adapter.capabilities(),
            coverage: adapter.coverage(),
            settings_schema: adapter.settings_schema(),
            actions: adapter.actions(),
        })
        .collect();
    Json(AdaptersResponse { adapters })
}

/// `GET /api/trade/accounts`. Scoped by the store itself, `total` included.
#[utoipa::path(
    get,
    path = "/api/trade/accounts",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = TradeAccountsPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_accounts(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<TradeAccountsPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state
        .trade_accounts
        .list_accounts(&ctx.user, limit, offset)?;
    let caller = ctx.user.user_id();
    Ok(Json(TradeAccountsPage {
        rows: page
            .rows
            .into_iter()
            .map(|row| TradeAccountDto::from_summary(row, caller))
            .collect(),
        total: page.total,
    }))
}

/// `POST /api/trade/accounts`: attaches an account and asks its adapter to
/// prepare it.
///
/// The adapter's `open_account` runs after the row is written, so a
/// credential the venue rejects is reported with the account already
/// saved — the alternative is a user re-typing every other field because
/// one key had a typo.
#[utoipa::path(
    post,
    path = "/api/trade/accounts",
    request_body = CreateTradeAccountRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_account(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateTradeAccountRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let adapter = state
        .runtime
        .trade()
        .adapter(&body.adapter_id)
        .map_err(HandlerError::from)?
        .clone();

    let id = state.trade_accounts.create_account(
        &ctx.user,
        &body.adapter_id,
        &body.label,
        &adapter.settings_schema(),
        &body.settings,
    )?;

    let (_, settings) = state.trade_accounts.settings_for(&ctx.user, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    if let Err(error) = adapter
        .open_account(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &body.label,
                settings: &settings,
            },
        )
        .await
    {
        tracing::warn!(%error, adapter = body.adapter_id, "adapter refused a newly attached account");
        return Err(error.into());
    }

    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/trade/accounts/{account_id}`: the account, its resolved access
/// and its health, in one round trip — what an account screen needs on
/// open, where three separate requests answered the same question before.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = TradeAccountStateDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_state(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<TradeAccountStateDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let call_ctx = context(&marks, &instruments);
    let account_ref = AccountRef {
        id,
        label: &account.label,
        settings: &settings,
    };
    let access = adapter.account_access(&call_ctx, account_ref).await?;
    let health = adapter.health(&call_ctx, account_ref).await?;
    Ok(Json(TradeAccountStateDto {
        account: TradeAccountDto::from_summary(account, ctx.user.user_id()),
        access: access.into(),
        health,
    }))
}

/// `PATCH /api/trade/accounts/{account_id}`: rename, or enable/disable.
#[utoipa::path(
    patch,
    path = "/api/trade/accounts/{account_id}",
    request_body = UpdateTradeAccountRequest,
    params(("account_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn update_account(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
    Json(body): Json<UpdateTradeAccountRequest>,
) -> Result<StatusCode, HandlerError> {
    let id = parse_account_id(&account_id)?;
    state
        .trade_accounts
        .update_account(&ctx.user, id, body.label.as_deref(), body.enabled)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/trade/accounts/{account_id}`.
///
/// The adapter is told first, but a refusal does not block the deletion: a
/// user removing an account must not be held hostage by a venue that is
/// down.
#[utoipa::path(
    delete,
    path = "/api/trade/accounts/{account_id}",
    params(("account_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_account(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let id = parse_account_id(&account_id)?;
    if let Ok((adapter, account, settings)) = resolve_for_read(&state, &ctx, id)
        && let Err(error) = adapter
            .close_account(AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            })
            .await
    {
        tracing::warn!(%error, adapter = account.adapter_id, "adapter failed to release an account");
    }
    state.trade_accounts.delete_account(&ctx.user, id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/trade/accounts/{account_id}/settings`: the stored settings,
/// **credentials redacted to `null`**, for the account's own owner.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/settings",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = TradeAccountSettingsDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_settings(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<TradeAccountSettingsDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let secrets_set = settings.secret_status(&adapter.settings_schema());
    Ok(Json(TradeAccountSettingsDto {
        account: TradeAccountDto::from_summary(account, ctx.user.user_id()),
        settings: crate::dto::settings_for_wire(&settings),
        secrets_set,
    }))
}

/// `PUT /api/trade/accounts/{account_id}/settings`.
///
/// A secret left absent or blank keeps whatever is stored — the store
/// applies that before validating, so a required credential already on file
/// does not have to be re-typed to save an unrelated change.
#[utoipa::path(
    put,
    path = "/api/trade/accounts/{account_id}/settings",
    request_body = ReplaceSettingsRequest,
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = TradeAccountSettingsDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn replace_settings(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
    Json(body): Json<ReplaceSettingsRequest>,
) -> Result<Json<TradeAccountSettingsDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, _) = resolve_for_read(&state, &ctx, id)?;
    let schema = adapter.settings_schema();
    let settings = state
        .trade_accounts
        .replace_settings(&ctx.user, id, &schema, body.settings)?;

    // Idempotent by contract, so re-running it on every settings change is
    // how an adapter learns about the change without a second hook.
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    adapter
        .open_account(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
        )
        .await?;

    let secrets_set = settings.secret_status(&schema);
    Ok(Json(TradeAccountSettingsDto {
        account: TradeAccountDto::from_summary(account, ctx.user.user_id()),
        settings: crate::dto::settings_for_wire(&settings),
        secrets_set,
    }))
}

/// `GET /api/trade/accounts/{account_id}/health`.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/health",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = HealthDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_health(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<HealthDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let health = adapter
        .health(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
        )
        .await?;
    Ok(Json(HealthDto { health }))
}

/// `GET /api/trade/accounts/{account_id}/balances`.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/balances",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = BalancesDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_balances(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<BalancesDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let balances = adapter
        .balances(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
        )
        .await?;
    Ok(Json(balances.into()))
}

/// `GET /api/trade/accounts/{account_id}/positions`.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/positions",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = Vec<PositionDto>),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_positions(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<PositionDto>>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let positions = adapter
        .positions(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
        )
        .await?;
    Ok(Json(positions.into_iter().map(Into::into).collect()))
}

/// Which orders `GET .../orders` should return.
#[derive(Debug, Deserialize)]
pub(crate) struct OrdersQuery {
    /// `open` (default) or `all`.
    #[serde(default)]
    status: Option<String>,
}

/// `GET /api/trade/accounts/{account_id}/orders`.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/orders",
    params(
        ("account_id" = String, Path),
        ("status" = Option<String>, Query, description = "`open` (default) or `all`"),
    ),
    responses(
        (status = 200, body = Vec<OrderDto>),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_orders(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
    Query(query): Query<OrdersQuery>,
) -> Result<Json<Vec<OrderDto>>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let filter = match query.status.as_deref() {
        None | Some("open") => OrderFilter::Open,
        Some("all") => OrderFilter::All,
        Some(other) => {
            return Err(HandlerError::BadRequest(format!(
                "`{other}` is not `open` or `all`"
            )));
        }
    };
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let orders = adapter
        .orders(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            filter,
        )
        .await?;
    Ok(Json(orders.into_iter().map(Into::into).collect()))
}

/// `GET /api/trade/accounts/{account_id}/fills`.
#[utoipa::path(
    get,
    path = "/api/trade/accounts/{account_id}/fills",
    params(("account_id" = String, Path)),
    responses(
        (status = 200, body = Vec<FillDto>),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn account_fills(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<FillDto>>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_read(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let fills = adapter
        .fills(
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            FILLS_LIMIT,
        )
        .await?;
    Ok(Json(fills.into_iter().map(Into::into).collect()))
}

/// Turns the request's flat `kind` + optional prices into the
/// [`OrderKind`] whose variant carries exactly the prices that kind needs.
///
/// A limit order with no price, or a market order carrying one, is rejected
/// here rather than silently ignored: the domain type cannot represent
/// either, and quietly dropping a price the user typed is how an order goes
/// out at the market instead of the level they meant.
fn order_kind(body: &PlaceOrderRequest) -> Result<OrderKind, HandlerError> {
    let limit = body.limit_price.map(Into::into);
    let trigger = body.trigger_price.map(Into::into);
    let refuse =
        |what: &str| HandlerError::BadRequest(format!("a {what} order does not take that price"));
    let require = |what: &str, which: &str| {
        HandlerError::BadRequest(format!("a {what} order needs a {which} price"))
    };
    match body.kind {
        OrderKindTag::Market => {
            if limit.is_some() || trigger.is_some() {
                return Err(refuse("market"));
            }
            Ok(OrderKind::Market)
        }
        OrderKindTag::Limit => {
            if trigger.is_some() {
                return Err(refuse("limit"));
            }
            Ok(OrderKind::Limit {
                price: limit.ok_or_else(|| require("limit", "limit"))?,
            })
        }
        OrderKindTag::Stop => {
            if limit.is_some() {
                return Err(refuse("stop"));
            }
            Ok(OrderKind::Stop {
                trigger: trigger.ok_or_else(|| require("stop", "trigger"))?,
            })
        }
        OrderKindTag::StopLimit => Ok(OrderKind::StopLimit {
            trigger: trigger.ok_or_else(|| require("stop-limit", "trigger"))?,
            price: limit.ok_or_else(|| require("stop-limit", "limit"))?,
        }),
        // `OrderKindTag` is `#[non_exhaustive]`: a kind this build cannot
        // assemble a request for is refused, never guessed at.
        other => Err(HandlerError::BadRequest(format!(
            "{other:?} orders are not supported by this server"
        ))),
    }
}

/// `POST /api/trade/accounts/{account_id}/orders`: places an order.
///
/// Owner-only, through `account_for_trading` — an operator holding
/// `Account`/`All` can see this account exists and still cannot spend from
/// it.
#[utoipa::path(
    post,
    path = "/api/trade/accounts/{account_id}/orders",
    request_body = PlaceOrderRequest,
    params(("account_id" = String, Path)),
    responses(
        (status = 201, body = OrderDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn place_order(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
    Json(body): Json<PlaceOrderRequest>,
) -> Result<(StatusCode, Json<OrderDto>), HandlerError> {
    let id = parse_account_id(&account_id)?;
    let instrument = InstrumentId::parse(&body.instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let kind = order_kind(&body)?;
    let client_order_id = body
        .client_order_id
        .as_deref()
        .map(ClientOrderId::new)
        .transpose()
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;

    let (_, account, settings) = resolve_for_trading(&state, &ctx, id)?;
    let request = OrderRequest {
        instrument,
        side: body.side,
        kind,
        quantity: body.quantity.into(),
        time_in_force: body.time_in_force,
        reduce_only: body.reduce_only,
        post_only: body.post_only,
        client_order_id,
        stop_loss: body.stop_loss.map(Into::into),
        take_profit: body.take_profit.map(Into::into),
    };

    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let order = state
        .runtime
        .trade()
        .place_order(
            &account.adapter_id,
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            request,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(order.into())))
}

/// `POST /api/trade/accounts/{account_id}/close`: closes an open position
/// by sending an opposite market order for exactly the size the adapter
/// reports right now.
///
/// Owner-only, through `account_for_trading`, exactly like `place_order` —
/// this is itself an order, and moves money the same way.
#[utoipa::path(
    post,
    path = "/api/trade/accounts/{account_id}/close",
    request_body = CloseRequest,
    params(("account_id" = String, Path)),
    responses(
        (status = 201, body = OrderDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn close_position(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(account_id): Path<String>,
    Json(body): Json<CloseRequest>,
) -> Result<(StatusCode, Json<OrderDto>), HandlerError> {
    let id = parse_account_id(&account_id)?;
    let position_id = senken_trade::PositionId::new(body.position_id);

    let (_, account, settings) = resolve_for_trading(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let order = state
        .runtime
        .trade()
        .close_position(
            &account.adapter_id,
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            &position_id,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(order.into())))
}

/// `DELETE /api/trade/accounts/{account_id}/orders/{order_id}`: cancels a
/// resting order.
#[utoipa::path(
    delete,
    path = "/api/trade/accounts/{account_id}/orders/{order_id}",
    params(("account_id" = String, Path), ("order_id" = String, Path)),
    responses(
        (status = 200, body = OrderDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn cancel_order(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path((account_id, order_id)): Path<(String, String)>,
) -> Result<Json<OrderDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (_, account, settings) = resolve_for_trading(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let order = state
        .runtime
        .trade()
        .cancel_order(
            &account.adapter_id,
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            &OrderId::new(order_id),
        )
        .await?;
    Ok(Json(order.into()))
}

/// `PATCH /api/trade/accounts/{account_id}/orders/{order_id}`: amends a
/// resting order's size, limit price or trigger price in place.
#[utoipa::path(
    patch,
    path = "/api/trade/accounts/{account_id}/orders/{order_id}",
    request_body = AmendOrderRequest,
    params(("account_id" = String, Path), ("order_id" = String, Path)),
    responses(
        (status = 200, body = OrderDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn amend_order(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path((account_id, order_id)): Path<(String, String)>,
    Json(body): Json<AmendOrderRequest>,
) -> Result<Json<OrderDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (_, account, settings) = resolve_for_trading(&state, &ctx, id)?;
    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let amendment = OrderAmendment {
        quantity: body.quantity.map(Into::into),
        limit_price: body.limit_price.map(Into::into),
        trigger_price: body.trigger_price.map(Into::into),
    };
    let order = state
        .runtime
        .trade()
        .modify_order(
            &account.adapter_id,
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            &OrderId::new(order_id),
            amendment,
        )
        .await?;
    Ok(Json(order.into()))
}

/// `POST /api/trade/accounts/{account_id}/actions/{action_id}`: runs one of
/// the adapter's own operations.
///
/// The parameters are validated against the action's declared form here,
/// server-side, whatever the client already checked. Owner-only: an
/// adapter's actions can move money (the simulator's own deposit does).
#[utoipa::path(
    post,
    path = "/api/trade/accounts/{account_id}/actions/{action_id}",
    request_body = RunActionRequest,
    params(("account_id" = String, Path), ("action_id" = String, Path)),
    responses(
        (status = 200, body = ActionOutcomeDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn run_action(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path((account_id, action_id)): Path<(String, String)>,
    Json(body): Json<RunActionRequest>,
) -> Result<Json<ActionOutcomeDto>, HandlerError> {
    let id = parse_account_id(&account_id)?;
    let (adapter, account, settings) = resolve_for_trading(&state, &ctx, id)?;
    let action = adapter
        .actions()
        .into_iter()
        .find(|action| action.id == action_id)
        .ok_or_else(|| {
            HandlerError::BadRequest(format!(
                "`{}` has no action called `{action_id}`",
                account.adapter_id
            ))
        })?;
    let params = action
        .form
        .validate(&body.params)
        .map_err(TradeError::from)?;

    let marks = StoredMarkPrice::new(state.clone());
    let instruments = CatalogInstruments::new(state.clone());
    let outcome = state
        .runtime
        .trade()
        .run_action(
            &account.adapter_id,
            &context(&marks, &instruments),
            AccountRef {
                id,
                label: &account.label,
                settings: &settings,
            },
            &action_id,
            params,
        )
        .await?;
    Ok(Json(ActionOutcomeDto {
        message: outcome.message,
    }))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, DEFAULT_ADMIN_EMAIL, IdentityStore};
    use senken_runtime::Runtime;
    use tempfile::TempDir;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
        serve_unfenced_test_server_with,
    };

    const USER_PASSWORD: &str = "a very long password";

    /// A runtime with the built-in simulator registered — the whole
    /// activation path a real server takes, not a stub adapter, so these
    /// tests exercise the plugin contract as well as the HTTP one.
    fn runtime_with_simulator() -> (TempDir, Runtime) {
        let dir = TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(senken_plugin_simulator::SimulatorPlugin::new(
                senken_storage::Storage::new(dir.path()),
            ))
            .build()
            .unwrap();
        (dir, runtime)
    }

    async fn login_token(addr: SocketAddr, email: &str, password: &str) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn admin_of(identity: &IdentityStore) -> AuthenticatedUser {
        let (_uid, session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        identity.resolve_session(session.reveal()).unwrap().unwrap()
    }

    /// A trader: `Account` and `Order` at `Scope::Own`.
    async fn trader(
        addr: SocketAddr,
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Trader", Some(USER_PASSWORD))
            .unwrap();
        for resource in [Resource::Account, Resource::Order] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        login_token(addr, email, USER_PASSWORD).await
    }

    async fn attach_simulator(addr: SocketAddr, token: &str, label: &str) -> String {
        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts"),
            token,
            serde_json::json!({
                "adapter_id": "simulator",
                "label": label,
                "settings": { "starting_balance": "50000.00", "currency": "USD" }
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        body_json(response).await["id"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn the_adapter_listing_carries_the_schema_a_client_builds_its_form_from() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;

        let body =
            body_json(get_auth(format!("http://{addr}/api/trade/adapters"), &token).await).await;

        let adapter = &body["adapters"][0];
        assert_eq!(adapter["id"], "simulator");
        assert_eq!(adapter["kind"], "simulation");
        assert_eq!(
            adapter["trades_real_money"], false,
            "the one fact a client must not get wrong about a simulated account"
        );
        assert_eq!(adapter["coverage"]["coverage"], "universal");
        let fields = adapter["settings_schema"]["fields"].as_array().unwrap();
        assert!(
            fields.iter().any(|field| field["key"] == "starting_balance"
                && field["type"] == "decimal"
                && field["scale"] == 2),
            "a money setting must reach the client as a scaled decimal, not a float: {fields:?}"
        );
        assert!(
            adapter["actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action["id"] == "reset" && action["destructive"] == true)
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_account_is_attached_listed_renamed_and_detached_over_http() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;

        let id = attach_simulator(addr, &token, "Growth").await;

        let page =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts"), &token).await).await;
        assert_eq!(page["total"], 1);
        assert_eq!(page["rows"][0]["label"], "Growth");
        assert_eq!(page["rows"][0]["owned"], true);

        let rename = reqwest::Client::new()
            .patch(format!("http://{addr}/api/trade/accounts/{id}"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "label": "Renamed" })).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(rename.status(), reqwest::StatusCode::NO_CONTENT);

        let delete = reqwest::Client::new()
            .delete(format!("http://{addr}/api/trade/accounts/{id}"))
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts"), &token).await).await;
        assert_eq!(page["total"], 0);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_starting_balance_reaches_the_adapter_and_comes_back_as_a_scaled_pair() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let balances = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{id}/balances"),
                &token,
            )
            .await,
        )
        .await;

        assert_eq!(balances["currency"], "USD");
        // 50 000.00 at the simulator's eight-decimal cash scale, as a
        // string — a JSON number would have gone through a double.
        assert_eq!(balances["balance"]["scale"], 8);
        assert_eq!(balances["balance"]["value"], "5000000000000");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn settings_come_back_with_the_credential_redacted_but_reported_as_absent() {
        // The simulator has no secret field, so this checks the shape a
        // client depends on: a `secrets_set` map that exists and is honest.
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let body = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{id}/settings"),
                &token,
            )
            .await,
        )
        .await;

        assert_eq!(body["settings"]["currency"], "USD");
        assert!(body["secrets_set"].is_object());

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_order_on_an_instrument_with_no_price_is_refused_with_a_fixable_message() {
        // The runtime here has no bar sources at all, so nothing has a
        // mark. The message has to name what the user can do about it
        // rather than being a bare failure.
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &token,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let message = body_json(response).await["error"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            message.contains("no instrument") || message.contains("no price"),
            "got {message}"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_limit_order_without_a_limit_price_is_refused_before_it_reaches_an_adapter() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &token,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "limit",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        assert!(
            body_json(response).await["error"]
                .as_str()
                .unwrap()
                .contains("needs a limit price")
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn one_user_cannot_see_or_trade_another_users_account_over_http() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let alice = trader(addr, &identity, &admin, "alice@example.com").await;
        let bob = trader(addr, &identity, &admin, "bob@example.com").await;
        let id = attach_simulator(addr, &alice, "Alice").await;

        let page =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts"), &bob).await).await;
        assert_eq!(page["total"], 0, "and the total must not leak it either");

        let settings = get_auth(
            format!("http://{addr}/api/trade/accounts/{id}/settings"),
            &bob,
        )
        .await;
        assert_eq!(settings.status(), reqwest::StatusCode::BAD_REQUEST);

        let order = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &bob,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;
        assert_eq!(order.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_admin_at_scope_all_sees_the_account_but_cannot_trade_it_over_http() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let alice = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &alice, "Alice").await;
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;

        let page =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts"), &admin_token).await)
                .await;
        assert_eq!(page["total"], 1, "an operator can see the account exists");
        assert_eq!(page["rows"][0]["owned"], false);

        let order = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &admin_token,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;
        assert_eq!(
            order.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "managing the platform is not the same authority as spending someone else's money"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_adapter_action_runs_through_its_declared_form() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/actions/deposit"),
            &token,
            serde_json::json!({ "params": { "amount": "250.00" } }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert!(
            body_json(response).await["message"]
                .as_str()
                .unwrap()
                .contains("250")
        );

        let balances = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{id}/balances"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(balances["balance"]["value"], "5025000000000");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_action_parameter_outside_its_declared_bounds_is_refused_server_side() {
        // The client's copy of the form is a courtesy to the person typing;
        // this is the check that actually holds.
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/actions/deposit"),
            &token,
            serde_json::json!({ "params": { "amount": "0.00" } }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_settings_key_the_adapter_does_not_declare_is_refused_rather_than_ignored() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts"),
            &token,
            serde_json::json!({
                "adapter_id": "simulator",
                "label": "Bad",
                "settings": { "unlimited_money": "true" }
            }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_second_account_with_the_same_name_on_one_adapter_conflicts() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        attach_simulator(addr, &token, "Growth").await;

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts"),
            &token,
            serde_json::json!({
                "adapter_id": "simulator",
                "label": "Growth",
                "settings": {}
            }),
        )
        .await;

        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_disabled_account_refuses_orders_while_still_listing() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        reqwest::Client::new()
            .patch(format!("http://{addr}/api/trade/accounts/{id}"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "enabled": false })).unwrap())
            .send()
            .await
            .unwrap();

        let order = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &token,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;
        assert_eq!(order.status(), reqwest::StatusCode::FORBIDDEN);

        let page =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts"), &token).await).await;
        assert_eq!(page["total"], 1);
        assert_eq!(page["rows"][0]["enabled"], false);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn getting_account_state_returns_the_account_its_access_and_its_health_together() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let id = attach_simulator(addr, &token, "Growth").await;

        let body =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts/{id}"), &token).await)
                .await;

        assert_eq!(body["account"]["label"], "Growth");
        assert_eq!(body["access"]["level"], "trade");
        assert_eq!(body["access"]["note"], serde_json::Value::Null);
        assert_eq!(body["health"]["state"], "connected");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn placing_an_order_on_a_read_only_account_is_forbidden_and_names_the_reason() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;

        let created = post_json_auth(
            format!("http://{addr}/api/trade/accounts"),
            &token,
            serde_json::json!({
                "adapter_id": "simulator",
                "label": "Investor",
                "settings": {
                    "starting_balance": "50000.00",
                    "currency": "USD",
                    "access": "read_only"
                }
            }),
        )
        .await;
        assert_eq!(created.status(), reqwest::StatusCode::CREATED);
        let id = body_json(created).await["id"].as_str().unwrap().to_owned();

        let order = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{id}/orders"),
            &token,
            serde_json::json!({
                "instrument": "okx-spot:BTCUSDT",
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 3, "value": "250" }
            }),
        )
        .await;
        assert_eq!(order.status(), reqwest::StatusCode::FORBIDDEN);
        let message = body_json(order).await["error"].as_str().unwrap().to_owned();
        assert!(
            message.contains("read-only"),
            "the 403 body must name the reason, got {message}"
        );

        // Reads are unaffected — the same account still answers a read with
        // 200, because a read-only account exists to be read.
        let positions = get_auth(
            format!("http://{addr}/api/trade/accounts/{id}/positions"),
            &token,
        )
        .await;
        assert_eq!(positions.status(), reqwest::StatusCode::OK);

        // The combined state endpoint reports the restriction too.
        let state =
            body_json(get_auth(format!("http://{addr}/api/trade/accounts/{id}"), &token).await)
                .await;
        assert_eq!(state["access"]["level"], "read_only");
        assert!(
            state["access"]["note"]
                .as_str()
                .unwrap()
                .contains("read-only")
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn every_trade_endpoint_refuses_an_anonymous_caller() {
        let (_runtime_dir, runtime) = runtime_with_simulator();
        let (handle, _identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();

        for path in ["/api/trade/adapters", "/api/trade/accounts"] {
            let response = reqwest::get(format!("http://{addr}{path}")).await.unwrap();
            assert_eq!(
                response.status(),
                reqwest::StatusCode::UNAUTHORIZED,
                "{path} must require a session"
            );
        }

        handle.shutdown().await.unwrap();
    }
}

#[cfg(test)]
mod end_to_end_tests {
    //! The whole trading path, with no stubbing between the halves: a fake
    //! venue's bars are fetched into the store, the engine reads a mark
    //! price back out of them, and a market order fills against that mark.
    //!
    //! This is the one test that exercises `trade_context::StoredMarkPrice`
    //! at all. Everything either side of it has its own unit tests; what
    //! only shows up here is whether the two actually meet.

    use std::net::SocketAddr;
    use std::time::Duration;

    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, DEFAULT_ADMIN_EMAIL, IdentityStore};
    use senken_series::Clock;

    use crate::bars_handlers::test_support::{
        runtime_with_fake_venue_and_simulator, test_instrument,
    };
    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
        serve_unfenced_test_server_with,
    };

    const USER_PASSWORD: &str = "a very long password";

    /// One minute in nanoseconds. The fake venue stamps its first bar at
    /// the requested range's own start, and the store refuses a 1-minute
    /// bar that does not sit on a minute boundary — as it should — so every
    /// range in this module is aligned to this.
    const MINUTE: i64 = 60 * 1_000_000_000;

    async fn login_token(addr: SocketAddr, email: &str, password: &str) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn admin_of(identity: &IdentityStore) -> AuthenticatedUser {
        let (_uid, session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        identity.resolve_session(session.reveal()).unwrap().unwrap()
    }

    async fn trader(
        addr: SocketAddr,
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Trader", Some(USER_PASSWORD))
            .unwrap();
        for resource in [Resource::Account, Resource::Order] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        login_token(addr, email, USER_PASSWORD).await
    }

    async fn wait_for_job(addr: SocketAddr, token: &str, job_id: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let body = body_json(
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), token).await,
                )
                .await;
                if body["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the backfill did not finish before the test's safety timeout");
    }

    #[tokio::test]
    async fn bars_in_the_store_become_the_mark_a_market_order_fills_against() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue_and_simulator(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = test_instrument();

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "E2E",
                    "settings": {
                        "starting_balance": "50000.00",
                        "fee_bps": 0,
                        "slippage_bps": 0
                    }
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Before any history exists, the order is refused by name rather
        // than filled at a guessed price.
        let too_early = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 0, "value": "2" }
            }),
        )
        .await;
        assert_eq!(too_early.status(), reqwest::StatusCode::BAD_REQUEST);
        assert!(
            body_json(too_early).await["error"]
                .as_str()
                .unwrap()
                .contains("no price is available"),
            "the one failure a user can fix themselves must say what to do"
        );

        // Load the last two hours of one-minute bars, exactly as opening a
        // chart on this instrument would.
        let now = senken_loader::SystemClock.now().as_nanos();
        let aligned_now = now - now.rem_euclid(MINUTE);
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            &admin_token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": aligned_now - 180 * MINUTE,
                "to": aligned_now,
            }),
        )
        .await;
        assert_eq!(ensure.status(), reqwest::StatusCode::ACCEPTED);
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_job(addr, &admin_token, &job_id).await;

        // Now the same order fills — at the fake venue's own close of 100,
        // which is the proof that the mark came from the stored bars and
        // not from anywhere else.
        let placed = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 0, "value": "2" }
            }),
        )
        .await;
        let status = placed.status();
        let order = body_json(placed).await;
        assert_eq!(status, reqwest::StatusCode::CREATED, "got {order}");
        assert_eq!(order["status"], "filled");
        assert_eq!(order["average_price"]["value"], "100");
        assert_eq!(order["average_price"]["scale"], 0);

        let positions = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/positions"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(positions[0]["side"], "long");
        assert_eq!(positions[0]["quantity"]["value"], "2");
        assert_eq!(
            positions[0]["mark_price"]["value"], "100",
            "an open position must be marked from the same source it filled against"
        );

        handle.shutdown().await.unwrap();
    }

    /// Someone who has only ever opened a chart on a venue that serves its
    /// own coarse candles still has real, current prices stored for that
    /// instrument — and every market order they placed used to come back
    /// "no price is available, load some history first", which they had
    /// just done. Nothing on screen said the granularity was what mattered.
    ///
    /// The venue here serves five-minute bars and nothing finer, so
    /// five-minute is the only series that can exist in the store for it.
    /// That is the exact shape the defect needed, and it is not exotic: it
    /// is what every venue whose finest candle is coarser than a minute
    /// looks like.
    #[tokio::test]
    async fn a_coarser_series_is_mark_enough_to_fill_a_market_order() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let runtime = crate::bars_handlers::test_support::runtime_with_5m_only_venue_and_simulator(
            runtime_dir.path(),
        );
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = format!(
            "{}:{}",
            crate::bars_handlers::test_support::TEST_SOURCE_5M_ONLY,
            crate::bars_handlers::test_support::TEST_SYMBOL
        );

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "COARSE",
                    "settings": {
                        "starting_balance": "50000.00",
                        "fee_bps": 0,
                        "slippage_bps": 0
                    }
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let now = senken_loader::SystemClock.now().as_nanos();
        let five = 5 * MINUTE;
        let aligned_now = now - now.rem_euclid(five);
        let job_id = body_json(
            post_json_auth(
                format!("http://{addr}/api/bars/ensure"),
                &admin_token,
                serde_json::json!({
                    "instrument": instrument,
                    "spec": "5m",
                    "from": aligned_now - 36 * five,
                    "to": aligned_now,
                }),
            )
            .await,
        )
        .await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_job(addr, &admin_token, &job_id).await;

        let placed = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 0, "value": "1" }
            }),
        )
        .await;
        let status = placed.status();
        let order = body_json(placed).await;
        assert_eq!(
            status,
            reqwest::StatusCode::CREATED,
            "five-minute bars are history; the order must not be refused for want of a price: {order}"
        );
        assert_eq!(order["status"], "filled");
        assert_eq!(order["average_price"]["value"], "100");

        handle.shutdown().await.unwrap();
    }

    /// Loads history and opens one long position, returning the running
    /// server, the account id and the trader's token — the setup both the
    /// test above and the one below need.
    async fn opened_position() -> (crate::ServerHandle, tempfile::TempDir, String, String) {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue_and_simulator(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = test_instrument();

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "E2E",
                    "settings": {
                        "starting_balance": "50000.00",
                        "fee_bps": 0,
                        "slippage_bps": 0
                    }
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let now = senken_loader::SystemClock.now().as_nanos();
        let aligned_now = now - now.rem_euclid(MINUTE);
        let job_id = body_json(
            post_json_auth(
                format!("http://{addr}/api/bars/ensure"),
                &admin_token,
                serde_json::json!({
                    "instrument": instrument,
                    "spec": "1m",
                    "from": aligned_now - 180 * MINUTE,
                    "to": aligned_now,
                }),
            )
            .await,
        )
        .await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_job(addr, &admin_token, &job_id).await;

        post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "buy",
                "kind": "market",
                "quantity": { "scale": 0, "value": "2" }
            }),
        )
        .await;

        (handle, runtime_dir, account, token)
    }

    #[tokio::test]
    async fn closing_a_position_banks_its_profit_and_leaves_nothing_open() {
        let (handle, _runtime_dir, account, token) = opened_position().await;
        let addr = handle.local_addr();
        let instrument = test_instrument();

        // Bought and sold at the same mark with fees turned off, so the
        // balance must come back to exactly where it started.
        let opening_balance = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/balances"),
                &token,
            )
            .await,
        )
        .await["balance"]["value"]
            .as_str()
            .unwrap()
            .to_owned();

        let closed = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "sell",
                "kind": "market",
                "quantity": { "scale": 0, "value": "2" }
            }),
        )
        .await;
        assert_eq!(closed.status(), reqwest::StatusCode::CREATED);

        let after = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/balances"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(after["balance"]["value"], opening_balance);
        assert!(
            body_json(
                get_auth(
                    format!("http://{addr}/api/trade/accounts/{account}/positions"),
                    &token,
                )
                .await,
            )
            .await
            .as_array()
            .unwrap()
            .is_empty(),
            "a fully closed position must not linger"
        );

        // And the execution log carries both fills.
        let fills = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/fills"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(fills.as_array().unwrap().len(), 2);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_resting_limit_order_is_reported_open_and_can_be_cancelled() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue_and_simulator(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = test_instrument();

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "E2E",
                    "settings": {}
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        // A limit order is accepted with no history loaded at all: it needs
        // a mark when it is *checked*, not when it is placed.
        let placed = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/orders"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "side": "buy",
                "kind": "limit",
                "quantity": { "scale": 0, "value": "1" },
                "limit_price": { "scale": 0, "value": "50" }
            }),
        )
        .await;
        assert_eq!(placed.status(), reqwest::StatusCode::CREATED);
        let order = body_json(placed).await;
        assert_eq!(order["status"], "open");
        assert_eq!(order["limit_price"]["value"], "50");
        let order_id = order["id"].as_str().unwrap().to_owned();

        let open = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/orders?status=open"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(open.as_array().unwrap().len(), 1);

        let cancelled = reqwest::Client::new()
            .delete(format!(
                "http://{addr}/api/trade/accounts/{account}/orders/{order_id}"
            ))
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(cancelled.status(), reqwest::StatusCode::OK);
        assert_eq!(body_json(cancelled).await["status"], "cancelled");

        assert!(
            body_json(
                get_auth(
                    format!("http://{addr}/api/trade/accounts/{account}/orders?status=open"),
                    &token,
                )
                .await,
            )
            .await
            .as_array()
            .unwrap()
            .is_empty()
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn closing_over_http_uses_the_positions_current_size_and_leaves_nothing_open() {
        let (handle, _runtime_dir, account, token) = opened_position().await;
        let addr = handle.local_addr();
        let instrument = test_instrument();

        let closed = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/close"),
            &token,
            serde_json::json!({ "position_id": instrument }),
        )
        .await;
        assert_eq!(closed.status(), reqwest::StatusCode::CREATED);
        let order = body_json(closed).await;
        assert_eq!(order["side"], "sell", "closing a long sells");
        assert_eq!(order["status"], "filled");
        assert_eq!(
            order["quantity"]["value"], "2",
            "the close must send exactly the size held, not a size the caller chose"
        );

        assert!(
            body_json(
                get_auth(
                    format!("http://{addr}/api/trade/accounts/{account}/positions"),
                    &token,
                )
                .await,
            )
            .await
            .as_array()
            .unwrap()
            .is_empty(),
            "a full close must leave no position behind"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn closing_a_position_that_is_not_open_is_a_bad_request() {
        let (handle, _runtime_dir, account, token) = opened_position().await;
        let addr = handle.local_addr();

        let response = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/close"),
            &token,
            serde_json::json!({ "position_id": "okx-spot:ETHUSDT" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn amending_a_resting_order_over_http_changes_its_price_and_size() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue_and_simulator(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = test_instrument();

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "E2E",
                    "settings": { "starting_balance": "50000.00" }
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        // History loaded so the mark is known (100) — a limit resting well
        // below it stays open rather than filling immediately.
        let now = senken_loader::SystemClock.now().as_nanos();
        let aligned_now = now - now.rem_euclid(MINUTE);
        let job_id = body_json(
            post_json_auth(
                format!("http://{addr}/api/bars/ensure"),
                &admin_token,
                serde_json::json!({
                    "instrument": instrument,
                    "spec": "1m",
                    "from": aligned_now - 180 * MINUTE,
                    "to": aligned_now,
                }),
            )
            .await,
        )
        .await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        wait_for_job(addr, &admin_token, &job_id).await;

        let order = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts/{account}/orders"),
                &token,
                serde_json::json!({
                    "instrument": instrument,
                    "side": "buy",
                    "kind": "limit",
                    "quantity": { "scale": 0, "value": "1" },
                    "limit_price": { "scale": 0, "value": "50" }
                }),
            )
            .await,
        )
        .await;
        assert_eq!(order["status"], "open");
        let order_id = order["id"].as_str().unwrap().to_owned();

        let amended = reqwest::Client::new()
            .patch(format!(
                "http://{addr}/api/trade/accounts/{account}/orders/{order_id}"
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "quantity": { "scale": 0, "value": "3" },
                    "limit_price": { "scale": 0, "value": "60" }
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(amended.status(), reqwest::StatusCode::OK);
        let body = body_json(amended).await;
        assert_eq!(body["quantity"]["value"], "3");
        assert_eq!(body["limit_price"]["value"], "60");
        assert_eq!(
            body["status"], "open",
            "60 is still below the mark of 100, so it must not have filled"
        );

        // And the amendment is what is actually stored, not just echoed.
        let stored = body_json(
            get_auth(
                format!("http://{addr}/api/trade/accounts/{account}/orders?status=open"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(stored[0]["limit_price"]["value"], "60");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn close_and_amend_are_both_forbidden_on_a_read_only_account() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue_and_simulator(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let admin = admin_of(&identity);
        let token = trader(addr, &identity, &admin, "alice@example.com").await;
        let instrument = test_instrument();

        let account = body_json(
            post_json_auth(
                format!("http://{addr}/api/trade/accounts"),
                &token,
                serde_json::json!({
                    "adapter_id": "simulator",
                    "label": "Investor",
                    "settings": {
                        "starting_balance": "50000.00",
                        "access": "read_only"
                    }
                }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let close = post_json_auth(
            format!("http://{addr}/api/trade/accounts/{account}/close"),
            &token,
            serde_json::json!({ "position_id": instrument }),
        )
        .await;
        assert_eq!(
            close.status(),
            reqwest::StatusCode::FORBIDDEN,
            "a read-only account must be refused before its positions are even read"
        );

        let amend = reqwest::Client::new()
            .patch(format!(
                "http://{addr}/api/trade/accounts/{account}/orders/does-not-matter"
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(
                    &serde_json::json!({ "quantity": { "scale": 0, "value": "1" } }),
                )
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(
            amend.status(),
            reqwest::StatusCode::FORBIDDEN,
            "a read-only account must be refused before its orders are even read"
        );

        handle.shutdown().await.unwrap();
    }
}
