//! Backs `wit/senken.wit`'s `http` interface with a real
//! [`senken_venue::VenueClient`] — the one path to the network a venue
//! plugin has, and the reason every byte it ever receives is charged
//! against a real [`senken_venue::LimitGroup`] before it leaves this
//! machine (see that crate's own docs, and `wit/senken.wit`'s `http`
//! interface, for why this exists at all).
//!
//! # Why a fetch call blocks its caller instead of the guest calling an
//! `async` import
//!
//! Every other `Store` this crate builds (`PluginState`, for the
//! `indicator-plugin`/`compiled-indicator` worlds) runs against a plain
//! synchronous `wasmtime::Engine`, driven from whatever thread happens to
//! call in — a chart replay loop, a backtest worker, this crate's own test
//! suite. Keeping [`VenuePluginState`](crate::wasi::VenuePluginState) on
//! that same synchronous engine (rather than standing up a second,
//! `async_support`-enabled `Engine`/`Linker` pair just for one interface)
//! means every mechanism this crate already proves — epoch deadlines, fuel,
//! the memory limiter, the circuit breaker, `crate::instance::guarded_call`
//! turning a trap into `Err` — applies to a venue plugin exactly as written,
//! with nothing to duplicate or keep in sync between two engines.
//!
//! The cost is that [`Host::fetch`] itself must be a plain, blocking `fn`:
//! it cannot `.await` a [`senken_venue::VenueClient::get`] call in place
//! without an async runtime under it. [`FetchExecutor`] pays that cost by
//! running every fetch on a small dedicated Tokio runtime and blocking the
//! calling thread on a channel until it answers — never on
//! `tokio::runtime::Handle::block_on`, which panics when the calling thread
//! is itself already inside a runtime's own worker (exactly the situation a
//! caller driving several plugin calls from an async context would be in).
//!
//! # Epoch interruption cannot bound this call, so a fetch timeout does
//!
//! `wit/senken.wit`'s epoch-deadline mechanism (see `crate::execution`)
//! only ever traps while the guest is *executing WASM instructions* — the
//! engine's epoch check is compiled into the guest's own code at loop
//! back-edges and function entries. While control is inside this native
//! Rust function, blocked on [`FetchExecutor::block_on`], no guest
//! instruction is running for that check to ever fire, so a network call
//! that never returns would hang the calling thread forever regardless of
//! how tight a live deadline was configured. [`FETCH_TIMEOUT`] is what
//! actually bounds this call — a second, independent mechanism for the one
//! part of a venue plugin's work an *instruction* budget cannot reach.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::source::SourceError;
use senken_venue::VenueClient;

use crate::bindings::{FetchError, HttpHost as Host};
use crate::wasi::VenuePluginState;

/// How long a single [`Host::fetch`] call is allowed to run before this
/// crate gives up on it and reports [`FetchError::Transport`] — see this
/// module's own docs for why epoch interruption cannot bound this instead.
///
/// Deliberately generous relative to a real request: `senken_venue::
/// VenueClient::get` already retries a retryable failure with jittered
/// backoff internally (`senken_venue::RetryPolicy::default()`), so this
/// budget has to cover a full retry sequence, not one round trip. Chosen to
/// comfortably exceed `senken_plugin::HTTP_REQUEST_TIMEOUT` (30s) plus that
/// backoff's own worst case, not measured against any real venue.
const FETCH_TIMEOUT: Duration = Duration::from_secs(45);

/// Runs every venue plugin's `fetch` call to completion, off whatever
/// thread happens to call into the plugin.
///
/// One instance is shared by every [`crate::venue::VenuePluginHost`] this
/// process builds — a dedicated runtime is cheap to keep alive for the
/// process's lifetime and expensive to spin up per call, and nothing about
/// it is per-plugin state (unlike the circuit breaker, log and health,
/// which `crate::venue::LoadedVenuePlugin` keeps one of each per plugin).
pub(crate) struct FetchExecutor {
    /// `Option` only so [`Drop`] can move it out — see that impl for why.
    runtime: Option<tokio::runtime::Runtime>,
}

impl FetchExecutor {
    /// # Errors
    /// If the underlying Tokio runtime cannot be built — the same
    /// conditions that would make any `tokio::runtime::Builder::build`
    /// call in this workspace fail (thread spawn failure, typically).
    pub(crate) fn new() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // Two workers: fetch traffic is I/O-bound (waiting on sockets,
            // not burning CPU), so this is not sized to plugin count —
            // it is sized to "more than one in-flight fetch should not
            // serialize behind the first," the same reasoning
            // `senken_venue::LimitGroup`'s own default concurrency ceiling
            // uses for a different budget.
            .worker_threads(2)
            .thread_name("senken-venue-fetch")
            .enable_all()
            .build()?;
        Ok(Self {
            runtime: Some(runtime),
        })
    }

    /// Runs `fut` on this executor's own runtime and blocks the calling
    /// thread until it finishes or [`FETCH_TIMEOUT`] elapses — never the
    /// calling thread's own runtime (there may not be one), and never
    /// `Handle::block_on`, which requires the calling thread not already be
    /// inside a runtime worker, a property this function's callers cannot
    /// promise.
    ///
    /// `None` on timeout. The spawned task itself is not cancelled — it
    /// keeps running to completion on this executor's runtime and its
    /// result is simply dropped, exactly like a `senken_venue::VenueClient`
    /// call a caller stopped `.await`-ing would be dropped by its own
    /// caller. A plugin that keeps timing out trips [`crate::circuit`] the
    /// same way a plugin that keeps trapping does, since this crate's own
    /// call sites report a timeout as an ordinary failure.
    fn block_on<F>(&self, fut: F) -> Option<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        self.runtime()?.spawn(async move {
            let _ = tx.send(fut.await);
        });
        rx.recv_timeout(FETCH_TIMEOUT).ok()
    }

    /// `None` only after [`Drop::drop`] has already taken the runtime —
    /// unreachable in practice, since nothing calls [`Self::block_on`] on a
    /// `FetchExecutor` that is already being dropped, but stated as an
    /// `Option` rather than asserted away so this can never panic if that
    /// ever stopped being true.
    fn runtime(&self) -> Option<&tokio::runtime::Runtime> {
        self.runtime.as_ref()
    }
}

impl Drop for FetchExecutor {
    /// Moves the runtime onto a fresh, throwaway thread purely to drop it
    /// there, rather than dropping it in place.
    ///
    /// Dropping a multi-thread [`tokio::runtime::Runtime`] blocks the
    /// dropping thread until every worker thread has joined — ordinary and
    /// fine from a plain thread, but `tokio` panics ("Cannot drop a runtime
    /// in a context where blocking is not allowed") if that thread is
    /// itself already inside another runtime's own worker, which is
    /// exactly where a [`crate::host::PluginHost`] (and the [`FetchExecutor`]
    /// it owns) is dropped in practice — an async HTTP handler's shutdown
    /// path, or this crate's own `#[tokio::test]`s. Spawning a bare
    /// `std::thread` to do the actual drop sidesteps the check entirely: a
    /// plain OS thread is never "inside a runtime" to begin with. The
    /// thread exits as soon as the runtime finishes shutting down and needs
    /// no handle kept anywhere — there is nothing left to join it against,
    /// the same way `crate::execution::EpochTicker`'s own background thread
    /// needs none once its stop flag is set.
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let _ = std::thread::Builder::new()
                .name("senken-venue-fetch-shutdown".to_owned())
                .spawn(move || drop(runtime));
        }
    }
}

/// One venue plugin's HTTP capability: the client it fetches through
/// (already bound to its own [`senken_venue::LimitGroup`] by whoever
/// registered this plugin — see `senken_runtime::plugin_host::DynamicVenues`),
/// the one origin its `fetch` calls may ever resolve against, and the
/// shared [`FetchExecutor`] that actually runs them.
///
/// Cheap to clone: every clone shares the same underlying client and
/// executor, exactly like [`VenueClient`] itself.
#[derive(Clone)]
pub(crate) struct HostHttp {
    client: VenueClient,
    /// The scheme+host every `fetch` call's `path` is resolved against —
    /// e.g. `https://www.okx.com`. Fixed at registration time, not
    /// something a plugin's own `fetch` call can name: see `wit/senken.wit`'s
    /// `http` interface docs for why a guest gets a path, never a URL.
    base_url: String,
    executor: Arc<FetchExecutor>,
}

impl HostHttp {
    pub(crate) fn new(
        client: VenueClient,
        base_url: impl Into<String>,
        executor: Arc<FetchExecutor>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
            executor,
        }
    }
}

/// Adds this crate's host implementation of `wit/senken.wit`'s `http`
/// interface to `linker`. Call once per venue [`wasmtime::component::Linker`]
/// — mirrors `crate::builtins::add_to_linker` exactly.
///
/// # Errors
/// Only if `wasmtime`'s own binding generation rejects a duplicate
/// registration, which does not happen when this runs once per linker.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<VenuePluginState>,
) -> wasmtime::Result<()> {
    crate::bindings::generated_venue::senken::plugin_api::http::add_to_linker::<
        VenuePluginState,
        wasmtime::component::HasSelf<VenuePluginState>,
    >(linker, |state| state)
}

impl Host for VenuePluginState {
    fn fetch(&mut self, path: String, cost: u32) -> Result<Vec<u8>, FetchError> {
        let url = format!("{}{path}", self.http.base_url);
        let client = self.http.client.clone();
        let outcome = self
            .http
            .executor
            .block_on(async move { client.get(&url, cost).await });
        match outcome {
            Some(Ok(body)) => Ok(body),
            Some(Err(source_error)) => Err(fetch_error_from_source_error(&source_error)),
            None => Err(FetchError::Transport(format!(
                "fetch did not complete within {FETCH_TIMEOUT:?}"
            ))),
        }
    }
}

/// Restates a [`SourceError`] — what [`VenueClient::get`] actually fails
/// with — as the `wit/senken.wit` `fetch-error` a guest can read.
fn fetch_error_from_source_error(error: &SourceError) -> FetchError {
    match error {
        SourceError::Transport { source } => FetchError::Transport(source.to_string()),
        SourceError::Http { status, body } => FetchError::Http((*status, body.clone())),
        SourceError::Rejected { reason } => FetchError::Rejected(reason.clone()),
        // `VenueClient::get` never actually produces `Decode` (parsing a
        // response body is the guest's own job, past this boundary) or any
        // variant this crate has not seen — `SourceError` is
        // `#[non_exhaustive]` precisely so a future venue-crate addition
        // does not silently fail to compile here. Restated as `Transport`
        // rather than guessed at more specifically: this arm exists for
        // exhaustiveness, not because it is expected to run.
        other => FetchError::Transport(other.to_string()),
    }
}
