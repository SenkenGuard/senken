//! Per-endpoint permission declarations and the middleware that enforces
//! them before a handler ever runs.
//!
//! [`mount`] is the *only* way this crate's router attaches a handler to a
//! path — there is no other call to [`axum::Router::route`] anywhere in
//! this crate. `permission` is a required, positional argument of `mount`,
//! so an endpoint added without one is a missing-argument compile error
//! (`E0061`), not a maintainer's discipline. This was verified by
//! experiment: temporarily removing the argument from one call site and
//! running `cargo build -p senken-api` fails with exactly that error,
//! naming the call site; restoring the argument builds clean again. See
//! this crate's implementation report for the transcript.
//!
//! This mirrors the compile-time techniques `senken-acl` and
//! `senken-plugin` already use elsewhere in this plan: forgetting a
//! rule is made a type error instead of a review comment.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::{Extension, Json, Router};

use senken_identity::AuthenticatedUser;

use crate::dto::ErrorBody;
use crate::{AppState, HandlerError};

/// What an endpoint requires before its handler runs.
///
/// Login/logout/set-password/me/the WS ticket exchange are
/// inherently about the caller's *own* account, the same category as
/// `senken_identity::IdentityStore::set_password` needing no grant — so
/// those endpoints need nothing beyond the variants below. The user/role/
/// grant management endpoints once needed a third variant
/// here, `Acl(Action, Resource)`, that re-checked `(Action, Resource)` at
/// the router level — but every mutation those endpoints front now performs
/// that exact check itself via `AuthenticatedUser::authorize`
/// so a second all-or-nothing gate here would only ever
/// have been checking the same thing twice, never tighter. `Acl` was
/// removed rather than left unused once its last caller was migrated.
#[derive(Debug, Clone, Copy)]
pub(crate) enum EndpointPermission {
    /// No authentication at all: `health`, the `OpenAPI` document, `login`
    /// (there is no session yet to present), and the WS upgrade itself
    /// (a browser cannot set an `Authorization` header on a WebSocket handshake, so that endpoint authenticates itself via the ticket in its query string instead of this middleware).
    Public,
    /// A valid session is required if one is presented, but its absence is
    /// allowed through to the handler — and the B4 fence is **not**
    /// checked here even when a session is presented. Used by exactly one
    /// endpoint, `set-password`: the anonymous first-run case has no
    /// session to present at all, and the fenced case is the one this
    /// endpoint exists to clear. The handler itself decides which case it
    /// is in and enforces what an anonymous call may target.
    AuthenticatedFenceExempt,
    /// A valid, unfenced session is required. Everything else that needs a
    /// caller's identity at all but scopes or checks further inside the
    /// handler itself — `logout`, `me`, requesting a WS ticket, the listing
    /// endpoints (`GET /api/users`, `GET /api/roles`), whose
    /// `senken_identity::IdentityStore::list_users`/`list_roles` already
    /// perform their own guarded, scope-aware check
    ///  every user/role/grant mutation, each of which now
    /// calls `AuthenticatedUser::authorize` on itself — a second
    /// all-or-nothing gate here could only ever be looser than that, never
    /// tighter, so it would add nothing.
    Authenticated,
}

/// The resolved caller, attached to the request by [`enforce_permission`]
/// and read back out by a handler via the `Extension` extractor. Carries
/// the raw session token alongside the checked [`AuthenticatedUser`]
/// because two handlers need it for reasons `AuthenticatedUser` itself
/// does not expose: `logout` deletes the session named by this exact
/// token, and issuing a WS ticket redeems the ticket back
/// into this same token later, on the WS upgrade, without ever leaving
/// this crate — the token is not logged, persisted, or served back to the
/// client again after login.
#[derive(Clone)]
pub(crate) struct AuthContext {
    pub(crate) user: AuthenticatedUser,
    pub(crate) token: String,
}

/// Pulls a bearer token out of an `Authorization` header (`Authorization: Bearer`, never a cookie or a query parameter).
fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn unauthorized() -> Response {
    HandlerError::Unauthorized.into_response()
}

fn forbidden_fenced() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(ErrorBody::new(
            "this account has not set a password yet".to_owned(),
        )),
    )
        .into_response()
}

fn internal_error(source: &senken_identity::IdentityError) -> Response {
    tracing::error!(%source, "resolving a session failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorBody::new("internal server error".to_owned())),
    )
        .into_response()
}

/// The middleware [`mount`] attaches to every route: resolves the session
/// (if any is required and presented), enforces the B4 fence for every
/// permission except [`EndpointPermission::AuthenticatedFenceExempt`], and
/// inserts an [`AuthContext`] extension the handler reads back — all before
/// the handler's own body runs at all ("the runtime checks before dispatch, so a handler cannot forget").
async fn enforce_permission(
    State((state, permission)): State<(AppState, EndpointPermission)>,
    mut request: Request,
    next: Next,
) -> Response {
    match permission {
        EndpointPermission::Public => next.run(request).await,
        EndpointPermission::AuthenticatedFenceExempt => {
            let token = extract_bearer(request.headers()).map(str::to_owned);
            match token {
                None => next.run(request).await,
                Some(token) => match state.identity.resolve_session(&token) {
                    Ok(Some(user)) => {
                        request.extensions_mut().insert(AuthContext { user, token });
                        next.run(request).await
                    }
                    Ok(None) => unauthorized(),
                    Err(source) => internal_error(&source),
                },
            }
        }
        EndpointPermission::Authenticated => {
            let Some(token) = extract_bearer(request.headers()).map(str::to_owned) else {
                return unauthorized();
            };
            match state.identity.resolve_session(&token) {
                Ok(Some(user)) => {
                    if !user.password_set() {
                        return forbidden_fenced();
                    }
                    request.extensions_mut().insert(AuthContext { user, token });
                    next.run(request).await
                }
                Ok(None) => unauthorized(),
                Err(source) => internal_error(&source),
            }
        }
    }
}

/// The only way this crate mounts a handler onto a path — see
/// the module docs for why `permission` being a required argument here is
/// load-bearing, not stylistic.
pub(crate) fn mount(
    router: Router<AppState>,
    state: &AppState,
    path: &str,
    method_router: MethodRouter<AppState>,
    permission: EndpointPermission,
) -> Router<AppState> {
    let guard = middleware::from_fn_with_state((state.clone(), permission), enforce_permission);
    router.route(path, method_router.route_layer(guard))
}

/// Extracts the [`AuthContext`] a required [`EndpointPermission::Authenticated`]
/// guard already attached. Panics only if called from a route that is not
/// guarded that way, which would itself be a bug in this crate's own route
/// table, not a reachable client input.
pub(crate) type Authed = Extension<AuthContext>;
