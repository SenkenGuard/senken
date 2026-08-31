//! Authorisation as a pure domain.
//!
//! This crate answers exactly one question — *may this actor perform this
//! action on this resource, and if so, how far does it reach* — and answers
//! it with **no I/O, no HTTP, no database, and no async**. A headless
//! backtest running as a user needs the same answer an HTTP handler does,
//! and has no runtime to ask (layer ownership Part C): a rule belongs in the lowest layer that can express it
//! completely, and authorisation is expressible with nothing more than
//! plain values.
//!
//! # Permissions are code, roles are data
//!
//! [`Action`], [`Resource`] and [`Scope`] are fixed by this crate and
//! checked by the compiler. [`Role`] is a named, runtime-editable set of
//! [`Grant`]s built from those fixed primitives — a superadmin can build a
//! "Charts Only" role without a deploy, but cannot invent a permission no
//! code checks, because there is no way to spell one: a role is a
//! collection of `(Action, Resource, Scope)` triples, not free text.
//!
//! # Forgetting a check must not compile
//!
//! [`decide`] is the only function in this crate that produces a
//! [`Decision`], and [`Decision`] is the only way to learn a [`Scope`] — its
//! fields are private and it has no public constructor. A caller cannot
//! query "what can this actor see" without calling `decide`, and cannot
//! call `decide` without supplying an [`Actor`], an [`Action`] and a
//! [`Resource`]. That closes the *forgetting-to-check* half of B7.
//!
//! The *forgetting-to-authorise-a-new-resource* half is closed by
//! `decide`'s `match` over every [`Resource`] variant, written with no
//! wildcard arm (see that function's source, `src/decision.rs`). `Resource`
//! is deliberately **not** `#[non_exhaustive]`: it is a closed set specific
//! to this crate, so the only way to add a variant is to edit this crate
//! directly — and doing so breaks `decide`'s match until the new variant is
//! handled. This was verified by experiment, not merely asserted: adding an
//! extra unit variant to `Resource` and running `cargo build` fails with
//! `error[E0004]: non-exhaustive patterns` pointing at `decide`'s `match`,
//! naming the new variant as unhandled; removing the addition restores a
//! clean build. `decide_is_exhaustive_over_every_resource_variant` in
//! `src/decision.rs` keeps a regression guard for the same property day to
//! day, since the compiler experiment itself leaves no trace in the tree.
//!
//! # Scope is data the caller must apply
//!
//! [`Decision::scope`] returns the [`Scope`] a storage layer must turn into
//! a `WHERE` clause, never a hint to filter results after fetching them —
//! post-fetch filtering still leaks existence through totals and
//! pagination. This crate does not build SQL (it has no database
//! dependency to build it with); it hands back the scope a query layer is
//! obligated to apply.
//!
//! # Plugin permissions: register, never grant
//!
//! [`PluginPermissionName`] and [`PluginNamespace`] model the namespaced
//! permission scheme (`<plugin-id>.<resource>:<operation>`) plugins declare
//! in their manifest. A [`PluginNamespace`] can only *name* permissions
//! within its own subtree ([`PluginNamespace::declare`],
//! [`PluginNamespace::admit`]) — this crate has no function anywhere that
//! turns a plugin permission into a [`Grant`], attaches one to a [`Role`]
//! or an [`Actor`], or otherwise reaches [`decide`]. A plugin is handed a
//! `PluginNamespace` and nothing else, the same capability-shaped design as
//! `senken_plugin::ActivationContext`: what it cannot reach,
//! it cannot do. See `src/plugin_permission.rs`'s module docs for the full
//! reasoning, and [`PluginPermissionState`]/[`PluginPermissionRecord`] for
//! the orphan state a permission enters when its plugin stops declaring it
//! while a role still references it.

mod action;
mod actor;
mod decision;
mod grant;
mod plugin_permission;
mod resource;
mod role;
mod scope;

pub use crate::action::Action;
pub use crate::actor::Actor;
pub use crate::decision::{Decision, decide};
pub use crate::grant::Grant;
pub use crate::plugin_permission::{
    NAMESPACE_SEPARATOR, OPERATION_SEPARATOR, PluginNamespace, PluginPermissionError,
    PluginPermissionName, PluginPermissionRecord, PluginPermissionState,
};
pub use crate::resource::Resource;
pub use crate::role::Role;
pub use crate::scope::Scope;
