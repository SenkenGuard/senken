//! Emits the core WebAssembly module for a checked program: the
//! flat-numeric ABI side of `wit/senken.wit`'s `compiled-indicator`
//! world, proven to wrap into a real component in
//! [`super::component::tests`] before any of this was written.
//!
//! # The one piece of state this module owns
//!
//! Every built-in's actual state (an `Ema`'s running average, a `Macd`'s
//! three internal `Ema`s, and so on) lives on the host, keyed by the slot
//! `crate::typeck` assigned each call site — see `wit/senken.wit`. The
//! only thing a compiled program's own linear memory ever holds is a few
//! bytes of scratch space to receive a multi-valued built-in's result
//! through the return pointer the canonical ABI requires once a result
//! does not fit in one flat value (`stochastic`, `macd`, `bollinger`), and
//! that space is reused, not accumulated: one shared pointer local, reset
//! to the same base offset at the start of every `on-bar` call, exactly as
//! proven in [`super::component::tests::multi_value_import_round_trips_through_a_return_pointer`].
//! A program that never calls one of those three built-ins needs no
//! memory at all, and gets none.

use std::collections::HashMap;

use wasm_encoder::{
    CodeSection, ConstExpr, EntityType, ExportKind, ExportSection, Function, FunctionSection,
    GlobalSection, GlobalType, ImportSection, MemArg, MemorySection, MemoryType, Module,
    TypeSection, ValType,
};

use crate::builtins::{BUILTINS, Builtin, ParamKind, ResultShape};
use crate::typeck::{Checked, CheckedArg, CheckedCall, CheckedExpr, CheckedLet};

/// The interface name and version `wit/senken.wit` declares for
/// `builtins` — matched exactly against what `wit-component` expects an
/// import from that interface to be named. Shared with `indicator-plugin`,
/// the world a Rust-authored plugin implements: there is exactly one
/// `builtins` interface in this workspace, not a copy per compiler.
const BUILTINS_INTERFACE: &str = "senken:plugin-api/builtins@0.1.0";

/// Where the bump allocator's free pointer resets to at the start of every
/// `on-bar` call. Nothing else in this module's memory is ever addressed,
/// so any small offset would do; `8` just avoids the null address.
const SCRATCH_BASE: i32 = 8;

/// The type index `emit_types_and_imports` always assigns `on-bar`'s own
/// shape.
const ON_BAR_TYPE: u32 = 0;

/// The type index `emit_types_and_imports` always assigns `cabi_realloc`'s
/// shape.
const REALLOC_TYPE: u32 = 1;

/// Emits the core module for `checked`.
pub(crate) fn emit(checked: &Checked) -> Vec<u8> {
    let used = used_builtins(checked);
    let needs_scratch = used
        .iter()
        .any(|b| matches!(b.result, ResultShape::Compound(_)));

    let mut module = Module::new();

    let (builtin_func_index, mut next_func) = emit_types_and_imports(&mut module, &used);

    // --- Functions ---------------------------------------------------------
    let mut functions = FunctionSection::new();
    let realloc_func_index = if needs_scratch {
        let index = next_func;
        functions.function(REALLOC_TYPE);
        next_func += 1;
        Some(index)
    } else {
        None
    };
    let on_bar_func_index = next_func;
    functions.function(ON_BAR_TYPE);
    module.section(&functions);

    // --- Memory and the bump pointer -------------------------------------
    if needs_scratch {
        let mut memory = MemorySection::new();
        memory.memory(MemoryType {
            minimum: 1,
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memory);

        let mut globals = GlobalSection::new();
        globals.global(
            GlobalType {
                val_type: ValType::I32,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i32_const(SCRATCH_BASE),
        );
        module.section(&globals);
    }

    // --- Exports ---------------------------------------------------------
    let mut exports = ExportSection::new();
    if needs_scratch {
        exports.export("memory", ExportKind::Memory, 0);
        exports.export(
            "cabi_realloc",
            ExportKind::Func,
            realloc_func_index.unwrap(),
        );
    }
    exports.export("on-bar", ExportKind::Func, on_bar_func_index);
    module.section(&exports);

    // --- Code ---------------------------------------------------------
    let mut code = CodeSection::new();

    if needs_scratch {
        code.function(&emit_realloc());
    }

    let ctx = Ctx {
        builtin_func_index: &builtin_func_index,
        realloc_func_index,
        ret_ptr_local: if needs_scratch {
            Some(5 + checked.let_count)
        } else {
            None
        },
    };
    code.function(&emit_on_bar(checked, &ctx));
    module.section(&code);

    module.finish()
}

/// Emits the type and import sections: one function type per fixed shape
/// (`on-bar`, `cabi_realloc`) plus one per built-in `used` actually calls,
/// then an import for each of those built-ins. Only the built-ins this
/// specific program calls are imported — an unused one never appears in
/// this module's import section at all, so a program that never calls
/// e.g. `macd` places no requirement on whoever instantiates it to supply
/// one.
///
/// Returns each imported built-in's function index (imports always occupy
/// the start of the function index space) and the next free function
/// index, for [`emit`] to hand out to `cabi_realloc` and `on-bar`.
fn emit_types_and_imports(
    module: &mut Module,
    used: &[&'static Builtin],
) -> (HashMap<&'static str, u32>, u32) {
    let mut types = TypeSection::new();
    let on_bar_params = [
        ValType::F64,
        ValType::F64,
        ValType::F64,
        ValType::F64,
        ValType::F64,
    ];
    types.ty().function(on_bar_params, [ValType::F64]);
    types.ty().function(
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32],
    );
    // Built-in types start after the two fixed ones above.
    let mut builtin_types = HashMap::new();
    for (offset, builtin) in used.iter().enumerate() {
        let (params, results) = wasm_signature(builtin);
        types.ty().function(params, results);
        let type_index = 2 + u32::try_from(offset).expect("far fewer built-ins than u32::MAX");
        builtin_types.insert(builtin.host_fn, type_index);
    }
    module.section(&types);

    let mut imports = ImportSection::new();
    let mut builtin_func_index = HashMap::new();
    for (index, builtin) in used.iter().enumerate() {
        imports.import(
            BUILTINS_INTERFACE,
            builtin.host_fn,
            EntityType::Function(builtin_types[builtin.host_fn]),
        );
        let func_index = u32::try_from(index).expect("far fewer built-ins than u32::MAX");
        builtin_func_index.insert(builtin.host_fn, func_index);
    }
    let next_func = u32::try_from(used.len()).expect("far fewer built-ins than u32::MAX");
    module.section(&imports);

    (builtin_func_index, next_func)
}

/// Everything [`compile_expr`] needs to know about the module it is
/// emitting into, threaded through instead of recomputed at every call.
struct Ctx<'a> {
    builtin_func_index: &'a HashMap<&'static str, u32>,
    realloc_func_index: Option<u32>,
    ret_ptr_local: Option<u32>,
}

/// A bump allocator that never frees. It is safe for this to never free
/// because nothing it ever hands out needs to outlive the `on-bar` call
/// that requested it — every allocation is scratch space for one
/// multi-valued built-in's result, read back into an `f64` on the wasm
/// value stack before anything else runs. `on-bar` resets the pointer this
/// reads and writes back to `SCRATCH_BASE` on every call (see
/// [`emit_on_bar`]), which is what keeps a session's worth of bars from
/// growing this without bound.
///
/// Params: `0=old_ptr 1=old_size 2=align 3=new_size`. All but `new_size`
/// are ignored — nothing is ever resized or freed.
fn emit_realloc() -> Function {
    let mut function = Function::new([]);
    function
        .instructions()
        .global_get(0)
        .local_get(3)
        .global_get(0)
        .i32_add()
        .global_set(0)
        .end();
    function
}

fn emit_on_bar(checked: &Checked, ctx: &Ctx<'_>) -> Function {
    let mut locals = Vec::new();
    if checked.let_count > 0 {
        locals.push((checked.let_count, ValType::F64));
    }
    if ctx.ret_ptr_local.is_some() {
        locals.push((1, ValType::I32));
    }
    let mut function = Function::new(locals);

    if ctx.ret_ptr_local.is_some() {
        function
            .instructions()
            .i32_const(SCRATCH_BASE)
            .global_set(0);
    }

    for CheckedLet { local, value } in &checked.lets {
        compile_expr(&mut function.instructions(), value, ctx);
        function.instructions().local_set(*local);
    }

    compile_expr(&mut function.instructions(), &checked.plot, ctx);
    function.instructions().end();
    function
}

/// Emits code that leaves exactly one `f64` on the value stack: `expr`'s
/// value.
fn compile_expr(ins: &mut wasm_encoder::InstructionSink<'_>, expr: &CheckedExpr, ctx: &Ctx<'_>) {
    match expr {
        CheckedExpr::BarField(field) => {
            ins.local_get(field.param_index());
        }
        CheckedExpr::Local(index) => {
            ins.local_get(*index);
        }
        CheckedExpr::Number(value) => {
            ins.f64_const((*value).into());
        }
        CheckedExpr::Unary(crate::ast::UnaryOp::Neg, operand) => {
            compile_expr(ins, operand, ctx);
            ins.f64_neg();
        }
        CheckedExpr::Binary(op, left, right) => {
            compile_expr(ins, left, ctx);
            compile_expr(ins, right, ctx);
            match op {
                crate::ast::BinaryOp::Add => ins.f64_add(),
                crate::ast::BinaryOp::Sub => ins.f64_sub(),
                crate::ast::BinaryOp::Mul => ins.f64_mul(),
                crate::ast::BinaryOp::Div => ins.f64_div(),
            };
        }
        CheckedExpr::Call(call) => {
            assert!(
                matches!(call.builtin.result, ResultShape::Scalar),
                "crate::typeck never lets a bare compound call reach codegen"
            );
            push_call_args(ins, call, ctx);
            ins.call(ctx.builtin_func_index[call.builtin.host_fn]);
        }
        CheckedExpr::Field(call, field_index) => {
            let ResultShape::Compound(fields) = call.builtin.result else {
                unreachable!("crate::typeck only produces Field for a Compound-result call")
            };
            let ret_ptr = ctx
                .ret_ptr_local
                .expect("a compound call always reserves the scratch local");
            let realloc = ctx
                .realloc_func_index
                .expect("a compound call always needs the bump allocator");
            let byte_size = i32::try_from(fields.len() * 8)
                .expect("a built-in reports at most a handful of values");

            // Allocate scratch space for the whole tuple, once, then read
            // back only the one field this expression asked for.
            ins.i32_const(0) // old_ptr
                .i32_const(0) // old_size
                .i32_const(8) // align
                .i32_const(byte_size) // new_size
                .call(realloc)
                .local_set(ret_ptr);

            push_call_args(ins, call, ctx);
            ins.local_get(ret_ptr);
            ins.call(ctx.builtin_func_index[call.builtin.host_fn]);

            let offset = u64::try_from(field_index * 8).expect("small, fixed field count");
            ins.local_get(ret_ptr);
            ins.f64_load(MemArg {
                offset,
                align: 3,
                memory_index: 0,
            });
        }
    }
}

/// Pushes `slot` followed by every argument, in the built-in's own
/// declared order — everything a call needs except the trailing return
/// pointer a compound result additionally takes (pushed by the caller,
/// [`compile_expr`]'s `Field` arm, since a scalar call has none).
fn push_call_args(ins: &mut wasm_encoder::InstructionSink<'_>, call: &CheckedCall, ctx: &Ctx<'_>) {
    ins.i32_const(i32::try_from(call.slot).expect("far fewer call sites than i32::MAX"));
    for arg in &call.args {
        match arg {
            CheckedArg::Series(expr) => compile_expr(ins, expr, ctx),
            CheckedArg::Period(value) => {
                ins.i32_const(i32::try_from(*value).expect("checked as u32 in crate::typeck"));
            }
            CheckedArg::Number(value) => {
                ins.f64_const((*value).into());
            }
        }
    }
    // Bar fields the built-in's own `handle_bar` reads that a trader never
    // writes — see `crate::builtins::ImplicitArg`. Always the *current*
    // bar's own field, so read straight from `on-bar`'s own parameters
    // rather than through anything a `let` could have shadowed.
    for field in call.builtin.implicit {
        ins.local_get(field.param_index());
    }
}

/// The lowered core wasm signature for calling `builtin` from the guest
/// side: `u32` and `f64` parameters flatten straight through, and a
/// result of more than one flat value becomes a trailing pointer
/// parameter with no results of its own — the same rule proven against
/// `wit-component`'s own validator in
/// [`super::component::tests::multi_value_import_round_trips_through_a_return_pointer`].
fn wasm_signature(builtin: &Builtin) -> (Vec<ValType>, Vec<ValType>) {
    let mut params = vec![ValType::I32]; // slot
    for param in builtin.params {
        params.push(match param {
            ParamKind::Series | ParamKind::Number => ValType::F64,
            ParamKind::Period => ValType::I32,
        });
    }
    // Implicit bar fields are always `f64` — see `ImplicitArg`.
    params.extend(builtin.implicit.iter().map(|_| ValType::F64));
    match builtin.result {
        ResultShape::Scalar => (params, vec![ValType::F64]),
        ResultShape::Compound(_) => {
            params.push(ValType::I32); // return pointer
            (params, vec![])
        }
    }
}

/// Walks `checked` to find every built-in it actually calls, in
/// `BUILTINS`' own fixed order — never a hash-set's iteration order —
/// so the module this emits is byte-for-byte identical across runs of the
/// same source, which is what makes a source-addressed registry of
/// compiled artifacts trustworthy at all.
fn used_builtins(checked: &Checked) -> Vec<&'static Builtin> {
    let mut names = std::collections::HashSet::new();
    for let_stmt in &checked.lets {
        collect_used(&let_stmt.value, &mut names);
    }
    collect_used(&checked.plot, &mut names);
    // `cabi_realloc` is not a `builtins` entry, so it never lands in
    // `names`; whether it is needed is decided in `emit` from whether any
    // collected built-in has a compound result.
    BUILTINS.iter().filter(|b| names.contains(b.name)).collect()
}

fn collect_used(expr: &CheckedExpr, names: &mut std::collections::HashSet<&'static str>) {
    match expr {
        CheckedExpr::BarField(_) | CheckedExpr::Local(_) | CheckedExpr::Number(_) => {}
        CheckedExpr::Unary(_, operand) => collect_used(operand, names),
        CheckedExpr::Binary(_, left, right) => {
            collect_used(left, names);
            collect_used(right, names);
        }
        CheckedExpr::Call(call) | CheckedExpr::Field(call, _) => collect_used_call(call, names),
    }
}

fn collect_used_call(call: &CheckedCall, names: &mut std::collections::HashSet<&'static str>) {
    names.insert(call.builtin.name);
    for arg in &call.args {
        if let CheckedArg::Series(expr) = arg {
            collect_used(expr, names);
        }
    }
}
