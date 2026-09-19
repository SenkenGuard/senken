//! Builds one of this venue's wasm crates (`wasm/`, `wasm-swap/`, ...) on
//! demand for `tests/wasm_parity.rs`, so that test proves the equivalence
//! property against genuinely compiled bytes rather than a description of
//! what the component would do.
//!
//! Mirrors `crates/plugin-host/tests/support/mod.rs` exactly: one shared
//! `target/fixture-wasm` (`AGENTS.md`), one process-wide build lock so
//! this crate's own parallel test harness never starts two `cargo build`
//! subprocesses at once — this machine must never run two Rust builds
//! concurrently.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Builds `wasm_dir` (a directory name under this crate, e.g. `"wasm"` or
/// `"wasm-swap"`) for `wasm32-wasip2` (release, the same profile
/// `plugins/build-venue.sh` uses) and returns the path to the compiled
/// component, whose Cargo package name must be `binary_stem` with `-`
/// replaced by `_` (`okx_venue`, `okx_venue_swap`, ...).
///
/// # Panics
/// If the build fails — a build failure here is a defect in this crate's
/// own wasm crate, never something a test is exercising.
pub(crate) fn build_okx_venue_component(wasm_dir: &str, binary_stem: &str) -> PathBuf {
    let _guard = BUILD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(wasm_dir);
    let shared_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture-wasm");
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        // The same shared build directory every wasm test fixture in this
        // workspace already uses — see that module's own docs for why.
        .env("CARGO_TARGET_DIR", &shared_target)
        .current_dir(&crate_dir)
        .status()
        .unwrap_or_else(|error| {
            panic!("spawning `cargo build` for {wasm_dir} must succeed: {error}")
        });
    assert!(status.success(), "{wasm_dir} failed to build");

    let wasm_path = shared_target
        .join("wasm32-wasip2/release")
        .join(format!("{binary_stem}.wasm"));
    assert!(
        wasm_path.is_file(),
        "expected {} to exist after building {wasm_dir}",
        wasm_path.display()
    );
    wasm_path
}

/// [`build_okx_venue_component`] for `wasm/`'s spot component specifically
/// — kept as its own function since every existing caller already names
/// it, rather than every one of them spelling out `"wasm"`/`"okx_venue"`.
///
/// # Panics
/// See [`build_okx_venue_component`].
pub(crate) fn build_okx_venue() -> PathBuf {
    build_okx_venue_component("wasm", "okx_venue")
}
