//! HTTP surface for `senken serve` and `senken gui`.
//!
//! `serve` and `gui` both call [`serve`], the one function that keeps the
//! two modes from diverging. Besides `GET /api/health` and (with the `web`
//! feature) the embedded `SvelteKit` build, this crate exposes: the auth
//! surface (login, logout, set-password, `me`, and a
//! WebSocket endpoint authenticated through a short-lived ticket exchange,
//! the user/role/grant management surface (`admin_handlers`); and, as
//! of, workspaces/layouts/panes/layers, bars (through
//! `senken-loader`'s resolution ladder), the indicator catalogue and
//! computation, and alerts. See this crate's internal `auth` module for how
//! every endpoint's required permission is declared and enforced (both
//! `auth` and every handler module are private — this crate's public
//! surface is just [`serve`], [`ServeOptions`] and [`ServerHandle`]).

mod admin_handlers;
mod alert_handlers;
#[cfg(feature = "web")]
mod assets;
mod auth;
mod bars_handlers;
mod cors;
mod dto;
mod error;
mod feed;
mod identity_handlers;
mod indicator_handlers;
mod instrument_handlers;
#[cfg(test)]
mod live_feed_tests;
mod notes_handlers;
mod openapi;
mod pagination;
mod source_handlers;
mod storage_handlers;
#[cfg(test)]
mod test_support;
mod watchlist_handlers;
mod workspace_handlers;
mod ws;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, patch, post, put};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_http::trace::TraceLayer;
use utoipa::ToSchema;

use senken_alerts::{AlertEngine, AlertStore};
use senken_chart::ChartWorkspaceStore;
use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};
use senken_notes::NoteStore;
use senken_runtime::Runtime;
use senken_subscription::{BookSource, IndicatorSessionRegistry, SubscriptionPool};
use senken_watchlist::WatchlistStore;

use crate::auth::{EndpointPermission, mount};
pub(crate) use crate::error::HandlerError;
pub use error::ApiError;

/// Where the server should bind, and the transport-level policy around it.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Interface to bind. `127.0.0.1` for a local-only server.
    pub host: IpAddr,
    /// Port to bind. `0` asks the OS to pick a free one.
    pub port: u16,
    /// Extra origins allowed to make cross-origin browser requests (plan
    /// 004 B15: CORS denies by default, allowing only the server's own
    /// origin; anything beyond that is this explicit, never-a-wildcard
    /// list). The server's own origin needs no entry — browsers do not
    /// apply CORS to same-origin requests at all.
    pub allowed_origins: Vec<String>,
}

/// A running server. Dropping this without calling [`ServerHandle::shutdown`]
/// leaves the server running detached on its `tokio` task; callers that need
/// a clean stop (closing the `gui` window, `serve` handling `SIGINT`) must
/// call `shutdown` explicitly.
#[derive(Debug)]
pub struct ServerHandle {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: JoinHandle<std::io::Result<()>>,
}

impl ServerHandle {
    /// The address the server actually bound. May differ from the address
    /// requested in [`ServeOptions`] — port `0` resolves to whatever the OS
    /// chose, and this is the only way to learn it.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Signals graceful shutdown and waits for the server task to exit.
    ///
    /// Safe to call more than once is not supported — this consumes the
    /// handle, matching there being exactly one shutdown per server.
    ///
    /// # Errors
    ///
    /// Returns [`ApiError::Join`] if the server task panicked, or
    /// [`ApiError::Serve`] if it exited with an I/O error.
    pub async fn shutdown(mut self) -> Result<(), ApiError> {
        // The receiver may already be gone if the server task ended on its
        // own (e.g. a bind-time error surfaced through the join below); a
        // failed send just means there is nothing left to signal.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        match self.join.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(source)) => Err(ApiError::Serve(source)),
            Err(source) => Err(ApiError::Join(source)),
        }
    }
}

/// Shared state every handler and every permission check reads from.
///
/// Cloning is cheap (every field is an `Arc` or a `Copy` value) — axum
/// clones this once per request via the `State` extractor.
#[derive(Clone)]
pub(crate) struct AppState {
    /// The accounts store this crate exposes over HTTP.
    pub(crate) identity: Arc<IdentityStore>,
    /// Chart workspace persistence: shares
    /// `identity`'s own SQLite connection (see `ChartWorkspaceStore::new`'s own
    /// docs) rather than opening a second one.
    pub(crate) workspace: Arc<ChartWorkspaceStore>,
    /// Standalone alerts, sharing `identity`'s
    /// connection the same way `workspace` does.
    pub(crate) alerts: Arc<AlertStore>,
    /// Watchlist groups and their membership, sharing `identity`'s
    /// connection the same way `workspace`/`alerts` do.
    pub(crate) watchlists: Arc<WatchlistStore>,
    /// Freeform notes, sharing `identity`'s connection the same way
    /// `watchlists` does.
    pub(crate) notes: Arc<NoteStore>,
    /// Storage, the instrument catalog and one `SeriesLoader` per
    /// registered bar source — everything `bars_handlers`
    /// and `indicator_handlers` resolve a request against.
    pub(crate) runtime: Arc<Runtime>,
    /// One live-price [`SubscriptionPool`] per source this build has a real
    /// venue protocol for — keyed by source id, built once
    /// by [`feed::build_feed_pools`]. A source with no entry here has no
    /// live feed in this build; `ws::subscribe` and `AlertEngine` both treat
    /// that as "nothing to lease" rather than an error.
    pub(crate) feed_pools: Arc<HashMap<String, SubscriptionPool>>,
    /// The running alert engine: leases the pools above on
    /// every enabled alert's own instrument, independent of any chart.
    pub(crate) alert_engine: Arc<AlertEngine>,
    /// Outstanding WS tickets.
    pub(crate) ws_tickets: Arc<ws::TicketStore>,
    /// Live indicator sessions, deduplicated by
    /// `senken_subscription::IndicatorSessionKey` across every WS connection
    /// this server handles — one registry per server (built fresh in
    /// [`serve_with_feed_pools`]), not one per process, so two servers
    /// running in the same process (as a test stands up) never share
    /// sessions with each other.
    pub(crate) indicator_sessions: Arc<IndicatorSessionRegistry>,
    /// Book-depth sources this build actually has, keyed by source id —
    /// registration is the capability declaration, the same contract every
    /// other market-data type in this project uses (see
    /// `senken_subscription::BookSource`'s own docs). Built once per server
    /// by [`source_handlers::build_book_sources`], the same way `feed_pools`
    /// is built once by [`feed::build_feed_pools`].
    pub(crate) book_sources: Arc<HashMap<String, Arc<dyn BookSource>>>,
    /// Login attempt counters.
    pub(crate) login_limiter: Arc<identity_handlers::LoginRateLimiter>,
    /// The address this server actually bound, for the B4 non-loopback
    /// warning below — read once at startup, never the requested address
    /// (port `0` would make that meaningless).
    pub(crate) bind_host: IpAddr,
}

/// Starts the server. `serve` and `gui` both call this one function — that
/// is what keeps the two modes from diverging.
///
/// `identity` is the accounts store: every endpoint but
/// `GET /api/health` and the `OpenAPI` document goes through it, directly or
/// (for workspaces/alerts) through a store sharing its connection. `runtime`
/// is what `bars_handlers`/`indicator_handlers` resolve a bars or indicator
/// request against — built once by the caller (`senken
/// serve`/`senken gui`, or a test's own fake-venue `Runtime`) so this
/// function never has to decide which plugins to activate itself.
///
/// # Errors
///
/// Returns [`ApiError::Bind`] if `options.host`/`options.port` cannot be
/// bound, or [`ApiError::LocalAddr`] if the bound address cannot be read
/// back from the socket.
pub async fn serve(
    options: ServeOptions,
    identity: Arc<IdentityStore>,
    runtime: Arc<Runtime>,
) -> Result<ServerHandle, ApiError> {
    // one `SubscriptionPool` per source this build has a real
    // live-price protocol for (today, only OKX — see `feed`'s own docs).
    let feed_pools = feed::build_feed_pools(runtime.marketdata()).await;
    serve_with_feed_pools(options, identity, runtime, feed_pools).await
}

/// [`serve`], parameterised over the live-feed pools instead of building
/// them itself — the seam a test uses to stand a real server up on a fake
/// venue's pool (never a real one; the access-boundary
/// constraint) rather than OKX's.
pub(crate) async fn serve_with_feed_pools(
    options: ServeOptions,
    identity: Arc<IdentityStore>,
    runtime: Arc<Runtime>,
    feed_pools: HashMap<String, SubscriptionPool>,
) -> Result<ServerHandle, ApiError> {
    let addr = SocketAddr::new(options.host, options.port);
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ApiError::Bind { addr, source })?;
    let local_addr = listener.local_addr().map_err(ApiError::LocalAddr)?;

    let workspace = Arc::new(ChartWorkspaceStore::new(&identity));
    let alerts = Arc::new(AlertStore::new(&identity));
    let watchlists = Arc::new(WatchlistStore::new(&identity));
    let notes = Arc::new(NoteStore::new(&identity));
    let feed_pools = Arc::new(feed_pools);
    // reconciles every already-enabled alert against those
    // pools right now, and stays running for the life of this server —
    // independent of any chart, any pane, any browser connection.
    let alert_engine = Arc::new(AlertEngine::start(
        Arc::clone(&alerts),
        (*feed_pools).clone(),
    ));
    let state = AppState {
        identity,
        workspace,
        alerts,
        watchlists,
        notes,
        runtime,
        feed_pools,
        alert_engine,
        ws_tickets: Arc::new(ws::TicketStore::default()),
        indicator_sessions: Arc::new(IndicatorSessionRegistry::default()),
        book_sources: Arc::new(source_handlers::build_book_sources()),
        login_limiter: Arc::new(identity_handlers::LoginRateLimiter::default()),
        bind_host: local_addr.ip(),
    };
    let app = router(state, &options.allowed_origins);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let join = tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            // The sender side is held by `ServerHandle`; if it is
            // dropped without sending, treat that the same as a signal
            // to stop rather than serving forever.
            let _ = shutdown_rx.await;
        })
        .await;
        if let Err(error) = &result {
            tracing::error!(%error, %local_addr, "senken-api server exited with an error");
        }
        result
    });

    tracing::info!(%local_addr, "senken-api listening");

    Ok(ServerHandle {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        join,
    })
}

/// `GET /api/health` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Health {
    status: &'static str,
    version: &'static str,
    /// `true` while this installation's seeded default admin
    /// (`senken_identity::DEFAULT_ADMIN_EMAIL`) has not set a password yet
    /// (the first-run fence) — a coordinator addition to Q8: the
    /// login page needs an honest, unauthenticated way to decide whether to
    /// show "set a password" or "log in" on first load.
    ///
    /// This is safe to expose without becoming an account-enumeration
    /// oracle (the concern for `login`/`set-password`) because
    /// it names one fixed, already-public account — the default admin
    /// email is documented, not a secret (see `DEFAULT_ADMIN_EMAIL`'s own
    /// docs) — rather than any account the caller supplies. It reveals
    /// nothing about how many accounts exist or which arbitrary email is
    /// fenced. It also needs no credential and is mounted at
    /// `EndpointPermission::Public` on an endpoint that is not
    /// rate-limited, because a fresh install must be discoverable on the
    /// very first request, before anyone could possibly hold a session.
    needs_setup: bool,
}

#[utoipa::path(get, path = "/api/health", responses((status = 200, body = Health)))]
pub(crate) async fn health(State(state): State<AppState>) -> Json<Health> {
    let needs_setup = state
        .identity
        .is_fenced(DEFAULT_ADMIN_EMAIL)
        .unwrap_or(false);
    Json(Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        needs_setup,
    })
}

async fn api_not_found() -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": "not found" })),
    )
}

/// Every request, regardless of path, is checked against the /// second requirement: "binding to a non-loopback address with the
/// password still unset logs a warning on every request, loudly enough
/// that a deployment cannot drift into it unnoticed." This is a `tracing`
/// warning rather than a response change — silently altering behaviour for
/// every request would be its own surprise — but it fires unconditionally,
/// including for `/api/health` and the SPA fallback, exactly because
/// "every request" is the point.
async fn warn_if_insecurely_bound(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !state.bind_host.is_loopback() {
        let still_fenced = state
            .identity
            .is_fenced(DEFAULT_ADMIN_EMAIL)
            .unwrap_or(false);
        if still_fenced {
            tracing::warn!(
                host = %state.bind_host,
                "senken-api is bound to a non-loopback address while the \
                 default admin has not set a password yet — \
                 anyone who can reach this address can set it"
            );
        }
    }
    next.run(request).await
}

/// Builds the whole router: the auth-guarded `/api/*` surface, CORS, the non-loopback warning, and — with the `web`
/// feature — the embedded SPA. Every `/api` route is added through
/// [`mount`], the sole place a permission can be (or fail to be) declared;
/// see `auth`'s module docs.
fn router(state: AppState, allowed_origins: &[String]) -> Router {
    let mut api = Router::new();
    api = mount(
        api,
        &state,
        "/health",
        get(health),
        EndpointPermission::Public,
    );
    api = mount(
        api,
        &state,
        "/openapi.json",
        get(openapi::openapi_json),
        EndpointPermission::Public,
    );
    api = mount(
        api,
        &state,
        "/login",
        post(identity_handlers::login),
        EndpointPermission::Public,
    );
    api = mount(
        api,
        &state,
        "/logout",
        post(identity_handlers::logout),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        &state,
        "/set-password",
        post(identity_handlers::set_password),
        EndpointPermission::AuthenticatedFenceExempt,
    );
    api = mount(
        api,
        &state,
        "/me",
        get(identity_handlers::me),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        &state,
        "/ws/ticket",
        post(ws::issue_ticket),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        &state,
        "/ws",
        get(ws::ws_handler),
        EndpointPermission::Public,
    );
    api = mount_admin_routes(api, &state);
    api = mount_workspace_routes(api, &state);
    api = mount_bars_routes(api, &state);
    api = mount_indicator_routes(api, &state);
    api = mount_alert_routes(api, &state);
    api = mount_instrument_routes(api, &state);
    api = mount_watchlist_routes(api, &state);
    api = mount_notes_routes(api, &state);
    api = mount_storage_routes(api, &state);
    let api: Router = api.fallback(api_not_found).with_state(state.clone());

    let router = Router::new().nest("/api", api);

    #[cfg(feature = "web")]
    let router = router.fallback(get(assets::fallback));
    #[cfg(not(feature = "web"))]
    let router = router.fallback(get(|| async {
        (axum::http::StatusCode::NOT_FOUND, "not found")
    }));

    router
        .layer(TraceLayer::new_for_http())
        .layer(cors::build(allowed_origins))
        .layer(middleware::from_fn_with_state(
            state,
            warn_if_insecurely_bound,
        ))
}

/// Mounts the user/role/grant management surface, split out
/// of [`router`] purely to keep that function's line count down — every
/// route here is still added through [`mount`], the sole place a
/// permission can be declared.
///
/// The two list endpoints (`GET /api/users`, `GET /api/roles`) rely on
/// `senken_identity::IdentityStore::list_users`/`list_roles` performing
/// their own guarded, scope-aware check, so
/// `Authenticated` is enough for them. `create_user`, `create_role`,
/// `assign_role` and `grant_direct` join them at plain `Authenticated` as of, and `revoke_direct` plus the four plugin-grant methods
/// join them in turn as of Q10.1: `senken_identity::IdentityStore` now
/// performs the same `AuthenticatedUser::authorize` check on every mutation
/// in this module internally (closing the headless bypass a non-HTTP caller
/// previously had for all nine), so mounting any of them at `Acl` as well
/// would only check the same thing twice, never tighter. No route here
/// relies solely on a router-level `Acl` guard any more.
fn mount_admin_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/users",
        get(admin_handlers::list_users),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users",
        post(admin_handlers::create_user),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/roles",
        get(admin_handlers::list_roles),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/roles",
        post(admin_handlers::create_role),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users/{user_id}/roles",
        post(admin_handlers::assign_role),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users/{user_id}/grants",
        post(admin_handlers::grant_direct),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users/{user_id}/grants/revoke",
        post(admin_handlers::revoke_direct),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users/{user_id}/plugin-grants",
        post(admin_handlers::grant_plugin_permission_to_user),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/users/{user_id}/plugin-grants/revoke",
        post(admin_handlers::revoke_plugin_permission_from_user),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/roles/{role_id}/plugin-grants",
        post(admin_handlers::grant_plugin_permission_to_role),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/roles/{role_id}/plugin-grants/revoke",
        post(admin_handlers::revoke_plugin_permission_from_role),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the workspace/layout/pane/item surface, split
/// out of [`router`] the same way [`mount_admin_routes`] is. Every route is
/// mounted at plain `EndpointPermission::Authenticated`:
/// `senken_chart::ChartWorkspaceStore` performs its own `AuthenticatedUser::authorize`
/// check on every read and write, so a router-level guard
/// beyond "a valid, unfenced session exists" would only check the same
/// thing twice.
fn mount_workspace_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/workspaces",
        get(workspace_handlers::list_workspaces),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces",
        post(workspace_handlers::create_workspace),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces/default",
        get(workspace_handlers::default_workspace),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces/{workspace_id}",
        patch(workspace_handlers::rename_workspace),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces/{workspace_id}",
        delete(workspace_handlers::delete_workspace),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces/{workspace_id}/settings",
        patch(workspace_handlers::update_workspace_settings),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/workspaces/{workspace_id}/layouts",
        get(workspace_handlers::list_layouts),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/layouts/{layout_id}",
        get(workspace_handlers::get_layout),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/layouts/{layout_id}",
        put(workspace_handlers::replace_layout),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/layers/{layer_id}",
        patch(workspace_handlers::update_layer),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/layers/{layer_id}",
        delete(workspace_handlers::delete_layer),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/drawings/{drawing_id}",
        patch(workspace_handlers::update_drawing),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/drawings/{drawing_id}",
        delete(workspace_handlers::delete_drawing),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the bars surface, split out of
/// [`router`] the same way [`mount_admin_routes`] is. Bars are not owned by
/// any one account — a chart's instrument/timeframe range is shared market
/// data, the same category `GET /api/health` and the `OpenAPI` document
/// already are, minus the "no credential at all" allowance those two have —
/// so `EndpointPermission::Authenticated` is the whole permission story
/// here: a valid, unfenced session, with no further `senken_acl::Resource`
/// to scope against.
fn mount_bars_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/bars/plan",
        get(bars_handlers::plan_bars),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/bars/range",
        get(bars_handlers::range_bars),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/bars/ensure",
        post(bars_handlers::ensure_bars),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/bars/m1-download",
        post(bars_handlers::download_m1),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/bars/jobs/{job_id}",
        get(bars_handlers::bar_job_status),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the indicator catalogue and computation surface, for the same reason [`mount_bars_routes`] mounts
/// plainly at `Authenticated`: an indicator's value over a public
/// instrument's bars is not owned by any one account either.
fn mount_indicator_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/indicators",
        get(indicator_handlers::list_indicators),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/indicators/compute",
        post(indicator_handlers::compute_indicator),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the alerts surface, split out of
/// [`router`] the same way [`mount_admin_routes`] is. Every route is
/// mounted at plain `EndpointPermission::Authenticated`, for the same
/// reason [`mount_workspace_routes`]'s routes are:
/// `senken_alerts::AlertStore` performs its own guarded check on every read
/// and write. `AlertStore::all_enabled_for_engine`/`record_fire` are
/// deliberately not mounted here at all (see that store's own docs) — they
/// answer "what does the server need to keep running", never a caller's own
/// request.
fn mount_alert_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/alerts",
        get(alert_handlers::list_alerts),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/alerts",
        post(alert_handlers::create_alert),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/alerts/{alert_id}",
        get(alert_handlers::get_alert),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/alerts/{alert_id}",
        delete(alert_handlers::delete_alert),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the watchlist-groups-and-membership surface, split out of
/// [`router`] the same way [`mount_alert_routes`] is. Every route is
/// mounted at plain `EndpointPermission::Authenticated`, for the same
/// reason [`mount_alert_routes`]'s routes are: `senken_watchlist::WatchlistStore`
/// performs its own guarded check on every read and write.
///
/// `/api/watchlists/reorder` is mounted before nothing needs ordering
/// between `mount` calls (each names its own full path), but the route
/// itself is deliberately a literal segment sharing `{group_id}`'s
/// position — see `watchlist_handlers`' own module docs for why that does
/// not collide, and `reordering_groups_persists_over_http` for the proof.
fn mount_watchlist_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/watchlists",
        get(watchlist_handlers::list_watchlist_groups),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists",
        post(watchlist_handlers::create_watchlist_group),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/reorder",
        post(watchlist_handlers::reorder_watchlist_groups),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/{group_id}",
        patch(watchlist_handlers::rename_watchlist_group),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/{group_id}",
        delete(watchlist_handlers::delete_watchlist_group),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/{group_id}/members",
        get(watchlist_handlers::list_watchlist_members),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/{group_id}/members",
        post(watchlist_handlers::add_watchlist_member),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/watchlists/{group_id}/members/reorder",
        post(watchlist_handlers::reorder_watchlist_members),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/watchlist-members/{member_id}",
        delete(watchlist_handlers::remove_watchlist_member),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the notes surface, split out of [`router`] the same way
/// [`mount_alert_routes`] is. Every route is mounted at plain
/// `EndpointPermission::Authenticated`, for the same reason
/// [`mount_alert_routes`]'s routes are: `senken_notes::NoteStore` performs
/// its own guarded check on every read and write.
fn mount_notes_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/notes",
        get(notes_handlers::list_notes),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/notes",
        post(notes_handlers::create_note),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/notes/{note_id}",
        get(notes_handlers::get_note),
        EndpointPermission::Authenticated,
    );
    api = mount(
        api,
        state,
        "/notes/{note_id}",
        put(notes_handlers::update_note),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/notes/{note_id}",
        delete(notes_handlers::delete_note),
        EndpointPermission::Authenticated,
    )
}

/// Mounts instrument search (the catalog-search gap this closes —
/// see `instrument_handlers`'s own docs). Market data is global and never
/// tenanted, so `Authenticated` is the whole permission story, the same
/// reasoning `mount_bars_routes`/`mount_indicator_routes` already apply.
fn mount_instrument_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/instruments",
        get(instrument_handlers::search_instruments),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/sources",
        get(source_handlers::list_sources),
        EndpointPermission::Authenticated,
    )
}

/// Mounts the storage usage/reclamation surface, split out of [`router`]
/// the same way [`mount_workspace_routes`] is — but, unlike every route
/// mounted at plain `Authenticated` above, each handler here calls
/// `senken_identity::AuthenticatedUser::authorize` on `senken_acl::Resource::Storage`
/// itself: `senken-store` has no notion of a user for a guarded store to
/// check against, so this is the one surface in this crate where the
/// router-level guard is only "a valid, unfenced session", with the real
/// `Action`/`Resource`/`Scope::All` check performed inside
/// `storage_handlers` on every call.
fn mount_storage_routes(mut api: Router<AppState>, state: &AppState) -> Router<AppState> {
    api = mount(
        api,
        state,
        "/storage",
        get(storage_handlers::storage_report),
        EndpointPermission::Authenticated,
    );
    mount(
        api,
        state,
        "/storage/delete",
        post(storage_handlers::delete_storage),
        EndpointPermission::Authenticated,
    )
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};

    use super::{ApiError, ServeOptions, serve};
    use crate::test_support::temp_identity_store;

    fn localhost_any_port() -> ServeOptions {
        ServeOptions {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            allowed_origins: Vec::new(),
        }
    }

    async fn serve_with_fresh_store() -> (super::ServerHandle, Arc<IdentityStore>) {
        let (_dir, store) = temp_identity_store();
        let store = Arc::new(store);
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = serve(localhost_any_port(), Arc::clone(&store), Arc::new(runtime))
            .await
            .unwrap();
        (handle, store)
    }

    /// The workspace's `reqwest` has no `json` feature (plugins decode
    /// bodies themselves for error control); tests follow the same
    /// `bytes()` + `serde_json` convention.
    async fn get_json(url: impl reqwest::IntoUrl) -> (reqwest::StatusCode, serde_json::Value) {
        let response = reqwest::get(url).await.unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn health_reports_ok_and_the_crate_version() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();

        let (_, body) = get_json(format!("http://{addr}/api/health")).await;

        assert_eq!(body["status"], "ok");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unmatched_api_path_is_a_json_404_not_the_spa_fallback() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();

        let (status, body) = get_json(format!("http://{addr}/api/does-not-exist")).await;
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "not found");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn port_zero_resolves_to_the_actually_bound_address() {
        let (handle, _store) = serve_with_fresh_store().await;
        assert_ne!(handle.local_addr().port(), 0);
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_releases_the_port() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();
        handle.shutdown().await.unwrap();

        // The port must be free again immediately — a leftover listener
        // would fail this bind.
        tokio::net::TcpListener::bind(addr).await.unwrap();
    }

    #[tokio::test]
    async fn binding_an_already_bound_port_reports_which_address_failed() {
        let (first, store) = serve_with_fresh_store().await;
        let addr = first.local_addr();

        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let error = serve(
            ServeOptions {
                host: addr.ip(),
                port: addr.port(),
                allowed_origins: Vec::new(),
            },
            store,
            Arc::new(runtime),
        )
        .await
        .unwrap_err();

        match error {
            ApiError::Bind { addr: reported, .. } => assert_eq!(reported, addr),
            other => panic!("expected ApiError::Bind, got {other:?}"),
        }

        first.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn the_openapi_document_is_served_and_lists_the_auth_endpoints() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();

        let (status, body) = get_json(format!("http://{addr}/api/openapi.json")).await;
        assert_eq!(status, reqwest::StatusCode::OK);
        let paths = body["paths"].as_object().expect("paths object");
        for path in ["/api/health", "/api/login", "/api/me"] {
            assert!(paths.contains_key(path), "missing {path} in {paths:?}");
        }

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_live_session_for_an_account_that_becomes_fenced_again_is_refused_with_403_not_401() {
        // "the B4 fence is a property of the account, not the
        // session — a token minted before the password is set stays
        // fenced." Today's identity store has no public way to *unset* a
        // password once it is set, so this simulates the scenario directly
        // at the database file `senken-identity`'s own store already
        // opened, the same technique a future "admin forces a password
        // reset" feature would produce for real. The point under test is
        // this crate's own middleware: it must re-check `password_set`
        // from a fresh `resolve_session` call, not trust that a resolvable
        // session implies an unfenced account.
        let (dir, store) = crate::test_support::temp_identity_store();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, "correct horse battery staple", None)
            .unwrap();
        let (_uid, token) = store
            .login(DEFAULT_ADMIN_EMAIL, "correct horse battery staple")
            .unwrap();
        let db_path = dir.path().join("accounts.db");
        drop(store);

        {
            let raw = rusqlite::Connection::open(&db_path).unwrap();
            raw.execute(
                "UPDATE users SET password_hash = NULL WHERE email = ?1",
                [DEFAULT_ADMIN_EMAIL],
            )
            .unwrap();
        }

        let store = Arc::new(IdentityStore::open(&db_path).unwrap());
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = serve(localhost_any_port(), Arc::clone(&store), Arc::new(runtime))
            .await
            .unwrap();
        let addr = handle.local_addr();

        let response = reqwest::Client::new()
            .get(format!("http://{addr}/api/me"))
            .header("authorization", format!("Bearer {}", token.reveal()))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        // set-password itself must still work for the now-fenced account.
        let set_password = reqwest::Client::new()
            .post(format!("http://{addr}/api/set-password"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({
                    "email": DEFAULT_ADMIN_EMAIL,
                    "new_password": "a brand new long password",
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(set_password.status(), reqwest::StatusCode::NO_CONTENT);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_request_with_no_credentials_at_all_is_401_not_403() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/me")).await.unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn the_default_admin_is_seeded_fenced_and_can_bootstrap_a_password() {
        let (handle, _store) = serve_with_fresh_store().await;
        let addr = handle.local_addr();

        let response = crate::test_support::post_json(
            format!("http://{addr}/api/set-password"),
            serde_json::json!({
                "email": DEFAULT_ADMIN_EMAIL,
                "new_password": "correct horse battery staple",
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);

        handle.shutdown().await.unwrap();
    }
}
