//! Note persistence: a user-owned freeform note, as one SQLite record
//! queried and mutated through the exact guarded-query pattern
//! `senken-identity`/`senken-chart` established: every read and write here
//! takes a [`senken_identity::AuthenticatedUser`], calls
//! [`senken_identity::AuthenticatedUser::authorize`] before touching a row,
//! and turns the [`senken_acl::Scope`] that comes back into a `WHERE`
//! clause — including in every listing's total, never a post-fetch filter.
//!
//! # One database, one schema-version owner
//!
//! `notes` references `users(id)`, so the natural home for this table is
//! the same SQLite file `senken-identity` already owns at
//! `.data/accounts/` — not a second database this crate would have to keep
//! referentially consistent with the first by hand. `senken-identity`
//! stays the file's single owner of `PRAGMA user_version`, creating the
//! table in its own schema module even though it never queries it; this
//! crate never opens its own connection, only a clone of that store's
//! connection via [`NoteStore::new`] →
//! [`senken_identity::IdentityStore::shared_connection`]. See
//! `senken-chart`'s module docs for the full reasoning behind this
//! shape — the same trade is made here for the same reasons.
//!
//! # Listings stay small
//!
//! [`NoteStore::list_notes`] returns [`NoteSummary`] — id, owner, title,
//! `updated_at` — never a note's `body`, so a listing page's payload does
//! not grow with how much a user has written in any one note.
//! [`NoteStore::get_note`] returns the full [`Note`], body included, for
//! when one note is actually being read or edited.

mod error;
mod id;
mod store;

pub use crate::error::NoteError;
pub use crate::id::NoteId;
pub use crate::store::{Note, NoteStore, NoteSummary};

// Re-exported for convenience: `list_notes` returns
// `senken_identity::Page<T>`, the exact same paginated-result shape that
// crate's own guarded queries return — reused rather than re-declared, the
// same choice `senken-chart`/`senken-watchlist` make for their own
// listings.
pub use senken_identity::Page;
