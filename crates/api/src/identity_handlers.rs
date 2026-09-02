//! Login, logout, set-password and `me`.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::ConnectInfo;
use axum::{Extension, Json, http::StatusCode};

use senken_core::IanaZone;
use senken_identity::IdentityError;

use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    GrantDto, LoginRequest, LoginResponse, MeResponse, SetPasswordRequest, SetZoneRequest,
    UserZoneResponse,
};

/// Sliding window a login attempt is counted in ("rate-limited per account and per source address").
const WINDOW: Duration = Duration::from_mins(1);
/// Attempts allowed per key (an account's lower-cased email, or a source
/// IP) within [`WINDOW`] before further attempts are refused.
const MAX_ATTEMPTS: usize = 10;

/// A hand-rolled login rate limiter (
/// "a hand-rolled counter is acceptable, silence is not"). `tower-governor`
/// was evaluated per the plan's suggestion but only rate-limits by
/// connection metadata available before the request body is read; this
/// plan requires limiting **by account too**, which is only known once the
/// login body is parsed, so the same primitive has to cover both keys
/// anyway. One small counter doing both is simpler than a library doing
/// one and a hand-rolled fallback doing the other.
#[derive(Default)]
pub(crate) struct LoginRateLimiter {
    attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LoginRateLimiter {
    /// Records one attempt against `key` and reports whether it is within
    /// the allowed rate. Call once per key per login attempt; a caller
    /// checking both an account key and an address key must check both and
    /// refuse the request if either is over the limit.
    fn check_and_record(&self, key: &str) -> bool {
        let mut attempts = self
            .attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let entry = attempts.entry(key.to_owned()).or_default();
        while let Some(&oldest) = entry.front() {
            if now.duration_since(oldest) > WINDOW {
                entry.pop_front();
            } else {
                break;
            }
        }
        if entry.len() >= MAX_ATTEMPTS {
            return false;
        }
        entry.push_back(now);
        true
    }
}

/// `POST /api/login`.
///
/// Rate limiting happens *before* `senken_identity::IdentityStore::login`
/// is ever called, on both keys. This is safe against the
/// account-enumeration concern that method's own dummy-hash verify exists
/// to close: the rate-limit decision depends only on how many requests a
/// key has already made, never on whether the account exists, so a
/// `429` response carries no more information than "this key has been
/// busy" regardless of which email was given.
#[utoipa::path(
    post,
    path = "/api/login",
    request_body = LoginRequest,
    responses(
        (status = 200, body = LoginResponse),
        (status = 401, description = "unknown email or wrong password", body = crate::dto::ErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn login(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, HandlerError> {
    let address_key = format!("addr:{}", addr.ip());
    let account_key = format!("acct:{}", body.email.trim().to_lowercase());
    let allowed = state.login_limiter.check_and_record(&address_key)
        & state.login_limiter.check_and_record(&account_key);
    if !allowed {
        return Err(HandlerError::TooManyRequests);
    }

    let (_user_id, token) = state.identity.login(&body.email, &body.password)?;
    Ok(Json(LoginResponse {
        token: token.reveal().to_owned(),
    }))
}

/// `POST /api/logout`. Deleting a session that does not exist (already
/// logged out, expired) is not an error — matches
/// `senken_identity::IdentityStore::logout`'s own docs.
#[utoipa::path(
    post,
    path = "/api/logout",
    responses((status = 204), (status = 401, body = crate::dto::ErrorBody))
)]
pub(crate) async fn logout(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    Extension(ctx): Authed,
) -> Result<StatusCode, HandlerError> {
    state.identity.logout(&ctx.token)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/me`. This reaches beyond the bare profile
/// shipped: the caller's roles and effective grants (for cosmetic UI use
/// only — see [`MeResponse`]'s own docs), and — per the
/// coordinator's addition to this stage — this is now the endpoint a
/// client should poll to detect "still authenticated," since it requires a
/// real session where `GET /api/health` does not.
#[utoipa::path(
    get,
    path = "/api/me",
    responses(
        (status = 200, body = MeResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, description = "account has not set a password yet", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn me(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    Extension(ctx): Authed,
) -> Result<Json<MeResponse>, HandlerError> {
    let profile = state.identity.get_own_profile(ctx.user.user_id())?;
    let roles = ctx.user.role_names().to_vec();
    let grants = ctx
        .user
        .effective_grants()
        .iter()
        .copied()
        .map(GrantDto::from)
        .collect();
    Ok(Json(MeResponse {
        id: profile.id.to_string(),
        email: profile.email,
        display_name: profile.display_name,
        disabled: profile.disabled,
        password_set: profile.password_set,
        roles,
        grants,
    }))
}

/// `GET /api/me/zone`: the caller's own stored display zone, or `null` if
/// none has been chosen yet. Self-scoped by construction, the same way
/// [`me`] is: `ctx.user.user_id()` — the id a real, checked session
/// resolved to — is the only source of the target account. There is no
/// path or body parameter that could name a *different* user's zone, so
/// there is nothing for a caller to spoof here.
#[utoipa::path(
    get,
    path = "/api/me/zone",
    responses(
        (status = 200, body = UserZoneResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, description = "account has not set a password yet", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_own_zone(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    Extension(ctx): Authed,
) -> Result<Json<UserZoneResponse>, HandlerError> {
    let zone = state.identity.get_zone(ctx.user.user_id())?;
    Ok(Json(UserZoneResponse { zone }))
}

/// `PUT /api/me/zone`: sets the caller's own display zone. Same self-scoping
/// as [`get_own_zone`] — `ctx.user.user_id()` is the only account this can
/// ever write to. `body.zone` is validated against the bundled time zone
/// database via [`IanaZone::new`] (reusing that type's own check rather than
/// reimplementing it) once the body has parsed, so an id the database does
/// not recognise gets this crate's uniform `400` + `ErrorBody` response
/// instead of axum's default rejection shape — see [`SetZoneRequest`]'s own
/// doc comment for why validation happens here and not during
/// deserialisation.
#[utoipa::path(
    put,
    path = "/api/me/zone",
    request_body = SetZoneRequest,
    responses(
        (status = 200, body = UserZoneResponse),
        (status = 400, description = "not a zone id the bundled database recognises", body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, description = "account has not set a password yet", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn set_own_zone(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    Extension(ctx): Authed,
    Json(body): Json<SetZoneRequest>,
) -> Result<Json<UserZoneResponse>, HandlerError> {
    let zone =
        IanaZone::new(body.zone).map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    state.identity.set_zone(ctx.user.user_id(), &zone)?;
    Ok(Json(UserZoneResponse { zone: Some(zone) }))
}

/// `POST /api/set-password` — the one endpoint the B4 fence exempts.
///
/// Two distinct callers reach this, distinguished by whether the shared
/// [`crate::auth::EndpointPermission::AuthenticatedFenceExempt`] guard
/// found and resolved a session:
///
/// - **Authenticated** (a valid `Authorization` header was presented): this
///   is a self-service password change. `body.email` is ignored — there is
///   no way to name a different account here — and
///   [`senken_identity::IdentityStore::set_password_for`] changes exactly
///   the caller's own account, keeping only the calling session alive.
/// - **Anonymous** (no `Authorization` header at all — the only way a
///   fenced account, which cannot log in, can ever reach this API):
///   `body.email` is required, and this handler first checks
///   [`senken_identity::IdentityStore::is_fenced`] before calling
///   [`senken_identity::IdentityStore::set_password`] with no session to
///   preserve. Without that check, `set_password` would happily overwrite
///   an *already-set* password for anyone who merely knows an email —
///   `set_password` itself does not defend against that (it is the
///   operation that clears the fence, so it cannot also require it be up).
///   An email that does not exist and one that already has a password
///   produce the identical response, so this endpoint cannot be used to
///   enumerate accounts either.
#[utoipa::path(
    post,
    path = "/api/set-password",
    request_body = SetPasswordRequest,
    responses(
        (status = 204),
        (status = 400, description = "ineligible for the anonymous path, or malformed", body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn set_password(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
    ctx: Option<Authed>,
    Json(body): Json<SetPasswordRequest>,
) -> Result<StatusCode, HandlerError> {
    if let Some(Extension(ctx)) = ctx {
        state
            .identity
            .set_password_for(ctx.user.user_id(), &body.new_password, &ctx.token)?;
        return Ok(StatusCode::NO_CONTENT);
    }

    let email = body
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .ok_or_else(|| HandlerError::BadRequest("email is required".to_owned()))?;

    let fenced = match state.identity.is_fenced(email) {
        Ok(fenced) => fenced,
        Err(IdentityError::UserNotFound) => false,
        Err(other) => return Err(other.into()),
    };
    if !fenced {
        // Deliberately the same status and message whether `email` does
        // not exist or already has a password set — see the doc comment
        // above.
        return Err(HandlerError::BadRequest(
            "cannot set a password for this account without an active session".to_owned(),
        ));
    }
    state
        .identity
        .set_password(email, &body.new_password, None)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        body_json, get_auth, post_json, post_json_auth, put_json_auth, temp_identity_store,
    };
    use crate::{ServeOptions, ServerHandle, serve};

    const ADMIN_PASSWORD: &str = "correct horse battery staple";

    /// A server whose default admin has already set a password — most of
    /// this module's tests are about the endpoints themselves, not the B4
    /// fence (which `lib.rs`'s tests own).
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

    async fn login(addr: std::net::SocketAddr, email: &str, password: &str) -> reqwest::Response {
        post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await
    }

    #[tokio::test]
    async fn logging_in_with_the_right_password_returns_a_token() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let response = login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["token"].as_str().unwrap().len() > 10);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unknown_email_and_a_wrong_password_produce_the_same_status_and_body() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let unknown = login(addr, "nobody@example.com", "irrelevant password").await;
        let unknown_status = unknown.status();
        let unknown_body = body_json(unknown).await;

        let wrong = login(addr, DEFAULT_ADMIN_EMAIL, "not the right password").await;
        let wrong_status = wrong.status();
        let wrong_body = body_json(wrong).await;

        assert_eq!(unknown_status, reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(unknown_status, wrong_status);
        assert_eq!(unknown_body, wrong_body);

        handle.shutdown().await.unwrap();
    }

    // Both are 401 and both are correct, but they are answers to different
    // questions: one is "sign in again", the other is "those details are
    // wrong". Sharing one body left a mistyped password telling the user
    // their session had expired, when they had never had one.
    #[tokio::test]
    async fn a_rejected_login_and_a_dead_session_do_not_send_the_same_401_body() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let rejected = login(addr, DEFAULT_ADMIN_EMAIL, "not the right password").await;
        assert_eq!(rejected.status(), reqwest::StatusCode::UNAUTHORIZED);
        let rejected_body = body_json(rejected).await;

        let no_session = reqwest::Client::new()
            .get(format!("http://{addr}/api/me"))
            .header("authorization", "Bearer not-a-real-token")
            .send()
            .await
            .unwrap();
        assert_eq!(no_session.status(), reqwest::StatusCode::UNAUTHORIZED);
        let no_session_body = body_json(no_session).await;

        assert_ne!(rejected_body, no_session_body);
        let message = rejected_body["error"].as_str().unwrap();
        // Says what went wrong without saying which half — an account that
        // exists and one that does not stay indistinguishable.
        assert!(
            message.contains("email") && message.contains("password"),
            "a rejected login should name what did not match, got {message:?}"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn repeated_login_attempts_for_the_same_account_are_eventually_rate_limited() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        // `MAX_ATTEMPTS` is 10; the 11th attempt for the same account
        // within the window must be refused with 429, never reaching
        // `IdentityStore::login` at all.
        let mut statuses = Vec::new();
        for _ in 0..11 {
            statuses.push(
                login(addr, DEFAULT_ADMIN_EMAIL, "wrong every time")
                    .await
                    .status(),
            );
        }
        assert!(
            statuses.contains(&reqwest::StatusCode::TOO_MANY_REQUESTS),
            "expected at least one 429 among {statuses:?}"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn me_without_a_token_is_unauthorized() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/me")).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn me_with_a_valid_token_returns_the_callers_own_profile() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token =
            body_json(login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await).await["token"]
                .as_str()
                .unwrap()
                .to_owned();

        let response = get_auth(format!("http://{addr}/api/me"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["email"], DEFAULT_ADMIN_EMAIL);
        assert_eq!(body["password_set"], true);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn logout_invalidates_the_session_so_a_later_call_is_unauthorized() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token =
            body_json(login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await).await["token"]
                .as_str()
                .unwrap()
                .to_owned();

        let logout = post_json_auth(
            format!("http://{addr}/api/logout"),
            &token,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(logout.status(), reqwest::StatusCode::NO_CONTENT);

        let me = get_auth(format!("http://{addr}/api/me"), &token).await;
        assert_eq!(me.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_authenticated_password_change_invalidates_every_other_session() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let session_a =
            body_json(login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await).await["token"]
                .as_str()
                .unwrap()
                .to_owned();
        let session_b =
            body_json(login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await).await["token"]
                .as_str()
                .unwrap()
                .to_owned();

        let change = post_json_auth(
            format!("http://{addr}/api/set-password"),
            &session_a,
            serde_json::json!({ "new_password": "a brand new long password" }),
        )
        .await;
        assert_eq!(change.status(), reqwest::StatusCode::NO_CONTENT);

        assert_eq!(
            get_auth(format!("http://{addr}/api/me"), &session_a)
                .await
                .status(),
            reqwest::StatusCode::OK,
            "the session that made the change survives"
        );
        assert_eq!(
            get_auth(format!("http://{addr}/api/me"), &session_b)
                .await
                .status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "every other session for the account is invalidated"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_anonymous_set_password_call_is_refused_once_the_account_is_no_longer_fenced() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let response = post_json(
            format!("http://{addr}/api/set-password"),
            serde_json::json!({
                "email": DEFAULT_ADMIN_EMAIL,
                "new_password": "an attempted takeover password",
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        // Proves it was actually refused, not silently accepted: the
        // original password still works.
        assert_eq!(
            login(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD)
                .await
                .status(),
            reqwest::StatusCode::OK
        );

        handle.shutdown().await.unwrap();
    }

    // --- required test: /api/me reflects a grant change without re-login

    #[tokio::test]
    async fn me_reflects_a_roles_grant_change_without_re_login() {
        // "sessions are opaque... permissions are read per
        // request from the store, so a role edit takes effect on the next
        // call rather than at the next expiry." There is no HTTP endpoint
        // to edit an existing role's *core* grants (only creating a role
        // with grants, and granting/revoking plugin permissions
        // afterward, are in this stage's scope), so this reaches the
        // database directly to produce the scenario -- the same technique
        // `crate::tests::a_live_session_for_an_account_that_becomes_fenced_again_is_refused_with_403_not_401`
        // already uses for a state the public API cannot otherwise reach.
        let (dir, store) = crate::test_support::temp_identity_store();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, "correct horse battery staple", None)
            .unwrap();
        let (_uid, admin_token) = store
            .login(DEFAULT_ADMIN_EMAIL, "correct horse battery staple")
            .unwrap();
        let admin = store
            .resolve_session(admin_token.reveal())
            .unwrap()
            .unwrap();
        let user_id = store
            .create_user(
                &admin,
                "live@example.com",
                "Live",
                Some("a very long password"),
            )
            .unwrap();
        let role_id = store.create_role(&admin, "Empty Role", "", &[]).unwrap();
        // `assign_role` rotates the target's sessions, but there is no
        // prior session here for it to invalidate.
        store.assign_role(&admin, user_id, role_id).unwrap();
        let (_uid, token) = store
            .login("live@example.com", "a very long password")
            .unwrap();
        let db_path = dir.path().join("accounts.db");
        drop(store);

        let store = std::sync::Arc::new(senken_identity::IdentityStore::open(&db_path).unwrap());
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = crate::serve(
            crate::ServeOptions {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                allowed_origins: Vec::new(),
            },
            store,
            std::sync::Arc::new(runtime),
        )
        .await
        .unwrap();
        let addr = handle.local_addr();

        let before = get_auth(format!("http://{addr}/api/me"), token.reveal()).await;
        assert_eq!(before.status(), reqwest::StatusCode::OK);
        assert_eq!(body_json(before).await["grants"], serde_json::json!([]));

        // Change what the role grants, entirely at the database layer --
        // never touching `assign_role`/`grant_direct`, which would rotate
        // sessions and defeat the point of this test.
        {
            let raw = rusqlite::Connection::open(&db_path).unwrap();
            raw.execute(
                "INSERT INTO role_grants (role_id, action, resource, scope)
                 VALUES (?1, 'view', 'chart_layout', 'own')",
                [role_id.to_string()],
            )
            .unwrap();
        }

        // The exact same token, no `/api/login` call in between.
        let after = get_auth(format!("http://{addr}/api/me"), token.reveal()).await;
        assert_eq!(after.status(), reqwest::StatusCode::OK);
        assert_eq!(
            body_json(after).await["grants"],
            serde_json::json!([{ "action": "View", "resource": "ChartLayout", "scope": "Own" }]),
            "the grant change must be visible on the very next call, with no re-login"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_anonymous_set_password_call_for_an_unknown_email_gets_the_identical_response() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let known_account_status = post_json(
            format!("http://{addr}/api/set-password"),
            serde_json::json!({
                "email": DEFAULT_ADMIN_EMAIL,
                "new_password": "an attempted takeover password",
            }),
        )
        .await
        .status();
        let unknown_account_status = post_json(
            format!("http://{addr}/api/set-password"),
            serde_json::json!({
                "email": "nobody@example.com",
                "new_password": "an attempted takeover password",
            }),
        )
        .await
        .status();

        assert_eq!(known_account_status, unknown_account_status);

        handle.shutdown().await.unwrap();
    }

    // --- GET/PUT /api/me/zone ------------------------------------------

    async fn login_token(addr: std::net::SocketAddr, email: &str, password: &str) -> String {
        body_json(login(addr, email, password).await).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Creates a second account (through the admin-only `POST /api/users`,
    /// which the seeded default admin's `Superadmin` role may always call)
    /// and returns its own session token — the second identity the
    /// cross-account isolation tests below need.
    async fn second_user_token(addr: std::net::SocketAddr, admin_token: &str) -> String {
        let response = post_json_auth(
            format!("http://{addr}/api/users"),
            admin_token,
            serde_json::json!({
                "email": "other@example.com",
                "display_name": "Other User",
                "initial_password": "a very long password",
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        login_token(addr, "other@example.com", "a very long password").await
    }

    #[tokio::test]
    async fn get_zone_without_a_token_is_unauthorized() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/me/zone"))
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_account_that_has_never_set_a_zone_reads_back_null_not_an_error() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;

        let response = get_auth(format!("http://{addr}/api/me/zone"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({ "zone": null })
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn setting_a_zone_and_reading_it_back_round_trips_over_http() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;

        let put = put_json_auth(
            format!("http://{addr}/api/me/zone"),
            &token,
            serde_json::json!({ "zone": "Asia/Tokyo" }),
        )
        .await;
        assert_eq!(put.status(), reqwest::StatusCode::OK);
        assert_eq!(
            body_json(put).await,
            serde_json::json!({ "zone": "Asia/Tokyo" })
        );

        let get = get_auth(format!("http://{addr}/api/me/zone"), &token).await;
        assert_eq!(
            body_json(get).await,
            serde_json::json!({ "zone": "Asia/Tokyo" })
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn putting_a_zone_id_the_bundled_database_does_not_recognise_is_a_bad_request() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;

        let response = put_json_auth(
            format!("http://{addr}/api/me/zone"),
            &token,
            serde_json::json!({ "zone": "Not/AZone" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        // Proves the bad request was actually refused, not silently
        // accepted: the account still reads back with no zone chosen.
        let get = get_auth(format!("http://{addr}/api/me/zone"), &token).await;
        assert_eq!(body_json(get).await, serde_json::json!({ "zone": null }));

        handle.shutdown().await.unwrap();
    }

    /// The property this endpoint pair exists to guarantee: a user may only
    /// ever read or write *their own* display zone. `get_own_zone`/
    /// `set_own_zone` take no user id from the path or body at all —
    /// `ctx.user.user_id()`, from a real resolved session, is the only
    /// source — so there is no request shape through which one account
    /// could even name a different one.
    ///
    /// This was verified by deliberately removing the guard: `set_own_zone`
    /// was temporarily changed to write to "the first user row in the
    /// database" (a `SELECT id FROM users ORDER BY created_at LIMIT 1`)
    /// instead of `ctx.user.user_id()` — simulating exactly the bug class
    /// this self-scoping prevents, a write target coming from anywhere
    /// other than the caller's own checked session. With that change in
    /// place this test failed exactly as predicted: account B's `PUT`
    /// silently landed on account A's row (the first user created) instead
    /// of B's own, so B's own `GET` still read back `zone: null` and the
    /// `right_zone` lookup panicked on `.unwrap()` of that `None` — proof
    /// the write reached the wrong account rather than B's own. Restoring
    /// `ctx.user.user_id()` made the test pass again. See this crate's
    /// implementation report for the exact diff and the panic output.
    #[tokio::test]
    async fn a_user_can_never_read_or_write_another_users_zone() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let admin_token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD).await;
        let other_token = second_user_token(addr, &admin_token).await;

        let admin_put = put_json_auth(
            format!("http://{addr}/api/me/zone"),
            &admin_token,
            serde_json::json!({ "zone": "America/New_York" }),
        )
        .await;
        assert_eq!(admin_put.status(), reqwest::StatusCode::OK);

        let other_put = put_json_auth(
            format!("http://{addr}/api/me/zone"),
            &other_token,
            serde_json::json!({ "zone": "Europe/London" }),
        )
        .await;
        assert_eq!(other_put.status(), reqwest::StatusCode::OK);

        let left_zone = body_json(
            get_auth(format!("http://{addr}/api/me/zone"), &admin_token).await,
        )
        .await["zone"]
            .as_str()
            .unwrap()
            .to_owned();
        let right_zone = body_json(
            get_auth(format!("http://{addr}/api/me/zone"), &other_token).await,
        )
        .await["zone"]
            .as_str()
            .unwrap()
            .to_owned();

        assert_eq!(left_zone, "America/New_York");
        assert_eq!(right_zone, "Europe/London");
        assert_ne!(
            left_zone, right_zone,
            "each account's own PUT must never be visible through the other account's session"
        );

        handle.shutdown().await.unwrap();
    }
}
