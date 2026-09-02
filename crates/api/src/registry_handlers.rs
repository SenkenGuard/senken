//! The indicator registry over HTTP: publish, search, install, revoke, and
//! claim a human-addressable handle for indicator-lang source.
//!
//! Mirrors `notes_handlers.rs` exactly for the guarded endpoints
//! (`publish`, `delete`, `list_mine`): each extracts `Extension(ctx): Authed`
//! and passes `&ctx.user` straight through to
//! `senken_indicator_registry::RegistryStore`, which performs its own
//! guarded check on every write and every scoped listing. `search`, `get`
//! and `install` take no `Authed` at all — a published indicator is public
//! and installable with no account, by design (see that crate's own module
//! docs). `set_my_handle`/`get_my_handle` take `Authed` but no grant check
//! beyond it: choosing your own address needs no permission, the same
//! reasoning `RegistryStore::set_handle`'s own docs give.
//!
//! `get_indicator`/`install_indicator`'s `{namespace}` path segment accepts
//! either form an installer might type: a raw account id (unchanged, so an
//! existing bookmark or script keeps working) or a claimed
//! `senken_indicator_registry::Handle`, resolved to the account it points
//! at by [`resolve_namespace_segment`]. This is what actually closes the
//! usability half of this crate's own "identity and naming" problem — a
//! `UserId` alone is safe to address by, but nobody types
//! `@550e8400-e29b-41d4-a716-446655440000/supertrend`.
//!
//! Every handler in this file is mounted in `lib.rs`'s
//! `mount_registry_routes` and listed in `openapi.rs`.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_identity::UserId;
use senken_indicator_registry::{Handle, IndicatorEntry, IndicatorSummary, Page, RegistryError};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::IdResponse;
use crate::pagination::{PaginationQuery, normalize_pagination};

/// This crate's own translation from `RegistryError`, colocated here
/// rather than added to `error.rs` as every other domain's mapping is —
/// see this file's module docs for why: `error.rs` sits outside this
/// file's ownership boundary for this change. Moving this into a
/// `From<RegistryError> for HandlerError` impl in `error.rs`, matching
/// every other guarded store's own conversion there, is a natural,
/// low-risk follow-up once this file is wired in.
fn map_registry_error(error: RegistryError) -> HandlerError {
    match error {
        RegistryError::Identity(source) => source.into(),
        RegistryError::ForeignNamespace => {
            HandlerError::Forbidden("you may only publish into your own namespace".to_owned())
        }
        RegistryError::NotFound => HandlerError::BadRequest("no such indicator".to_owned()),
        RegistryError::InvalidName(name) => {
            HandlerError::BadRequest(format!("`{name}` is not a valid indicator name"))
        }
        RegistryError::InvalidSource(source) => {
            HandlerError::BadRequest(format!("indicator source does not compile: {source}"))
        }
        RegistryError::LanguageVersionTooNew { required, host } => {
            HandlerError::BadRequest(format!(
                "this indicator needs language version {required}, but this host only understands up to {host}"
            ))
        }
        RegistryError::InvalidHandle(handle) => {
            HandlerError::BadRequest(format!("`{handle}` is not a valid registry handle"))
        }
        RegistryError::HandleTaken(handle) => {
            HandlerError::Conflict(format!("handle `{handle}` is already taken"))
        }
        RegistryError::HandleNotFound(handle) => {
            HandlerError::BadRequest(format!("no account has claimed the handle `{handle}`"))
        }
        RegistryError::HandleNotSet => {
            HandlerError::Forbidden("choose a registry handle before publishing".to_owned())
        }
        RegistryError::Database(source) => {
            tracing::error!(%source, "indicator registry: database error");
            HandlerError::Internal
        }
        // `RegistryError` is `#[non_exhaustive]`: a future variant must
        // fail closed rather than being silently accepted as a 500 with no
        // record of what happened.
        other => {
            tracing::error!(?other, "indicator registry: unmapped error variant");
            HandlerError::Internal
        }
    }
}

/// Resolves an HTTP path segment addressing a registry namespace: either a
/// raw [`UserId`], tried first so an existing bookmark or script naming
/// one keeps working unmodified, or — for anything that fails to parse as
/// one — a claimed [`Handle`], resolved to the account it points at. This
/// is the human-facing half of installing: nobody types
/// `@550e8400-e29b-41d4-a716-446655440000/supertrend`, and this lets them
/// type `@alice/supertrend` instead (the `@` is a caller convention, not
/// required by this parser — it is stripped if present).
fn resolve_namespace_segment(state: &AppState, raw: &str) -> Result<UserId, HandlerError> {
    let raw = raw.strip_prefix('@').unwrap_or(raw);
    if let Ok(user_id) = raw.parse::<UserId>() {
        return Ok(user_id);
    }
    let handle = Handle::new(raw).map_err(|_| {
        HandlerError::BadRequest("not a valid registry namespace or handle".to_owned())
    })?;
    state
        .registry
        .resolve_handle(&handle)
        .map_err(map_registry_error)
}

/// A published indicator without its source, as returned by a listing —
/// see [`IndicatorEntryDto`] for the full row.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorSummaryDto {
    /// This entry's id.
    pub id: String,
    /// The publishing account's id — this indicator's namespace. The
    /// qualified name is `{namespace}/{name}`.
    pub namespace: String,
    /// The indicator's name within its namespace.
    pub name: String,
    /// The indicator language version this entry was last published
    /// against.
    pub language_version: String,
    /// Unix timestamp this entry was first published.
    pub created_at: i64,
    /// Unix timestamp of the last successful publish to this entry.
    pub updated_at: i64,
}

impl From<IndicatorSummary> for IndicatorSummaryDto {
    fn from(summary: IndicatorSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            namespace: summary.namespace.to_string(),
            name: summary.name,
            language_version: summary.language_version,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/registry/indicators` and `GET /api/registry/indicators/mine`
/// response body. Scope reaches the query, including this `total` — see
/// `senken_indicator_registry::RegistryStore::list_mine`'s own docs.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RegistryPage {
    /// The rows for this page.
    pub rows: Vec<IndicatorSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

impl From<Page<IndicatorSummary>> for RegistryPage {
    fn from(page: Page<IndicatorSummary>) -> Self {
        Self {
            rows: page
                .rows
                .into_iter()
                .map(IndicatorSummaryDto::from)
                .collect(),
            total: page.total,
        }
    }
}

/// A full published indicator, source included — `GET
/// /api/registry/indicators/{namespace}/{name}` only.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorEntryDto {
    /// This entry's id.
    pub id: String,
    /// See [`IndicatorSummaryDto::namespace`].
    pub namespace: String,
    /// See [`IndicatorSummaryDto::name`].
    pub name: String,
    /// The indicator-lang source exactly as published.
    pub source: String,
    /// See [`IndicatorSummaryDto::language_version`].
    pub language_version: String,
    /// Unix timestamp this entry was first published.
    pub created_at: i64,
    /// Unix timestamp of the last successful publish to this entry.
    pub updated_at: i64,
}

impl From<IndicatorEntry> for IndicatorEntryDto {
    fn from(entry: IndicatorEntry) -> Self {
        Self {
            id: entry.id.to_string(),
            namespace: entry.namespace.to_string(),
            name: entry.name,
            source: entry.source,
            language_version: entry.language_version,
            created_at: entry.created_at,
            updated_at: entry.updated_at,
        }
    }
}

/// `POST /api/registry/indicators` request body. Carries no `namespace`
/// field on purpose: a publish always targets the caller's own account —
/// see `senken_indicator_registry`'s own module docs for why a namespace
/// is an account id rather than a client-chosen string in the first
/// place, which is exactly what makes deriving it from the session, never
/// accepting it as input, both correct and safe here.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PublishIndicatorRequest {
    /// The indicator's name within the caller's own namespace.
    pub name: String,
    /// The indicator-lang source to publish.
    pub source: String,
}

/// `?query=&limit=&offset=` for [`search_indicators`].
#[derive(Debug, Deserialize)]
pub(crate) struct SearchQuery {
    #[serde(default)]
    pub(crate) query: Option<String>,
    #[serde(default)]
    pub(crate) limit: Option<u32>,
    #[serde(default)]
    pub(crate) offset: Option<u32>,
}

/// `POST /api/registry/indicators`. Requires a session — publishing needs
/// an account; installing does not (see this crate's module docs).
#[utoipa::path(
    post,
    path = "/api/registry/indicators",
    request_body = PublishIndicatorRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn publish_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<PublishIndicatorRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state
        .registry
        .publish(&ctx.user, ctx.user.user_id(), &body.name, &body.source)
        .map_err(map_registry_error)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/registry/indicators`. The public catalog: no session
/// required, every published indicator across every namespace is
/// searchable by anyone.
#[utoipa::path(
    get,
    path = "/api/registry/indicators",
    params(
        ("query" = Option<String>, Query, description = "matches indicator names containing this text"),
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = RegistryPage),
    )
)]
pub(crate) async fn search_indicators(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<RegistryPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(PaginationQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let page = state
        .registry
        .search(query.query.as_deref(), limit, offset)
        .map_err(map_registry_error)?;
    Ok(Json(page.into()))
}

/// `GET /api/registry/indicators/mine`. Scoped by
/// `RegistryStore::list_mine` itself — an ordinary author sees only what
/// they have published, an actor granted wider access sees every
/// namespace's, and the reported `total` already respects that scope too.
#[utoipa::path(
    get,
    path = "/api/registry/indicators/mine",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = RegistryPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_my_indicators(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<RegistryPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state
        .registry
        .list_mine(&ctx.user, limit, offset)
        .map_err(map_registry_error)?;
    Ok(Json(page.into()))
}

/// `GET /api/registry/indicators/{namespace}/{name}`: the full published
/// indicator, source included. Public, like [`search_indicators`].
#[utoipa::path(
    get,
    path = "/api/registry/indicators/{namespace}/{name}",
    params(
        ("namespace" = String, Path, description = "the publishing account's id, or its claimed handle"),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, body = IndicatorEntryDto),
        (status = 400, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_indicator(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<Json<IndicatorEntryDto>, HandlerError> {
    let namespace = resolve_namespace_segment(&state, &namespace)?;
    let entry = state
        .registry
        .get(namespace, &name)
        .map_err(map_registry_error)?;
    Ok(Json(entry.into()))
}

/// `POST /api/registry/indicators/{namespace}/{name}/install`: fetches the
/// current published source, checks its recorded language version against
/// this host's own, and — only once that check passes — compiles it with
/// `senken_indicator_lang::compile`, right here, on this host. Public: no
/// account is required to install (see this crate's module docs).
///
/// The response body is the compiled `compiled-indicator` WebAssembly
/// component's raw bytes (`Content-Type: application/wasm`), not JSON —
/// this is the artifact "compiled on the installing machine" actually
/// means, not a description of one. The language version it was compiled
/// against is echoed in the `X-Indicator-Language-Version` header for a
/// caller that wants it without a second round trip to
/// [`get_indicator`].
#[utoipa::path(
    post,
    path = "/api/registry/indicators/{namespace}/{name}/install",
    params(
        ("namespace" = String, Path, description = "the publishing account's id, or its claimed handle"),
        ("name" = String, Path),
    ),
    responses(
        (status = 200, description = "the compiled WebAssembly component, `application/wasm`"),
        (status = 400, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn install_indicator(
    State(state): State<AppState>,
    Path((namespace, name)): Path<(String, String)>,
) -> Result<impl IntoResponse, HandlerError> {
    let namespace = resolve_namespace_segment(&state, &namespace)?;
    let installed = state
        .registry
        .install(namespace, &name)
        .map_err(map_registry_error)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/wasm".to_owned()),
            (
                header::HeaderName::from_static("x-indicator-language-version"),
                installed.language_version,
            ),
        ],
        Bytes::from(installed.component),
    ))
}

/// `DELETE /api/registry/indicators/{name}`: revokes the caller's own
/// published entry. Carries no request body and no `namespace` path
/// segment — a delete always targets the caller's own namespace, the same
/// server-derives-identity shape [`PublishIndicatorRequest`] already
/// establishes for publishing, so this can never reach another author's
/// entry regardless of what the caller's grants say (see
/// `senken_indicator_registry::RegistryStore::delete`'s own docs). An
/// indicator someone else has already installed is unaffected: installing
/// copies the compiled bytes to that machine, so nothing here reaches
/// back into a copy that already left this registry.
#[utoipa::path(
    delete,
    path = "/api/registry/indicators/{name}",
    params(
        ("name" = String, Path),
    ),
    responses(
        (status = 204, description = "indicator revoked"),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(name): Path<String>,
) -> Result<StatusCode, HandlerError> {
    state
        .registry
        .delete(&ctx.user, ctx.user.user_id(), &name)
        .map_err(map_registry_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/registry/handle` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetHandleRequest {
    /// The handle to claim: lowercase ASCII letters, digits and hyphens
    /// only, 3-32 characters, starting and ending with a letter or digit
    /// (see `senken_indicator_registry::Handle`).
    pub handle: String,
}

/// `GET /api/registry/handle` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct HandleResponse {
    /// The caller's own claimed registry handle, or `null` if they have
    /// not chosen one yet.
    pub handle: Option<String>,
}

/// `PUT /api/registry/handle`: claims, or replaces, the caller's own
/// registry handle — the human-readable address other users type instead
/// of the caller's raw account id (`alice` in `alice/supertrend` rather
/// than the account's own id). Requires a session; the target account is
/// always the caller's own, never one named in the request body.
/// [`publish_indicator`] refuses to run until this has succeeded at least
/// once — see `senken_indicator_registry`'s own module docs for why.
#[utoipa::path(
    put,
    path = "/api/registry/handle",
    request_body = SetHandleRequest,
    responses(
        (status = 204, description = "handle claimed"),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody, description = "another account already holds this handle"),
    )
)]
pub(crate) async fn set_my_handle(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<SetHandleRequest>,
) -> Result<StatusCode, HandlerError> {
    let handle = Handle::new(&body.handle).map_err(map_registry_error)?;
    state
        .registry
        .set_handle(ctx.user.user_id(), &handle)
        .map_err(map_registry_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/registry/handle`: reports the caller's own claimed registry
/// handle, or `null` if they have not chosen one yet.
#[utoipa::path(
    get,
    path = "/api/registry/handle",
    responses(
        (status = 200, body = HandleResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_my_handle(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<HandleResponse>, HandlerError> {
    let handle = state
        .registry
        .get_handle(ctx.user.user_id())
        .map_err(map_registry_error)?;
    Ok(Json(HandleResponse {
        handle: handle.map(|handle| handle.to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_indicator_registry::{Handle, RegistryStore};

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, delete_auth, delete_no_auth, get_auth, post_json,
        post_json_auth, put_json_auth, serve_unfenced_test_server,
    };

    const VALID_SOURCE: &str = "let fast = ema(close, 5)\nplot fast\n";

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

    /// Creates an ordinary author account, grants it the full
    /// `IndicatorRegistry` grant set, and claims a registry handle
    /// (derived from `email`'s local part) for it directly through
    /// `RegistryStore` rather than a second HTTP round trip through `PUT
    /// /api/registry/handle` (exercised on its own by
    /// [`claiming_and_reading_back_a_handle_over_http_round_trips`]). It
    /// shares `identity`'s own connection the same way the live server's
    /// `RegistryStore` does, so a handle claimed here is exactly as real as
    /// one claimed through that endpoint.
    async fn author_token(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> (String, senken_identity::UserId) {
        let user_id = identity
            .create_user(
                admin,
                email,
                "Indicator Author",
                Some("a very long password"),
            )
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::IndicatorRegistry, Scope::Own),
                )
                .unwrap();
        }
        let local_part = email.split('@').next().unwrap();
        RegistryStore::new(identity)
            .set_handle(user_id, &Handle::new(local_part).unwrap())
            .unwrap();
        let token = login_token(addr, email, "a very long password").await;
        (token, user_id)
    }

    #[tokio::test]
    async fn publishing_then_installing_over_http_returns_a_real_wasm_component() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let (alice_token, alice_id) =
            author_token(addr, &identity, &admin, "alice@example.com").await;

        let publish = post_json_auth(
            format!("http://{addr}/api/registry/indicators"),
            &alice_token,
            serde_json::json!({ "name": "rsi-cross", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(publish.status(), reqwest::StatusCode::CREATED);

        let search = body_json(
            get_auth(
                format!("http://{addr}/api/registry/indicators?query=rsi"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(search["total"], 1);

        let install = reqwest::Client::new()
            .post(format!(
                "http://{addr}/api/registry/indicators/{alice_id}/rsi-cross/install"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(install.status(), reqwest::StatusCode::OK);
        assert_eq!(
            install.headers().get("content-type").unwrap(),
            "application/wasm"
        );
        let bytes = install.bytes().await.unwrap();
        assert_eq!(&bytes[0..4], b"\0asm");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_client_supplied_namespace_field_in_the_request_body_is_ignored() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let (alice_token, alice_id) =
            author_token(addr, &identity, &admin, "alice2@example.com").await;
        let (_bob_token, bob_id) = author_token(addr, &identity, &admin, "bob2@example.com").await;

        // `PublishIndicatorRequest` has no `namespace` field at all (see its
        // own docs) -- an extra one in the request body is simply unknown
        // data to `Json`'s deserializer, not something that can steer which
        // account the entry lands under. This is the HTTP-layer half of the
        // impersonation defence; `senken_indicator_registry`'s own test
        // suite proves the store-level guard beneath it by removing it and
        // watching the equivalent test fail.
        let publish = post_json_auth(
            format!("http://{addr}/api/registry/indicators"),
            &alice_token,
            serde_json::json!({ "name": "whatever", "source": VALID_SOURCE, "namespace": bob_id.to_string() }),
        )
        .await;
        assert_eq!(publish.status(), reqwest::StatusCode::CREATED);

        let entry = body_json(
            get_auth(
                format!("http://{addr}/api/registry/indicators/{alice_id}/whatever"),
                &alice_token,
            )
            .await,
        )
        .await;
        assert_eq!(
            entry["namespace"],
            alice_id.to_string(),
            "the injected `namespace` field must not have moved this entry into bob's namespace"
        );

        let bobs_lookup = get_auth(
            format!("http://{addr}/api/registry/indicators/{bob_id}/whatever"),
            &alice_token,
        )
        .await;
        assert_eq!(bobs_lookup.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_request_with_no_session_cannot_publish() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let response = post_json(
            format!("http://{addr}/api/registry/indicators"),
            serde_json::json!({ "name": "nope", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn installing_by_a_claimed_handle_reaches_the_same_entry_as_the_raw_account_id() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let (alice_token, alice_id) =
            author_token(addr, &identity, &admin, "handle-alice@example.com").await;

        let publish = post_json_auth(
            format!("http://{addr}/api/registry/indicators"),
            &alice_token,
            serde_json::json!({ "name": "macd-signal", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(publish.status(), reqwest::StatusCode::CREATED);

        // Nobody types the raw account id -- this is the actual point of
        // adding a handle. `author_token` claimed `handle-alice` for this
        // account, so both the bare handle and an `@`-prefixed form (the
        // human convention this crate's own docs describe) must resolve
        // to exactly the entry the raw account id reaches.
        for namespace_segment in ["handle-alice", "@handle-alice"] {
            let get_by_handle = body_json(
                get_auth(
                    format!(
                        "http://{addr}/api/registry/indicators/{namespace_segment}/macd-signal"
                    ),
                    &alice_token,
                )
                .await,
            )
            .await;
            assert_eq!(get_by_handle["namespace"], alice_id.to_string());

            let install_by_handle = reqwest::Client::new()
                .post(format!(
                    "http://{addr}/api/registry/indicators/{namespace_segment}/macd-signal/install"
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(install_by_handle.status(), reqwest::StatusCode::OK);
            let bytes = install_by_handle.bytes().await.unwrap();
            assert_eq!(&bytes[0..4], b"\0asm");
        }

        // An unclaimed handle is a `400`, not a `500` -- the same
        // "no such indicator" shape a bad raw id already gets.
        let unclaimed = get_auth(
            format!("http://{addr}/api/registry/indicators/nobody-claimed-this/macd-signal"),
            &alice_token,
        )
        .await;
        assert_eq!(unclaimed.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn claiming_and_reading_back_a_handle_over_http_round_trips() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
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
                "carol@example.com",
                "Carol",
                Some("a very long password"),
            )
            .unwrap();
        let _ = user_id;
        let token = login_token(addr, "carol@example.com", "a very long password").await;

        // No handle claimed yet.
        let before =
            body_json(get_auth(format!("http://{addr}/api/registry/handle"), &token).await).await;
        assert_eq!(before["handle"], serde_json::Value::Null);

        let claim = put_json_auth(
            format!("http://{addr}/api/registry/handle"),
            &token,
            serde_json::json!({ "handle": "carol" }),
        )
        .await;
        assert_eq!(claim.status(), reqwest::StatusCode::NO_CONTENT);

        let after =
            body_json(get_auth(format!("http://{addr}/api/registry/handle"), &token).await).await;
        assert_eq!(after["handle"], "carol");

        // Another account cannot claim the same handle.
        let dave_id = identity
            .create_user(
                &admin,
                "dave@example.com",
                "Dave",
                Some("a very long password"),
            )
            .unwrap();
        let _ = dave_id;
        let dave_token = login_token(addr, "dave@example.com", "a very long password").await;
        let conflict = put_json_auth(
            format!("http://{addr}/api/registry/handle"),
            &dave_token,
            serde_json::json!({ "handle": "carol" }),
        )
        .await;
        assert_eq!(conflict.status(), reqwest::StatusCode::CONFLICT);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn deleting_an_indicator_over_http_removes_it_and_a_missing_session_is_401() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let (alice_token, alice_id) =
            author_token(addr, &identity, &admin, "delete-alice@example.com").await;

        let publish = post_json_auth(
            format!("http://{addr}/api/registry/indicators"),
            &alice_token,
            serde_json::json!({ "name": "to-delete", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(publish.status(), reqwest::StatusCode::CREATED);

        // No session at all: `401`, never a `500` and never a silent no-op.
        let unauthenticated =
            delete_no_auth(format!("http://{addr}/api/registry/indicators/to-delete")).await;
        assert_eq!(unauthenticated.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Still installable before the delete.
        let still_there = get_auth(
            format!("http://{addr}/api/registry/indicators/{alice_id}/to-delete"),
            &alice_token,
        )
        .await;
        assert_eq!(still_there.status(), reqwest::StatusCode::OK);

        let delete = delete_auth(
            format!("http://{addr}/api/registry/indicators/to-delete"),
            &alice_token,
        )
        .await;
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let gone = get_auth(
            format!("http://{addr}/api/registry/indicators/{alice_id}/to-delete"),
            &alice_token,
        )
        .await;
        assert_eq!(
            gone.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "a revoked entry must read back as \"no such indicator\", not still be installable"
        );

        handle.shutdown().await.unwrap();
    }
}
