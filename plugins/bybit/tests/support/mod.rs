//! Builds `wasm/` (the `bybit-venue` component) on demand for
//! `tests/wasm_parity.rs`, so that test proves the equivalence property
//! against genuinely compiled bytes rather than a description of what the
//! component would do.
//!
//! Mirrors `plugins/okx/tests/support/mod.rs` (itself mirroring
//! `crates/plugin-host/tests/support/mod.rs`) exactly: one shared
//! `target/fixture-wasm` (`AGENTS.md`), one process-wide build lock so
//! this crate's own parallel test harness never starts two `cargo build`
//! subprocesses at once — this machine must never run two Rust builds
//! concurrently.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Builds `wasm/`'s `bybit-venue` crate for `wasm32-wasip2` (release, the
/// same profile `plugins/build-venue.sh` uses) and returns the path to the
/// compiled component.
///
/// # Panics
/// If the build fails — a build failure here is a defect in this crate's
/// own `wasm/` crate, never something a test is exercising.
pub(crate) fn build_bybit_venue() -> PathBuf {
    let _guard = BUILD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let wasm_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("wasm");
    let shared_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture-wasm");
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        // The same shared build directory every wasm test fixture in this
        // workspace already uses — see that module's own docs for why.
        .env("CARGO_TARGET_DIR", &shared_target)
        .current_dir(&wasm_dir)
        .status()
        .expect("spawning `cargo build` for bybit-venue must succeed");
    assert!(status.success(), "bybit-venue failed to build");

    let wasm_path = shared_target.join("wasm32-wasip2/release/bybit_venue.wasm");
    assert!(
        wasm_path.is_file(),
        "expected {} to exist after building bybit-venue",
        wasm_path.display()
    );
    wasm_path
}
