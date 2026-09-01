//! Watchlists over HTTP.
//!
//! Mirrors `alert_handlers` exactly: every handler extracts
//! `Extension(ctx): Authed` and passes `&ctx.user` straight through to
//! `senken_watchlist::WatchlistStore`, which performs its own guarded check
//! on every read and write — a second permission gate here would only ever
//! check the same thing twice, never tighter.
//!
//! # `/api/watchlists/reorder` vs `/api/watchlists/{group_id}`
//!
//! `reorder` is a literal path segment sharing a position with the
//! `{group_id}` parameter, so whether the two collide in axum's router is
//! not something to assume. They do not: axum's matcher (`matchit`) prefers
//! a literal segment over a parameter at the same position, and in any case
//! the two only ever share a *method* (`GET`/`PATCH`/`DELETE` on
//! `{group_id}` are all distinct handlers from `POST reorder`) — but the
//! deciding proof is `reordering_groups_persists_over_http` below, which
//! calls `POST /api/watchlists/reorder` against a real router and asserts
//! the reorder actually took effect rather than being swallowed as
//! "not a valid group id" by the wrong route.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_marketdata::InstrumentId;
use senken_watchlist::{WatchlistGroupId, WatchlistMemberId};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    AddWatchlistMemberRequest, CreateWatchlistGroupRequest, IdResponse,
    RenameWatchlistGroupRequest, ReorderWatchlistGroupsRequest, ReorderWatchlistMembersRequest,
    WatchlistGroupDto, WatchlistGroupsPage, WatchlistMemberDto,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`WatchlistGroupId`], failing with
/// `400` (not `500`) for a malformed one.
fn parse_group_id(raw: &str) -> Result<WatchlistGroupId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid watchlist group id".to_owned()))
}

/// The [`WatchlistMemberId`] counterpart of [`parse_group_id`].
fn parse_member_id(raw: &str) -> Result<WatchlistMemberId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid watchlist member id".to_owned()))
}

/// `GET /api/watchlists`. Scoped by `WatchlistStore::list_groups` itself —
/// a superadmin sees every group, an ordinary user sees only their own, and
/// the reported `total` already respects that scope too.
#[utoipa::path(
    get,
    path = "/api/watchlists",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = WatchlistGroupsPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_watchlist_groups(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<WatchlistGroupsPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.watchlists.list_groups(&ctx.user, limit, offset)?;
    Ok(Json(WatchlistGroupsPage {
        rows: page.rows.into_iter().map(WatchlistGroupDto::from).collect(),
        total: page.total,
    }))
}

/// `POST /api/watchlists`.
#[utoipa::path(
    post,
    path = "/api/watchlists",
    request_body = CreateWatchlistGroupRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_watchlist_group(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateWatchlistGroupRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state.watchlists.create_group(&ctx.user, &body.name)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `PATCH /api/watchlists/{group_id}`.
#[utoipa::path(
    patch,
    path = "/api/watchlists/{group_id}",
    request_body = RenameWatchlistGroupRequest,
    params(("group_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn rename_watchlist_group(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(group_id): Path<String>,
    Json(body): Json<RenameWatchlistGroupRequest>,
) -> Result<StatusCode, HandlerError> {
    let group_id = parse_group_id(&group_id)?;
    state
        .watchlists
        .rename_group(&ctx.user, group_id, &body.name)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/watchlists/{group_id}`.
#[utoipa::path(
    delete,
    path = "/api/watchlists/{group_id}",
    params(("group_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_watchlist_group(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(group_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let group_id = parse_group_id(&group_id)?;
    state.watchlists.delete_group(&ctx.user, group_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/watchlists/reorder`. See this module's docs for why this
/// literal path does not collide with `/api/watchlists/{group_id}`.
#[utoipa::path(
    post,
    path = "/api/watchlists/reorder",
    request_body = ReorderWatchlistGroupsRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn reorder_watchlist_groups(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<ReorderWatchlistGroupsRequest>,
) -> Result<StatusCode, HandlerError> {
    let ids = body
        .ids
        .iter()
        .map(|raw| parse_group_id(raw))
        .collect::<Result<Vec<_>, _>>()?;
    state.watchlists.reorder_groups(&ctx.user, &ids)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/watchlists/{group_id}/members`.
#[utoipa::path(
    get,
    path = "/api/watchlists/{group_id}/members",
    params(("group_id" = String, Path)),
    responses(
        (status = 200, body = Vec<WatchlistMemberDto>),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_watchlist_members(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(group_id): Path<String>,
) -> Result<Json<Vec<WatchlistMemberDto>>, HandlerError> {
    let group_id = parse_group_id(&group_id)?;
    let members = state.watchlists.list_members(&ctx.user, group_id)?;
    Ok(Json(
        members.into_iter().map(WatchlistMemberDto::from).collect(),
    ))
}

/// `POST /api/watchlists/{group_id}/members`. Adding an instrument the
/// group already holds is idempotent — see `WatchlistStore::add_member`'s
/// own docs — so this always returns `201` with that member's id, never a
/// conflict.
#[utoipa::path(
    post,
    path = "/api/watchlists/{group_id}/members",
    request_body = AddWatchlistMemberRequest,
    params(("group_id" = String, Path)),
    responses(
        (status = 201, body = IdResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn add_watchlist_member(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(group_id): Path<String>,
    Json(body): Json<AddWatchlistMemberRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let group_id = parse_group_id(&group_id)?;
    let instrument = InstrumentId::parse(&body.instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let id = state
        .watchlists
        .add_member(&ctx.user, group_id, &instrument)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `DELETE /api/watchlist-members/{member_id}` — a distinct top-level
/// resource path (not nested under `/api/watchlists/{group_id}`) because a
/// member's own id already uniquely identifies it, the same reasoning
/// `/api/layers/{id}`/`/api/drawings/{id}` apply to a pane item.
#[utoipa::path(
    delete,
    path = "/api/watchlist-members/{member_id}",
    params(("member_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn remove_watchlist_member(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(member_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let member_id = parse_member_id(&member_id)?;
    state.watchlists.remove_member(&ctx.user, member_id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/watchlists/{group_id}/members/reorder`.
#[utoipa::path(
    post,
    path = "/api/watchlists/{group_id}/members/reorder",
    request_body = ReorderWatchlistMembersRequest,
    params(("group_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn reorder_watchlist_members(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(group_id): Path<String>,
    Json(body): Json<ReorderWatchlistMembersRequest>,
) -> Result<StatusCode, HandlerError> {
    let group_id = parse_group_id(&group_id)?;
    let ids = body
        .ids
        .iter()
        .map(|raw| parse_member_id(raw))
        .collect::<Result<Vec<_>, _>>()?;
    state
        .watchlists
        .reorder_members(&ctx.user, group_id, &ids)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
        serve_unfenced_test_server,
    };

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

    async fn watchlist_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Watchlist User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Watchlist, Scope::Own),
                )
                .unwrap();
        }
        login_token(addr, email, "a very long password").await
    }

    #[tokio::test]
    async fn a_group_is_created_listed_renamed_and_deleted_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = watchlist_user(addr, &identity, &admin, "alice@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/watchlists"),
            &alice_token,
            serde_json::json!({ "name": "Majors" }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::CREATED);
        let group_id = body_json(create).await["id"].as_str().unwrap().to_owned();

        let page =
            body_json(get_auth(format!("http://{addr}/api/watchlists"), &alice_token).await).await;
        assert_eq!(page["total"], 1);
        assert_eq!(page["rows"][0]["name"], "Majors");

        let rename = reqwest::Client::new()
            .patch(format!("http://{addr}/api/watchlists/{group_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "name": "Renamed" })).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(rename.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/watchlists"), &alice_token).await).await;
        assert_eq!(page["rows"][0]["name"], "Renamed");

        let delete = reqwest::Client::new()
            .delete(format!("http://{addr}/api/watchlists/{group_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/watchlists"), &alice_token).await).await;
        assert_eq!(page["total"], 0);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn adding_the_same_instrument_twice_does_not_produce_two_rows_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = watchlist_user(addr, &identity, &admin, "alice2@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/watchlists"),
            &alice_token,
            serde_json::json!({ "name": "Majors" }),
        )
        .await;
        let group_id = body_json(create).await["id"].as_str().unwrap().to_owned();

        let add_body = serde_json::json!({ "instrument": "okx-spot:BTCUSDT" });
        let first = post_json_auth(
            format!("http://{addr}/api/watchlists/{group_id}/members"),
            &alice_token,
            add_body.clone(),
        )
        .await;
        assert_eq!(first.status(), reqwest::StatusCode::CREATED);
        let first_id = body_json(first).await["id"].as_str().unwrap().to_owned();

        let second = post_json_auth(
            format!("http://{addr}/api/watchlists/{group_id}/members"),
            &alice_token,
            add_body,
        )
        .await;
        assert_eq!(second.status(), reqwest::StatusCode::CREATED);
        let second_id = body_json(second).await["id"].as_str().unwrap().to_owned();
        assert_eq!(
            first_id, second_id,
            "adding an existing instrument must return the same member, not a duplicate"
        );

        let members = body_json(
            get_auth(
                format!("http://{addr}/api/watchlists/{group_id}/members"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(members.as_array().unwrap().len(), 1);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn reordering_groups_persists_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = watchlist_user(addr, &identity, &admin, "alice3@example.com").await;

        let first = body_json(
            post_json_auth(
                format!("http://{addr}/api/watchlists"),
                &alice_token,
                serde_json::json!({ "name": "First" }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let second = body_json(
            post_json_auth(
                format!("http://{addr}/api/watchlists"),
                &alice_token,
                serde_json::json!({ "name": "Second" }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Proves `POST /api/watchlists/reorder` reaches
        // `reorder_watchlist_groups`, not `PATCH /api/watchlists/{group_id}`
        // misparsing "reorder" as a group id — see this module's docs.
        let reorder = post_json_auth(
            format!("http://{addr}/api/watchlists/reorder"),
            &alice_token,
            serde_json::json!({ "ids": [second, first] }),
        )
        .await;
        assert_eq!(reorder.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/watchlists"), &alice_token).await).await;
        assert_eq!(page["rows"][0]["id"], second);
        assert_eq!(page["rows"][0]["position"], 0);
        assert_eq!(page["rows"][1]["id"], first);
        assert_eq!(page["rows"][1]["position"], 1);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn two_users_cannot_reach_each_others_watchlists_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = watchlist_user(addr, &identity, &admin, "alice4@example.com").await;
        let bob_token = watchlist_user(addr, &identity, &admin, "bob4@example.com").await;

        let alice_group = body_json(
            post_json_auth(
                format!("http://{addr}/api/watchlists"),
                &alice_token,
                serde_json::json!({ "name": "Alice's" }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = reqwest::Client::new()
            .patch(format!("http://{addr}/api/watchlists/{alice_group}"))
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
    async fn a_request_with_no_session_is_401() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/watchlists"))
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_invalid_instrument_string_is_400() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = watchlist_user(addr, &identity, &admin, "alice5@example.com").await;

        let group_id = body_json(
            post_json_auth(
                format!("http://{addr}/api/watchlists"),
                &alice_token,
                serde_json::json!({ "name": "Majors" }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = post_json_auth(
            format!("http://{addr}/api/watchlists/{group_id}/members"),
            &alice_token,
            serde_json::json!({ "instrument": "not-a-valid-instrument" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }
}
