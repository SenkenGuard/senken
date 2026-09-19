//! Indicators an account wrote and compiled themselves, over HTTP.
//!
//! Every handler extracts `Extension(ctx): Authed` and passes `&ctx.user`
//! straight through to `senken_indicator_registry::UserIndicatorStore`,
//! which performs its own guarded check on every read and write — the
//! same shape `notes_handlers` uses for `senken_notes::NoteStore`.
//!
//! Saving (`create_my_indicator`/`update_my_indicator`) and explicitly
//! recompiling (`recompile_my_indicator`) all funnel through
//! [`compile_and_record`]: store the source first, then attempt to
//! compile it, then record whichever outcome that attempt actually had.
//! A compile failure is still a `200` — the source was accepted and
//! stored either way, and a mistake in a trader's own Rust is not a
//! server error.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_identity::AuthenticatedUser;
use senken_indicator_compile::{CompileError, CompileRequest};
use senken_indicator_registry::{CompileOutcome, UserIndicatorId};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    CreateUserIndicatorRequest, IndicatorToolchainStatusResponse, SaveUserIndicatorResponse,
    UpdateUserIndicatorRequest, UserIndicatorDiagnosticDto, UserIndicatorDto,
    UserIndicatorSummaryDto,
};

/// A compile is given up to two minutes — the same budget `TradingView`'s
/// own compile-time allowance cites — before this crate kills the build
/// and reports a timeout rather than waiting indefinitely on a runaway
/// `cargo build`.
const COMPILE_TIMEOUT: Duration = Duration::from_mins(2);

/// Parses an HTTP path segment as a [`UserIndicatorId`], failing with
/// `400` (not `500`) for a malformed one.
fn parse_user_indicator_id(raw: &str) -> Result<UserIndicatorId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid indicator id".to_owned()))
}

/// The catalog name a user indicator's compiled component is registered
/// under — `my/<slug>`. Defined once, here, since both the compile path
/// (registering it) and the future compute path (spawning it) must agree
/// on exactly the same string.
fn catalog_name(slug: &str) -> String {
    format!("my/{slug}")
}

/// `GET /api/my/indicators`.
#[utoipa::path(
    get,
    path = "/api/my/indicators",
    responses(
        (status = 200, body = Vec<UserIndicatorSummaryDto>),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_my_indicators(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<Vec<UserIndicatorSummaryDto>>, HandlerError> {
    let rows = state.user_indicators.list(&ctx.user)?;
    Ok(Json(
        rows.into_iter()
            .map(UserIndicatorSummaryDto::from)
            .collect(),
    ))
}

/// `GET /api/my/indicators/{id}`: the full row, source included.
#[utoipa::path(
    get,
    path = "/api/my/indicators/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = UserIndicatorDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_my_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(id): Path<String>,
) -> Result<Json<UserIndicatorDto>, HandlerError> {
    let id = parse_user_indicator_id(&id)?;
    let indicator = state.user_indicators.get(&ctx.user, id)?;
    Ok(Json(indicator.into()))
}

/// `POST /api/my/indicators`: creates a new indicator, then compiles it
/// immediately.
#[utoipa::path(
    post,
    path = "/api/my/indicators",
    request_body = CreateUserIndicatorRequest,
    responses(
        (status = 200, body = SaveUserIndicatorResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody),
        (status = 503, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_my_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateUserIndicatorRequest>,
) -> Result<Json<SaveUserIndicatorResponse>, HandlerError> {
    let id = state
        .user_indicators
        .create(&ctx.user, &body.title, &body.source)?;
    let response = compile_and_record(&state, &ctx.user, id).await?;
    Ok(Json(response))
}

/// `PUT /api/my/indicators/{id}`: saves a new title/source, then compiles.
#[utoipa::path(
    put,
    path = "/api/my/indicators/{id}",
    request_body = UpdateUserIndicatorRequest,
    params(("id" = String, Path)),
    responses(
        (status = 200, body = SaveUserIndicatorResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 409, body = crate::dto::ErrorBody),
        (status = 503, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn update_my_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserIndicatorRequest>,
) -> Result<Json<SaveUserIndicatorResponse>, HandlerError> {
    let id = parse_user_indicator_id(&id)?;
    state
        .user_indicators
        .update_source(&ctx.user, id, body.title.as_deref(), &body.source)?;
    let response = compile_and_record(&state, &ctx.user, id).await?;
    Ok(Json(response))
}

/// `POST /api/my/indicators/{id}/compile`: recompiles the source already
/// on file, without changing it.
#[utoipa::path(
    post,
    path = "/api/my/indicators/{id}/compile",
    params(("id" = String, Path)),
    responses(
        (status = 200, body = SaveUserIndicatorResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
        (status = 503, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn recompile_my_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(id): Path<String>,
) -> Result<Json<SaveUserIndicatorResponse>, HandlerError> {
    let id = parse_user_indicator_id(&id)?;
    let response = compile_and_record(&state, &ctx.user, id).await?;
    Ok(Json(response))
}

/// `DELETE /api/my/indicators/{id}`: removes the row and unloads it from
/// this account's own runtime catalog.
#[utoipa::path(
    delete,
    path = "/api/my/indicators/{id}",
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_my_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let id = parse_user_indicator_id(&id)?;
    // Read the row first so the catalog name to unload (`my/<slug>`) is
    // known even after the row itself is gone — `delete` below removes
    // it, and there is no "slug of an id that no longer exists" query
    // left to make afterward.
    let indicator = state.user_indicators.get(&ctx.user, id)?;
    state.user_indicators.delete(&ctx.user, id)?;
    state
        .runtime
        .user_indicators()
        .unload(ctx.user.user_id(), &catalog_name(&indicator.slug));
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/my/indicators/toolchain`: whether this server can compile a
/// Rust indicator right now, so the authoring panel can disable Save with
/// a reason instead of letting a save fail with no explanation.
#[utoipa::path(
    get,
    path = "/api/my/indicators/toolchain",
    responses(
        (status = 200, body = IndicatorToolchainStatusResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn my_indicator_toolchain(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
) -> Json<IndicatorToolchainStatusResponse> {
    Json(match state.compile_service.as_ref() {
        crate::CompileServiceHandle::Available(_) => IndicatorToolchainStatusResponse {
            available: true,
            reason: None,
        },
        crate::CompileServiceHandle::Unavailable(reason) => IndicatorToolchainStatusResponse {
            available: false,
            reason: Some(reason.clone()),
        },
    })
}

/// Compiles `id`'s current source and records the outcome: on success,
/// replaces the stored component and (re)registers it into `auth`'s own
/// runtime catalog under `my/<slug>`; on failure, records the diagnostics
/// and leaves whatever was registered before untouched — a typo must
/// never take a working indicator off a chart that already uses it.
///
/// # Errors
/// [`HandlerError::ServiceUnavailable`] if this server has no compile
/// toolchain; otherwise whatever [`senken_indicator_registry::UserIndicatorError`]
/// the store reports for `id`.
async fn compile_and_record(
    state: &AppState,
    auth: &AuthenticatedUser,
    id: UserIndicatorId,
) -> Result<SaveUserIndicatorResponse, HandlerError> {
    let service = match state.compile_service.as_ref() {
        crate::CompileServiceHandle::Available(service) => service,
        crate::CompileServiceHandle::Unavailable(reason) => {
            return Err(HandlerError::ServiceUnavailable(format!(
                "this server cannot compile indicators right now: {reason}"
            )));
        }
    };
    let indicator = state.user_indicators.get(auth, id)?;
    let crate_name = format!("user-indicator-{id}");
    let outcome = service
        .compile(
            CompileRequest {
                source: &indicator.source,
                crate_name: &crate_name,
            },
            COMPILE_TIMEOUT,
        )
        .await;

    match outcome {
        Ok(wasm) => {
            state.user_indicators.record_compile(
                auth,
                id,
                CompileOutcome::Success {
                    wasm: wasm.clone(),
                    api_version: senken_plugin_host::SUPPORTED_API_VERSION.to_owned(),
                },
            )?;
            let name = catalog_name(&indicator.slug);
            if let Err(source) = state
                .runtime
                .user_indicators()
                .load(auth.user_id(), &name, &wasm)
            {
                // The source compiled — this is a bug in the bridge (a
                // component `senken-indicator-compile` produced that
                // `senken-plugin-host` then refuses), not a mistake in the
                // trader's own Rust, so it is logged in full server-side
                // rather than shown as a compile diagnostic.
                tracing::error!(%source, indicator = %name, "a compiled user indicator failed to register");
            }
            Ok(SaveUserIndicatorResponse {
                id: id.to_string(),
                compiled: true,
                diagnostics: None,
            })
        }
        Err(CompileError::Failed {
            diagnostics,
            log_tail,
        }) => {
            tracing::warn!(indicator = %id, log_tail, "user indicator compile failed");
            let message = diagnostics
                .first()
                .map_or_else(|| "compile failed".to_owned(), |d| d.message.clone());
            let dtos = diagnostics
                .into_iter()
                .map(UserIndicatorDiagnosticDto::from)
                .collect();
            record_failure(state, auth, id, message, dtos)
        }
        Err(CompileError::Rejected(reason)) => record_failure(
            state,
            auth,
            id,
            reason.clone(),
            vec![no_span_diagnostic(reason)],
        ),
        Err(CompileError::Timeout(duration)) => {
            let message = format!("compile did not finish within {duration:?}");
            record_failure(
                state,
                auth,
                id,
                message.clone(),
                vec![no_span_diagnostic(message)],
            )
        }
        Err(CompileError::TooLarge) => {
            let message = "the compiled component is too large".to_owned();
            record_failure(
                state,
                auth,
                id,
                message.clone(),
                vec![no_span_diagnostic(message)],
            )
        }
        Err(other @ (CompileError::Toolchain(_) | CompileError::Io(_))) => {
            tracing::error!(error = %other, indicator = %id, "user indicator compile could not even run");
            Err(HandlerError::Internal)
        }
    }
}

/// A diagnostic with no line/column — for a compile that never reached
/// `rustc` at all (rejected before running, timed out, or produced an
/// oversized component).
fn no_span_diagnostic(message: String) -> UserIndicatorDiagnosticDto {
    UserIndicatorDiagnosticDto {
        line: None,
        column: None,
        message,
    }
}

/// Records a failed compile attempt and builds the response body for it —
/// the shape every failing arm of [`compile_and_record`] needs, so each one
/// differs only in the message and diagnostics it produces.
fn record_failure(
    state: &AppState,
    auth: &AuthenticatedUser,
    id: UserIndicatorId,
    message: String,
    diagnostics: Vec<UserIndicatorDiagnosticDto>,
) -> Result<SaveUserIndicatorResponse, HandlerError> {
    state
        .user_indicators
        .record_compile(auth, id, CompileOutcome::Failure { message })?;
    Ok(SaveUserIndicatorResponse {
        id: id.to_string(),
        compiled: false,
        diagnostics: Some(diagnostics),
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};
    use senken_indicator_compile::{CompileService, Toolchain};

    use crate::CompileServiceHandle;
    use crate::bars_handlers::test_support::{runtime_with_fake_venue, test_instrument};
    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, delete_auth, delete_no_auth, get_auth, post_json,
        post_json_auth, put_json_auth, serve_unfenced_test_server_with_compile_service,
        temp_empty_runtime,
    };

    /// Every real-`cargo` compile in this module is serialized behind one
    /// lock — this machine must never run two Rust builds at once, the
    /// same rule `senken-indicator-compile`'s own tests observe.
    static BUILD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn plugin_api_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin-api")
    }

    /// This repository's shared `target/fixture-wasm` — reused so a real
    /// compile here pays for a warm dependency cache instead of every test
    /// separately compiling `wit-bindgen` and friends from cold.
    fn shared_target_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture-wasm")
    }

    /// A copy of `crates/plugin-host/tests/fixtures/deterministic/src/lib.rs`
    /// — a real, already-proven-correct indicator, used the same way
    /// `senken-indicator-compile`'s own tests reuse it, rather than
    /// inventing a new source to trust.
    const VALID_SOURCE: &str = r#"
use std::cell::Cell;

use senken_plugin_api::{
    Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue, PlotValue,
};

struct Deterministic;

impl Guest for Deterministic {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        IndicatorDescriptor {
            id: "deterministic".into(),
            title: "Deterministic".into(),
            short_title: "DET".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance {
    bars_seen: Cell<u32>,
}

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance {
            bars_seen: Cell::new(0),
        }
    }

    fn handle_bar(&self, bar: Bar) -> OnBarResult {
        self.bars_seen.set(self.bars_seen.get() + 1);
        let mut acc: u64 = 0;
        let iterations = 1000 + (bar.close.value.unsigned_abs() % 1000);
        for i in 0..iterations {
            acc = acc.wrapping_add(i ^ bar.close.value.unsigned_abs());
        }
        OnBarResult {
            plots: vec![PlotValue {
                field: "acc".into(),
                value: acc as f64,
            }],
            drawables: vec![],
        }
    }

    fn initialized(&self) -> bool {
        self.bars_seen.get() > 0
    }

    fn reset(&self) {
        self.bars_seen.set(0);
    }
}

senken_plugin_api::export!(Deterministic);
"#;

    /// Same shape as [`VALID_SOURCE`], with one deliberate type error on a
    /// known line: assigning a string literal to a `u32` binding.
    const TYPE_ERROR_SOURCE: &str = r#"
use senken_plugin_api::{Guest, GuestInstance, IndicatorDescriptor, OnBarResult, Bar, ParamValue};

struct Broken;

impl Guest for Broken {
    type Instance = Instance;

    fn descriptor() -> IndicatorDescriptor {
        let x: u32 = "not a number";
        IndicatorDescriptor {
            id: "broken".into(),
            title: "Broken".into(),
            short_title: "BRK".into(),
            legend: String::new(),
            params: vec![],
            plots: vec![],
        }
    }
}

struct Instance;

impl GuestInstance for Instance {
    fn new(_params: Vec<ParamValue>) -> Self {
        Instance
    }

    fn handle_bar(&self, _bar: Bar) -> OnBarResult {
        OnBarResult { plots: vec![], drawables: vec![] }
    }

    fn initialized(&self) -> bool {
        true
    }

    fn reset(&self) {}
}

senken_plugin_api::export!(Broken);
"#;

    /// The 1-based line [`TYPE_ERROR_SOURCE`]'s bad assignment sits on —
    /// counted by hand from the literal above, and asserted against
    /// directly so a future edit to the fixture that silently moves the
    /// line is caught by this test failing, not by it passing for the
    /// wrong reason.
    const TYPE_ERROR_LINE: u32 = 10;

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

    /// Creates `email` and grants every action on `Resource::UserIndicator`
    /// at `Scope::Own` — the same shape `notes_handlers`'s own `notes_user`
    /// helper grants for `Resource::Note`, since a freshly created account
    /// has no grant on either resource by default.
    async fn indicator_user(
        addr: std::net::SocketAddr,
        identity: &IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Indicator User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::UserIndicator, Scope::Own),
                )
                .unwrap();
        }
        login_token(addr, email, "a very long password").await
    }

    /// A real, warm-cache [`CompileService`] behind
    /// [`CompileServiceHandle::Available`], writing into `work_dir` (kept
    /// alive by the caller for the life of the test).
    async fn available_compile_service(work_dir: &std::path::Path) -> Arc<CompileServiceHandle> {
        let toolchain = Toolchain::detect()
            .await
            .expect("this workspace's own rust-toolchain.toml pins wasm32-wasip2");
        let service = CompileService::new(toolchain, work_dir, plugin_api_path())
            .with_target_dir(shared_target_dir());
        Arc::new(CompileServiceHandle::Available(service))
    }

    /// Ensures a 20-bar 1-minute range is resolvable on the fake venue's
    /// own test instrument and returns `(instrument, from, to)` — the same
    /// `ensure` -> poll shape `indicator_handlers`'s own tests duplicate
    /// inline for the identical reason (`compute_indicator` needs real
    /// bars underneath any indicator, built-in or not).
    async fn ensure_range(addr: std::net::SocketAddr, token: &str) -> (String, i64, i64) {
        let instrument = test_instrument();
        let range_to: i64 = 20 * 60 * 1_000_000_000;
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            token,
            serde_json::json!({ "instrument": instrument, "spec": "1m", "from": 0, "to": range_to }),
        )
        .await;
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let response =
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), token).await;
                if body_json(response).await["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        (instrument, 0, range_to)
    }

    #[tokio::test]
    async fn saving_a_valid_indicator_returns_compiled_true_and_it_appears_in_indicators_for_its_owner()
     {
        let _guard = BUILD_LOCK.lock().await;
        let work_dir = tempfile::tempdir().unwrap();
        let compile_service = available_compile_service(work_dir.path()).await;
        let (runtime_dir, runtime) = temp_empty_runtime();
        let (handle, identity, _dir) =
            serve_unfenced_test_server_with_compile_service(runtime, compile_service).await;
        let addr = handle.local_addr();

        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = indicator_user(addr, &identity, &admin, "alice@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/my/indicators"),
            &alice_token,
            serde_json::json!({ "title": "Deterministic", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::OK);
        let created = body_json(create).await;
        assert_eq!(
            created["compiled"], true,
            "a real, already-proven indicator must compile: {created:?}"
        );
        assert!(created["diagnostics"].is_null());

        let catalogue =
            body_json(get_auth(format!("http://{addr}/api/indicators"), &alice_token).await).await;
        let names: Vec<&str> = catalogue
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"my/deterministic"),
            "the compiled indicator must join the catalogue for its own owner: {names:?}"
        );

        handle.shutdown().await.unwrap();
        drop(runtime_dir);
    }

    #[tokio::test]
    async fn saving_a_broken_indicator_returns_diagnostics_with_a_line_and_keeps_the_previous_compiled_one_in_the_catalog()
     {
        let _guard = BUILD_LOCK.lock().await;
        let work_dir = tempfile::tempdir().unwrap();
        let compile_service = available_compile_service(work_dir.path()).await;
        let (runtime_dir, runtime) = temp_empty_runtime();
        let (handle, identity, _dir) =
            serve_unfenced_test_server_with_compile_service(runtime, compile_service).await;
        let addr = handle.local_addr();

        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = indicator_user(addr, &identity, &admin, "alice@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/my/indicators"),
            &alice_token,
            serde_json::json!({ "title": "Deterministic", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::OK);
        let created = body_json(create).await;
        assert_eq!(
            created["compiled"], true,
            "the precondition indicator must compile: {created:?}"
        );
        let id = created["id"].as_str().unwrap().to_owned();

        let update = put_json_auth(
            format!("http://{addr}/api/my/indicators/{id}"),
            &alice_token,
            serde_json::json!({ "source": TYPE_ERROR_SOURCE }),
        )
        .await;
        assert_eq!(
            update.status(),
            reqwest::StatusCode::OK,
            "a compile mistake is still a 200"
        );
        let updated = body_json(update).await;
        assert_eq!(updated["compiled"], false);
        let diagnostics = updated["diagnostics"].as_array().unwrap();
        assert!(!diagnostics.is_empty(), "the type error must be reported");
        assert_eq!(
            diagnostics[0]["line"], TYPE_ERROR_LINE,
            "the reported line must match where the mistake actually is: {diagnostics:?}"
        );

        let catalogue =
            body_json(get_auth(format!("http://{addr}/api/indicators"), &alice_token).await).await;
        let names: Vec<&str> = catalogue
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"my/deterministic"),
            "the previously compiled component must stay registered after a failed save: {names:?}"
        );

        handle.shutdown().await.unwrap();
        drop(runtime_dir);
    }

    #[tokio::test]
    async fn another_user_cannot_read_update_delete_or_compute_my_indicator() {
        let _guard = BUILD_LOCK.lock().await;
        let work_dir = tempfile::tempdir().unwrap();
        let compile_service = available_compile_service(work_dir.path()).await;
        let venue_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(venue_dir.path());
        let (handle, identity, _dir) =
            serve_unfenced_test_server_with_compile_service(runtime, compile_service).await;
        let addr = handle.local_addr();

        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = indicator_user(addr, &identity, &admin, "alice@example.com").await;
        let bob_token = indicator_user(addr, &identity, &admin, "bob@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/my/indicators"),
            &alice_token,
            serde_json::json!({ "title": "Deterministic", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::OK);
        let id = body_json(create).await["id"].as_str().unwrap().to_owned();

        let anonymous_delete =
            delete_no_auth(format!("http://{addr}/api/my/indicators/{id}")).await;
        assert_eq!(anonymous_delete.status(), reqwest::StatusCode::UNAUTHORIZED);

        let get = get_auth(format!("http://{addr}/api/my/indicators/{id}"), &bob_token).await;
        assert_eq!(
            get.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "another account's indicator must read back as absent, the same as a genuinely unknown id"
        );

        let update = put_json_auth(
            format!("http://{addr}/api/my/indicators/{id}"),
            &bob_token,
            serde_json::json!({ "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(update.status(), reqwest::StatusCode::BAD_REQUEST);

        let delete = delete_auth(format!("http://{addr}/api/my/indicators/{id}"), &bob_token).await;
        assert_eq!(delete.status(), reqwest::StatusCode::BAD_REQUEST);

        let (instrument, from, to) = ensure_range(addr, &bob_token).await;
        let compute = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &bob_token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": from,
                "to": to,
                "indicator": { "name": "my/deterministic", "params": "{}" },
            }),
        )
        .await;
        assert_eq!(
            compute.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "bob never compiled anything under this slug in his own catalog"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn when_the_toolchain_is_missing_save_returns_503_with_a_product_message_and_the_source_is_still_stored()
     {
        let (runtime_dir, runtime) = temp_empty_runtime();
        let compile_service = Arc::new(CompileServiceHandle::Unavailable(
            "wasm32-wasip2 target not installed".to_owned(),
        ));
        let (handle, identity, _dir) =
            serve_unfenced_test_server_with_compile_service(runtime, compile_service).await;
        let addr = handle.local_addr();

        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = indicator_user(addr, &identity, &admin, "alice@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/my/indicators"),
            &alice_token,
            serde_json::json!({ "title": "Deterministic", "source": VALID_SOURCE }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(create).await;
        let message = body["error"].as_str().unwrap();
        assert!(
            message.contains("wasm32-wasip2 target not installed"),
            "the reason this server cannot compile must reach the caller: {message}"
        );

        let list =
            body_json(get_auth(format!("http://{addr}/api/my/indicators"), &alice_token).await)
                .await;
        let rows = list.as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "create() stores the row before ever attempting to compile it"
        );
        assert_eq!(rows[0]["compiled"], false);

        handle.shutdown().await.unwrap();
        drop(runtime_dir);
    }
}
