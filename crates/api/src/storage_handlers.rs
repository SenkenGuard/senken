//! Disk usage reporting and reclamation over HTTP.
//!
//! Unlike every other domain handler in this crate, `senken-store` has no
//! notion of a user — disk usage is a property of the server, not any one
//! account — so authorisation is not delegated to a store the way
//! `senken_alerts::AlertStore` etc. check themselves on every call. Each
//! handler here calls `AuthenticatedUser::authorize` directly, and both
//! require `Scope::All`: a `Scope::Own` grant means nothing against a
//! shared filesystem, so [`require_storage_all`] refuses it with the same
//! error an actor holding no grant at all gets, rather than pretending a
//! narrower scope is a smaller, honest truth.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::{Extension, Json};

use senken_acl::{Action, Resource, Scope};
use senken_identity::AuthenticatedUser;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    DeleteStorageRequest, DeleteStorageResponse, MarketDataUsageDto, StorageDatabaseDto,
    StorageReportDto,
};

/// Requires `action` on `Resource::Storage`, and specifically at
/// `Scope::All` — the one resource in this crate where `Scope::Own` is not
/// merely unused but meaningless, since nothing under `sources/` or in the
/// accounts database is owned by the caller alone.
fn require_storage_all(user: &AuthenticatedUser, action: Action) -> Result<(), HandlerError> {
    let scope = user.authorize(action, Resource::Storage)?;
    if scope == Scope::All {
        Ok(())
    } else {
        Err(HandlerError::Forbidden(
            "you do not have permission to do that".to_owned(),
        ))
    }
}

/// Sums a file's size, plus its `-wal`/`-shm` siblings when SQLite has
/// them open — a write-ahead log can hold real, not-yet-checkpointed
/// data, so leaving it out would under-report a database that is actively
/// being written to. A missing file (the base path, or either sibling)
/// contributes nothing, not an error.
fn file_and_wal_bytes(path: &Path) -> u64 {
    let mut total = std::fs::metadata(path).map_or(0, |m| m.len());
    for suffix in ["-wal", "-shm"] {
        let mut sibling = path.as_os_str().to_owned();
        sibling.push(suffix);
        total += std::fs::metadata(PathBuf::from(sibling)).map_or(0, |m| m.len());
    }
    total
}

/// Every database this server keeps outside `senken-store`'s Parquet
/// layout — today, just the accounts database everything but market data
/// lives in (workspaces, alerts, watchlists, notes all share its
/// connection; see `senken_identity::IdentityStore::shared_connection`'s
/// own docs). Reported as one figure per database, never a fake tree.
fn database_usage(state: &AppState) -> Vec<StorageDatabaseDto> {
    let Some(path) = state.identity.db_path() else {
        // Never actually reachable — this crate's identity store is always
        // opened against a real file — but a missing path is "nothing to
        // report", not a reason to fail the whole request.
        return Vec::new();
    };
    vec![StorageDatabaseDto {
        label: "Accounts".to_owned(),
        bytes: file_and_wal_bytes(&path),
        path: path.display().to_string(),
    }]
}

/// `GET /api/storage`. Requires `Action::View` on `Resource::Storage` at
/// `Scope::All`.
#[utoipa::path(
    get,
    path = "/api/storage",
    responses(
        (status = 200, body = StorageReportDto),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn storage_report(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<StorageReportDto>, HandlerError> {
    require_storage_all(&ctx.user, Action::View)?;
    let sources = state.runtime.store().usage()?;
    Ok(Json(StorageReportDto {
        data_dir: state.runtime.storage().data_dir().display().to_string(),
        market_data: MarketDataUsageDto::from(sources),
        databases: database_usage(&state),
    }))
}

/// `POST /api/storage/delete`. Requires `Action::Delete` on
/// `Resource::Storage` at `Scope::All`. `series_id` with no `symbol` is
/// rejected — there is no whole-source concept of "this one series" to
/// narrow to.
#[utoipa::path(
    post,
    path = "/api/storage/delete",
    request_body = DeleteStorageRequest,
    responses(
        (status = 200, body = DeleteStorageResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_storage(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<DeleteStorageRequest>,
) -> Result<Json<DeleteStorageResponse>, HandlerError> {
    require_storage_all(&ctx.user, Action::Delete)?;
    let store = state.runtime.store();
    let freed_bytes = match (body.symbol.as_deref(), body.series_id.as_deref()) {
        (Some(symbol), Some(series_id)) => {
            store.delete_series(&body.source_id, symbol, series_id)?
        }
        (Some(symbol), None) => store.delete_instrument(&body.source_id, symbol)?,
        (None, Some(_)) => {
            return Err(HandlerError::BadRequest(
                "series_id requires symbol to be given too".to_owned(),
            ));
        }
        (None, None) => store.delete_source(&body.source_id)?,
    };
    Ok(Json(DeleteStorageResponse { freed_bytes }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_runtime::Runtime;
    use tempfile::TempDir;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json_auth, serve_unfenced_test_server_with,
    };

    /// Writes `len` zero bytes at `path`, creating parent directories.
    fn write_sized(path: &std::path::Path, len: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![0u8; len]).unwrap();
    }

    /// A runtime whose data directory already has a small real tree under
    /// it — built before `Runtime::builder().build()` runs, so `Store`
    /// (rooted at the very same directory) sees it immediately.
    fn runtime_with_fixture_tree() -> (TempDir, Runtime) {
        let dir = TempDir::new().unwrap();
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/trades/f1.parquet"),
            1_234,
        );
        write_sized(
            &dir.path()
                .join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m/f1.parquet"),
            500,
        );
        let runtime = Runtime::builder().data_dir(dir.path()).build().unwrap();
        (dir, runtime)
    }

    /// Grants `user_id` `Scope::All` on `Resource::Storage` for `actions`.
    fn grant_storage(
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        user_id: senken_identity::UserId,
        actions: &[Action],
    ) {
        for action in actions {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(*action, Resource::Storage, Scope::All),
                )
                .unwrap();
        }
    }

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

    #[tokio::test]
    async fn an_admin_sees_the_full_report() {
        let (_data_dir, runtime) = runtime_with_fixture_tree();
        let (handle, _identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();

        let token = login_token(addr, DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD).await;

        let body = body_json(get_auth(format!("http://{addr}/api/storage"), &token).await).await;
        assert_eq!(body["market_data"]["total_bytes"], 1_734);
        let sources = body["market_data"]["sources"].as_array().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0]["source_id"], "binance-spot");
        let instruments = sources[0]["instruments"].as_array().unwrap();
        assert_eq!(instruments[0]["symbol"], "BTCUSDT");
        let series = instruments[0]["series"].as_array().unwrap();
        assert_eq!(series.len(), 2);
        // Sorted biggest first: trades (1234) before the bars series (500).
        assert_eq!(series[0]["kind"], "trades");
        assert_eq!(series[0]["label"], "Trades");
        assert_eq!(series[1]["kind"], "bars");
        assert_eq!(series[1]["label"], "1m · venue");

        // The accounts database is reported as a single figure.
        let databases = body["databases"].as_array().unwrap();
        assert_eq!(databases.len(), 1);
        assert_eq!(databases[0]["label"], "Accounts");
        assert!(databases[0]["bytes"].as_u64().unwrap() > 0);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_user_without_the_storage_grant_gets_403_not_401() {
        let (_data_dir, runtime) = runtime_with_fixture_tree();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
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
                "nostorage@example.com",
                "No Storage",
                Some("a very long password"),
            )
            .unwrap();
        let token = login_token(addr, "nostorage@example.com", "a very long password").await;

        let response = get_auth(format!("http://{addr}/api/storage"), &token).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "a valid session with no Storage grant must be 403, never a logout-triggering 401"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_scope_own_grant_on_storage_is_refused_like_no_grant_at_all() {
        // A per-user scope on a shared filesystem would be a lie — `Own`
        // must be refused exactly like having no grant, not silently
        // treated as `All`.
        let (_data_dir, runtime) = runtime_with_fixture_tree();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
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
                Grant::new(Action::View, Resource::Storage, Scope::Own),
            )
            .unwrap();
        let token = login_token(addr, "ownscope@example.com", "a very long password").await;

        let response = get_auth(format!("http://{addr}/api/storage"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_series_id_with_no_symbol_is_400() {
        let (_data_dir, runtime) = runtime_with_fixture_tree();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
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
                "deleter@example.com",
                "Deleter",
                Some("a very long password"),
            )
            .unwrap();
        grant_storage(&identity, &admin, user_id, &[Action::Delete]);
        let token = login_token(addr, "deleter@example.com", "a very long password").await;

        let response = post_json_auth(
            format!("http://{addr}/api/storage/delete"),
            &token,
            serde_json::json!({
                "source_id": "binance-spot",
                "series_id": "venue-1m",
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn deleting_a_series_over_http_actually_reduces_the_reported_total() {
        let (_data_dir, runtime) = runtime_with_fixture_tree();
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
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
                "reclaimer@example.com",
                "Reclaimer",
                Some("a very long password"),
            )
            .unwrap();
        grant_storage(&identity, &admin, user_id, &[Action::View, Action::Delete]);
        let token = login_token(addr, "reclaimer@example.com", "a very long password").await;

        let before = body_json(get_auth(format!("http://{addr}/api/storage"), &token).await).await;
        assert_eq!(before["market_data"]["total_bytes"], 1_734);

        let delete = post_json_auth(
            format!("http://{addr}/api/storage/delete"),
            &token,
            serde_json::json!({
                "source_id": "binance-spot",
                "symbol": "BTCUSDT",
                "series_id": "venue-1m",
            }),
        )
        .await;
        assert_eq!(delete.status(), reqwest::StatusCode::OK);
        let freed = body_json(delete).await;
        assert_eq!(freed["freed_bytes"], 500);

        let after = body_json(get_auth(format!("http://{addr}/api/storage"), &token).await).await;
        assert_eq!(after["market_data"]["total_bytes"], 1_234);

        handle.shutdown().await.unwrap();
    }
}
