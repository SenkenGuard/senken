//! Workspaces, layouts, panes and pane items over HTTP.
//!
//! Every handler here extracts `Extension(ctx): Authed` and passes
//! `&ctx.user` straight through to `senken_chart::ChartWorkspaceStore`,
//! which performs its own `AuthenticatedUser::authorize` check on every
//! read and write — the same "the store checks itself" shape
//! `admin_handlers` already established for `senken-identity`. Every route
//! this module's handlers are mounted on (see `crate::router`) is still
//! declared through `crate::auth::mount` at `EndpointPermission::Authenticated`: a second all-or-nothing gate here would only ever be
//! checking the same thing twice, never tighter.
//!
//! # `layers`/`drawings` are one table underneath, two routes on top
//!
//! `senken-chart` stores every pane item — a computed indicator, a
//! referenced overlay instrument, or an anchored drawing — in one table
//! (`PaneItemRecord`/`ItemSource`). The wire API keeps its two separate
//! shapes (`layers[]`/`drawings[]` on a pane, `/api/layers/{id}` and
//! `/api/drawings/{id}` for per-item mutation) unchanged: this module is
//! where that split lives now, translating between the wire DTOs
//! (`crate::dto::workspace`) and the unified domain calls. `/api/layers/
//! {id}` can only ever produce a `Computed`/`Referenced` update and
//! `/api/drawings/{id}` only an `Anchored` one — the wire type itself makes
//! the other family unreachable — and the store's own
//! `ItemSourceMismatch` check catches the one case the type system cannot:
//! a `layers`-shaped body aimed at an id that is actually a drawing.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_chart::{
    ChartLayoutId, ChartWorkspaceId, DrawingKind, DrawingStyle, ItemSource, LayoutPreset,
    PaneInput, PaneItemId, PaneItemInput, Slot,
};
use senken_marketdata::InstrumentId;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    CreateWorkspaceRequest, DefaultWorkspaceResponse, DrawingInputDto, DrawingKindDto, IdResponse,
    LayerInputDto, LayerKindDto, LayoutDetailDto, LayoutSummaryDto, PaneInputDto,
    RenameWorkspaceRequest, ReplaceLayoutRequest, UpdateWorkspaceSettingsRequest, WorkspaceDto,
    WorkspacesPage,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`ChartWorkspaceId`], failing with
/// `400` (not `500`) for a malformed one.
fn parse_workspace_id(raw: &str) -> Result<ChartWorkspaceId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid workspace id".to_owned()))
}

/// The [`ChartLayoutId`] counterpart of [`parse_workspace_id`].
fn parse_layout_id(raw: &str) -> Result<ChartLayoutId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid layout id".to_owned()))
}

/// The [`PaneItemId`] counterpart of [`parse_workspace_id`], used by both
/// `/api/layers/{id}` and `/api/drawings/{id}` — see this module's docs.
fn parse_pane_item_id(raw: &str) -> Result<PaneItemId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid item id".to_owned()))
}

/// Converts one wire [`LayerKindDto`] into the `(ItemSource, Slot)` pair
/// `senken_chart` stores a layer as, parsing `instrument` and failing with
/// `400` if it does not parse.
fn layer_kind_from_dto(dto: LayerKindDto) -> Result<(ItemSource, Slot), HandlerError> {
    Ok(match dto {
        LayerKindDto::OverlayInstrument { instrument } => (
            ItemSource::Referenced {
                instrument: InstrumentId::parse(&instrument)
                    .map_err(|source| HandlerError::BadRequest(source.to_string()))?,
            },
            Slot::Main,
        ),
        LayerKindDto::IndicatorOverlay { name, params } => {
            (ItemSource::Computed { name, params }, Slot::Main)
        }
        LayerKindDto::IndicatorSubPane { name, params } => {
            (ItemSource::Computed { name, params }, Slot::Sub(0))
        }
    })
}

/// Converts one wire [`LayerInputDto`] into a [`PaneItemInput`]. `position`
/// is left at whatever the caller passes in `position` — see
/// [`pane_input_from_dto`] and `ChartWorkspaceStore::update_pane_item`'s
/// own docs for why a layer's wire `position` is never trusted verbatim.
fn layer_input_from_dto(dto: LayerInputDto, position: u32) -> Result<PaneItemInput, HandlerError> {
    let (source, slot) = layer_kind_from_dto(dto.kind)?;
    Ok(PaneItemInput {
        position,
        slot,
        visible: dto.visible,
        style: dto.style,
        source,
    })
}

/// Converts one wire [`DrawingKindDto`] into the domain [`DrawingKind`] it
/// names. Infallible — every field on the wire shape already matches the
/// domain shape field-for-field; [`ChartWorkspaceStore::replace_layout`](senken_chart::ChartWorkspaceStore::replace_layout)
/// is what validates a drawing's style.
fn drawing_kind_from_dto(dto: DrawingKindDto) -> DrawingKind {
    dto.into()
}

/// Converts one wire [`DrawingInputDto`] into a [`PaneItemInput`], the
/// same way [`layer_input_from_dto`] does for a layer.
fn drawing_input_from_dto(
    dto: DrawingInputDto,
    position: u32,
) -> Result<PaneItemInput, HandlerError> {
    let instrument = dto
        .instrument
        .as_deref()
        .map(InstrumentId::parse)
        .transpose()
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    Ok(PaneItemInput {
        position,
        slot: Slot::Main,
        visible: dto.visible,
        style: DrawingStyle {
            color: dto.color,
            width: dto.width,
            line_style: dto.line_style.into(),
        }
        .to_json(),
        source: ItemSource::Anchored {
            kind: drawing_kind_from_dto(dto.kind),
            instrument,
        },
    })
}

/// Builds a pane's unified `items` list from its wire `layers`/`drawings`
/// arrays: layers first, drawings after, each renumbered by array order
/// into one shared position space — `senken_chart`'s own `(pane_id,
/// position)` uniqueness now spans both, where before each had its own.
/// `GET /api/layouts/{id}` applies the exact same split-and-renumber rule
/// in reverse (`PaneDto::from`), so a client that reads a layout and PUTs
/// it straight back sees stable ids and stable relative ordering.
fn pane_input_from_dto(dto: PaneInputDto) -> Result<PaneInput, HandlerError> {
    let instrument = InstrumentId::parse(&dto.instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let timeframe = dto
        .timeframe
        .parse()
        .map_err(|source: senken_series::ParseBarSpecError| {
            HandlerError::BadRequest(source.to_string())
        })?;
    // Each half keeps the caller's own relative order (its wire
    // `position`, still meaningful *within* `layers` or *within*
    // `drawings`) before being renumbered into the one shared space
    // `chart_pane_items` now uses.
    let mut layers = dto.layers;
    layers.sort_by_key(|layer| layer.position);
    let mut drawings = dto.drawings;
    drawings.sort_by_key(|drawing| drawing.position);

    let mut items = Vec::with_capacity(layers.len() + drawings.len());
    for (index, layer) in layers.into_iter().enumerate() {
        items.push(layer_input_from_dto(
            layer,
            u32::try_from(index).unwrap_or(u32::MAX),
        )?);
    }
    let base = u32::try_from(items.len()).unwrap_or(u32::MAX);
    for (index, drawing) in drawings.into_iter().enumerate() {
        let position = base.saturating_add(u32::try_from(index).unwrap_or(u32::MAX));
        items.push(drawing_input_from_dto(drawing, position)?);
    }
    Ok(PaneInput {
        position: dto.position,
        instrument,
        timeframe,
        items,
        settings: dto.settings,
    })
}

/// `GET /api/workspaces`. Scoped by
/// `ChartWorkspaceStore::list_workspaces` itself — a superadmin
/// sees every workspace, an ordinary user sees only their own, and the
/// reported `total` already respects that scope too.
#[utoipa::path(
    get,
    path = "/api/workspaces",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = WorkspacesPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_workspaces(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<WorkspacesPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.workspace.list_workspaces(&ctx.user, limit, offset)?;
    Ok(Json(WorkspacesPage {
        rows: page.rows.into_iter().map(WorkspaceDto::from).collect(),
        total: page.total,
    }))
}

/// `POST /api/workspaces`.
#[utoipa::path(
    post,
    path = "/api/workspaces",
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateWorkspaceRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state.workspace.create_workspace(&ctx.user, &body.name)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/workspaces/default` ("default-on-first-open belongs on the server"). Returns the caller's default workspace and its
/// one layout, creating both on the very first call for this account and
/// returning the same pair on every later one.
#[utoipa::path(
    get,
    path = "/api/workspaces/default",
    responses(
        (status = 200, body = DefaultWorkspaceResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn default_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<DefaultWorkspaceResponse>, HandlerError> {
    let (workspace_id, layout_id) = state.workspace.get_or_create_default_workspace(&ctx.user)?;
    Ok(Json(DefaultWorkspaceResponse {
        workspace_id: workspace_id.to_string(),
        layout_id: layout_id.to_string(),
    }))
}

/// `PATCH /api/workspaces/{workspace_id}`.
#[utoipa::path(
    patch,
    path = "/api/workspaces/{workspace_id}",
    request_body = RenameWorkspaceRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn rename_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
    Json(body): Json<RenameWorkspaceRequest>,
) -> Result<StatusCode, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    state
        .workspace
        .rename_workspace(&ctx.user, workspace_id, &body.name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/workspaces/{workspace_id}`.
#[utoipa::path(
    delete,
    path = "/api/workspaces/{workspace_id}",
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    state.workspace.delete_workspace(&ctx.user, workspace_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /api/workspaces/{workspace_id}/settings`. `settings` is opaque
/// JSON-object text this crate never interprets — see
/// [`UpdateWorkspaceSettingsRequest`]'s own docs.
#[utoipa::path(
    patch,
    path = "/api/workspaces/{workspace_id}/settings",
    request_body = UpdateWorkspaceSettingsRequest,
    params(("workspace_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn update_workspace_settings(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
    Json(body): Json<UpdateWorkspaceSettingsRequest>,
) -> Result<StatusCode, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    state
        .workspace
        .update_workspace_settings(&ctx.user, workspace_id, &body.settings)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/workspaces/{workspace_id}/layouts`.
#[utoipa::path(
    get,
    path = "/api/workspaces/{workspace_id}/layouts",
    responses(
        (status = 200, body = Vec<LayoutSummaryDto>),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_layouts(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
) -> Result<Json<Vec<LayoutSummaryDto>>, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let layouts = state.workspace.list_layouts(&ctx.user, workspace_id)?;
    Ok(Json(
        layouts.into_iter().map(LayoutSummaryDto::from).collect(),
    ))
}

/// `GET /api/layouts/{layout_id}`: one layout with its full
/// nested pane/layer structure.
#[utoipa::path(
    get,
    path = "/api/layouts/{layout_id}",
    responses(
        (status = 200, body = LayoutDetailDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_layout(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(layout_id): Path<String>,
) -> Result<Json<LayoutDetailDto>, HandlerError> {
    let layout_id = parse_layout_id(&layout_id)?;
    let detail = state.workspace.get_layout(&ctx.user, layout_id)?;
    Ok(Json(detail.into()))
}

/// `PUT /api/layouts/{layout_id}`: replaces a
/// layout's whole pane/layer structure in one transaction.
#[utoipa::path(
    put,
    path = "/api/layouts/{layout_id}",
    request_body = ReplaceLayoutRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn replace_layout(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(layout_id): Path<String>,
    Json(body): Json<ReplaceLayoutRequest>,
) -> Result<StatusCode, HandlerError> {
    let layout_id = parse_layout_id(&layout_id)?;
    let preset: LayoutPreset =
        body.preset
            .parse()
            .map_err(|source: senken_chart::ParseLayoutPresetError| {
                HandlerError::BadRequest(source.to_string())
            })?;
    let panes = body
        .panes
        .into_iter()
        .map(pane_input_from_dto)
        .collect::<Result<Vec<_>, _>>()?;
    state
        .workspace
        .replace_layout(&ctx.user, layout_id, preset, &panes)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /api/layers/{layer_id}`.
#[utoipa::path(
    patch,
    path = "/api/layers/{layer_id}",
    request_body = LayerInputDto,
    params(("layer_id" = String, Path)),
    responses((status = 204), (status = 400, body = crate::dto::ErrorBody), (status = 401, body = crate::dto::ErrorBody), (status = 403, body = crate::dto::ErrorBody))
)]
pub(crate) async fn update_layer(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(item_id): Path<String>,
    Json(body): Json<LayerInputDto>,
) -> Result<StatusCode, HandlerError> {
    // `position` is never read by `update_pane_item` (see its own docs) —
    // `0` here is a placeholder, not a claim about the item's real
    // position.
    let input = layer_input_from_dto(body, 0)?;
    state
        .workspace
        .update_pane_item(&ctx.user, parse_pane_item_id(&item_id)?, &input)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/layers/{layer_id}`.
#[utoipa::path(
    delete,
    path = "/api/layers/{layer_id}",
    params(("layer_id" = String, Path)),
    responses((status = 204), (status = 400, body = crate::dto::ErrorBody), (status = 401, body = crate::dto::ErrorBody), (status = 403, body = crate::dto::ErrorBody))
)]
pub(crate) async fn delete_layer(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(item_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    state
        .workspace
        .delete_pane_item(&ctx.user, parse_pane_item_id(&item_id)?)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /api/drawings/{drawing_id}`.
#[utoipa::path(
    patch,
    path = "/api/drawings/{drawing_id}",
    request_body = DrawingInputDto,
    params(("drawing_id" = String, Path)),
    responses((status = 204), (status = 400, body = crate::dto::ErrorBody), (status = 401, body = crate::dto::ErrorBody), (status = 403, body = crate::dto::ErrorBody))
)]
pub(crate) async fn update_drawing(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(item_id): Path<String>,
    Json(body): Json<DrawingInputDto>,
) -> Result<StatusCode, HandlerError> {
    // Same placeholder-position note as `update_layer`.
    let input = drawing_input_from_dto(body, 0)?;
    state
        .workspace
        .update_pane_item(&ctx.user, parse_pane_item_id(&item_id)?, &input)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/drawings/{drawing_id}`.
#[utoipa::path(
    delete,
    path = "/api/drawings/{drawing_id}",
    params(("drawing_id" = String, Path)),
    responses((status = 204), (status = 400, body = crate::dto::ErrorBody), (status = 401, body = crate::dto::ErrorBody), (status = 403, body = crate::dto::ErrorBody))
)]
pub(crate) async fn delete_drawing(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(item_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    state
        .workspace
        .delete_pane_item(&ctx.user, parse_pane_item_id(&item_id)?)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json_auth, serve_unfenced_test_server,
    };

    async fn login_token(addr: std::net::SocketAddr, email: &str, password: &str) -> String {
        let response = crate::test_support::post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn patch_settings(
        addr: std::net::SocketAddr,
        token: &str,
        workspace_id: &str,
        settings: &str,
    ) -> reqwest::Response {
        reqwest::Client::new()
            .patch(format!(
                "http://{addr}/api/workspaces/{workspace_id}/settings"
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "settings": settings })).unwrap())
            .send()
            .await
            .unwrap()
    }

    async fn charts_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Charts User", Some("a very long password"))
            .unwrap();
        for resource in [Resource::ChartWorkspace, Resource::ChartLayout] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        login_token(addr, email, "a very long password").await
    }

    #[tokio::test]
    async fn two_users_cannot_see_each_others_workspaces_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();

        let alice_token = charts_user(addr, &identity, &admin, "alice@example.com").await;
        let bob_token = charts_user(addr, &identity, &admin, "bob@example.com").await;

        post_json_auth(
            format!("http://{addr}/api/workspaces"),
            &alice_token,
            serde_json::json!({ "name": "Alice's Charts" }),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/workspaces"),
            &bob_token,
            serde_json::json!({ "name": "Bob's Charts" }),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/workspaces"),
            &bob_token,
            serde_json::json!({ "name": "Bob's Second" }),
        )
        .await;

        let alice_page =
            body_json(get_auth(format!("http://{addr}/api/workspaces"), &alice_token).await).await;
        assert_eq!(
            alice_page["total"], 1,
            "alice must see only her own workspace"
        );
        assert_eq!(alice_page["rows"].as_array().unwrap().len(), 1);

        let bob_page =
            body_json(get_auth(format!("http://{addr}/api/workspaces"), &bob_token).await).await;
        assert_eq!(
            bob_page["total"], 2,
            "the total must respect scope too, or pagination leaks how many workspaces exist"
        );

        // The wrong user cannot rename or delete alice's workspace either —
        // 403, not 401 (the required permission-enforcement test).
        let alice_workspace_id = alice_page["rows"][0]["id"].as_str().unwrap();
        // `PATCH` has no `post_json_auth`-style helper in this crate's test
        // support, so this uses a raw client directly.
        let response = reqwest::Client::new()
            .patch(format!("http://{addr}/api/workspaces/{alice_workspace_id}"))
            .header("authorization", format!("Bearer {bob_token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "name": "Hijacked" })).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "403, not 401 -- bob has a valid session, he just may not touch alice's row"
        );

        handle.shutdown().await.unwrap();
    }

    // Settings text a client made up is that client's mistake, and must read
    // as one. Falling through to a 500 would blame the server for it — and a
    // 500 is what an operator's alerting pages on.
    #[tokio::test]
    async fn malformed_pane_settings_are_refused_as_a_bad_request_not_a_server_error() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(
            addr,
            &identity,
            &admin,
            "alice-bad-pane-settings@example.com",
        )
        .await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [{
                        "position": 0,
                        "instrument": "binance-spot:BTCUSDT",
                        "timeframe": "1h",
                        "layers": [],
                        "drawings": [],
                        "settings": "{not valid json"
                    }]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn workspace_settings_accepts_a_json_object_and_rejects_non_objects_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice-settings@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let workspace_id = default["workspace_id"].as_str().unwrap();

        let ok = patch_settings(addr, &alice_token, workspace_id, r#"{"theme":"dark"}"#).await;
        assert_eq!(ok.status(), reqwest::StatusCode::NO_CONTENT);

        let array = patch_settings(addr, &alice_token, workspace_id, "[1,2,3]").await;
        assert_eq!(array.status(), reqwest::StatusCode::BAD_REQUEST);

        let number = patch_settings(addr, &alice_token, workspace_id, "42").await;
        assert_eq!(number.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn workspace_settings_written_through_patch_are_read_back_from_the_listing() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(
            addr,
            &identity,
            &admin,
            "alice-settings-roundtrip@example.com",
        )
        .await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let workspace_id = default["workspace_id"].as_str().unwrap();

        // A freshly created workspace starts with empty settings — the
        // baseline this test's later assertion actually proves something
        // against, rather than happening to match by coincidence.
        let before =
            body_json(get_auth(format!("http://{addr}/api/workspaces"), &alice_token).await).await;
        assert_eq!(before["rows"][0]["settings"], "{}");

        let patched = patch_settings(
            addr,
            &alice_token,
            workspace_id,
            r#"{"candleColor":"blue"}"#,
        )
        .await;
        assert_eq!(patched.status(), reqwest::StatusCode::NO_CONTENT);

        let after =
            body_json(get_auth(format!("http://{addr}/api/workspaces"), &alice_token).await).await;
        assert_eq!(
            after["rows"][0]["settings"], r#"{"candleColor":"blue"}"#,
            "settings written through PATCH must be readable back from GET /api/workspaces"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn opening_charts_with_no_workspace_creates_exactly_one_default_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice2@example.com").await;

        let first = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let second = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            first, second,
            "the second open must return the same default"
        );

        let page =
            body_json(get_auth(format!("http://{addr}/api/workspaces"), &alice_token).await).await;
        assert_eq!(
            page["total"], 1,
            "the second open must not have created a second workspace"
        );

        let layout_id = first["layout_id"].as_str().unwrap();
        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            layout["panes"].as_array().unwrap().len(),
            1,
            "the default layout is never an empty state"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replacing_a_layout_persists_the_new_pane_and_layer_structure_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice3@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "2h",
                    "panes": [
                        {
                            "position": 0,
                            "instrument": "binance-spot:ETHUSDT",
                            "timeframe": "15m",
                            "layers": [
                                {
                                    "position": 0,
                                    "kind": { "kind": "indicator_sub_pane", "name": "RSI", "params": "{\"period\":14}" },
                                    "visible": true
                                }
                            ]
                        },
                        {
                            "position": 1,
                            "instrument": "binance-spot:SOLUSDT",
                            "timeframe": "1h",
                            "layers": []
                        }
                    ]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(layout["layout"]["preset"], "2h");
        assert_eq!(layout["panes"].as_array().unwrap().len(), 2);
        assert_eq!(layout["panes"][0]["instrument"], "binance-spot:ETHUSDT");
        assert_eq!(layout["panes"][0]["layers"][0]["kind"]["name"], "RSI");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replacing_a_layout_persists_drawings_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice-drawings@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [
                        {
                            "position": 0,
                            "instrument": "binance-spot:BTCUSDT",
                            "timeframe": "1h",
                            "layers": [],
                            "drawings": [
                                {
                                    "position": 0,
                                    "kind": { "kind": "horizontal_line", "price": 2450.5 },
                                    "color": "#f2f2ef",
                                    "width": 2,
                                    "line_style": "DASHED"
                                },
                                {
                                    "position": 1,
                                    "kind": {
                                        "kind": "trend_line",
                                        "start": { "time": 1_700_000_000_000_000_000i64, "price": 100.0 },
                                        "end": { "time": 1_700_003_600_000_000_000i64, "price": 101.5 }
                                    },
                                    "color": "#7aa7e8",
                                    "width": 1,
                                    "line_style": "SOLID"
                                }
                            ]
                        }
                    ]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(layout["panes"][0]["drawings"].as_array().unwrap().len(), 2);
        assert_eq!(
            layout["panes"][0]["drawings"][0]["kind"]["kind"],
            "horizontal_line"
        );
        assert_eq!(layout["panes"][0]["drawings"][0]["kind"]["price"], 2450.5);
        assert_eq!(layout["panes"][0]["drawings"][0]["line_style"], "DASHED");
        assert_eq!(
            layout["panes"][0]["drawings"][1]["kind"]["kind"],
            "trend_line"
        );
        assert_eq!(
            layout["panes"][0]["drawings"][1]["kind"]["start"]["time"],
            1_700_000_000_000_000_000i64
        );

        handle.shutdown().await.unwrap();
    }

    // A drawing's anchors are prices and instants on one market. The wire
    // carries which one, so a pane can show a reader only the drawings that
    // belong to whatever it is currently displaying instead of painting one
    // instrument's levels over another's candles.
    #[tokio::test]
    async fn a_drawing_keeps_the_instrument_it_was_drawn_against_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice-drawn-on@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let put = |body: serde_json::Value| {
            let token = alice_token.clone();
            async move {
                reqwest::Client::new()
                    .put(format!("http://{addr}/api/layouts/{layout_id}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(serde_json::to_vec(&body).unwrap())
                    .send()
                    .await
                    .unwrap()
            }
        };

        let drawn_on_eth = serde_json::json!({
            "position": 0,
            "kind": { "kind": "horizontal_line", "price": 2450.5 },
            "color": "#f2f2ef",
            "width": 2,
            "line_style": "DASHED",
            "instrument": "binance-spot:ETHUSDT"
        });
        // A second drawing that names no instrument at all — what a client
        // written before this field existed sends.
        let unattributed = serde_json::json!({
            "position": 1,
            "kind": { "kind": "horizontal_line", "price": 10.0 },
            "color": "#f2f2ef",
            "width": 1,
            "line_style": "SOLID"
        });

        let response = put(serde_json::json!({
            "preset": "1",
            "panes": [{
                "position": 0,
                "instrument": "binance-spot:ETHUSDT",
                "timeframe": "1h",
                "layers": [],
                "drawings": [drawn_on_eth.clone(), unattributed]
            }]
        }))
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            layout["panes"][0]["drawings"][0]["instrument"],
            "binance-spot:ETHUSDT"
        );
        assert!(
            layout["panes"][0]["drawings"][1]["instrument"].is_null(),
            "a drawing that named no instrument must not be given the pane's"
        );

        handle.shutdown().await.unwrap();
    }

    // Which instrument a pane is showing and which one a drawing was made on
    // are separate facts: moving the pane must not silently re-attribute the
    // annotation to whatever is now on screen.
    #[tokio::test]
    async fn moving_a_pane_to_another_instrument_leaves_its_drawings_attribution_alone() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token =
            charts_user(addr, &identity, &admin, "alice-moved-pane@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [{
                        "position": 0,
                        "instrument": "binance-spot:SOLUSDT",
                        "timeframe": "1h",
                        "layers": [],
                        "drawings": [{
                            "position": 0,
                            "kind": { "kind": "horizontal_line", "price": 2450.5 },
                            "color": "#f2f2ef",
                            "width": 2,
                            "line_style": "DASHED",
                            "instrument": "binance-spot:ETHUSDT"
                        }]
                    }]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(layout["panes"][0]["instrument"], "binance-spot:SOLUSDT");
        assert_eq!(
            layout["panes"][0]["drawings"][0]["instrument"],
            "binance-spot:ETHUSDT"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_drawing_naming_an_unparseable_instrument_is_refused() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token =
            charts_user(addr, &identity, &admin, "alice-bad-drawn-on@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [{
                        "position": 0,
                        "instrument": "binance-spot:BTCUSDT",
                        "timeframe": "1h",
                        "layers": [],
                        "drawings": [{
                            "position": 0,
                            "kind": { "kind": "horizontal_line", "price": 1.0 },
                            "color": "#f2f2ef",
                            "width": 1,
                            "line_style": "SOLID",
                            "instrument": "not an instrument id"
                        }]
                    }]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replace_layout_rejects_a_drawing_with_an_out_of_range_width_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token =
            charts_user(addr, &identity, &admin, "alice-bad-drawing@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [
                        {
                            "position": 0,
                            "instrument": "binance-spot:BTCUSDT",
                            "timeframe": "1h",
                            "layers": [],
                            "drawings": [
                                {
                                    "position": 0,
                                    "kind": { "kind": "horizontal_line", "price": 1.0 },
                                    "color": "#f2f2ef",
                                    "width": 9,
                                    "line_style": "SOLID"
                                }
                            ]
                        }
                    ]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_ray_a_fib_retracement_and_a_text_note_round_trip_over_http_and_visible_persists() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = charts_user(addr, &identity, &admin, "alice-new-tools@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        let anchor = serde_json::json!({ "time": 1_700_000_000_000_000_000i64, "price": 100.0 });
        let end = serde_json::json!({ "time": 1_700_003_600_000_000_000i64, "price": 101.5 });
        let ray = serde_json::json!({ "position": 0, "visible": false, "color": "#7aa7e8", "width": 1, "line_style": "SOLID",
            "kind": { "kind": "ray", "start": anchor, "end": end } });
        let fib = serde_json::json!({ "position": 1, "color": "#7aa7e8", "width": 1, "line_style": "SOLID",
            "kind": { "kind": "fib_retracement", "start": anchor, "end": end } });
        let note = serde_json::json!({ "position": 2, "color": "#7aa7e8", "width": 1, "line_style": "SOLID",
            "kind": { "kind": "text_note", "at": anchor, "text": "Breakout level", "anchor": "above" } });
        let body = serde_json::json!({ "preset": "1", "panes": [{
            "position": 0, "instrument": "binance-spot:BTCUSDT", "timeframe": "1h",
            "layers": [], "drawings": [ray, fib, note],
        }] });

        let response = reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&body).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        let drawings = layout["panes"][0]["drawings"].as_array().unwrap();
        assert_eq!(drawings.len(), 3);
        assert_eq!(drawings[0]["kind"]["kind"], "ray");
        assert_eq!(
            drawings[0]["visible"], false,
            "a drawing's visible flag must round-trip over HTTP — it never could before schema v8"
        );
        assert_eq!(drawings[1]["kind"]["kind"], "fib_retracement");
        assert_eq!(drawings[2]["kind"]["kind"], "text_note");
        assert_eq!(drawings[2]["kind"]["text"], "Breakout level");
        assert_eq!(drawings[2]["kind"]["anchor"], "above");
        assert_eq!(
            drawings[2]["visible"], true,
            "a drawing with no `visible` in the request must default to shown"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn patching_a_drawing_through_the_layer_endpoint_is_refused_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token =
            charts_user(addr, &identity, &admin, "alice-family-guard@example.com").await;

        let default = body_json(
            get_auth(
                format!("http://{addr}/api/workspaces/default"),
                &alice_token,
            )
            .await,
        )
        .await;
        let layout_id = default["layout_id"].as_str().unwrap();

        reqwest::Client::new()
            .put(format!("http://{addr}/api/layouts/{layout_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "preset": "1",
                    "panes": [{
                        "position": 0,
                        "instrument": "binance-spot:BTCUSDT",
                        "timeframe": "1h",
                        "layers": [],
                        "drawings": [{
                            "position": 0,
                            "kind": { "kind": "horizontal_line", "price": 1.0 },
                            "color": "#ffffff",
                            "width": 1,
                            "line_style": "SOLID"
                        }]
                    }]
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();

        let layout = body_json(
            get_auth(
                format!("http://{addr}/api/layouts/{layout_id}"),
                &alice_token,
            )
            .await,
        )
        .await;
        let drawing_id = layout["panes"][0]["drawings"][0]["id"].as_str().unwrap();

        // `/api/layers/{id}` can only ever build a `Computed`/`Referenced`
        // update (the wire `LayerKindDto` has no anchored variant), so
        // aiming it at an id that is actually a drawing must be refused —
        // proof the `ItemSourceMismatch` guard, not just the type system,
        // is reachable over HTTP.
        let response = reqwest::Client::new()
            .patch(format!("http://{addr}/api/layers/{drawing_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "position": 0,
                    "kind": { "kind": "indicator_overlay", "name": "EMA", "params": "{\"period\":20}" },
                    "visible": true
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }
}
