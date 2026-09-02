//! Dashboard workspaces and their widget grid over HTTP.
//!
//! Every handler here extracts `Extension(ctx): Authed` and passes
//! `&ctx.user` straight through to
//! `senken_dashboard::DashboardWorkspaceStore`, which performs its own
//! `AuthenticatedUser::authorize` check on every read and write — the same
//! "the store checks itself" shape `workspace_handlers` already
//! established for `senken-chart`.
//!
//! `GET /api/dashboard/widgets/catalog` is the one handler here that takes
//! no store at all: `senken_dashboard::WidgetRegistry::builtin()` is pure,
//! in-memory data, so there is nothing to authorize or fail — every
//! authenticated caller sees the same catalog.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_dashboard::{DashboardWidgetId, DashboardWorkspaceId, WidgetPlacementInput};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    CreateDashboardWorkspaceRequest, DashboardLayoutDto, DashboardWidgetCatalogResponse,
    DashboardWidgetDefinitionDto, DashboardWorkspaceDto, DashboardWorkspacesPage,
    DefaultDashboardWorkspaceResponse, IdResponse, RenameDashboardWorkspaceRequest,
    ReplaceDashboardLayoutRequest,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`DashboardWorkspaceId`], failing with
/// `400` (not `500`) for a malformed one.
fn parse_workspace_id(raw: &str) -> Result<DashboardWorkspaceId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid dashboard workspace id".to_owned()))
}

/// Converts one wire [`crate::dto::DashboardWidgetPlacementRequest`] into a
/// [`WidgetPlacementInput`], failing with `400` if `id` is present but not
/// a valid [`DashboardWidgetId`].
fn widget_placement_from_dto(
    dto: crate::dto::DashboardWidgetPlacementRequest,
) -> Result<WidgetPlacementInput, HandlerError> {
    let id = dto
        .id
        .map(|raw| {
            raw.parse::<DashboardWidgetId>()
                .map_err(|_| HandlerError::BadRequest("not a valid widget id".to_owned()))
        })
        .transpose()?;
    Ok(WidgetPlacementInput {
        id,
        provider_id: dto.provider_id,
        widget_type_id: dto.widget_type_id,
        position_x: dto.position_x,
        position_y: dto.position_y,
        width: dto.width,
        height: dto.height,
        visible: dto.visible,
        config: dto.config,
        config_schema_version: dto.config_schema_version,
    })
}

/// `GET /api/dashboard/workspaces`. Scoped by
/// `DashboardWorkspaceStore::list_workspaces` itself — a superadmin sees
/// every workspace, an ordinary user sees only their own, and the
/// reported `total` already respects that scope too.
#[utoipa::path(
    get,
    path = "/api/dashboard/workspaces",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = DashboardWorkspacesPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_dashboard_workspaces(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<DashboardWorkspacesPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.dashboard.list_workspaces(&ctx.user, limit, offset)?;
    Ok(Json(DashboardWorkspacesPage {
        rows: page
            .rows
            .into_iter()
            .map(DashboardWorkspaceDto::from)
            .collect(),
        total: page.total,
    }))
}

/// `POST /api/dashboard/workspaces`.
#[utoipa::path(
    post,
    path = "/api/dashboard/workspaces",
    request_body = CreateDashboardWorkspaceRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_dashboard_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateDashboardWorkspaceRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state.dashboard.create_workspace(&ctx.user, &body.name)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/dashboard/workspaces/default`: the caller's default
/// workspace, created on first call.
#[utoipa::path(
    get,
    path = "/api/dashboard/workspaces/default",
    responses(
        (status = 200, body = DefaultDashboardWorkspaceResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn default_dashboard_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<DefaultDashboardWorkspaceResponse>, HandlerError> {
    let workspace_id = state.dashboard.get_or_create_default_workspace(&ctx.user)?;
    Ok(Json(DefaultDashboardWorkspaceResponse {
        workspace_id: workspace_id.to_string(),
    }))
}

/// `PATCH /api/dashboard/workspaces/{id}`.
#[utoipa::path(
    patch,
    path = "/api/dashboard/workspaces/{workspace_id}",
    request_body = RenameDashboardWorkspaceRequest,
    params(("workspace_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn rename_dashboard_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
    Json(body): Json<RenameDashboardWorkspaceRequest>,
) -> Result<StatusCode, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    state
        .dashboard
        .rename_workspace(&ctx.user, workspace_id, &body.name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/dashboard/workspaces/{id}`. Deleting a user's last
/// workspace is not specially refused — `GET .../default` re-creates one
/// the moment none exists, the same healing `senken-chart`'s own
/// `delete_workspace` relies on.
#[utoipa::path(
    delete,
    path = "/api/dashboard/workspaces/{workspace_id}",
    params(("workspace_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_dashboard_workspace(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    state.dashboard.delete_workspace(&ctx.user, workspace_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/dashboard/workspaces/{id}/layout`: the workspace's full grid.
/// Whether a given widget's provider is still available is not decided
/// here — a caller cross-references each `widget_type_id` against `GET
/// /api/dashboard/widgets/catalog` itself to decide whether to render it
/// for real or as a placeholder.
#[utoipa::path(
    get,
    path = "/api/dashboard/workspaces/{workspace_id}/layout",
    params(("workspace_id" = String, Path)),
    responses(
        (status = 200, body = DashboardLayoutDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_dashboard_layout(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
) -> Result<Json<DashboardLayoutDto>, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let layout = state.dashboard.get_layout(&ctx.user, workspace_id)?;
    Ok(Json(layout.into()))
}

/// `PUT /api/dashboard/workspaces/{id}/layout`: replaces the workspace's
/// entire widget grid in one transaction. Add, move, resize and delete are
/// all just different snapshots through this one call — see
/// `senken_dashboard::DashboardWorkspaceStore::replace_layout`'s own docs.
/// `409` on `expected_revision` mismatch: another write (most likely a
/// second open tab) landed first, and the caller's snapshot is discarded
/// rather than silently overwriting it.
///
/// Responds with the full layout, not just the new revision: a newly
/// added widget's id is assigned by the server and never appears in the
/// request, so reading it back here (rather than a second `GET`) is the
/// only way a caller learns it in time to name that widget by id on its
/// very next move or resize.
#[utoipa::path(
    put,
    path = "/api/dashboard/workspaces/{workspace_id}/layout",
    request_body = ReplaceDashboardLayoutRequest,
    params(("workspace_id" = String, Path)),
    responses(
        (status = 200, body = DashboardLayoutDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn replace_dashboard_layout(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(workspace_id): Path<String>,
    Json(body): Json<ReplaceDashboardLayoutRequest>,
) -> Result<Json<DashboardLayoutDto>, HandlerError> {
    let workspace_id = parse_workspace_id(&workspace_id)?;
    let widgets = body
        .widgets
        .into_iter()
        .map(widget_placement_from_dto)
        .collect::<Result<Vec<_>, _>>()?;
    state.dashboard.replace_layout(
        &ctx.user,
        workspace_id,
        body.expected_revision,
        body.columns,
        &widgets,
    )?;
    let layout = state.dashboard.get_layout(&ctx.user, workspace_id)?;
    Ok(Json(layout.into()))
}

/// `GET /api/dashboard/widgets/catalog`: every widget type this build's
/// server currently knows how to serve. Unlike every other handler in
/// this module, this one takes no store — the registry is pure, in-memory
/// data with nothing to authorize.
#[utoipa::path(
    get,
    path = "/api/dashboard/widgets/catalog",
    responses(
        (status = 200, body = DashboardWidgetCatalogResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn dashboard_widget_catalog(
    Extension(_ctx): Authed,
) -> Json<DashboardWidgetCatalogResponse> {
    let registry = senken_dashboard::WidgetRegistry::builtin();
    Json(DashboardWidgetCatalogResponse {
        widgets: registry
            .catalog()
            .into_iter()
            .map(DashboardWidgetDefinitionDto::from)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json_auth, put_json_auth,
        serve_unfenced_test_server,
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

    /// Creates an ordinary account with exactly the grants a real
    /// "Dashboard User" role would carry — View/Create/Edit/Delete on
    /// `DashboardWorkspace`, at `Scope::Own`.
    async fn dashboard_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Dashboard User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::DashboardWorkspace, Scope::Own),
                )
                .unwrap();
        }
        login_token(addr, email, "a very long password").await
    }

    #[tokio::test]
    async fn a_request_with_no_session_is_401() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/dashboard/workspaces"))
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn two_users_cannot_see_each_others_dashboard_workspaces_and_the_total_respects_scope_too()
     {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();

        let alice_token = dashboard_user(addr, &identity, &admin, "alice@example.com").await;
        let bob_token = dashboard_user(addr, &identity, &admin, "bob@example.com").await;

        post_json_auth(
            format!("http://{addr}/api/dashboard/workspaces"),
            &alice_token,
            serde_json::json!({ "name": "Alice's Dash" }),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/dashboard/workspaces"),
            &bob_token,
            serde_json::json!({ "name": "Bob's Dash" }),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/dashboard/workspaces"),
            &bob_token,
            serde_json::json!({ "name": "Bob's Second" }),
        )
        .await;

        let alice_page = body_json(
            get_auth(
                format!("http://{addr}/api/dashboard/workspaces"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            alice_page["total"], 1,
            "alice must see only her own workspace"
        );
        assert_eq!(alice_page["rows"].as_array().unwrap().len(), 1);

        let bob_page = body_json(
            get_auth(
                format!("http://{addr}/api/dashboard/workspaces"),
                &bob_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            bob_page["total"], 2,
            "the total must respect scope too, or pagination leaks how many workspaces exist"
        );
        assert_eq!(bob_page["rows"].as_array().unwrap().len(), 2);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replace_dashboard_layout_rejects_a_stale_revision_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = dashboard_user(addr, &identity, &admin, "alice2@example.com").await;

        let created = body_json(
            post_json_auth(
                format!("http://{addr}/api/dashboard/workspaces"),
                &alice_token,
                serde_json::json!({ "name": "Contested" }),
            )
            .await,
        )
        .await;
        let workspace_id = created["id"].as_str().unwrap();

        // Two tabs both read revision 0.
        let first = put_json_auth(
            format!("http://{addr}/api/dashboard/workspaces/{workspace_id}/layout"),
            &alice_token,
            serde_json::json!({
                "expected_revision": 0,
                "columns": 12,
                "widgets": [],
            }),
        )
        .await;
        assert_eq!(first.status(), reqwest::StatusCode::OK);

        // The second tab, still holding the stale revision it opened with,
        // must be refused rather than silently overwriting the first save.
        let second = put_json_auth(
            format!("http://{addr}/api/dashboard/workspaces/{workspace_id}/layout"),
            &alice_token,
            serde_json::json!({
                "expected_revision": 0,
                "columns": 12,
                "widgets": [],
            }),
        )
        .await;
        assert_eq!(second.status(), reqwest::StatusCode::CONFLICT);

        handle.shutdown().await.unwrap();
    }
}
