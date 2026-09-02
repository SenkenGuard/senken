//! Proves `wit/senken.wit` is host-compilable, not only guest-compilable.
//!
//! The runtime that actually loads a compiled plugin (`crates/plugin-host`)
//! will call `wasmtime::component::bindgen!` against this same file to get
//! its own side of the ABI; this test calls it here, once, so a WIT change
//! that only the guest side (`wit_bindgen::generate!` in `src/lib.rs`)
//! happens to tolerate cannot land silently. `wasmtime` stays a
//! dev-dependency for exactly this reason — see this crate's `Cargo.toml`.

// Private, so the macro's undocumented output never reaches a public
// surface and `missing_docs` has nothing to say about it.
mod host {
    wasmtime::component::bindgen!({
        path: "../../wit/senken.wit",
        world: "indicator-plugin",
    });
}

/// `host::IndicatorPlugin` is the world-level binding the real runtime
/// instantiates a compiled component against. Naming it in a function
/// signature, compiled against this file's real `wasmtime` dependency, is
/// the actual proof this test exists for: if the WIT stopped being
/// host-compilable, this file would fail to compile before the test below
/// ever ran.
fn accepts_the_generated_world_type(_: &host::IndicatorPlugin) {}

#[test]
fn host_bindings_generate_and_a_component_model_engine_accepts_them() {
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config).expect("component-model engine constructs");
    let linker = wasmtime::component::Linker::<()>::new(&engine);

    let _ = accepts_the_generated_world_type;
    let _ = (engine, linker);
}
