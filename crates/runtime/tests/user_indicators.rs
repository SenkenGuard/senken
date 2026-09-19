//! Proves `senken_runtime::user_indicators::UserIndicators` against a real
//! compiled `.wasm` component — the same `dyn-ema` fixture
//! `dynamic_indicators.rs` uses — rather than a description of what one
//! would do.

mod support;

use senken_identity::UserId;
use senken_plugin_host::{PluginHost, PluginLimits};
use senken_runtime::user_indicators::UserIndicators;

fn ema_fixture() -> Vec<u8> {
    std::fs::read(support::build_fixture("dyn-ema")).unwrap()
}

#[test]
fn a_users_compiled_indicator_is_in_their_catalog_and_not_in_another_users() {
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let registry = UserIndicators::new(host);
    let alice = UserId::new();
    let bob = UserId::new();
    let wasm = ema_fixture();

    // The fixture's own `descriptor().id` is `DynEma` — `load` must still
    // key this account's catalog by the name given here, not that
    // self-reported id, which is the whole property this test exists to
    // pin down.
    registry.load(alice, "my/ema-clone", &wasm).unwrap();

    let alice_catalog = registry.catalog(alice);
    assert_eq!(alice_catalog.len(), 1);
    assert_eq!(alice_catalog[0].id, "my/ema-clone");

    assert!(registry.catalog(bob).is_empty(), "bob has compiled nothing");
    assert!(
        registry
            .spawn(bob, "my/ema-clone", r#"{"period":5}"#)
            .is_err(),
        "alice's indicator must not be reachable through bob's own catalog"
    );
    assert!(
        registry
            .spawn(alice, "my/ema-clone", r#"{"period":5}"#)
            .is_ok()
    );
}

#[test]
fn unloading_removes_it_from_the_catalog() {
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let registry = UserIndicators::new(host);
    let alice = UserId::new();
    let wasm = ema_fixture();

    registry.load(alice, "my/ema-clone", &wasm).unwrap();
    assert_eq!(registry.catalog(alice).len(), 1);

    registry.unload(alice, "my/ema-clone");
    assert!(registry.catalog(alice).is_empty());
}
