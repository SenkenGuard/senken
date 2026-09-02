//! A registry for indicator-lang source: publish, search, install.
//!
//! # Source, never a binary
//!
//! This registry stores and serves **source code**, never a compiled
//! artifact. `senken_indicator_lang::compile` is called on the *installing*
//! machine, every time — at publish time as well, but only to validate that
//! what is being stored actually compiles, not to produce anything that is
//! ever loaded. This is not a convenience: it is what removes the
//! reproducible-build problem a binary-publishing registry cannot avoid.
//! If a publisher could upload compiled bytes alongside source, nothing
//! would prove the two match — a dishonest publisher could publish
//! innocuous source next to a malicious binary, and no reviewer could tell
//! without independently recompiling anyway. Publishing source means
//! **what you read is what you run**, because the machine installing it is
//! the machine that compiles it. It also means there is no build farm to
//! operate, secure, or trust, and every published artifact is a few
//! kilobytes of readable, forkable, reviewable text.
//!
//! # What the compiled-source model does *not* need to solve
//!
//! Binary provenance stops being a question at all. What remains — the
//! problem this crate actually has to solve — is **publisher identity and
//! naming**: on real registries that skipped this, attackers have
//! typosquatted or impersonated legitimate publishers to harvest
//! credentials from users who trusted a familiar-looking name. Two
//! defences close that:
//!
//! - **Every name is namespaced by its publishing account.** A published
//!   indicator's qualified name is `{namespace}/{name}`, where `namespace`
//!   is the publishing account's own id — never a self-chosen display
//!   string. Two authors may publish the same `name` in their own
//!   namespaces with no collision, because the stored uniqueness is
//!   `(namespace, name)`; and nobody may publish into another account's
//!   namespace, because [`RegistryStore::publish`] rejects a `namespace`
//!   argument that is not the caller's own `AuthenticatedUser::user_id`
//!   before anything else runs. The account id is what closes
//!   impersonation, and it stays what every stored row and every
//!   authorisation check reasons about — a [`Handle`] (below) never
//!   replaces it, only points at it.
//! - **A [`Handle`] is the address a human actually types**, because a
//!   `UserId` alone is safe and unusable at once: nobody types
//!   `@550e8400-e29b-41d4-a716-446655440000/supertrend`. A handle is
//!   [validated](Handle::new) (lowercase letters, digits and hyphens only)
//!   and globally unique — enforced by the same kind of database
//!   constraint that already makes `(namespace, name)` a structural
//!   guarantee rather than a convention — so it closes the same
//!   impersonation question a handle-only design would otherwise reopen:
//!   two accounts can never hold the same handle, so an installer typing
//!   `@alice/supertrend` reaches exactly one account, not whichever one
//!   most recently claimed the name. [`RegistryStore::publish`] refuses to
//!   run for an account that has not claimed one yet — see
//!   [`RegistryError::HandleNotSet`] — because a published entry nobody
//!   can address is not meaningfully published at all.
//! - **The indicator language's version is recorded on every publish**,
//!   and an installing host that is too old for what it fetches is
//!   refused with a message naming both versions — never a silent failure
//!   to load. See [`HOST_LANGUAGE_VERSION`] for exactly what "the
//!   language's version" means here and why.
//!
//! # Publishing needs an account; installing does not
//!
//! [`RegistryStore::publish`], [`RegistryStore::delete`] and
//! [`RegistryStore::list_mine`] take an
//! [`senken_identity::AuthenticatedUser`] and go through
//! [`senken_identity::AuthenticatedUser::authorize`], the same guarded
//! shape `senken_notes`/`senken_dashboard` use — `delete` additionally
//! reuses `publish`'s own `ensure_owns_namespace`, since revoking your own
//! published entry is the same identity fact publishing under your own
//! namespace already is, never a grant an admin's wider scope can extend
//! to someone else's entry. [`RegistryStore::search`],
//! [`RegistryStore::get`] and [`RegistryStore::install`] take none at
//! all — a published indicator is public, browsable and installable by
//! design, the same way this workspace already treats market data as
//! global with no owner to check a grant against. [`RegistryStore::set_handle`]/
//! [`RegistryStore::get_handle`] take a bare [`senken_identity::UserId`],
//! not an `AuthenticatedUser` — choosing your own address needs no grant,
//! the same reasoning `senken_identity::IdentityStore::set_zone` documents
//! for itself; this is safe only because every caller supplies a
//! `UserId` it already obtained from a resolved session, never one taken
//! from a request parameter naming someone else.
//!
//! A revoked entry does not reach back into anyone who already installed
//! it: [`RegistryStore::install`] copies the compiled bytes to the
//! installing machine, so [`RegistryStore::delete`] only ever removes the
//! ability to search for or newly install that entry, never bytes someone
//! else already holds.
//!
//! # What is out of scope here
//!
//! Signing and a trust root, moderation, and ratings/reviews are none of
//! this crate's job — the workspace's design record marks all three as
//! deliberately undecided, with the manifest and UI surface left open for
//! them to land later without a shape change. This crate also keeps no
//! version history per indicator: publishing again under a name you
//! already own replaces that entry's source in place, which is everything
//! "publish, search, install" needs.
//!
//! # One database, one schema-version owner
//!
//! Registry entries and registry handles both reference `users(id)`, so
//! their tables live in the same SQLite file `senken-identity` already
//! owns at `.data/accounts/` — not a second database this crate would
//! have to keep referentially consistent with the first by hand.
//! `senken-identity` stays the file's single owner of `PRAGMA
//! user_version`, creating both tables in its own schema module even
//! though it never queries either; this crate never opens its own
//! connection, only a clone of that store's connection via
//! [`RegistryStore::new`] -> [`senken_identity::IdentityStore::shared_connection`].
//! See `senken-chart`'s module docs for the full reasoning behind this
//! shape — the same trade is made here for the same reasons.

mod error;
mod handle;
mod id;
mod store;
mod version;

pub use crate::error::RegistryError;
pub use crate::handle::Handle;
pub use crate::id::IndicatorEntryId;
pub use crate::store::{IndicatorEntry, IndicatorSummary, InstalledIndicator, RegistryStore};
pub use crate::version::HOST_LANGUAGE_VERSION;

// Re-exported for convenience: `list_mine`/`search` return
// `senken_identity::Page<T>`, the exact same paginated-result shape every
// other guarded listing in this workspace returns.
pub use senken_identity::Page;
