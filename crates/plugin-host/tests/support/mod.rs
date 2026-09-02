//! Compiles the deliberately broken fixtures under `tests/fixtures/*` to
//! real `wasm32-wasip2` components on demand, so this crate's tests prove
//! the contract against genuine compiled bytes rather than a description of
//! what a broken plugin would do.
//!
//! Every fixture is its own tiny, workspace-isolated crate (see any
//! `tests/fixtures/*/Cargo.toml` for why) built with a plain `cargo build`
//! subprocess. Builds are serialized behind one process-wide lock: this
//! machine must never run two Rust builds at once, and `cargo test`'s
//! default parallel test harness would otherwise start one `cargo build`
//! per fixture-dependent test function at the same time.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Builds the fixture crate named `fixture-{name}` under `tests/fixtures/`
/// and returns the path to its compiled component.
///
/// # Panics
/// If the fixture fails to build — a build failure here is a bug in this
/// crate's own test fixtures, never something a test is exercising.
pub(crate) fn build_fixture(name: &str) -> PathBuf {
    let _guard = BUILD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--target", "wasm32-wasip2"])
        .current_dir(&fixture_dir)
        .status()
        .expect("spawning `cargo build` for a test fixture must succeed");
    assert!(status.success(), "fixture `{name}` failed to build");

    let binary_name = format!("fixture_{}.wasm", name.replace('-', "_"));
    let wasm_path = fixture_dir
        .join("target/wasm32-wasip2/debug")
        .join(&binary_name);
    assert!(
        wasm_path.is_file(),
        "expected {} to exist after building fixture `{name}`",
        wasm_path.display()
    );
    wasm_path
}
