//! Proves `RuntimeBuilder::build` performs the two data-directory startup
//! scans this crate promises:
//!
//! - a `.wasm` file dropped by hand under `<data_dir>/indicator-plugins/`
//!   is registered as a `PluginOrigin::DataDirectory` dynamic indicator by
//!   the time `build()` returns, and a broken one is recorded as a visible
//!   failed entry rather than aborting startup;
//! - the widget-plugin package store is opened (its `packages/` directory
//!   created) so a package dropped by hand under
//!   `<data_dir>/widget-plugins/packages/` is picked up the moment anything
//!   calls `list`/`refresh` on `Runtime::widget_plugins()`.

mod support;

use senken_runtime::Runtime;
use senken_runtime::plugin_host::{DynamicIndicatorState, PluginOrigin};

#[test]
fn a_wasm_file_dropped_under_the_data_directory_is_registered_at_startup() {
    let dir = tempfile::TempDir::new().unwrap();
    let wasm = std::fs::read(support::build_fixture("dyn-ema")).unwrap();
    let plugin_dir = dir.path().join("indicator-plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("dyn-ema.wasm"), &wasm).unwrap();

    let runtime = Runtime::builder().data_dir(dir.path()).build().unwrap();

    let status = runtime
        .dynamic_indicators()
        .all()
        .into_iter()
        .find(|status| status.id == "DynEma")
        .expect("the data-directory plugin must be registered by the time build() returns");
    assert_eq!(status.origin, PluginOrigin::DataDirectory);
    assert_eq!(status.state, DynamicIndicatorState::Active);

    runtime.shutdown().unwrap();
}

/// The property `AGENTS.md` names directly: "kegagalan memuat satu plugin
/// tidak boleh menjatuhkan startup — ia jadi satu entri gagal yang
/// terlihat." A garbage `.wasm` file under the data directory must still
/// let the runtime start, with the failure recorded and visible through
/// `dynamic_indicators().all()` rather than swallowed or fatal.
#[test]
fn a_broken_wasm_file_under_the_data_directory_does_not_fail_startup_and_is_recorded() {
    let dir = tempfile::TempDir::new().unwrap();
    let plugin_dir = dir.path().join("indicator-plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("broken.wasm"), b"not a real component").unwrap();

    let runtime = Runtime::builder()
        .data_dir(dir.path())
        .build()
        .expect("one broken data-directory plugin must never fail the whole startup");

    let statuses = runtime.dynamic_indicators().all();
    assert_eq!(
        statuses.len(),
        1,
        "the broken entry must still be recorded, visibly, not silently dropped"
    );
    assert!(matches!(
        statuses[0].state,
        DynamicIndicatorState::FailedToLoad { .. }
    ));
    assert_eq!(statuses[0].origin, PluginOrigin::DataDirectory);

    runtime.shutdown().unwrap();
}

#[test]
fn the_widget_plugin_store_is_opened_at_startup_with_its_directory_ready() {
    let dir = tempfile::TempDir::new().unwrap();

    let runtime = Runtime::builder().data_dir(dir.path()).build().unwrap();

    assert!(
        dir.path().join("widget-plugins").join("packages").is_dir(),
        "the package directory must already exist so a manual drop-in has somewhere to land"
    );
    assert!(runtime.widget_plugins().list().unwrap().is_empty());

    runtime.shutdown().unwrap();
}
