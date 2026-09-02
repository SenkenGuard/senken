//! Dynamic widget UI packages over HTTP: the effective catalog a
//! dashboard's "add widget" picker and placeholder check read from, package
//! management (install/list/enable/disable/uninstall/refresh), and the
//! isolated static-asset server a sandboxed iframe loads a widget's bundle
//! from.
//!
//! Every route here is mounted in `lib.rs`'s `mount_widget_plugin_routes`
//! (and, for the asset route, `widget_plugin_asset_layer`) and listed in
//! `openapi.rs`. This crate's own convention (see `crate::auth`'s module
//! docs) is that **every** route is attached through `mount()`, which takes
//! a required [`crate::auth::EndpointPermission`] — and `mount()` takes
//! `&AppState`. None of the handlers below touch `AppState` at all (they
//! take `Extension<Arc<senken_plugin::widget_package::WidgetPackageStore>>`
//! instead, which is generic over any router state); `lib.rs` clones the
//! store out of `AppState::runtime` once and adds it as a router-wide
//! `Extension` layer, so this module never needs to know `AppState` exists.
//!
//! # Why the asset route answers with plain status codes, not `ErrorBody`
//!
//! Every other handler in this crate returns `Result<_, HandlerError>`,
//! which serializes a failure as a JSON [`crate::dto::ErrorBody`] — right
//! for a JSON API, wrong for a byte-stream asset server a `<script src>` or
//! this platform's own sandboxed iframe fetches directly. A missing script
//! must 404 the way any static file server 404s, not hand back
//! `{"error":"..."}` with a `400` a browser's network tab will not explain.
//! [`widget_plugin_asset`] is deliberately the one handler in this module
//! (and, so far, this crate) that builds an [`axum::response::Response`] by
//! hand instead of going through that conversion.

use std::path::Path as FsPath;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_acl::{Action, Resource, Scope};
use senken_identity::AuthenticatedUser;
use senken_plugin::widget_package::{
    DataSource, GridSize, InstalledPackage, PackageStatus, ValidatedWidgetContribution,
    WidgetPackageStore,
};

use crate::HandlerError;
use crate::auth::Authed;

/// The body-size ceiling the integrator's `mount()` call for `POST
/// /api/widget-plugins` should apply in place of the router-wide JSON
/// default — mirrors `indicator_handlers::INDICATOR_PLUGIN_MAX_BYTES`
/// exactly, for the same reason (this body is a compiled/packaged
/// artifact, not JSON). Matches
/// `senken_plugin::widget_package::store`'s own archive size limit, so a
/// request this crate would reject for being too large and one the store
/// would reject for the same reason agree.
pub(crate) const WIDGET_PLUGIN_PACKAGE_MAX_BYTES: usize = 32 * 1024 * 1024;

/// The exact sandbox Content Security Policy this platform's design record
/// specifies for a dynamic widget's `index.html` — applied as a response
/// header (not a `<meta>` tag, which cannot express `frame-ancestors` and
/// is weaker for `form-action`/`base-uri`) whenever this server answers
/// with `text/html`.
const WIDGET_SANDBOX_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none';";

/// Checks `action` on `senken_acl::Resource::WidgetPlugin` and requires
/// `Scope::All` — a widget UI package is third-party code the server runs
/// in a sandboxed iframe for every user of this server, not any one
/// account's own property, the same "not owned by any one account" shape
/// `storage_handlers::require_storage_all` uses for disk usage. Its own
/// resource variant, not `Resource::Storage`: administering what a server
/// keeps on disk and administering which third-party UI code it runs are
/// different administrative concerns that a role or a direct grant may need
/// to scope independently.
fn require_widget_plugins_all(
    user: &AuthenticatedUser,
    action: Action,
) -> Result<(), HandlerError> {
    let scope = user.authorize(action, Resource::WidgetPlugin)?;
    if scope == Scope::All {
        Ok(())
    } else {
        Err(HandlerError::Forbidden(
            "you do not have permission to do that".to_owned(),
        ))
    }
}

/// A grid size, in grid cells, on the wire.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WidgetPluginGridSizeDto {
    /// Width, in grid columns.
    pub width: u32,
    /// Height, in grid rows.
    pub height: u32,
}

impl From<GridSize> for WidgetPluginGridSizeDto {
    fn from(size: GridSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

/// Where a widget's data comes from, on the wire.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WidgetPluginDataSourceDto {
    /// Reads real, live data.
    Live,
    /// Renders a fixed or synthetic example rather than anything real. The
    /// dashboard's host frame draws a mockup label over any widget that
    /// reports this — never the widget itself, so it cannot suppress its
    /// own label.
    Mock,
}

impl From<DataSource> for WidgetPluginDataSourceDto {
    fn from(data_source: DataSource) -> Self {
        match data_source {
            DataSource::Live => Self::Live,
            DataSource::Mock => Self::Mock,
        }
    }
}

/// One widget a plugin package contributes, on the wire — the effective
/// catalog entry a dashboard's "add widget" picker and placeholder check
/// read.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WidgetPluginDefinitionDto {
    /// `<provider_id>/<widget id>`.
    pub widget_type_id: String,
    /// The package that contributes this widget — the `widget_type_id`
    /// prefix, split out once here so a caller never re-derives it.
    pub provider_id: String,
    /// Display title, for the "add widget" picker and the widget's own
    /// header.
    pub title: String,
    /// A one-line description, for the "add widget" picker.
    pub description: String,
    /// A free-text grouping label for the "add widget" picker.
    pub category: String,
    /// The size a newly added instance starts at.
    pub default_size: WidgetPluginGridSizeDto,
    /// The smallest size this widget can be resized to.
    pub min_size: WidgetPluginGridSizeDto,
    /// The largest size this widget can be resized to, if bounded.
    pub max_size: Option<WidgetPluginGridSizeDto>,
    /// The version of this widget's own `config` shape.
    pub config_schema_version: u32,
    /// A JSON-object schema for this widget's `config`; the host never
    /// interprets its fields.
    pub config_schema: serde_json::Value,
    /// Permission names this widget needs granted before it renders for
    /// real (not yet enforced by this build).
    pub required_permissions: Vec<String>,
    /// Host capability names this widget needs (not yet enforced by this
    /// build).
    pub required_capabilities: Vec<String>,
    /// Where this widget's data comes from.
    pub data_source: WidgetPluginDataSourceDto,
    /// The URL, on this server's own origin, serving this widget's entry
    /// document — already resolved from the package's `web/`-relative
    /// `entry`, so a caller never builds this path itself. Point a
    /// sandboxed `<iframe src>` at exactly this.
    pub entry_url: String,
}

impl From<ValidatedWidgetContribution> for WidgetPluginDefinitionDto {
    fn from(widget: ValidatedWidgetContribution) -> Self {
        // `widget_type_id` is always `<provider_id>/<widget id>` (see
        // `senken_plugin::widget_package::manifest`'s own docs: the id is
        // derived, never read from the manifest as a whole value), so
        // splitting it back apart here is exact, not a guess.
        let provider_id = widget.widget_type_id.split_once('/').map_or_else(
            || widget.widget_type_id.clone(),
            |(provider, _)| provider.to_owned(),
        );
        let entry_url = format!("/widget-plugin-assets/{provider_id}/{}", widget.entry);
        Self {
            widget_type_id: widget.widget_type_id,
            provider_id,
            title: widget.title,
            description: widget.description,
            category: widget.category,
            default_size: widget.default_size.into(),
            min_size: widget.min_size.into(),
            max_size: widget.max_size.map(Into::into),
            config_schema_version: widget.config_schema_version,
            config_schema: widget.config_schema,
            required_permissions: widget.required_permissions,
            required_capabilities: widget.required_capabilities,
            data_source: widget.data_source.into(),
            entry_url,
        }
    }
}

/// `GET /api/widget-plugins/catalog` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WidgetPluginCatalogResponse {
    /// Every widget every currently active widget plugin package
    /// contributes.
    pub widgets: Vec<WidgetPluginDefinitionDto>,
}

/// Where an installed package currently stands, on the wire.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum WidgetPluginStatusDto {
    /// Enabled, and its manifest validated cleanly.
    Active,
    /// An admin has disabled this package.
    Disabled,
    /// Enabled, but its manifest failed to validate (or its files are
    /// unreadable).
    Failed {
        /// Why — shown to whoever installed it.
        reason: String,
    },
}

impl From<PackageStatus> for WidgetPluginStatusDto {
    fn from(status: PackageStatus) -> Self {
        match status {
            PackageStatus::Active => Self::Active,
            PackageStatus::Disabled => Self::Disabled,
            PackageStatus::Failed(reason) => Self::Failed { reason },
        }
    }
}

/// One installed widget plugin package, on the wire.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WidgetPluginPackageDto {
    /// The package's own id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The package's own version string.
    pub version: String,
    /// A one-line description.
    pub description: String,
    /// The admin-controlled enable/disable flag.
    pub enabled: bool,
    /// This package's current status.
    pub status: WidgetPluginStatusDto,
    /// A SHA-256 hex digest of this package's own `manifest.json` bytes.
    pub digest: String,
    /// How many widgets this package declares (`0` while
    /// [`WidgetPluginStatusDto::Disabled`] or
    /// [`WidgetPluginStatusDto::Failed`], since neither contributes
    /// anything to the effective catalog).
    pub widget_count: usize,
    /// `true` for the package this server installs on every fresh start.
    /// It can still be disabled like any other package; it cannot be
    /// uninstalled — `DELETE /api/widget-plugins/{id}` refuses it (see
    /// `senken_plugin::widget_package::WidgetPackageError::CannotUninstallBuiltIn`).
    pub is_builtin: bool,
}

impl From<InstalledPackage> for WidgetPluginPackageDto {
    fn from(package: InstalledPackage) -> Self {
        Self {
            id: package.id,
            name: package.name,
            version: package.version,
            description: package.description,
            enabled: package.enabled,
            widget_count: package.widgets.len(),
            status: package.status.into(),
            digest: package.digest,
            is_builtin: package.is_builtin,
        }
    }
}

/// `GET /api/widget-plugins` and `POST /api/widget-plugins/refresh`
/// response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WidgetPluginListResponse {
    /// Every installed package, in a stable order.
    pub packages: Vec<WidgetPluginPackageDto>,
}

/// `POST /api/widget-plugins` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstallWidgetPluginResponse {
    /// The installed package's own id, read from its manifest — never
    /// chosen by the caller.
    pub id: String,
}

/// `POST /api/widget-plugins/{id}/enabled` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetWidgetPluginEnabledRequest {
    /// The new enable/disable flag.
    pub enabled: bool,
}

/// `GET /api/widget-plugins/catalog`: every widget every currently active
/// widget plugin package contributes. **Route this at `Authenticated`** —
/// every signed-in caller sees the same catalog, the same choice
/// `dashboard_handlers::dashboard_widget_catalog` already makes for the
/// built-in catalog.
///
/// A caller merges this with `GET /api/dashboard/widgets/catalog` to get
/// the full effective catalog a dashboard's "add widget" picker and
/// placeholder check need — merging is left to the caller since the two
/// live in different crates this module does not depend on.
#[utoipa::path(
    get,
    path = "/api/widget-plugins/catalog",
    responses(
        (status = 200, body = WidgetPluginCatalogResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn widget_plugin_catalog(
    Extension(_ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
) -> Result<Json<WidgetPluginCatalogResponse>, HandlerError> {
    let widgets = store
        .effective_widget_catalog()?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(WidgetPluginCatalogResponse { widgets }))
}

/// `GET /api/widget-plugins`: every installed package, enabled or not.
/// **Route this at `Authenticated`** — the handler itself requires
/// `Action::View` on `Resource::WidgetPlugin` at `Scope::All` (see
/// [`require_widget_plugins_all`]'s own docs on why its own resource).
#[utoipa::path(
    get,
    path = "/api/widget-plugins",
    responses(
        (status = 200, body = WidgetPluginListResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_widget_plugins(
    Extension(ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
) -> Result<Json<WidgetPluginListResponse>, HandlerError> {
    require_widget_plugins_all(&ctx.user, Action::View)?;
    let packages = store.list()?.into_iter().map(Into::into).collect();
    Ok(Json(WidgetPluginListResponse { packages }))
}

/// `POST /api/widget-plugins`: installs a package from the raw bytes of a
/// zip archive (its `manifest.json` at the archive root, its assets under
/// `web/`). **Route this at `Authenticated`, with a body-size limit of
/// [`WIDGET_PLUGIN_PACKAGE_MAX_BYTES`]** in place of the router-wide JSON
/// default — see `indicator_handlers::upload_indicator_plugin`'s own
/// `mount()` call for the exact pattern to copy. The handler itself
/// requires `Action::Create` on `Resource::WidgetPlugin` at `Scope::All`:
/// installing code that will run on this server (even sandboxed) is an
/// admin action, for every plugin kind this server loads.
#[utoipa::path(
    post,
    path = "/api/widget-plugins",
    request_body(content = Vec<u8>, content_type = "application/zip"),
    responses(
        (status = 201, body = InstallWidgetPluginResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn install_widget_plugin(
    Extension(ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<InstallWidgetPluginResponse>), HandlerError> {
    require_widget_plugins_all(&ctx.user, Action::Create)?;
    let id = store.install(&body)?;
    Ok((
        StatusCode::CREATED,
        Json(InstallWidgetPluginResponse { id }),
    ))
}

/// `POST /api/widget-plugins/{id}/enabled`: flips whether this package's
/// widgets are in the effective catalog, without touching its files or
/// anything a dashboard has stored about a placed instance of one of its
/// widgets. **Route this at `Authenticated`** — the handler requires
/// `Action::Edit` on `Resource::WidgetPlugin` at `Scope::All`.
#[utoipa::path(
    post,
    path = "/api/widget-plugins/{id}/enabled",
    params(("id" = String, Path)),
    request_body = SetWidgetPluginEnabledRequest,
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn set_widget_plugin_enabled(
    Extension(ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
    Path(id): Path<String>,
    Json(body): Json<SetWidgetPluginEnabledRequest>,
) -> Result<StatusCode, HandlerError> {
    require_widget_plugins_all(&ctx.user, Action::Edit)?;
    store.set_enabled(&id, body.enabled)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/widget-plugins/{id}`: removes a package's files entirely.
/// **Route this at `Authenticated`** — the handler requires `Action::Delete`
/// on `Resource::WidgetPlugin` at `Scope::All`.
#[utoipa::path(
    delete,
    path = "/api/widget-plugins/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn uninstall_widget_plugin(
    Extension(ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
    Path(id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    require_widget_plugins_all(&ctx.user, Action::Delete)?;
    store.uninstall(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/widget-plugins/refresh`: an explicit rescan of the data
/// directory, for the direct-file-drop install path — refresh is
/// explicit, never a filesystem watcher, since a watcher can fire mid-copy
/// and read a half-written file. **Route this at `Authenticated`** — the
/// handler requires `Action::View` on `Resource::WidgetPlugin` at
/// `Scope::All`, same as the plain listing.
#[utoipa::path(
    post,
    path = "/api/widget-plugins/refresh",
    responses(
        (status = 200, body = WidgetPluginListResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn refresh_widget_plugins(
    Extension(ctx): Authed,
    Extension(store): Extension<Arc<WidgetPackageStore>>,
) -> Result<Json<WidgetPluginListResponse>, HandlerError> {
    require_widget_plugins_all(&ctx.user, Action::View)?;
    let packages = store.refresh()?.into_iter().map(Into::into).collect();
    Ok(Json(WidgetPluginListResponse { packages }))
}

/// `GET /widget-plugin-assets/{id}/{*path}`: streams one static file out of
/// package `id`'s own `web/` directory. **Route this at `Public`** — no
/// session is required (a sandboxed iframe without `allow-same-origin` has
/// no way to present a bearer token, and never should: these bytes are
/// exactly what an admin already chose to install, not user data), and it
/// should live **outside** `/api` so it can eventually move to a genuinely
/// separate origin.
///
/// **This is not yet served from an isolated origin** — that requires a
/// second bound listener (or a distinct hostname in front of this one),
/// which is server-bootstrap wiring (`ServeOptions`, the listener bind)
/// this module has no access to. The load-bearing isolation control is
/// still in place regardless: the platform's design record is explicit
/// that a sandboxed iframe with no `allow-same-origin` gets an **opaque**
/// origin no matter what origin served it from (see the design record's
/// own note on this exact point), so the sandboxing on the `<iframe>`
/// itself (built by the widget host component, not this handler) is what
/// actually isolates a widget from the parent page and from every other
/// widget. Serving from a second origin is real defense in depth on top of
/// that and is named as a follow-up in the implementation report.
///
/// Returns plain HTTP status codes rather than a `HandlerError`/`ErrorBody`
/// JSON envelope — see this module's own doc comment on why.
pub(crate) async fn widget_plugin_asset(
    Extension(store): Extension<Arc<WidgetPackageStore>>,
    Path((id, path)): Path<(String, String)>,
) -> Response {
    let resolved = match store.resolve_asset(&id, &path) {
        Ok(resolved) => resolved,
        Err(source) => {
            tracing::warn!(%source, %id, %path, "widget plugin asset: rejected path");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let Some(file_path) = resolved else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let bytes = match tokio::fs::read(&file_path).await {
        Ok(bytes) => bytes,
        Err(source) => {
            tracing::error!(%source, path = %file_path.display(), "widget plugin asset: read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let content_type = content_type_for(&path);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type);
    if content_type.starts_with("text/html") {
        response = response.header(header::CONTENT_SECURITY_POLICY, WIDGET_SANDBOX_CSP);
    }
    response
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// A conservative, fixed extension-to-MIME table for the handful of file
/// types a widget bundle plausibly ships — not a general-purpose sniffing
/// library, since every byte served here came from a zip archive an admin
/// already chose to install (the untrusted step was validating and
/// extracting it in `senken_plugin::widget_package::store`, not guessing
/// its `Content-Type` afterward).
fn content_type_for(path: &str) -> &'static str {
    let extension = FsPath::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::SocketAddr;

    use axum::Router;
    use axum::routing::{delete, get, post};
    use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};
    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::auth::AuthContext;

    const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";

    /// Parses a response body as JSON. This workspace's own convention
    /// (see `crate::test_support`'s module doc) is `reqwest` with no
    /// `json` feature enabled, so every test decodes bytes by hand instead
    /// of using `Response::json`.
    async fn body_json(response: reqwest::Response) -> serde_json::Value {
        let bytes = response.bytes().await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// `POST`/`PATCH`-style JSON body, matching the same convention.
    fn json_body(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    /// Builds the exact router `mount()` would end up producing for this
    /// module's routes, minus the real bearer-token-extraction middleware
    /// (`crate::auth::enforce_permission`, already exercised by that
    /// module's own tests) — `Extension<AuthContext>` is inserted directly
    /// per request instead, the same "resolved caller" shape the real
    /// middleware would have attached. This is what lets these tests run
    /// against real HTTP, real `AuthenticatedUser::authorize` calls and a
    /// real [`WidgetPackageStore`] without needing `AppState` (which this
    /// module's handlers deliberately do not depend on — see the module
    /// doc comment).
    fn test_router(store: Arc<WidgetPackageStore>, auth: AuthContext) -> Router {
        Router::new()
            .route("/api/widget-plugins/catalog", get(widget_plugin_catalog))
            .route(
                "/api/widget-plugins",
                get(list_widget_plugins).post(install_widget_plugin),
            )
            .route(
                "/api/widget-plugins/{id}/enabled",
                post(set_widget_plugin_enabled),
            )
            .route("/api/widget-plugins/{id}", delete(uninstall_widget_plugin))
            .route("/api/widget-plugins/refresh", post(refresh_widget_plugins))
            .route(
                "/widget-plugin-assets/{id}/{*path}",
                get(widget_plugin_asset),
            )
            .layer(Extension(store))
            .layer(Extension(auth))
    }

    async fn spawn(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (addr, handle)
    }

    fn temp_identity() -> (TempDir, IdentityStore) {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD, None)
            .unwrap();
        (dir, store)
    }

    fn admin_auth_context(identity: &IdentityStore) -> AuthContext {
        let (_uid, token) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let user = identity.resolve_session(token.reveal()).unwrap().unwrap();
        AuthContext {
            user,
            token: token.reveal().to_owned(),
        }
    }

    /// An ordinary account with no grants at all on `Resource::WidgetPlugin` —
    /// exactly the account `require_widget_plugins_all` must refuse.
    fn powerless_auth_context(identity: &IdentityStore, admin: &AuthContext) -> AuthContext {
        let user_id = identity
            .create_user(
                &admin.user,
                "powerless@example.com",
                "Powerless",
                Some("a very long password"),
            )
            .unwrap();
        let _ = user_id;
        let (_uid, token) = identity
            .login("powerless@example.com", "a very long password")
            .unwrap();
        let user = identity.resolve_session(token.reveal()).unwrap().unwrap();
        AuthContext {
            user,
            token: token.reveal().to_owned(),
        }
    }

    fn manifest_json(provider_id: &str) -> String {
        format!(
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
        )
    }

    fn valid_package_zip(provider_id: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("manifest.json", options).unwrap();
            writer
                .write_all(manifest_json(provider_id).as_bytes())
                .unwrap();
            writer.start_file("web/index.html", options).unwrap();
            writer
                .write_all(b"<!doctype html><title>clock</title>")
                .unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    fn temp_store() -> (TempDir, Arc<WidgetPackageStore>) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(WidgetPackageStore::open(dir.path()).unwrap());
        (dir, store)
    }

    #[tokio::test]
    async fn installing_a_package_makes_it_reachable_through_the_catalog_and_the_asset_server() {
        let (_dir, store) = temp_store();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), admin)).await;
        let client = reqwest::Client::new();

        let install = client
            .post(format!("http://{addr}/api/widget-plugins"))
            .header("content-type", "application/zip")
            .body(valid_package_zip("acme-widgets"))
            .send()
            .await
            .unwrap();
        assert_eq!(install.status(), reqwest::StatusCode::CREATED);
        let body = body_json(install).await;
        assert_eq!(body["id"], "acme-widgets");

        let catalog = body_json(
            client
                .get(format!("http://{addr}/api/widget-plugins/catalog"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        let widgets = catalog["widgets"].as_array().unwrap();
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0]["widget_type_id"], "acme-widgets/clock");
        let entry_url = widgets[0]["entry_url"].as_str().unwrap().to_owned();
        assert_eq!(entry_url, "/widget-plugin-assets/acme-widgets/index.html");

        let asset = client
            .get(format!("http://{addr}{entry_url}"))
            .send()
            .await
            .unwrap();
        assert_eq!(asset.status(), reqwest::StatusCode::OK);
        assert_eq!(
            asset.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
        assert!(asset.headers().contains_key("content-security-policy"));
        let text = asset.text().await.unwrap();
        assert!(text.contains("clock"));

        handle.abort();
    }

    #[tokio::test]
    async fn disabling_removes_the_widget_from_the_catalog_and_404s_its_asset_then_enabling_restores_both()
     {
        let (_dir, store) = temp_store();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), admin)).await;
        let client = reqwest::Client::new();

        client
            .post(format!("http://{addr}/api/widget-plugins"))
            .header("content-type", "application/zip")
            .body(valid_package_zip("acme-widgets"))
            .send()
            .await
            .unwrap();

        // Mutate first: prove the property actually catches the thing —
        // before disabling, both the catalog entry and the asset are
        // reachable.
        let before = body_json(
            client
                .get(format!("http://{addr}/api/widget-plugins/catalog"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(before["widgets"].as_array().unwrap().len(), 1);
        let before_asset = client
            .get(format!(
                "http://{addr}/widget-plugin-assets/acme-widgets/index.html"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(before_asset.status(), reqwest::StatusCode::OK);

        let disable = client
            .post(format!(
                "http://{addr}/api/widget-plugins/acme-widgets/enabled"
            ))
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "enabled": false })))
            .send()
            .await
            .unwrap();
        assert_eq!(disable.status(), reqwest::StatusCode::NO_CONTENT);

        let after = body_json(
            client
                .get(format!("http://{addr}/api/widget-plugins/catalog"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            after["widgets"].as_array().unwrap().is_empty(),
            "a disabled package's widget must be gone from the effective catalog"
        );
        let after_asset = client
            .get(format!(
                "http://{addr}/widget-plugin-assets/acme-widgets/index.html"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(
            after_asset.status(),
            reqwest::StatusCode::NOT_FOUND,
            "a disabled package's assets must not be served"
        );

        let enable = client
            .post(format!(
                "http://{addr}/api/widget-plugins/acme-widgets/enabled"
            ))
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "enabled": true })))
            .send()
            .await
            .unwrap();
        assert_eq!(enable.status(), reqwest::StatusCode::NO_CONTENT);

        let restored = body_json(
            client
                .get(format!("http://{addr}/api/widget-plugins/catalog"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            restored, before,
            "re-enabling must restore the exact same catalog entry, unchanged"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn an_account_with_no_storage_grant_is_forbidden_from_every_admin_action() {
        let (_dir, store) = temp_store();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let powerless = powerless_auth_context(&identity, &admin);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), powerless)).await;
        let client = reqwest::Client::new();

        let list = client
            .get(format!("http://{addr}/api/widget-plugins"))
            .send()
            .await
            .unwrap();
        assert_eq!(list.status(), reqwest::StatusCode::FORBIDDEN);

        let install = client
            .post(format!("http://{addr}/api/widget-plugins"))
            .header("content-type", "application/zip")
            .body(valid_package_zip("acme-widgets"))
            .send()
            .await
            .unwrap();
        assert_eq!(install.status(), reqwest::StatusCode::FORBIDDEN);

        handle.abort();
    }

    #[tokio::test]
    async fn a_package_declaring_an_unavailable_extension_point_is_rejected_with_a_named_reason() {
        let (_dir, store) = temp_store();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), admin)).await;
        let client = reqwest::Client::new();

        let manifest = r#"{
            "id": "acme-widgets",
            "name": "Acme",
            "version": "1.0.0",
            "contributes": [{ "point": "topbar.item" }]
        }"#;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("manifest.json", options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            writer.finish().unwrap();
        }

        let install = client
            .post(format!("http://{addr}/api/widget-plugins"))
            .header("content-type", "application/zip")
            .body(buf.into_inner())
            .send()
            .await
            .unwrap();
        assert_eq!(install.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = body_json(install).await;
        let message = body["error"].as_str().unwrap();
        assert!(
            message.contains("topbar.item"),
            "the rejection must name the specific point, got {message:?}"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn a_path_traversal_asset_request_is_rejected_not_served() {
        let (_dir, store) = temp_store();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), admin)).await;
        let client = reqwest::Client::new();

        client
            .post(format!("http://{addr}/api/widget-plugins"))
            .header("content-type", "application/zip")
            .body(valid_package_zip("acme-widgets"))
            .send()
            .await
            .unwrap();

        let response = client
            .get(format!(
                "http://{addr}/widget-plugin-assets/acme-widgets/..%2F..%2Fmanifest.json"
            ))
            .send()
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            reqwest::StatusCode::OK,
            "a traversal attempt must never be served"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn the_builtin_package_can_be_disabled_over_http_but_never_deleted() {
        let (_dir, store) = temp_store();
        store.ensure_builtin_installed().unwrap();
        let (_identity_dir, identity) = temp_identity();
        let admin = admin_auth_context(&identity);
        let (addr, handle) = spawn(test_router(Arc::clone(&store), admin)).await;
        let client = reqwest::Client::new();

        let listed = client
            .get(format!("http://{addr}/api/widget-plugins"))
            .send()
            .await
            .unwrap();
        let body = body_json(listed).await;
        let packages = body["packages"].as_array().unwrap();
        let builtin = packages
            .iter()
            .find(|p| p["id"] == senken_plugin::widget_package::BUILTIN_PACKAGE_ID)
            .expect("the built-in package must be listed");
        assert_eq!(builtin["is_builtin"], true);

        // Mutate first: disabling it must still work before proving delete
        // is refused.
        let disable = client
            .post(format!(
                "http://{addr}/api/widget-plugins/{}/enabled",
                senken_plugin::widget_package::BUILTIN_PACKAGE_ID
            ))
            .header("content-type", "application/json")
            .body(json_body(&serde_json::json!({ "enabled": false })))
            .send()
            .await
            .unwrap();
        assert_eq!(disable.status(), reqwest::StatusCode::NO_CONTENT);

        let delete = client
            .delete(format!(
                "http://{addr}/api/widget-plugins/{}",
                senken_plugin::widget_package::BUILTIN_PACKAGE_ID
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = body_json(delete).await;
        let message = body["error"].as_str().unwrap();
        assert!(message.contains("cannot be uninstalled"), "got {message:?}");

        handle.abort();
    }
}
