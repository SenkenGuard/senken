//! Shared test scaffolding (test-only): a fresh, temp-file-backed
//! [`IdentityStore`] every integration test in this crate needs, since
//! [`crate::serve`] now requires one, plus small `reqwest` helpers matching
//! this workspace's "no `json` feature" convention (see this crate's own
//! `[dev-dependencies]`: plugins decode bodies themselves for error
//! control, so integration tests build/parse JSON by hand too).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};
use senken_runtime::Runtime;
use senken_subscription::{BookSessionRegistry, SubscriptionPool};
use tempfile::TempDir;

use crate::{ServeOptions, ServerHandle};

/// A fresh accounts database, seeded with the default superadmin per plan
/// 004 B4. The returned [`TempDir`] must be kept alive for as long as the
/// store is used — dropping it deletes the database file.
pub(crate) fn temp_identity_store() -> (TempDir, IdentityStore) {
    let dir = TempDir::new().expect("creating a tempdir cannot fail in a test");
    let store = IdentityStore::open(dir.path().join("accounts.db")).expect("opening a fresh store");
    (dir, store)
}

/// A [`Runtime`] with no plugins registered — enough to satisfy
/// [`crate::serve`]'s signature for a test that never touches bars or
/// indicators (an auth, admin, workspace or alerts test, say). Building an
/// empty runtime cannot fail: it only creates empty directories.
pub(crate) fn temp_empty_runtime() -> (TempDir, Runtime) {
    let dir = TempDir::new().expect("creating a tempdir cannot fail in a test");
    let runtime = Runtime::builder()
        .data_dir(dir.path())
        .build()
        .expect("an empty runtime always builds");
    (dir, runtime)
}

/// `localhost:0` — every integration test in this crate binds an
/// OS-assigned port on loopback only.
pub(crate) fn localhost_any_port() -> ServeOptions {
    ServeOptions {
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        allowed_origins: Vec::new(),
    }
}

/// The password every test server in this module seeds the default admin
/// with.
pub(crate) const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";

/// A running server whose default admin has already set a password,
/// built on `runtime` — the caller supplies it so a test that needs a fake
/// bar source (bars, indicators) can register one, while a test that does
/// not (workspaces, alerts) can pass [`temp_empty_runtime`]'s output.
/// Returns the [`IdentityStore`] the server runs on too, for tests that
/// need to create users or resolve sessions directly rather than through a
/// second HTTP round trip.
pub(crate) async fn serve_unfenced_test_server_with(
    runtime: Runtime,
) -> (ServerHandle, Arc<IdentityStore>, TempDir) {
    let (dir, store) = temp_identity_store();
    store
        .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD, None)
        .unwrap();
    let store = Arc::new(store);
    let handle = crate::serve(localhost_any_port(), Arc::clone(&store), Arc::new(runtime))
        .await
        .unwrap();
    (handle, store, dir)
}

/// [`serve_unfenced_test_server_with`] backed by an empty runtime — for the
/// workspace and alerts integration tests, which never touch bars or
/// indicators.
pub(crate) async fn serve_unfenced_test_server()
-> (ServerHandle, Arc<IdentityStore>, TempDir, TempDir) {
    let (runtime_dir, runtime) = temp_empty_runtime();
    let (handle, store, dir) = serve_unfenced_test_server_with(runtime).await;
    (handle, store, dir, runtime_dir)
}

/// As [`serve_unfenced_test_server_with`], but with an injected set of
/// live-feed pools instead of whatever `crate::feed::build_feed_pools`
/// would build from `runtime`'s own marketdata sources — the seam
/// `live_feed_tests` uses to run the real WS/alert-engine wiring against a
/// fake venue's pool rather than ever dialling OKX.
pub(crate) async fn serve_unfenced_test_server_with_feed(
    runtime: Runtime,
    feed_pools: HashMap<String, SubscriptionPool>,
) -> (ServerHandle, Arc<IdentityStore>, TempDir) {
    let (dir, store) = temp_identity_store();
    store
        .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD, None)
        .unwrap();
    let store = Arc::new(store);
    let handle = crate::serve_with_feed_pools(
        localhost_any_port(),
        Arc::clone(&store),
        Arc::new(runtime),
        feed_pools,
    )
    .await
    .unwrap();
    (handle, store, dir)
}

/// As [`serve_unfenced_test_server_with_feed`], but with an explicit
/// [`BookSessionRegistry`] — the seam `live_feed_tests` uses to run the
/// depth poll loop on a fast, test-scale cadence
/// (`BookSessionRegistry::with_interval`) rather than waiting a full second
/// per snapshot.
///
/// The depth *source* is not injected here: it reaches the server the only
/// way it can in production, by a plugin registering it into `runtime`.
pub(crate) async fn serve_unfenced_test_server_with_book(
    runtime: Runtime,
    feed_pools: HashMap<String, SubscriptionPool>,
    book_sessions: Arc<BookSessionRegistry>,
) -> (ServerHandle, Arc<IdentityStore>, TempDir) {
    let (dir, store) = temp_identity_store();
    store
        .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD, None)
        .unwrap();
    let store = Arc::new(store);
    let handle = crate::serve_with_feed_pools_and_book(
        localhost_any_port(),
        Arc::clone(&store),
        Arc::new(runtime),
        feed_pools,
        book_sessions,
    )
    .await
    .unwrap();
    (handle, store, dir)
}

/// `POST url` with a JSON body, no `Authorization` header.
pub(crate) async fn post_json(
    url: impl reqwest::IntoUrl,
    body: serde_json::Value,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .await
        .unwrap()
}

/// `POST url` with a JSON body and `Authorization: Bearer <token>`.
pub(crate) async fn post_json_auth(
    url: impl reqwest::IntoUrl,
    token: &str,
    body: serde_json::Value,
) -> reqwest::Response {
    reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(serde_json::to_vec(&body).unwrap())
        .send()
        .await
        .unwrap()
}

/// `GET url` with `Authorization: Bearer <token>`.
pub(crate) async fn get_auth(url: impl reqwest::IntoUrl, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap()
}

/// Parses a response body as JSON, tolerating an empty (`204`) body as
/// `Value::Null`.
pub(crate) async fn body_json(response: reqwest::Response) -> serde_json::Value {
    let bytes = response.bytes().await.unwrap();
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    }
}
