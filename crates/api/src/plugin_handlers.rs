//! The unified plugin system over HTTP: one list for every venue plugin
//! compiled into this binary and every package discovered on disk, plus
//! the one switch a caller needs to enable or disable one.
//!
//! Every route here is mounted in `lib.rs`'s `mount_plugin_routes` and
//! listed in `openapi.rs`. Unlike `widget_plugin_handlers`, these handlers
//! read [`AppState`] directly (`state.runtime`, `state.identity`) rather
//! than a router-wide `Extension` — nothing here needs to be reachable
//! outside the `/api` router the way the widget-plugin asset server does.
//!
//! # Reading the list needs no grant; changing anything does
//!
//! `GET /api/plugins`/`GET /api/plugins/{id}` are mounted at
//! `EndpointPermission::Authenticated` with **no further check** in the
//! handler: every signed-in caller needs to know which venues are active
//! to make sense of a chart or the instrument search, the same reasoning
//! `dashboard_handlers::dashboard_widget_catalog` already applies to the
//! built-in widget catalog. `POST`/`DELETE` handlers additionally call
//! [`require_plugins_all`], mirroring
//! `widget_plugin_handlers::require_widget_plugins_all` against
//! `senken_acl::Resource::Plugin` instead.
//!
//! # When a static plugin's toggle takes effect immediately, and when it needs a restart
//!
//! Whether a static `Plugin` *activates* at all is still fixed at startup
//! (`senken_runtime::RuntimeBuilder::build` decides it once, from
//! `plugin_state`) — there is no live path to run `Plugin::activate` after
//! the fact, so turning on a plugin that never activated this run still
//! needs a restart. But a plugin that *did* activate stays registered
//! either way, exactly like a package: `set_plugin_enabled` flips a live
//! flag (`senken_runtime::Runtime::set_static_plugin_enabled`) that every
//! `MarketDataSource`/`BarSource`/`BookSource`/`FeedSource` it registered
//! already checks on every call, alongside persisting the admin's decision
//! (`senken_identity::IdentityStore::set_plugin_enabled`) so it survives
//! the next restart too. `needs_restart: true` is reserved for the two
//! cases that genuinely cannot apply live: enabling a plugin that never
//! activated, and disabling one that also registered a trade adapter — an
//! adapter already holding credentials and possibly open positions is
//! never turned off by this handler on its own (see
//! `senken_runtime::drain_registrations`'s own docs). `restart_reason`
//! says in each case what is actually still pending, rather than a blind
//! "restart to apply". A package's toggle was always live this way —
//! `WidgetPackageStore`/`DynamicVenues` both support enabling and disabling
//! a *running* registration — and never sets either field.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_acl::{Action, Resource, Scope};
use senken_identity::AuthenticatedUser;
use senken_plugin::ContributionKind;
use senken_runtime::{PluginKind, PluginListing, PluginListingState};

use crate::auth::Authed;
use crate::{AppState, HandlerError};

/// The body-size ceiling `crate::mount_plugin_routes` applies to
/// `POST /api/plugins`, in place of the router-wide JSON default — this
/// body is a packaged or compiled artifact, not JSON. Matches
/// `widget_plugin_handlers::WIDGET_PLUGIN_PACKAGE_MAX_BYTES` exactly, and
/// the plugin package store's own archive size limit, so a request this
/// crate would reject for being too large and one the store would reject
/// for the same reason agree.
pub(crate) const PLUGIN_PACKAGE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// The first four bytes of a WebAssembly binary module (`\0asm`) — how
/// [`install_plugin`] tells a bare `.wasm` upload apart from a zip archive
/// (which instead starts `PK\x03\x04`) without trusting a client-supplied
/// `Content-Type`.
const WASM_MAGIC: &[u8] = b"\0asm";

/// Checks `action` on `senken_acl::Resource::Plugin` and requires
/// `Scope::All` — installing, updating, enabling/disabling, changing
/// settings on, or removing a plugin is one administrative concern
/// regardless of whether it is a venue, a trade adapter, or a dashboard
/// widget (see `Resource::Plugin`'s own docs for why this is one resource,
/// not one per contribution kind).
fn require_plugins_all(user: &AuthenticatedUser, action: Action) -> Result<(), HandlerError> {
    let scope = user.authorize(action, Resource::Plugin)?;
    if scope == Scope::All {
        Ok(())
    } else {
        Err(HandlerError::Forbidden(
            "you do not have permission to do that".to_owned(),
        ))
    }
}

/// What kind of plugin one is, on the wire.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PluginKindDto {
    /// A `Plugin` compiled into this binary.
    Static,
    /// A package discovered under the data directory.
    Package,
}

impl From<PluginKind> for PluginKindDto {
    fn from(kind: PluginKind) -> Self {
        match kind {
            PluginKind::Static => Self::Static,
            PluginKind::Package => Self::Package,
        }
    }
}

/// A named extension point, on the wire.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContributionKindDto {
    /// Market data, bars, depth or a live feed for a venue.
    Venue,
    /// A trading adapter.
    TradeAdapter,
    /// A dashboard widget UI.
    DashboardWidget,
    /// A compiled indicator.
    Indicator,
}

impl From<ContributionKind> for ContributionKindDto {
    fn from(kind: ContributionKind) -> Self {
        match kind {
            ContributionKind::Venue => Self::Venue,
            ContributionKind::TradeAdapter => Self::TradeAdapter,
            ContributionKind::DashboardWidget => Self::DashboardWidget,
            ContributionKind::Indicator => Self::Indicator,
        }
    }
}

/// A plugin's current state, on the wire.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum PluginStateDto {
    /// Contributing normally.
    Active,
    /// Installed but deliberately turned off.
    Disabled,
    /// Failed to load, or auto-disabled by its own circuit breaker.
    Failed {
        /// Why — shown to whoever can see this plugin.
        reason: String,
    },
}

impl From<PluginListingState> for PluginStateDto {
    fn from(state: PluginListingState) -> Self {
        match state {
            PluginListingState::Active => Self::Active,
            PluginListingState::Disabled => Self::Disabled,
            PluginListingState::Failed(reason) => Self::Failed { reason },
        }
    }
}

/// One plugin, on the wire — the same row for a static venue and a
/// package.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub(crate) struct PluginDto {
    /// Stable identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version string (empty for a dynamic venue with no manifest of its
    /// own read yet).
    pub version: String,
    /// Static or package.
    pub kind: PluginKindDto,
    /// What this plugin declares it contributes.
    pub contributes: Vec<ContributionKindDto>,
    /// Its current state.
    pub state: PluginStateDto,
    /// The admin-controlled enable/disable flag. For a static plugin whose
    /// flag has never been set, this reflects whatever actually activated
    /// at startup — a completely fresh install seeds only a couple of
    /// venues as enabled (see `senken_runtime`'s default set), so every
    /// other static plugin reads back `false` until someone opts it in.
    pub enabled: bool,
    /// `true` when [`Self::enabled`] does not yet match what is actually
    /// running and applying it needs a restart — always `false` for a
    /// package, whose toggle is live, and for a static plugin whose toggle
    /// already applied live; see this module's own doc comment for the two
    /// cases that cannot.
    pub needs_restart: bool,
    /// What is still pending when [`Self::needs_restart`] is `true` — `None`
    /// whenever it is `false`. Always naming the actual reason (enabling a
    /// plugin that never activated, or a trade adapter that stays connected)
    /// rather than a blind "restart to apply" is the whole point of this
    /// field; see this module's own doc comment.
    pub restart_reason: Option<String>,
}

/// `true` when a static plugin declares a live feed for its own id but that
/// feed has no running [`senken_subscription::SubscriptionPool`] yet.
///
/// `crate::feed::build_feed_pools` runs exactly once, at server startup,
/// from a snapshot of `Runtime::feed_sources()` taken at that moment. A
/// plugin activated afterward through
/// `senken_runtime::Runtime::activate_stored_plugin` registers its
/// `FeedSource` into that same list live (see that method's own docs), so
/// this check sees it — but no pool is ever built for it this run, since
/// nothing re-runs `build_feed_pools`. This is also a true, if rarer,
/// answer for a plugin active since boot whose catalog fetch failed when
/// pools were first built; either way, `state.feed_pools` missing an entry
/// this plugin's own `FeedSource` claims to serve is the one fact that
/// matters to a caller deciding whether to expect a live price.
fn static_listing_lacks_live_feed(id: &str, state: &AppState) -> bool {
    let declares_feed = state
        .runtime
        .feed_sources()
        .iter()
        .any(|feed| feed.source_ids().iter().any(|served| served == id));
    declares_feed && !state.feed_pools.contains_key(id)
}

/// Builds the wire DTO for one catalog entry, resolving a **static**
/// plugin's admin-set flag (if any) against `state.identity` — a package's
/// own `enabled`/`state` from [`PluginListing`] already reflects live
/// truth, so only the static branch needs a second lookup.
fn to_dto(listing: PluginListing, state: &AppState) -> PluginDto {
    let identity = &state.identity;
    let contributes = listing
        .contributes
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let (enabled, needs_restart, restart_reason) = match listing.kind {
        PluginKind::Static => {
            let stored = identity.plugin_enabled(&listing.id).unwrap_or_else(|source| {
                tracing::warn!(%source, plugin = listing.id, "could not read a static plugin's stored enabled flag");
                None
            });
            // `senken_runtime::Runtime::plugin_catalog` already resolves
            // `listing.state` against this plugin's own live gate for
            // everything gateable — a static plugin that activated and
            // registered no trade adapter reports `state` in step with its
            // flag the instant `set_static_plugin_enabled` runs. With
            // nobody's decision on record, the honest answer is whatever is
            // actually running, not a hardcoded guess: a plugin nobody has
            // touched needs no restart pill.
            let actually_active = matches!(listing.state, PluginListingState::Active);
            let desired = stored.unwrap_or(actually_active);
            let mut needs_restart = desired != actually_active;
            // Given the above, a mismatch that survives this far has
            // exactly one of two causes: the plugin never activated at
            // boot and someone wants it on (nothing was ever registered
            // for a flag to gate, so only a restart starts it), or it
            // holds a trade adapter, which `drain_registrations`
            // deliberately never gates live — `listing.state` then keeps
            // reporting active no matter the flag, on purpose, so it never
            // claims "disabled" while it still trades.
            let mut restart_reason = needs_restart.then(|| {
                if desired {
                    "This plugin needs Senken to restart before it can start.".to_owned()
                } else {
                    "Trading through this plugin stays active until Senken restarts.".to_owned()
                }
            });
            // A third case, independent of the mismatch above: a plugin
            // that activated live (at boot or afterward) and is fully
            // running still has no live price stream this run when its own
            // `FeedSource` was registered after `crate::feed::build_feed_pools`
            // already ran — see `static_listing_lacks_live_feed`'s own docs.
            // Reusing `needs_restart`/`restart_reason` for this, the same
            // way a trade adapter's own remainder is reported above, is
            // deliberate: both are "this plugin is genuinely active, but
            // one specific capability is not, until Senken restarts."
            if !needs_restart
                && desired
                && actually_active
                && static_listing_lacks_live_feed(&listing.id, state)
            {
                needs_restart = true;
                restart_reason =
                    Some("Live prices for this venue start after Senken restarts.".to_owned());
            }
            (desired, needs_restart, restart_reason)
        }
        PluginKind::Package => (
            !matches!(
                listing.state,
                PluginListingState::Disabled | PluginListingState::Failed(_)
            ),
            false,
            None,
        ),
    };
    PluginDto {
        id: listing.id,
        name: listing.name,
        version: listing.version,
        kind: listing.kind.into(),
        contributes,
        state: listing.state.into(),
        enabled,
        needs_restart,
        restart_reason,
    }
}

/// `GET /api/plugins` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PluginListResponse {
    /// Every plugin this runtime knows about, static and package alike.
    pub plugins: Vec<PluginDto>,
}

/// `POST /api/plugins/{id}/enabled` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetPluginEnabledRequest {
    /// The new enable/disable flag.
    pub enabled: bool,
}

/// `GET /api/plugins`: every plugin this runtime knows about. **Route this
/// at `Authenticated`** — no further check: any signed-in caller needs to
/// know which venues are active.
#[utoipa::path(
    get,
    path = "/api/plugins",
    responses(
        (status = 200, body = PluginListResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_plugins(
    Extension(_ctx): Authed,
    State(state): State<AppState>,
) -> Json<PluginListResponse> {
    let plugins = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .map(|listing| to_dto(listing, &state))
        .collect();
    Json(PluginListResponse { plugins })
}

/// `GET /api/plugins/{id}`: one plugin's row from the same catalog
/// [`list_plugins`] reads. **Route this at `Authenticated`**, same
/// reasoning.
#[utoipa::path(
    get,
    path = "/api/plugins/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = PluginDto),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 404, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_plugin(
    Extension(_ctx): Authed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PluginDto>, HandlerError> {
    let listing = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .find(|listing| listing.id == id)
        .ok_or_else(|| HandlerError::BadRequest(format!("no plugin with id {id:?} is known")))?;
    Ok(Json(to_dto(listing, &state)))
}

/// `POST /api/plugins/{id}/enabled`: enables or disables one plugin.
/// **Route this at `Authenticated`** — the handler requires `Action::Edit`
/// on `Resource::Plugin` at `Scope::All`.
///
/// A package (a `dashboard.widget`/`indicator` contribution through
/// `WidgetPackageStore`, or a `venue` contribution already loaded into
/// `DynamicVenues`) is toggled live. A static plugin's flag is only
/// recorded — see this module's own doc comment for why applying it needs
/// a restart, which the response's `needs_restart` states rather than
/// hides.
#[utoipa::path(
    post,
    path = "/api/plugins/{id}/enabled",
    params(("id" = String, Path)),
    request_body = SetPluginEnabledRequest,
    responses(
        (status = 200, body = PluginDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn set_plugin_enabled(
    Extension(ctx): Authed,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetPluginEnabledRequest>,
) -> Result<Json<PluginDto>, HandlerError> {
    require_plugins_all(&ctx.user, Action::Edit)?;

    let listing = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .find(|listing| listing.id == id)
        .ok_or_else(|| HandlerError::BadRequest(format!("no plugin with id {id:?} is known")))?;

    match listing.kind {
        PluginKind::Static => {
            state.identity.set_plugin_enabled(&id, body.enabled)?;
            // Applies live wherever that is possible at all (see this
            // module's own doc comment). `NeverActivated` is not a failure
            // here — the admin's choice above is already persisted, and the
            // mismatch it leaves behind is exactly what makes `to_dto`
            // report `needs_restart` honestly; any other error is a defect
            // worth logging but not worth failing an otherwise-successful
            // request over.
            if let Err(source) = state.runtime.set_static_plugin_enabled(&id, body.enabled)
                && !matches!(
                    source,
                    senken_runtime::StaticPluginToggleError::NeverActivated(_)
                )
            {
                tracing::warn!(%source, plugin = id, "could not apply a static plugin's toggle live");
            }
        }
        PluginKind::Package => {
            // A package's id lives in one of two registries depending on
            // what it contributes (see `Runtime::plugin_catalog`'s own
            // docs on why they are not yet merged into one lookup): try
            // the widget/indicator package store first, then the loaded
            // dynamic venues, and report whichever actually knows this id.
            let in_packages = state
                .runtime
                .widget_plugins()
                .list()
                .is_ok_and(|packages| packages.iter().any(|package| package.id == id));
            if in_packages {
                state
                    .runtime
                    .widget_plugins()
                    .set_enabled(&id, body.enabled)?;
            } else {
                state
                    .runtime
                    .dynamic_venues()
                    .set_enabled(&id, body.enabled)
                    .map_err(|source| {
                        HandlerError::BadRequest(format!(
                            "could not change {id:?}'s enabled flag: {source}"
                        ))
                    })?;
            }
        }
    }

    let refreshed = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .find(|listing| listing.id == id)
        .ok_or_else(|| HandlerError::BadRequest(format!("no plugin with id {id:?} is known")))?;
    Ok(Json(to_dto(refreshed, &state)))
}

/// `POST /api/plugins` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstallPluginResponse {
    /// The installed package's id.
    pub id: String,
}

/// `POST /api/plugins`: installs a package from either the raw bytes of a
/// zip archive (its `manifest.json` at the archive root) or a bare
/// compiled `.wasm` indicator component — the shape
/// `POST /api/indicators/plugins` has always accepted, wrapped
/// automatically into a generated package so the same upload works through
/// this one unified endpoint (see
/// `senken_plugin::widget_package::WidgetPackageStore::install_bare_wasm_as_indicator`).
/// Which shape the body is gets decided by [`WASM_MAGIC`], never a
/// client-supplied `Content-Type`. **Route this at `Authenticated`, with a
/// body-size limit of [`PLUGIN_PACKAGE_MAX_BYTES`]** in place of the
/// router-wide JSON default — see
/// `widget_plugin_handlers::install_widget_plugin`'s own `mount()` call for
/// the exact pattern to copy. The handler itself requires `Action::Create`
/// on `Resource::Plugin` at `Scope::All`.
#[utoipa::path(
    post,
    path = "/api/plugins",
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses(
        (status = 201, body = InstallPluginResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn install_plugin(
    Extension(ctx): Authed,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<InstallPluginResponse>), HandlerError> {
    require_plugins_all(&ctx.user, Action::Create)?;
    let id = if body.starts_with(WASM_MAGIC) {
        state
            .runtime
            .widget_plugins()
            .install_bare_wasm_as_indicator(&body)?
    } else {
        state.runtime.widget_plugins().install(&body)?
    };
    Ok((StatusCode::CREATED, Json(InstallPluginResponse { id })))
}

/// `DELETE /api/plugins/{id}`: uninstalls a package's files entirely. A
/// static plugin — compiled into this binary, not a package on disk — has
/// nothing to remove: an admin can only disable it (see
/// `set_plugin_enabled`), never delete it, so this refuses with `409` and a
/// message that says exactly that rather than a generic bad request.
/// **Route this at `Authenticated`** — the handler requires
/// `Action::Delete` on `Resource::Plugin` at `Scope::All`.
///
/// Removing a package's files never touches the market data it already
/// downloaded: a venue's `<data>/sources/<id>` directory is
/// owned by `senken-marketdata`/`senken-store`, an entirely different tree
/// from the plugin package this deletes.
#[utoipa::path(
    delete,
    path = "/api/plugins/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn uninstall_plugin(
    Extension(ctx): Authed,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    require_plugins_all(&ctx.user, Action::Delete)?;

    let listing = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .find(|listing| listing.id == id)
        .ok_or_else(|| HandlerError::BadRequest(format!("no plugin with id {id:?} is known")))?;

    if listing.kind == PluginKind::Static {
        return Err(HandlerError::Conflict(
            "Built-in plugins can be disabled but not removed".to_owned(),
        ));
    }

    state.runtime.widget_plugins().uninstall(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/plugins/refresh`: an explicit rescan of the plugin package
/// directory, for the direct-file-drop install path — refresh is
/// explicit, never a filesystem watcher, since a watcher can fire mid-copy
/// and read a half-written file (mirrors
/// `widget_plugin_handlers::refresh_widget_plugins` exactly). **Route this
/// at `Authenticated`** — the handler requires `Action::View` on
/// `Resource::Plugin` at `Scope::All`, same as the plain listing (reading
/// needs no grant, but an explicit rescan is still an administrative
/// action, not something every signed-in caller can trigger for a server
/// none of them may have files on disk for).
#[utoipa::path(
    post,
    path = "/api/plugins/refresh",
    responses(
        (status = 200, body = PluginListResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn refresh_plugins(
    Extension(ctx): Authed,
    State(state): State<AppState>,
) -> Result<Json<PluginListResponse>, HandlerError> {
    require_plugins_all(&ctx.user, Action::View)?;
    state.runtime.widget_plugins().refresh()?;
    let plugins = state
        .runtime
        .plugin_catalog()
        .into_iter()
        .map(|listing| to_dto(listing, &state))
        .collect();
    Ok(Json(PluginListResponse { plugins }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};
    use senken_marketdata::{
        Instrument, InstrumentId, InstrumentStatus, MarketDataSource, SourceError,
    };
    use senken_plugin::{ActivationContext, ContributionKind, Plugin, PluginError, PluginManifest};
    use senken_runtime::Runtime;
    use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, delete_auth, get_auth, post_bytes_auth, post_json,
        post_json_auth, serve_unfenced_test_server_with,
    };

    /// A minimal static venue plugin, standing in for one of the 22 real
    /// ones — this module's own tests only need *a* static plugin, not a
    /// real venue's HTTP client.
    struct FakeVenuePlugin;

    struct FakeVenueSource;

    #[async_trait]
    impl MarketDataSource for FakeVenueSource {
        fn id(&self) -> &'static str {
            "fake-venue"
        }
        fn name(&self) -> &'static str {
            "Fake Venue"
        }
        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(Vec::new())
        }
    }

    impl Plugin for FakeVenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "fake-venue".to_owned(),
                name: "Fake Venue".to_owned(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![ContributionKind::Venue],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(FakeVenueSource));
            Ok(())
        }
    }

    fn runtime_with_fake_venue() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(FakeVenuePlugin)
            .build()
            .unwrap();
        (dir, runtime)
    }

    /// A static venue plugin whose catalog is never empty while enabled —
    /// unlike [`FakeVenuePlugin`] above (built for tests that only care
    /// whether *something* is registered), this one has a real instrument,
    /// so a test can prove it becomes searchable.
    struct RichFakeVenuePlugin(&'static str);

    struct RichFakeVenueSource(&'static str);

    #[async_trait]
    impl MarketDataSource for RichFakeVenueSource {
        fn id(&self) -> &str {
            self.0
        }
        fn name(&self) -> &str {
            self.0
        }
        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(vec![
                Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")
                    .with_status(InstrumentStatus::Trading),
            ])
        }
    }

    impl Plugin for RichFakeVenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![ContributionKind::Venue],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(RichFakeVenueSource(self.0)));
            Ok(())
        }
    }

    /// As [`RichFakeVenuePlugin`], but also declares a live feed for its
    /// own source id — the fixture
    /// [`enabling_a_never_activated_plugin_with_a_live_feed_reports_the_stream_remainder_honestly`]
    /// needs to prove the open question this work answers: a plugin
    /// activated after `crate::feed::build_feed_pools` already ran gets its
    /// `FeedSource` registered live, but no [`senken_subscription::SubscriptionPool`]
    /// this run.
    struct LiveFeedVenuePlugin(&'static str);

    struct StubProtocol;

    #[async_trait]
    impl VenueProtocol for StubProtocol {
        fn url(&self) -> &'static str {
            "wss://stub.example"
        }
        fn venue(&self) -> &'static str {
            "stub"
        }
        fn subscribe_frame(&self, _instrument: &InstrumentId) -> Result<String, ConnectionError> {
            Ok("subscribe".to_owned())
        }
        fn unsubscribe_frame(&self, _instrument: &InstrumentId) -> Result<String, ConnectionError> {
            Ok("unsubscribe".to_owned())
        }
        fn parse_message(&self, _text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
            Vec::new()
        }
    }

    struct StubFeed {
        ids: Vec<String>,
    }

    impl FeedSource for StubFeed {
        fn source_ids(&self) -> &[String] {
            &self.ids
        }
        fn serves_quotes(&self) -> bool {
            false
        }
        fn protocol(&self, _symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
            Arc::new(StubProtocol)
        }
    }

    impl Plugin for LiveFeedVenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![ContributionKind::Venue],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(RichFakeVenueSource(self.0)));
            context.register_feed_source(Arc::new(StubFeed {
                ids: vec![self.0.to_owned()],
            }));
            Ok(())
        }
    }

    /// Builds a [`Runtime`] with `plugin` stored, disabled, at boot — the
    /// state every required test of the live-toggle work starts from: an
    /// admin explicitly turned it off (or it was simply never in the
    /// default-enabled set) before this process ever started.
    fn runtime_with_disabled_plugin(
        plugin: impl Plugin + 'static,
        id: &str,
    ) -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("plugin-state.db")).unwrap());
        identity.set_plugin_enabled(id, false).unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(identity)
            .plugin(plugin)
            .build()
            .unwrap();
        (dir, runtime)
    }

    /// A plugin registering only a [`senken_trade::TradeAdapter`] — the
    /// shape `simulator` ships as today, and the one this module's own
    /// scope-decision test needs: it must never be gated live.
    struct FakeTradeOnlyPlugin;

    struct FakeAdapter;

    #[async_trait]
    impl senken_trade::TradeAdapter for FakeAdapter {
        fn id(&self) -> &'static str {
            "fake-trade-only"
        }
        fn name(&self) -> &'static str {
            "Fake Trade Only"
        }
        fn kind(&self) -> senken_trade::AdapterKind {
            senken_trade::AdapterKind::Simulation
        }
        fn capabilities(&self) -> senken_trade::AdapterCapabilities {
            senken_trade::AdapterCapabilities::market_only()
        }
        fn coverage(&self) -> senken_trade::InstrumentCoverage {
            senken_trade::InstrumentCoverage::Universal
        }
        fn settings_schema(&self) -> senken_trade::SettingsSchema {
            senken_trade::SettingsSchema::default()
        }
        async fn open_account(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<(), senken_trade::TradeError> {
            Ok(())
        }
        async fn health(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<senken_trade::AdapterHealth, senken_trade::TradeError> {
            Ok(senken_trade::AdapterHealth::Connected)
        }
        async fn balances(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            account: senken_trade::AccountRef<'_>,
        ) -> Result<senken_trade::AccountBalances, senken_trade::TradeError> {
            Ok(senken_trade::AccountBalances {
                account_id: account.id,
                currency: "USD".to_owned(),
                balance: senken_core::decimal::Scaled::new(2, 0),
                equity: senken_core::decimal::Scaled::new(2, 0),
                unrealized_pnl: senken_core::decimal::Scaled::new(2, 0),
                margin_used: None,
                margin_available: None,
                assets: Vec::new(),
            })
        }
        async fn positions(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<Vec<senken_trade::Position>, senken_trade::TradeError> {
            Ok(Vec::new())
        }
        async fn orders(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
            _filter: senken_trade::OrderFilter,
        ) -> Result<Vec<senken_trade::Order>, senken_trade::TradeError> {
            Ok(Vec::new())
        }
        async fn place_order(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
            _request: senken_trade::OrderRequest,
        ) -> Result<senken_trade::Order, senken_trade::TradeError> {
            Err(senken_trade::TradeError::unsupported(
                "fake-trade-only",
                "trading",
            ))
        }
    }

    impl Plugin for FakeTradeOnlyPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "fake-trade-only".to_owned(),
                name: "Fake Trade Only".to_owned(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![ContributionKind::TradeAdapter],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_trade_adapter(Arc::new(FakeAdapter));
            Ok(())
        }
    }

    fn runtime_with_fake_trade_only_plugin() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(FakeTradeOnlyPlugin)
            .build()
            .unwrap();
        (dir, runtime)
    }

    async fn admin_token(addr: std::net::SocketAddr) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_TEST_PASSWORD }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
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

    #[tokio::test]
    async fn the_list_includes_the_static_plugin_and_reads_with_no_grant_at_all() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = get_auth(format!("http://{addr}/api/plugins"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        let plugins = body["plugins"].as_array().unwrap();
        let fake = plugins
            .iter()
            .find(|p| p["id"] == "fake-venue")
            .expect("the static plugin activated at build time must be listed");
        assert_eq!(fake["kind"], "static");
        assert_eq!(fake["state"]["state"], "active");
        assert_eq!(fake["enabled"], true);
        assert_eq!(
            fake["needs_restart"], false,
            "a plugin nobody has toggled must not claim a restart is pending"
        );
        assert_eq!(fake["restart_reason"], serde_json::Value::Null);
        assert!(
            fake["contributes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == "venue")
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn disabling_a_static_venue_plugin_applies_immediately_and_re_enabling_restores_it() {
        // `fake-venue` registers only a `MarketDataSource` — no trade
        // adapter — so this is the fully-live-togglable case: disabling it
        // must take effect the instant the request returns, exactly like a
        // package's own toggle, with no restart pill at all.
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = post_json_auth(
            format!("http://{addr}/api/plugins/fake-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["enabled"], false,
            "the stored intent must read back as off"
        );
        assert_eq!(
            body["state"]["state"], "disabled",
            "a plugin with nothing left ungated must honestly report itself disabled"
        );
        assert_eq!(
            body["needs_restart"], false,
            "the toggle already applied live — there is nothing left for a restart to do"
        );
        assert_eq!(body["restart_reason"], serde_json::Value::Null);

        // Proves the "already applied" half directly, not just via the same
        // response: a second, independent read agrees.
        let listed =
            body_json(get_auth(format!("http://{addr}/api/plugins/fake-venue"), &token).await)
                .await;
        assert_eq!(listed["state"]["state"], "disabled");
        assert_eq!(listed["needs_restart"], false);

        // Re-enabling must be just as immediate, with no rebuild of the
        // runtime in between — the property this whole change exists for.
        let response = post_json_auth(
            format!("http://{addr}/api/plugins/fake-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": true }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["enabled"], true);
        assert_eq!(body["state"]["state"], "active");
        assert_eq!(body["needs_restart"], false);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn disabling_a_trade_adapter_plugin_never_applies_live_and_says_why() {
        // The scope decision this work made explicit: a registered trade
        // adapter already holds credentials and may be managing open
        // positions, so `set_plugin_enabled` never turns it off live. The
        // response must keep saying so — `needs_restart: true` with a
        // reason naming what is actually still pending — rather than
        // quietly reporting success.
        let (_dir, runtime) = runtime_with_fake_trade_only_plugin();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = post_json_auth(
            format!("http://{addr}/api/plugins/fake-trade-only/enabled"),
            &token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["enabled"], false,
            "the stored intent must still read back as off"
        );
        assert_eq!(
            body["state"]["state"], "active",
            "a plugin whose adapter still trades must never claim to be disabled"
        );
        assert_eq!(
            body["needs_restart"], true,
            "trading cannot be turned off live, so this must never read false"
        );
        assert_eq!(
            body["restart_reason"],
            "Trading through this plugin stays active until Senken restarts."
        );

        handle.shutdown().await.unwrap();
    }

    /// Required test 3: `POST /api/plugins/{id}/enabled` on a venue that
    /// never activated at boot applies immediately — its instrument becomes
    /// searchable in the very same process, no restart — and
    /// `needs_restart` reads back honestly `false` because this plugin
    /// declares neither a trade adapter nor a live feed, so nothing about
    /// it is left pending.
    #[tokio::test]
    async fn enabling_a_never_activated_venue_over_http_applies_immediately_and_needs_restart_is_honest()
     {
        let (_dir, runtime) =
            runtime_with_disabled_plugin(RichFakeVenuePlugin("late-http-venue"), "late-http-venue");
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let before =
            body_json(get_auth(format!("http://{addr}/api/plugins/late-http-venue"), &token).await)
                .await;
        assert_eq!(before["state"]["state"], "disabled");
        assert_eq!(before["enabled"], false);

        let response = post_json_auth(
            format!("http://{addr}/api/plugins/late-http-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": true }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["enabled"], true);
        assert_eq!(
            body["state"]["state"], "active",
            "activating a never-activated plugin must apply immediately, not just persist an intent"
        );
        assert_eq!(
            body["needs_restart"], false,
            "this plugin has no trade adapter and no live feed, so nothing about \
             enabling it live is left pending"
        );
        assert_eq!(body["restart_reason"], serde_json::Value::Null);

        // The concrete proof of "applies immediately": the instrument is
        // searchable right now, in this same process, no restart.
        let search = body_json(
            get_auth(
                format!("http://{addr}/api/instruments?q=late-http-venue:btc"),
                &token,
            )
            .await,
        )
        .await;
        assert_eq!(
            search["total"], 1,
            "the newly-enabled venue's instrument must be searchable with no rebuild"
        );

        handle.shutdown().await.unwrap();
    }

    /// Answers the live-stream open question concretely: a plugin activated
    /// after `crate::feed::build_feed_pools` already ran (server startup)
    /// gets everything else live — its state reads `active`, `enabled` is
    /// `true` — but has no running `SubscriptionPool` this run, and the
    /// response says so through the very same `needs_restart`/
    /// `restart_reason` fields a trade adapter's own remainder already
    /// uses, in product language rather than an internal term.
    #[tokio::test]
    async fn enabling_a_never_activated_plugin_with_a_live_feed_reports_the_stream_remainder_honestly()
     {
        let (_dir, runtime) = runtime_with_disabled_plugin(
            LiveFeedVenuePlugin("late-http-feed-venue"),
            "late-http-feed-venue",
        );
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = post_json_auth(
            format!("http://{addr}/api/plugins/late-http-feed-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": true }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["enabled"], true,
            "the admin's choice must read back as on"
        );
        assert_eq!(
            body["state"]["state"], "active",
            "instruments and bars are live immediately regardless of the feed"
        );
        assert_eq!(
            body["needs_restart"], true,
            "no SubscriptionPool exists yet for a feed registered after server startup"
        );
        assert_eq!(
            body["restart_reason"],
            "Live prices for this venue start after Senken restarts."
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_user_with_no_plugin_grant_gets_403_not_401_on_a_mutation_but_can_still_read() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        identity
            .create_user(
                &admin,
                "noplugin@example.com",
                "No Plugin Grant",
                Some("a very long password"),
            )
            .unwrap();
        let token = login_token(addr, "noplugin@example.com", "a very long password").await;

        // Reading needs no grant at all.
        let list = get_auth(format!("http://{addr}/api/plugins"), &token).await;
        assert_eq!(list.status(), reqwest::StatusCode::OK);

        // Mutating does, and a denial must be 403, never a logout-triggering 401.
        let response = post_json_auth(
            format!("http://{addr}/api/plugins/fake-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_scope_own_grant_on_plugin_is_refused_like_no_grant_at_all() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let user_id = identity
            .create_user(
                &admin,
                "ownscope@example.com",
                "Own Scope",
                Some("a very long password"),
            )
            .unwrap();
        identity
            .grant_direct(
                &admin,
                user_id,
                Grant::new(Action::Edit, Resource::Plugin, Scope::Own),
            )
            .unwrap();
        let token = login_token(addr, "ownscope@example.com", "a very long password").await;

        let response = post_json_auth(
            format!("http://{addr}/api/plugins/fake-venue/enabled"),
            &token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "a server-wide administrative action must never accept Own"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unknown_plugin_id_is_a_bad_request_not_a_panic() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = get_auth(format!("http://{addr}/api/plugins/does-not-exist"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    /// A minimal, valid plugin package archive: a `manifest.json` declaring
    /// one `dashboard.widget` contribution, plus the `web/index.html` its
    /// `entry` names — enough to install through `POST /api/plugins`
    /// without needing a compiled `.wasm` component at all.
    fn valid_widget_package_zip(provider_id: &str) -> Vec<u8> {
        use std::io::Write as _;
        use zip::write::SimpleFileOptions;

        let manifest = format!(
            r#"{{
                "id": "{provider_id}",
                "name": "Example Widgets",
                "version": "1.0.0",
                "contributes": [{{
                    "point": "dashboard.widget",
                    "widget": {{
                        "apiVersion": "senken.widget/v1",
                        "id": "clock",
                        "title": "Clock",
                        "defaultSize": {{ "width": 3, "height": 2 }},
                        "minSize": {{ "width": 2, "height": 2 }},
                        "dataSource": "live",
                        "entry": "index.html"
                    }}
                }}]
            }}"#
        );
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("manifest.json", options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            writer.start_file("web/index.html", options).unwrap();
            writer
                .write_all(b"<!doctype html><title>clock</title>")
                .unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[tokio::test]
    async fn installing_a_zip_package_lists_it_and_deleting_it_removes_it_again() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let install = post_bytes_auth(
            format!("http://{addr}/api/plugins"),
            &token,
            "application/zip",
            valid_widget_package_zip("acme-widgets"),
        )
        .await;
        assert_eq!(install.status(), reqwest::StatusCode::CREATED);
        let installed = body_json(install).await;
        assert_eq!(installed["id"], "acme-widgets");

        let listed = body_json(get_auth(format!("http://{addr}/api/plugins"), &token).await).await;
        let row = listed["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "acme-widgets")
            .expect("an installed package must appear in the unified catalog");
        assert_eq!(row["kind"], "package");
        assert_eq!(row["state"]["state"], "active");

        let delete = delete_auth(format!("http://{addr}/api/plugins/acme-widgets"), &token).await;
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let after = body_json(get_auth(format!("http://{addr}/api/plugins"), &token).await).await;
        assert!(
            !after["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == "acme-widgets"),
            "an uninstalled package must not be listed any more"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn uploading_a_bare_wasm_wraps_it_into_an_indicator_package() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        // Not a real, loadable wasm32-wasip2 component — this proves the
        // upload is *wrapped into a package*, not that the resulting
        // component actually links (that is `senken-plugin-host`'s own
        // concern, exercised elsewhere against a real fixture).
        let mut body = b"\0asm".to_vec();
        body.extend_from_slice(b"pretend-component-bytes");

        let install = post_bytes_auth(
            format!("http://{addr}/api/plugins"),
            &token,
            "application/wasm",
            body,
        )
        .await;
        assert_eq!(install.status(), reqwest::StatusCode::CREATED);
        let installed = body_json(install).await;
        let id = installed["id"].as_str().unwrap().to_owned();
        assert!(
            id.starts_with("indicator-"),
            "a bare .wasm upload must be wrapped into a generated indicator package, not rejected"
        );

        let row =
            body_json(get_auth(format!("http://{addr}/api/plugins/{id}"), &token).await).await;
        assert!(
            row["contributes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == "indicator")
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn deleting_a_static_plugin_is_a_conflict_not_a_delete() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = delete_auth(format!("http://{addr}/api/plugins/fake-venue"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);

        // Refused, not removed: the static plugin must still be listed and
        // active afterward.
        let listed = body_json(get_auth(format!("http://{addr}/api/plugins"), &token).await).await;
        assert!(
            listed["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["id"] == "fake-venue")
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_user_with_no_plugin_grant_is_refused_install_and_delete_with_403_not_401_and_keeps_their_session()
     {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        identity
            .create_user(
                &admin,
                "noplugin2@example.com",
                "No Plugin Grant",
                Some("a very long password"),
            )
            .unwrap();
        let token = login_token(addr, "noplugin2@example.com", "a very long password").await;

        let install = post_bytes_auth(
            format!("http://{addr}/api/plugins"),
            &token,
            "application/zip",
            valid_widget_package_zip("someone-elses-widgets"),
        )
        .await;
        assert_eq!(install.status(), reqwest::StatusCode::FORBIDDEN);

        let delete = delete_auth(format!("http://{addr}/api/plugins/fake-venue"), &token).await;
        assert_eq!(delete.status(), reqwest::StatusCode::FORBIDDEN);

        // A 403 must never behave like a logout: the same token still reads
        // the list without needing to sign in again.
        let still_works = get_auth(format!("http://{addr}/api/plugins"), &token).await;
        assert_eq!(still_works.status(), reqwest::StatusCode::OK);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn deleting_an_unknown_package_is_a_bad_request_not_a_panic() {
        let (_dir, runtime) = runtime_with_fake_venue();
        let (handle, _identity, _tmp) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response =
            delete_auth(format!("http://{addr}/api/plugins/does-not-exist"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }
}
