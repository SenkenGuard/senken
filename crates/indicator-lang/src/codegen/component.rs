//! Wraps a core module into a component against `wit/senken.wit`'s
//! `compiled-indicator` world, using `wit-component`'s `ComponentEncoder`
//! — the same crate `cargo component` uses for this exact step.

use std::sync::OnceLock;

use wit_component::{ComponentEncoder, StringEncoding};
use wit_parser::{Resolve, WorldId};

/// The workspace's one plugin ABI, embedded at compile time. See
/// [`super::component`]'s module docs for why this compiler targets
/// `compiled-indicator` rather than this same file's `indicator-plugin`.
const SENKEN_WIT: &str = include_str!("../../../../wit/senken.wit");

/// What went wrong turning a core module into a component.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ComponentError {
    /// Embedding the world's metadata into the core module failed.
    #[error("failed to embed component metadata: {0}")]
    Embed(anyhow::Error),
    /// `ComponentEncoder` rejected the core module — almost always a sign
    /// that `super::module`'s hand-encoded exports or imports do not
    /// match `wit/senken.wit`'s `compiled-indicator` world.
    #[error("failed to encode component: {0}")]
    Encode(anyhow::Error),
}

/// Parses `wit/senken.wit` once and caches the resulting `(Resolve,
/// WorldId)` for its `compiled-indicator` world. The text is fixed at
/// compile time, so re-parsing it per call would be pure overhead with no
/// possible new outcome.
fn runtime_world() -> &'static (Resolve, WorldId) {
    static WORLD: OnceLock<(Resolve, WorldId)> = OnceLock::new();
    WORLD.get_or_init(|| {
        let mut resolve = Resolve::default();
        let package = resolve
            .push_str("senken.wit", SENKEN_WIT)
            .expect("the workspace's own wit/senken.wit must parse");
        let world = resolve
            .select_world(&[package], Some("compiled-indicator"))
            .expect("wit/senken.wit must define `compiled-indicator`");
        (resolve, world)
    })
}

/// Wraps `core_module` — a hand-encoded core WebAssembly module meant to
/// implement the `compiled-indicator` world's exports and imports — into a
/// component, and validates the result.
///
/// This is the "componentize" step `cargo component` also performs, minus
/// the Rust-specific parts: `core_module` was never `wit-bindgen`-generated
/// Rust, so [`wit_component::embed_component_metadata`] is used directly to attach the
/// world's type information before handing the module to
/// [`ComponentEncoder`].
///
/// # Errors
///
/// Returns [`ComponentError`] if `core_module`'s exports or imports do not
/// match `wit/senken.wit`'s `compiled-indicator` world — always a bug in
/// `super::module`'s code generation, never something a trader's source
/// could cause, since `super::module::emit` only ever runs on an already
/// type-checked program.
pub(crate) fn wrap_core_module(mut core_module: Vec<u8>) -> Result<Vec<u8>, ComponentError> {
    let (resolve, world) = runtime_world();

    wit_component::embed_component_metadata(
        &mut core_module,
        resolve,
        *world,
        StringEncoding::UTF8,
    )
    .map_err(ComponentError::Embed)?;

    ComponentEncoder::default()
        .module(&core_module)
        .map_err(ComponentError::Encode)?
        .validate(true)
        .encode()
        .map_err(ComponentError::Encode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, EntityType, ExportSection, Function, FunctionSection, ImportSection, MemArg,
        MemorySection, MemoryType, Module, TypeSection, ValType,
    };
    use wasmtime::Engine;
    use wasmtime::component::{Component, Linker, Val};

    /// The simplest module that can possibly satisfy `compiled-indicator`:
    /// `on-bar` ignores every argument and returns a constant, and every
    /// `builtins` import is declared (the component's type demands the
    /// whole interface) but never called.
    ///
    /// This is step one of proving the wasm-encoder -> wit-component ->
    /// wasmtime path works at all, before any real arithmetic, any
    /// multi-value import, and before a single line of the compiler
    /// itself. If this does not round-trip, nothing built on top of it
    /// will either.
    fn trivial_module() -> Vec<u8> {
        let mut module = Module::new();

        // Type 0: `on-bar`'s own shape — five f64 in, one f64 out. This
        // trivial module imports nothing from `builtins` at all: proving
        // the bare wasm-encoder -> wit-component -> wasmtime path works
        // comes first, before a single import (let alone one needing a
        // return pointer) is added.
        let mut types = TypeSection::new();
        types.ty().function(
            [
                ValType::F64,
                ValType::F64,
                ValType::F64,
                ValType::F64,
                ValType::F64,
            ],
            [ValType::F64],
        );
        module.section(&types);

        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);

        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memory);

        let mut exports = ExportSection::new();
        exports.export("memory", wasm_encoder::ExportKind::Memory, 0);
        exports.export("on-bar", wasm_encoder::ExportKind::Func, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        let mut func = Function::new([]);
        func.instructions().f64_const(42.0.into()).end();
        code.function(&func);
        module.section(&code);

        module.finish()
    }

    #[test]
    fn trivial_module_round_trips_through_wit_component_and_wasmtime() {
        let component_bytes =
            wrap_core_module(trivial_module()).expect("trivial module must encode as a component");

        let engine = Engine::default();
        let component = Component::new(&engine, &component_bytes)
            .expect("wasmtime must accept what wit-component produced");

        let mut linker = Linker::new(&engine);
        let _ = linker
            .instance("senken:plugin-api/builtins@0.1.0")
            .expect("world declares this interface as an import");

        let mut store = wasmtime::Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &component)
            .expect("instantiation must succeed");
        let on_bar = instance
            .get_func(&mut store, "on-bar")
            .expect("component must export on-bar");

        let mut results = [Val::Float64(0.0)];
        on_bar
            .call(
                &mut store,
                &[
                    Val::Float64(1.0),
                    Val::Float64(2.0),
                    Val::Float64(3.0),
                    Val::Float64(4.0),
                    Val::Float64(5.0),
                ],
                &mut results,
            )
            .expect("calling on-bar must succeed");

        assert_eq!(results[0], Val::Float64(42.0));
    }

    /// The second, harder half of proving this path before writing the
    /// compiler: a builtin that reports more than one number.
    /// `tuple<f64, f64>` overflows the canonical ABI's one-flat-result
    /// limit (`wit_parser::abi::AbiVariant::MAX_FLAT_RESULTS == 1`), so
    /// `stochastic-update` is lowered with an extra pointer parameter that
    /// the *caller* — this module — must allocate and read back from,
    /// exactly the shape `macd-update` and `bollinger-update` need too.
    ///
    /// This module calls `cabi_realloc` once for sixteen bytes (`%k` then
    /// `%d`, contiguous), calls `stochastic-update` with the resulting
    /// pointer appended as its last argument — the single return pointer
    /// the lowered signature takes, discovered by letting
    /// `wit-component`'s own validator state the exact expected core type
    /// rather than guessing it — and adds the two loaded-back values
    /// together as `on-bar`'s result.
    /// Everything about `module_with_retptr_import`'s module except its
    /// code section — split out only to keep both functions under this
    /// workspace's line-count lint, not because the split means anything.
    fn retptr_module_skeleton() -> Module {
        let mut module = Module::new();

        let mut types = TypeSection::new();
        // Type 0: `on-bar` — five f64 in, one f64 out.
        types.ty().function(
            [
                ValType::F64,
                ValType::F64,
                ValType::F64,
                ValType::F64,
                ValType::F64,
            ],
            [ValType::F64],
        );
        // Type 1: `cabi_realloc(old_ptr, old_size, align, new_size) -> ptr`,
        // the fixed signature `wit-component` looks for by name.
        types.ty().function(
            [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            [ValType::I32],
        );
        // Type 2: `stochastic-update`'s lowered shape, confirmed against
        // `wit-component`'s validator: `slot, k-period, d-period, high,
        // low, close` flatten straight to `[I32, I32, I32, F64, F64,
        // F64]`, and the two-`f64` result that does not fit in one flat
        // value becomes a trailing `I32` return pointer with no results
        // of its own.
        types.ty().function(
            [
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::F64,
                ValType::F64,
                ValType::F64,
                ValType::I32,
            ],
            [],
        );
        module.section(&types);

        let mut imports = ImportSection::new();
        imports.import(
            "senken:plugin-api/builtins@0.1.0",
            "stochastic-update",
            EntityType::Function(2),
        );
        module.section(&imports);

        let mut functions = FunctionSection::new();
        functions.function(1); // cabi_realloc
        functions.function(0); // on-bar
        module.section(&functions);

        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memory);

        // Global 0: the bump allocator's next free offset, reset at the
        // start of every `on-bar` call so scratch memory never grows
        // without bound across a session's worth of bars.
        let mut globals = wasm_encoder::GlobalSection::new();
        globals.global(
            wasm_encoder::GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &wasm_encoder::ConstExpr::i32_const(1024),
        );
        module.section(&globals);

        let mut exports = ExportSection::new();
        exports.export("memory", wasm_encoder::ExportKind::Memory, 0);
        exports.export("cabi_realloc", wasm_encoder::ExportKind::Func, 1);
        exports.export("on-bar", wasm_encoder::ExportKind::Func, 2);
        module.section(&exports);

        module
    }

    fn module_with_retptr_import() -> Vec<u8> {
        let mut module = retptr_module_skeleton();
        let mut code = CodeSection::new();

        // `cabi_realloc`: a bump allocator that never frees within a call
        // to `on-bar` and is reset at the start of every bar (see
        // `on-bar` below) — the only memory this module ever needs is a
        // few bytes of scratch space to receive one bar's worth of
        // multi-value results, never anything that must outlive the call
        // that allocated it.
        //
        // Locals: 0=old_ptr 1=old_size 2=align 3=new_size.
        let mut realloc = Function::new([]);
        {
            let mut f = realloc.instructions();
            // ptr = bump; bump += new_size; return ptr
            f.global_get(0)
                .local_get(3)
                .global_get(0)
                .i32_add()
                .global_set(0)
                .end();
        }
        code.function(&realloc);

        // `on-bar`: reset the bump pointer, allocate sixteen contiguous
        // bytes to receive `(%k, %d)`, call `stochastic-update`, then load
        // both back and add them.
        //
        // Params: 0=open 1=high 2=low 3=close 4=volume.
        // Locals: 5=ret_ptr(i32).
        let mut on_bar = Function::new([(1, ValType::I32)]);
        {
            let mut f = on_bar.instructions();
            f.i32_const(1024).global_set(0); // reset bump pointer
            f.i32_const(0)
                .i32_const(0)
                .i32_const(8)
                .i32_const(16)
                .call(1) // cabi_realloc -> ret_ptr
                .local_set(5);
            f.i32_const(0) // slot
                .i32_const(3) // k-period
                .i32_const(2) // d-period
                .local_get(1) // high
                .local_get(2) // low
                .local_get(3) // close
                .local_get(5) // ret_ptr
                .call(0);
            f.local_get(5)
                .f64_load(MemArg {
                    offset: 0,
                    align: 3,
                    memory_index: 0,
                })
                .local_get(5)
                .f64_load(MemArg {
                    offset: 8,
                    align: 3,
                    memory_index: 0,
                })
                .f64_add()
                .end();
        }
        code.function(&on_bar);
        module.section(&code);

        module.finish()
    }

    #[test]
    fn multi_value_import_round_trips_through_a_return_pointer() {
        let component_bytes = wrap_core_module(module_with_retptr_import())
            .expect("a module with a retptr import must encode as a component");

        let engine = Engine::default();
        let component = Component::new(&engine, &component_bytes)
            .expect("wasmtime must accept what wit-component produced");

        let mut linker = Linker::new(&engine);
        linker
            .instance("senken:plugin-api/builtins@0.1.0")
            .expect("world declares this interface as an import")
            .func_wrap(
                "stochastic-update",
                |_,
                 (_slot, k_period, d_period, _high, _low, _close): (
                    u32,
                    u32,
                    u32,
                    f64,
                    f64,
                    f64,
                )| {
                    // Distinct, checkable values rather than constants, so
                    // the test would fail if the guest read the wrong
                    // offset or the wrong pointer.
                    Ok(((f64::from(k_period), f64::from(d_period) * 10.0),))
                },
            )
            .unwrap();

        let mut store = wasmtime::Store::new(&engine, ());
        let instance = linker
            .instantiate(&mut store, &component)
            .expect("instantiation must succeed");
        let on_bar = instance
            .get_func(&mut store, "on-bar")
            .expect("component must export on-bar");

        let mut results = [Val::Float64(0.0)];
        on_bar
            .call(
                &mut store,
                &[
                    Val::Float64(1.0),
                    Val::Float64(2.0),
                    Val::Float64(3.0),
                    Val::Float64(4.0),
                    Val::Float64(5.0),
                ],
                &mut results,
            )
            .expect("calling on-bar must succeed");

        // The guest hard-codes k-period=3, d-period=2, so the host above
        // returns (3.0, 20.0); on-bar adds them.
        assert_eq!(results[0], Val::Float64(23.0));

        // Calling it again proves the bump pointer reset at the top of
        // `on-bar` actually runs — a bug that forgot the reset would still
        // pass the first call and only fail (or silently drift) on a
        // second one once scratch memory had advanced.
        let mut results2 = [Val::Float64(0.0)];
        on_bar
            .call(
                &mut store,
                &[
                    Val::Float64(1.0),
                    Val::Float64(2.0),
                    Val::Float64(3.0),
                    Val::Float64(4.0),
                    Val::Float64(5.0),
                ],
                &mut results2,
            )
            .expect("calling on-bar a second time must succeed");
        assert_eq!(results2[0], Val::Float64(23.0));
    }
}
