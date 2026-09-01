//! Workspaces, layouts, panes and layers over HTTP.
//!
//! Every handler here extracts `Extension(ctx): Authed` and passes
//! `&ctx.user` straight through to `senken_workspace::WorkspaceStore`, which
//! performs its own `AuthenticatedUser::authorize` check on every read and
//! write — the same "the store checks itself" shape
//! `admin_handlers` already established for `senken-identity`. Every route
//! this module's handlers are mounted on (see `crate::router`) is still
//! declared through `crate::auth::mount` at `EndpointPermission::Authenticated`: a second all-or-nothing gate here would only ever be
//! checking the same thing twice, never tighter.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_workspace::{
    DrawingId, DrawingInput, DrawingKind, DrawingLineStyle, DrawingPoint, DrawingStyle, LayerId,
    LayerInput, LayerKind, LayoutId, LayoutPreset, PaneInput, WorkspaceId,
};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    CreateWorkspaceRequest, DefaultWorkspaceResponse, DrawingInputDto, DrawingKindDto,
    DrawingLineStyleDto, DrawingPointDto, IdResponse, LayerInputDto, LayerKindDto, LayoutDetailDto,
    LayoutSummaryDto, PaneInputDto, RenameWorkspaceRequest, ReplaceLayoutRequest, WorkspaceDto,
    WorkspacesPage,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`WorkspaceId`], failing with `400` (not
/// `500`) for a malformed one.
fn parse_workspace_id(raw: &str) -> Result<WorkspaceId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid workspace id".to_owned()))
}

/// The [`LayoutId`] counterpart of [`parse_workspace_id`].
fn parse_layout_id(raw: &str) -> Result<LayoutId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid layout id".to_owned()))
}

fn parse_layer_id(raw: &str) -> Result<LayerId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid layer id".to_owned()))
}

fn parse_drawing_id(raw: &str) -> Result<DrawingId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid drawing id".to_owned()))
}

/// Converts one wire [`LayerKindDto`] into the domain [`LayerKind`] it
/// names, parsing `instrument` and failing with `400` if it does not parse.
fn layer_kind_from_dto(dto: LayerKindDto) -> Result<LayerKind, HandlerError> {
    Ok(match dto {
        LayerKindDto::OverlayInstrument { instrument } => LayerKind::OverlayInstrument {
            instrument: InstrumentId::parse(&instrument)
                .map_err(|source| HandlerError::BadRequest(source.to_string()))?,
        },
        LayerKindDto::IndicatorOverlay { name, params } => {
            LayerKind::IndicatorOverlay { name, params }
        }
        LayerKindDto::IndicatorSubPane { name, params } => {
            LayerKind::IndicatorSubPane { name, params }
        }
    })
}

fn layer_input_from_dto(dto: LayerInputDto) -> Result<LayerInput, HandlerError> {
    Ok(LayerInput {
        position: dto.position,
        kind: layer_kind_from_dto(dto.kind)?,
        visible: dto.visible,
        style: dto.style,
    })
}

fn drawing_point_from_dto(dto: DrawingPointDto) -> DrawingPoint {
    DrawingPoint {
        time: UnixNanos::from_nanos(dto.time),
        price: dto.price,
    }
}

/// Converts one wire [`DrawingKindDto`] into the domain [`DrawingKind`] it
/// names. Infallible — every field on the wire shape already matches the
/// domain shape field-for-field; [`WorkspaceStore::replace_layout`](senken_workspace::WorkspaceStore::replace_layout)
/// is what validates a drawing's style.
fn drawing_kind_from_dto(dto: DrawingKindDto) -> DrawingKind {
    match dto {
        DrawingKindDto::HorizontalLine { price } => DrawingKind::HorizontalLine { price },
        DrawingKindDto::TrendLine { start, end } => DrawingKind::TrendLine {
            start: drawing_point_from_dto(start),
            end: drawing_point_from_dto(end),
        },
        DrawingKindDto::Rectangle { start, end } => DrawingKind::Rectangle {
            start: drawing_point_from_dto(start),
            end: drawing_point_from_dto(end),
        },
    }
}

fn drawing_line_style_from_dto(dto: DrawingLineStyleDto) -> DrawingLineStyle {
    match dto {
        DrawingLineStyleDto::Solid => DrawingLineStyle::Solid,
        DrawingLineStyleDto::Dashed => DrawingLineStyle::Dashed,
        DrawingLineStyleDto::Dotted => DrawingLineStyle::Dotted,
    }
}

fn drawing_input_from_dto(dto: DrawingInputDto) -> DrawingInput {
    DrawingInput {
        position: dto.position,
        kind: drawing_kind_from_dto(dto.kind),
        style: DrawingStyle {
            color: dto.color,
            width: dto.width,
            line_style: drawing_line_style_from_dto(dto.line_style),
        },
    }
}

fn pane_input_from_dto(dto: PaneInputDto) -> Result<PaneInput, HandlerError> {
    let instrument = InstrumentId::parse(&dto.instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let timeframe = dto
        .timeframe
        .parse()
        .map_err(|source: senken_series::ParseBarSpecError| {
            HandlerError::BadRequest(source.to_string())
        })?;
    let layers = dto
        .layers
        .into_iter()
        .map(layer_input_from_dto)
        .collect::<Result<Vec<_>, _>>()?;
    let drawings = dto
        .drawings
        .into_iter()
        .map(drawing_input_from_dto)
        .collect();
    Ok(PaneInput {
        position: dto.position,
        instrument,
        timeframe,
        layers,
        drawings,
        settings: dto.settings,
    })
}

/// `GET /api/workspaces`. Scoped by
/// `WorkspaceStore::list_workspaces` itself — a superadmin
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
            .map_err(|source: senken_workspace::ParseLayoutPresetError| {
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
    Path(layer_id): Path<String>,
    Json(body): Json<LayerInputDto>,
) -> Result<StatusCode, HandlerError> {
    state.workspace.update_layer(
        &ctx.user,
        parse_layer_id(&layer_id)?,
        &layer_input_from_dto(body)?,
    )?;
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
    Path(layer_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    state
        .workspace
        .delete_layer(&ctx.user, parse_layer_id(&layer_id)?)?;
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
    Path(drawing_id): Path<String>,
    Json(body): Json<DrawingInputDto>,
) -> Result<StatusCode, HandlerError> {
    state.workspace.update_drawing(
        &ctx.user,
        parse_drawing_id(&drawing_id)?,
        &drawing_input_from_dto(body),
    )?;
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
    Path(drawing_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    state
        .workspace
        .delete_drawing(&ctx.user, parse_drawing_id(&drawing_id)?)?;
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

    async fn charts_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Charts User", Some("a very long password"))
            .unwrap();
        for resource in [Resource::Workspace, Resource::Layout] {
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
}
