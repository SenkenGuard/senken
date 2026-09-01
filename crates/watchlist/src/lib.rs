//! Watchlist persistence: a user-owned group of watched instruments and
//! its membership, as SQLite records queried and mutated through the exact
//! guarded-query pattern `senken-identity`/`senken-chart` established:
//! every read and write here takes a
//! [`senken_identity::AuthenticatedUser`], calls
//! [`senken_identity::AuthenticatedUser::authorize`] before touching a row,
//! and turns the [`senken_acl::Scope`] that comes back into a `WHERE`
//! clause — including in every listing's total, never a post-fetch filter.
//!
//! # One database, one schema-version owner
//!
//! `watchlist_groups` and `watchlist_members` reference `users(id)`, so the
//! natural home for these two tables is the same SQLite file
//! `senken-identity` already owns at `.data/accounts/` — not a second
//! database this crate would have to keep referentially consistent with
//! the first by hand. `senken-identity` stays the file's single owner of
//! `PRAGMA user_version`, creating both tables in its own schema module
//! even though it never queries them; this crate never opens its own
//! connection, only a clone of that store's connection via
//! [`WatchlistStore::new`] → [`senken_identity::IdentityStore::shared_connection`].
//! See `senken-chart`'s module docs for the full reasoning behind this
//! shape — the same trade is made here for the same reasons.
//!
//! # Structure
//!
//! A [`WatchlistStore`] holds watchlist groups, each with zero or more
//! members. A group ([`WatchlistGroupSummary`]) is a name and a display
//! position; a member ([`WatchlistMember`]) is one watched
//! [`senken_marketdata::InstrumentId`] and its own position within the
//! group. A member row carries no owner column of its own — its owner is
//! read through its group, the same relationship a chart pane has to its
//! workspace — so deleting a group cascades its members with nothing left
//! to orphan.
//!
//! Adding an instrument a group already holds is idempotent
//! ([`WatchlistStore::add_member`] returns the existing row rather than
//! erroring), since a watchlist is closer to a set of instruments than a
//! strictly ordered log — the client that dragged an instrument onto a
//! group it is already in should not have to handle a conflict.
//!
//! # What this crate does not do
//!
//! It does not validate that a stored `InstrumentId` refers to a real,
//! catalogued instrument, and it does not render anything. Persistence and
//! authorisation are this crate's whole job.

mod error;
mod id;
mod store;

pub use crate::error::WatchlistError;
pub use crate::id::{WatchlistGroupId, WatchlistMemberId};
pub use crate::store::{WatchlistGroupSummary, WatchlistMember, WatchlistStore};

// Re-exported for convenience: every listing here returns
// `senken_identity::Page<T>`, the exact same paginated-result shape that
// crate's own guarded queries return — reused rather than re-declared, the
// same choice `senken-chart` makes for its own listings.
pub use senken_identity::Page;
