//! The capability-zero WASI surface a plugin's `Store` runs against.
//!
//! A component compiled for `wasm32-wasip2` imports `wasi:io`, `wasi:cli`
//! and `wasi:clocks` interfaces purely as a side effect of linking against
//! Rust's standard library — nothing in `wit/senken.wit`'s own
//! `indicator-plugin` world asks for them. This module links exactly that
//! minimal, unavoidable set and nothing past it.
//!
//! `wasi:filesystem` and `wasi:sockets` are never added to the linker at
//! all — not restricted, not sandboxed, simply absent. A plugin whose code
//! path can reach either one (a `std::fs` or `std::net` call anywhere it
//! can be reached from, whether or not that path executes) carries that
//! import in its compiled component unconditionally, because the
//! component model resolves every import at instantiation time before any
//! guest code runs at all. So a plugin that tries either fails to
//! *instantiate* — this crate's definition of "load" — rather than
//! failing on whatever bar happens to trigger the call.
//!
//! This is deliberately not `wasmtime_wasi::p2::add_to_linker_sync`: that
//! convenience function wires up `wasi:filesystem` and `wasi:sockets` too,
//! which is exactly the capability this crate exists to withhold. Each
//! interface below is added with its own `add_to_linker` call instead.

use std::sync::Arc;

use wasmtime::component::{HasData, Linker, ResourceTable};
use wasmtime_wasi::cli::{WasiCli, WasiCliView as _};
use wasmtime_wasi::clocks::{WasiClocks, WasiClocksView as _};
use wasmtime_wasi::p2::bindings::{cli, random, sync};
use wasmtime_wasi::random::{WasiRandom, WasiRandomView as _};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

use crate::builtins::BuiltinState;
use crate::health::{MemoryLimiter, RuntimeHealth};
use crate::log::PluginLog;

/// [`HasData`] for `wasi:io/*`, mirroring the private marker type
/// `wasmtime-wasi` uses for the same purpose internally — that type is not
/// exported, so this crate names its own.
struct HasIo;

impl HasData for HasIo {
    type Data<'a> = &'a mut ResourceTable;
}

/// Adds the fixed, capability-zero WASI surface to `linker`. Call once per
/// [`wasmtime::Engine`] — the same linker is reused to instantiate every
/// plugin loaded through it.
///
/// # Errors
/// Only if `wasmtime-wasi`'s own binding generation rejects a duplicate
/// registration, which does not happen when this function runs once per
/// linker as intended.
pub(crate) fn add_sandboxed_wasi_to_linker<T: WasiView>(
    linker: &mut Linker<T>,
) -> wasmtime::Result<()> {
    // `wasi:io` — required by any component that links wasi-libc at all,
    // regardless of what the guest's own code does.
    wasmtime_wasi_io::bindings::wasi::io::error::add_to_linker::<T, HasIo>(linker, |t| {
        t.ctx().table
    })?;
    sync::io::poll::add_to_linker::<T, HasIo>(linker, |t| t.ctx().table)?;
    sync::io::streams::add_to_linker::<T, HasIo>(linker, |t| t.ctx().table)?;

    // `wasi:clocks` — `std::time::{Instant, SystemTime}` reach these.
    // Read-only and non-blocking: granting them cannot let a plugin affect
    // anything outside its own `Store`.
    sync::clocks::wall_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;
    sync::clocks::monotonic_clock::add_to_linker::<T, WasiClocks>(linker, T::clocks)?;

    // `wasi:random` — `std::collections::HashMap`'s default hasher seeds
    // from this. Nothing here is a capability past the sandbox: what a
    // guest can do with its own randomness stays inside its own `Store`.
    random::random::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure::add_to_linker::<T, WasiRandom>(linker, T::random)?;
    random::insecure_seed::add_to_linker::<T, WasiRandom>(linker, T::random)?;

    // `wasi:cli` — argv/env/exit/stdio and their terminal-detection
    // siblings, all part of the reactor-adapter surface a `wasm32-wasip2`
    // component links against unconditionally. `exit` does not exit this
    // process: its host implementation (in `wasmtime-wasi`) returns an
    // `Err` carrying the requested status, which this crate's call sites
    // convert to `PluginHostError::Trap` like any other guest failure.
    cli::exit::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::environment::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    sync::cli::stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    sync::cli::stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    sync::cli::stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_input::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_output::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdin::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stdout::add_to_linker::<T, WasiCli>(linker, T::cli)?;
    cli::terminal_stderr::add_to_linker::<T, WasiCli>(linker, T::cli)?;

    Ok(())
}

/// One plugin instance's `Store<T>` data: the WASI context wired to its own
/// [`PluginLog`], and the resource table every WASI host implementation
/// above needs.
///
/// Deliberately holds no `senken-*` domain type — see this crate's own
/// scope, which stops at loading and running a component. Dropping this
/// (which happens when the owning `wasmtime::Store` is dropped) frees the
/// `ResourceTable` and every WASI handle in it; the guest's linear memory
/// is freed by `Store`'s own drop, not by anything here.
pub(crate) struct PluginState {
    ctx: WasiCtx,
    table: ResourceTable,
    /// The memory ceiling installed via [`wasmtime::Store::limiter`] — see
    /// `host.rs`, which is the only place that reaches into this field.
    /// The sandbox keeps a guest from reaching *out*; this is what stops it
    /// exhausting the host *from inside*, and nothing complains if a
    /// `Store` is ever built without wiring it in, so every `Store` this
    /// crate creates does so in the same place `PluginState` itself is
    /// built, not left to each call site to remember. Also records every
    /// granted memory growth into this instance's shared
    /// [`RuntimeHealth`], which is how [`crate::PluginHealth::peak_memory_bytes`]
    /// gets a real number rather than only the ceiling.
    pub(crate) limits: MemoryLimiter,
    /// This instance's own built-in indicator state, keyed by call-site
    /// slot — see `crate::builtins`. Lives here, one per `Store`, so one
    /// plugin instance's `ema(close, 20)` never sees another instance's
    /// bars.
    pub(crate) builtins: BuiltinState,
}

impl PluginState {
    /// A fresh state whose stdout and stderr both append to `log`, capped
    /// at `max_memory_bytes` of linear memory (every granted growth
    /// recorded into `health`), with no preopened directory, no allowed
    /// network, no inherited environment or arguments — the null `WasiCtx`
    /// a fresh plugin instance starts from.
    pub(crate) fn new(
        log: &PluginLog,
        max_memory_bytes: usize,
        health: Arc<RuntimeHealth>,
    ) -> Self {
        let ctx = WasiCtx::builder()
            .stdout(log.stdout())
            .stderr(log.stderr())
            .build();
        Self {
            ctx,
            table: ResourceTable::new(),
            limits: MemoryLimiter::new(max_memory_bytes, health),
            builtins: BuiltinState::default(),
        }
    }
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}
