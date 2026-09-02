//! Wraps a hand-encoded core WebAssembly module — the flat-numeric ABI
//! described in this crate's `README.md` — into a genuine component
//! against `wit/senken.wit`'s `compiled-indicator` world.
//!
//! # Why `compiled-indicator`, not `indicator-plugin`
//!
//! `wit/senken.wit`'s `indicator-plugin` world — the one a Rust-authored
//! plugin implements — exports the `indicator` interface: a descriptor
//! carrying strings and lists, and a resource (`instance`) with
//! canonical-ABI lifecycle methods. None of that is reachable from
//! indicator-lang source — the language has no way to name a title,
//! declare a configurable parameter, or otherwise produce a string or a
//! list. Targeting that world directly would mean writing, by hand, the
//! canonical-ABI glue `wit-bindgen` normally generates for records, lists,
//! strings and resources — real engineering, but glue that is identical
//! for *every* compiled program and has nothing to do with this one
//! program's arithmetic. It belongs with whatever loads a compiled program
//! into the full plugin world, not with the compiler that produces it.
//!
//! `compiled-indicator`, defined alongside `indicator-plugin` in the same
//! file, is the boundary this compiler actually needs: scalars and small
//! fixed-size tuples only, so every value at the boundary either fits in
//! the canonical ABI's sixteen flat parameters and one flat result
//! directly, or — for the three built-ins that report more than one
//! number — spills through a small, fixed-size, per-call scratch buffer
//! that is reset at the start of every bar. There is still no string, list
//! or resource anywhere in it. It imports the very same `builtins`
//! interface `indicator-plugin` does, so a compiled indicator-lang program
//! and a Rust-authored plugin agree on exactly one definition of what
//! calling a built-in means.

pub(crate) mod component;
pub(crate) mod module;
