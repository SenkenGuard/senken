//! Two unrelated things share this crate for now: [`RegistryStore`], a
//! public registry for publishing, searching and installing indicator
//! source across accounts (currently unmounted — see its own module
//! docs), and [`UserIndicatorStore`], the guarded store behind an
//! account's own compiled Rust indicators, which is what
//! `user_indicator_handlers` actually serves. They stay in one crate
//! because both are "indicators a database row identifies by owner and
//! name" — the same reasoning that keeps this workspace's other
//! guarded-query crates (`senken-notes`, `senken-watchlist`) narrow rather
//! than one per table.
//!
//! # One database, one schema-version owner
//!
//! Every table both stores use references `users(id)`, so they live in
//! the same SQLite file `senken-identity` already owns at
//! `.data/accounts/` — not a second database this crate would have to
//! keep referentially consistent with the first by hand. `senken-identity`
//! stays the file's single owner of `PRAGMA user_version`, creating every
//! table in its own schema module even though it never queries any of
//! them; this crate never opens its own connection, only a clone of that
//! store's connection via [`RegistryStore::new`]/[`UserIndicatorStore::new`]
//! -> [`senken_identity::IdentityStore::shared_connection`]. See
//! `senken-chart`'s module docs for the full reasoning behind this shape —
//! the same trade is made here for the same reasons.
//!
//! # What is out of scope here
//!
//! Signing and a trust root, moderation, and ratings/reviews were never
//! this crate's job even when the registry was active. This crate also
//! keeps no version history per indicator: publishing again under a name
//! you already own replaces that entry's source in place.

mod error;
mod id;
mod store;
mod user_error;
mod user_store;
mod version;

pub use crate::error::RegistryError;
pub use crate::id::{IndicatorEntryId, UserIndicatorId};
pub use crate::store::{IndicatorEntry, IndicatorSummary, InstalledIndicator, RegistryStore};
pub use crate::user_error::UserIndicatorError;
pub use crate::user_store::{
    CompileOutcome, CompiledUserIndicator, UserIndicator, UserIndicatorStore, UserIndicatorSummary,
};
pub use crate::version::HOST_LANGUAGE_VERSION;

// Re-exported for convenience: `list_mine`/`search` return
// `senken_identity::Page<T>`, the exact same paginated-result shape every
// other guarded listing in this workspace returns.
pub use senken_identity::Page;
