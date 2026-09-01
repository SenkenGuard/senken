//! User, role and grant management: the endpoints Q6 found
//! missing when it built the Users & Roles settings section against a
//! server that had none — `senken_identity::IdentityStore` already had
//! `create_user`, `create_role`, `assign_role`, `grant_direct` and
//! `list_users`; its brief simply never listed the HTTP endpoints for
//! them. This module is exactly that HTTP layer, plus the plugin-grant and
//! `revoke_direct`/`list_roles` methods added to
//! `senken-identity` alongside it.
//!
//! Every route these handlers are mounted on (see `crate::router`) declares
//! its required permission through `crate::auth::mount`.
//! The two list endpoints (`GET /api/users`, `GET /api/roles`) rely on
//! `senken_identity::IdentityStore::list_users`/`list_roles` performing
//! their own guarded, scope-aware check and so need only
//! `EndpointPermission::Authenticated`.
//!
//! That same pattern covers `create_user`, `create_role`,
//! `assign_role` and `grant_direct`: those four took no `AuthenticatedUser`
//! at all until this cleanup, which meant a non-HTTP caller (a headless
//! backtest, a CLI, a test — the whole reason authorisation lives
//! in `senken-identity` rather than only here) could call them with no
//! check whatsoever. They now call `AuthenticatedUser::authorize`
//! themselves, so they too are mounted at plain `EndpointPermission::Authenticated`
//!   — a router-level `Acl` guard in front of a store that already checks
//! would only be checking the same thing twice, never tighter.
//!
//! The same gap is closed for the remaining mutations here —
//! `revoke_direct` and the four plugin-grant methods — which Q9.3 flagged
//! but left for future work. All five now take an `AuthenticatedUser` and
//! check it themselves, so every handler below extracts `Extension(ctx):
//! Authed` and passes `&ctx.user` through rather than relying on the
//! router, and every route in `crate::lib::mount_admin_routes` is mounted
//! at plain `EndpointPermission::Authenticated`. No mutation in this module
//! relies solely on a router-level `Acl` guard any more.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_acl::{Grant, PluginPermissionName};
use senken_identity::{RoleId, UserId};

use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    AssignRoleRequest, CreateRoleRequest, CreateUserRequest, GrantDto, IdResponse,
    PluginGrantRequest, RoleSummaryDto, RolesPage, UserSummaryDto, UsersPage,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`UserId`], failing with `400` (not
/// `500`) for a malformed one — a client typo is a bad request, not a
/// server error.
fn parse_user_id(raw: &str) -> Result<UserId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid user id".to_owned()))
}

/// The [`RoleId`] counterpart of [`parse_user_id`].
fn parse_role_id(raw: &str) -> Result<RoleId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid role id".to_owned()))
}

/// Parses a plugin permission's full name, failing with `400` for one that
/// does not parse as `<plugin-id>.<resource>:<operation>` —
/// distinct from [`senken_identity::IdentityError::PluginPermissionNotFound`],
/// which means the string parsed fine but no plugin has ever registered it.
fn parse_plugin_permission_name(raw: &str) -> Result<PluginPermissionName, HandlerError> {
    PluginPermissionName::parse(raw).map_err(|source| HandlerError::BadRequest(source.to_string()))
}

/// `GET /api/users`. Scoped by
/// `senken_identity::IdentityStore::list_users` itself —
/// a superadmin sees every account, an ordinary user sees only their own
/// row, and the reported `total` already respects that scope too.
#[utoipa::path(
    get,
    path = "/api/users",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = UsersPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_users(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<UsersPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.identity.list_users(&ctx.user, limit, offset)?;
    Ok(Json(UsersPage {
        rows: page.rows.into_iter().map(UserSummaryDto::from).collect(),
        total: page.total,
    }))
}

/// `POST /api/users`.
/// Mounted at plain `EndpointPermission::Authenticated`:
/// `senken_identity::IdentityStore::create_user` now performs its own
/// `Action::Create`/`Resource::User` check via `AuthenticatedUser::authorize`
/// (the same guarded shape `list_users` already had), so a second
/// all-or-nothing gate here would only be checking the same thing twice —
/// the required test "an ordinary user cannot create a user — 403, not 401"
/// is now enforced by the store itself, one layer below this handler.
#[utoipa::path(
    post,
    path = "/api/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, description = "email already registered", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_user(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state.identity.create_user(
        &ctx.user,
        &body.email,
        &body.display_name,
        body.initial_password.as_deref(),
    )?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/roles`. Scoped by
/// `senken_identity::IdentityStore::list_roles` itself, the same way
/// [`list_users`] is.
#[utoipa::path(
    get,
    path = "/api/roles",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = RolesPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_roles(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<RolesPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.identity.list_roles(&ctx.user, limit, offset)?;
    Ok(Json(RolesPage {
        rows: page.rows.into_iter().map(RoleSummaryDto::from).collect(),
        total: page.total,
    }))
}

/// `POST /api/roles`.
/// Mounted at plain `EndpointPermission::Authenticated` — see
/// [`create_user`]'s doc for why: `IdentityStore::create_role` now checks
/// `Action::Create`/`Resource::Role` itself, so the required test "an
/// ordinary user cannot create a role — 403, not 401" is enforced there.
#[utoipa::path(
    post,
    path = "/api/roles",
    request_body = CreateRoleRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_role(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateRoleRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let grants: Vec<Grant> = body.grants.into_iter().map(Grant::from).collect();
    let id = state
        .identity
        .create_role(&ctx.user, &body.name, &body.description, &grants)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `POST /api/users/{user_id}/roles`: assigns an existing
/// role to an existing user. Mounted at plain
/// `EndpointPermission::Authenticated` (guard moved to the store in Q9.3,
/// see [`create_user`]'s doc): `IdentityStore::assign_role` now checks
/// `Action::Edit`/`Resource::User` itself — assigning a role is a change to
/// the target *user's* record, the same category as [`grant_direct`].
#[utoipa::path(
    post,
    path = "/api/users/{user_id}/roles",
    request_body = AssignRoleRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn assign_role(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(user_id): Path<String>,
    Json(body): Json<AssignRoleRequest>,
) -> Result<StatusCode, HandlerError> {
    let user_id = parse_user_id(&user_id)?;
    let role_id = parse_role_id(&body.role_id)?;
    state.identity.assign_role(&ctx.user, user_id, role_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/users/{user_id}/grants`: attaches a direct
/// grant to a user, independent of any role. Mounted at plain
/// `EndpointPermission::Authenticated` (guard moved to the store in Q9.3,
/// see [`create_user`]'s doc): `IdentityStore::grant_direct` now checks
/// `Action::Edit`/`Resource::User` itself.
#[utoipa::path(
    post,
    path = "/api/users/{user_id}/grants",
    request_body = GrantDto,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn grant_direct(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(user_id): Path<String>,
    Json(body): Json<GrantDto>,
) -> Result<StatusCode, HandlerError> {
    let user_id = parse_user_id(&user_id)?;
    state
        .identity
        .grant_direct(&ctx.user, user_id, body.into())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/users/{user_id}/grants/revoke`: the inverse of
/// [`grant_direct`]. Mounted at plain `EndpointPermission::Authenticated`
/// (guard moved to the store in Q10.1, see the module doc):
/// `IdentityStore::revoke_direct` now checks `Action::Edit`/`Resource::User`
/// itself.
#[utoipa::path(
    post,
    path = "/api/users/{user_id}/grants/revoke",
    request_body = GrantDto,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn revoke_direct(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(user_id): Path<String>,
    Json(body): Json<GrantDto>,
) -> Result<StatusCode, HandlerError> {
    let user_id = parse_user_id(&user_id)?;
    state
        .identity
        .revoke_direct(&ctx.user, user_id, body.into())?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/users/{user_id}/plugin-grants`: grants a
/// plugin permission to a user directly, by name — an opaque grant, never
/// interpreted, unlike [`grant_direct`]'s structured `(Action, Resource,
/// Scope)`. Mounted at plain `EndpointPermission::Authenticated` (guard
/// moved to the store in Q10.1, see the module doc):
/// `IdentityStore::grant_plugin_permission_to_user` now checks
/// `Action::Edit`/`Resource::User` itself.
#[utoipa::path(
    post,
    path = "/api/users/{user_id}/plugin-grants",
    request_body = PluginGrantRequest,
    responses(
        (status = 204),
        (status = 400, description = "malformed name, unregistered, or orphaned", body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn grant_plugin_permission_to_user(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(user_id): Path<String>,
    Json(body): Json<PluginGrantRequest>,
) -> Result<StatusCode, HandlerError> {
    let user_id = parse_user_id(&user_id)?;
    let name = parse_plugin_permission_name(&body.name)?;
    state
        .identity
        .grant_plugin_permission_to_user(&ctx.user, user_id, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/users/{user_id}/plugin-grants/revoke`: the
/// inverse of [`grant_plugin_permission_to_user`]. Mounted at plain
/// `EndpointPermission::Authenticated` (guard moved to the store in Q10.1,
/// see the module doc).
#[utoipa::path(
    post,
    path = "/api/users/{user_id}/plugin-grants/revoke",
    request_body = PluginGrantRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn revoke_plugin_permission_from_user(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(user_id): Path<String>,
    Json(body): Json<PluginGrantRequest>,
) -> Result<StatusCode, HandlerError> {
    let user_id = parse_user_id(&user_id)?;
    let name = parse_plugin_permission_name(&body.name)?;
    state
        .identity
        .revoke_plugin_permission_from_user(&ctx.user, user_id, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/roles/{role_id}/plugin-grants`: grants a
/// plugin permission to every user holding `role_id`. Mounted at plain
/// `EndpointPermission::Authenticated` (guard moved to the store in Q10.1,
/// see the module doc): `IdentityStore::grant_plugin_permission_to_role`
/// now checks `Action::Edit`/`Resource::Role` itself.
#[utoipa::path(
    post,
    path = "/api/roles/{role_id}/plugin-grants",
    request_body = PluginGrantRequest,
    responses(
        (status = 204),
        (status = 400, description = "malformed name, unregistered, or orphaned", body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn grant_plugin_permission_to_role(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(role_id): Path<String>,
    Json(body): Json<PluginGrantRequest>,
) -> Result<StatusCode, HandlerError> {
    let role_id = parse_role_id(&role_id)?;
    let name = parse_plugin_permission_name(&body.name)?;
    state
        .identity
        .grant_plugin_permission_to_role(&ctx.user, role_id, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/roles/{role_id}/plugin-grants/revoke`: the
/// inverse of [`grant_plugin_permission_to_role`]. Mounted at plain
/// `EndpointPermission::Authenticated` (guard moved to the store in Q10.1,
/// see the module doc).
#[utoipa::path(
    post,
    path = "/api/roles/{role_id}/plugin-grants/revoke",
    request_body = PluginGrantRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn revoke_plugin_permission_from_role(
    State(state): State<crate::AppState>,
    Extension(ctx): Authed,
    Path(role_id): Path<String>,
    Json(body): Json<PluginGrantRequest>,
) -> Result<StatusCode, HandlerError> {
    let role_id = parse_role_id(&role_id)?;
    let name = parse_plugin_permission_name(&body.name)?;
    state
        .identity
        .revoke_plugin_permission_from_role(&ctx.user, role_id, &name)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use senken_acl::{PluginPermissionName, PluginPermissionRecord};
    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        body_json, get_auth, post_json, post_json_auth, temp_identity_store,
    };
    use crate::{ServeOptions, ServerHandle, serve};

    const ADMIN_PASSWORD: &str = "correct horse battery staple";

    async fn serve_unfenced() -> (ServerHandle, tempfile::TempDir) {
        let (dir, store) = temp_identity_store();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD, None)
            .unwrap();
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = serve(
            ServeOptions {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                allowed_origins: Vec::new(),
            },
            Arc::new(store),
            Arc::new(runtime),
        )
        .await
        .unwrap();
        (handle, dir)
    }

    /// Like [`serve_unfenced`], but also hands the test the `Arc<IdentityStore>`
    /// the server runs on — needed by tests that register a plugin
    /// permission directly (there is no HTTP endpoint for that; a plugin's
    /// own activation reconciliation owns it — this
    /// milestone only exposes granting and revoking an already-registered
    /// one).
    async fn serve_unfenced_with_store() -> (
        ServerHandle,
        Arc<senken_identity::IdentityStore>,
        tempfile::TempDir,
    ) {
        let (dir, store) = temp_identity_store();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD, None)
            .unwrap();
        let store = Arc::new(store);
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = serve(
            ServeOptions {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                allowed_origins: Vec::new(),
            },
            Arc::clone(&store),
            Arc::new(runtime),
        )
        .await
        .unwrap();
        (handle, store, dir)
    }

    async fn login_token(addr: std::net::SocketAddr, email: &str, password: &str) -> String {
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

    async fn create_user(
        addr: std::net::SocketAddr,
        admin_token: &str,
        email: &str,
        password: &str,
    ) -> String {
        let response = post_json_auth(
            format!("http://{addr}/api/users"),
            admin_token,
            serde_json::json!({
                "email": email,
                "display_name": "Test User",
                "initial_password": password,
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        body_json(response).await["id"].as_str().unwrap().to_owned()
    }

    // --- required test: ordinary user cannot create a user or role, 403 -

    #[tokio::test]
    async fn an_ordinary_user_cannot_create_a_user_and_gets_403_not_401() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        create_user(
            addr,
            &admin_token,
            "ordinary@example.com",
            "a very long password",
        )
        .await;
        let ordinary_token =
            login_token(addr, "ordinary@example.com", "a very long password").await;

        let response = post_json_auth(
            format!("http://{addr}/api/users"),
            &ordinary_token,
            serde_json::json!({
                "email": "other@example.com",
                "display_name": "Other",
                "initial_password": "a very long password",
            }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "403, not 401 -- Q3's client treats an authenticated-but-forbidden \
             caller very differently from one with no credential at all"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_ordinary_user_cannot_create_a_role_and_gets_403_not_401() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        create_user(
            addr,
            &admin_token,
            "ordinary2@example.com",
            "a very long password",
        )
        .await;
        let ordinary_token =
            login_token(addr, "ordinary2@example.com", "a very long password").await;

        let response = post_json_auth(
            format!("http://{addr}/api/roles"),
            &ordinary_token,
            serde_json::json!({ "name": "Sneaky Role", "description": "", "grants": [] }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn creating_a_user_with_no_credentials_at_all_is_401_not_403() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let response = post_json(
            format!("http://{addr}/api/users"),
            serde_json::json!({
                "email": "nobody@example.com",
                "display_name": "Nobody",
                "initial_password": "a very long password",
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    // --- required test: list_users respects scope in rows and total -----

    #[tokio::test]
    async fn list_users_over_http_respects_scope_in_both_rows_and_total() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;

        for i in 0..3 {
            create_user(
                addr,
                &admin_token,
                &format!("other{i}@example.com"),
                "a very long password",
            )
            .await;
        }
        let scoped_id = create_user(
            addr,
            &admin_token,
            "scoped@example.com",
            "a very long password",
        )
        .await;
        let grant = post_json_auth(
            format!("http://{addr}/api/users/{scoped_id}/grants"),
            &admin_token,
            serde_json::json!({ "action": "View", "resource": "User", "scope": "Own" }),
        )
        .await;
        assert_eq!(grant.status(), reqwest::StatusCode::NO_CONTENT);

        // Granting rotates sessions; log in fresh.
        let scoped_token = login_token(addr, "scoped@example.com", "a very long password").await;
        let response = get_auth(format!("http://{addr}/api/users"), &scoped_token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["total"], 1,
            "the total must respect scope too, or pagination leaks how many accounts exist"
        );
        let rows = body["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["email"], "scoped@example.com");

        // The admin, holding Scope::All, sees everyone.
        let admin_view = get_auth(format!("http://{addr}/api/users"), &admin_token).await;
        assert_eq!(
            body_json(admin_view).await["total"],
            5,
            "admin + 3 others + the scoped user"
        );

        handle.shutdown().await.unwrap();
    }

    // --- required test: superadmin creates a user, a role, assigns it, --
    // --- and the user's effective permissions change accordingly --------

    #[tokio::test]
    async fn a_superadmin_creates_a_user_and_role_assigns_it_and_the_users_permissions_change() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;

        let user_id = create_user(
            addr,
            &admin_token,
            "promote@example.com",
            "a very long password",
        )
        .await;

        let create_role = post_json_auth(
            format!("http://{addr}/api/roles"),
            &admin_token,
            serde_json::json!({
                "name": "Charts Only",
                "description": "",
                "grants": [{ "action": "View", "resource": "ChartLayout", "scope": "Own" }],
            }),
        )
        .await;
        assert_eq!(create_role.status(), reqwest::StatusCode::CREATED);
        let role_id = body_json(create_role).await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let assign = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/roles"),
            &admin_token,
            serde_json::json!({ "role_id": role_id }),
        )
        .await;
        assert_eq!(assign.status(), reqwest::StatusCode::NO_CONTENT);

        // Assigning a role rotates the target's sessions; log in
        // fresh, then confirm `/api/me` reports the new role and grant --
        // the exact flow Q6 could not exercise with no endpoint to call.
        let user_token = login_token(addr, "promote@example.com", "a very long password").await;
        let me = get_auth(format!("http://{addr}/api/me"), &user_token).await;
        assert_eq!(me.status(), reqwest::StatusCode::OK);
        let body = body_json(me).await;
        assert_eq!(body["roles"], serde_json::json!(["Charts Only"]));
        assert_eq!(
            body["grants"],
            serde_json::json!([{ "action": "View", "resource": "ChartLayout", "scope": "Own" }])
        );

        handle.shutdown().await.unwrap();
    }

    // --- direct grants: grant, list, revoke ------------------------------

    #[tokio::test]
    async fn granting_then_revoking_a_direct_grant_is_reflected_in_me() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        let user_id = create_user(
            addr,
            &admin_token,
            "directgrant@example.com",
            "a very long password",
        )
        .await;

        let grant_body =
            serde_json::json!({ "action": "Delete", "resource": "Adapter", "scope": "All" });
        let grant = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/grants"),
            &admin_token,
            grant_body.clone(),
        )
        .await;
        assert_eq!(grant.status(), reqwest::StatusCode::NO_CONTENT);

        let token_a = login_token(addr, "directgrant@example.com", "a very long password").await;
        let me = body_json(get_auth(format!("http://{addr}/api/me"), &token_a).await).await;
        assert_eq!(me["grants"], serde_json::json!([grant_body.clone()]));

        let revoke = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/grants/revoke"),
            &admin_token,
            grant_body,
        )
        .await;
        assert_eq!(revoke.status(), reqwest::StatusCode::NO_CONTENT);

        let token_b = login_token(addr, "directgrant@example.com", "a very long password").await;
        let me = body_json(get_auth(format!("http://{addr}/api/me"), &token_b).await).await;
        assert_eq!(me["grants"], serde_json::json!([]));

        handle.shutdown().await.unwrap();
    }

    // --- plugin permission grant/revoke over HTTP ---------

    #[tokio::test]
    async fn granting_an_unregistered_plugin_permission_over_http_is_400() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        let user_id = create_user(
            addr,
            &admin_token,
            "pluguser@example.com",
            "a very long password",
        )
        .await;

        let response = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/plugin-grants"),
            &admin_token,
            serde_json::json!({ "name": "mychart.dashboard:view" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn granting_then_revoking_a_registered_plugin_permission_to_a_user_over_http() {
        let (handle, store, _dir) = serve_unfenced_with_store().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        store
            .save_plugin_permissions("mychart", &[PluginPermissionRecord::registered(name)])
            .unwrap();
        let user_id = create_user(
            addr,
            &admin_token,
            "pluguser2@example.com",
            "a very long password",
        )
        .await;

        let grant = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/plugin-grants"),
            &admin_token,
            serde_json::json!({ "name": "mychart.dashboard:view" }),
        )
        .await;
        assert_eq!(grant.status(), reqwest::StatusCode::NO_CONTENT);

        let revoke = post_json_auth(
            format!("http://{addr}/api/users/{user_id}/plugin-grants/revoke"),
            &admin_token,
            serde_json::json!({ "name": "mychart.dashboard:view" }),
        )
        .await;
        assert_eq!(revoke.status(), reqwest::StatusCode::NO_CONTENT);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn granting_a_plugin_permission_to_a_role_over_http() {
        let (handle, store, _dir) = serve_unfenced_with_store().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        store
            .save_plugin_permissions("mychart", &[PluginPermissionRecord::registered(name)])
            .unwrap();

        let create_role = post_json_auth(
            format!("http://{addr}/api/roles"),
            &admin_token,
            serde_json::json!({ "name": "Chart Viewers", "description": "", "grants": [] }),
        )
        .await;
        let role_id = body_json(create_role).await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let grant = post_json_auth(
            format!("http://{addr}/api/roles/{role_id}/plugin-grants"),
            &admin_token,
            serde_json::json!({ "name": "mychart.dashboard:view" }),
        )
        .await;
        assert_eq!(grant.status(), reqwest::StatusCode::NO_CONTENT);

        let revoke = post_json_auth(
            format!("http://{addr}/api/roles/{role_id}/plugin-grants/revoke"),
            &admin_token,
            serde_json::json!({ "name": "mychart.dashboard:view" }),
        )
        .await;
        assert_eq!(revoke.status(), reqwest::StatusCode::NO_CONTENT);

        handle.shutdown().await.unwrap();
    }

    // --- list_roles ------------------------------------------------------

    #[tokio::test]
    async fn list_roles_over_http_includes_the_seeded_superadmin_and_a_created_role() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        post_json_auth(
            format!("http://{addr}/api/roles"),
            &admin_token,
            serde_json::json!({ "name": "Charts Only", "description": "", "grants": [] }),
        )
        .await;

        let response = get_auth(format!("http://{addr}/api/roles"), &admin_token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["total"], 2);
        let names: Vec<String> = body["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_owned())
            .collect();
        assert!(names.contains(&"Superadmin".to_owned()));
        assert!(names.contains(&"Charts Only".to_owned()));

        handle.shutdown().await.unwrap();
    }
}
