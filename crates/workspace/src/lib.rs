//! Chart workspace persistence: workspace → layout
//! → panes → layers, as SQLite records with an owner rather
//! than JSON blobs, queried and mutated through the exact
//! guarded-query pattern `senken-identity` established
//! : every read and write here takes a
//! [`senken_identity::AuthenticatedUser`], calls
//! [`senken_identity::AuthenticatedUser::authorize`] before touching a row,
//! and turns the [`senken_acl::Scope`] that comes back into a `WHERE`
//! clause — including in every listing's total, never a post-fetch filter.
//!
//! # Structure
//!
//! A [`WorkspaceStore`] holds workspaces, each with one or more layouts
//! ("tabs"). A layout has a [`LayoutPreset`] (which pane-grid arrangement
//! it uses) and a fixed number of panes matching it. A pane
//! ([`PaneRecord`]) has one main instrument and timeframe, plus zero or
//! more layers ([`LayerRecord`]): an overlay instrument, or an indicator
//! placed as an overlay or in its own sub-pane ([`LayerKind`]).
//! [`WorkspaceStore::replace_layout`] writes a layout's whole pane/layer/
//! drawing structure in one transaction, so a failure partway
//! through — a duplicate grid position, say — can never leave a layout with
//! fewer panes than either its old or its new state ever had.
//!
//! A pane also holds zero or more drawings ([`DrawingRecord`]): a
//! horizontal line, a trend line, or a rectangle, each with its own
//! geometry ([`DrawingKind`]) and colour/width/line-style
//! ([`DrawingStyle`]). Strategy layers and everything else
//! marks out of scope are still not modelled here.
//!
//! # One database, one schema-version owner
//!
//! Workspaces reference `users(id)`, so the natural home for these four
//! tables is the same SQLite file `senken-identity` already owns at
//! `.data/accounts/` — not a second database this crate
//! would have to keep referentially consistent with the first by hand.
//!
//! The risk this avoids: **two crates each opening their
//! own connection to that file and independently stamping `PRAGMA
//! user_version`** is a race and a maintenance trap — whichever crate's
//! migration runs second on a fresh file would either clobber the other's
//! tables or disagree about what version the file is on.
//!
//! The fix here is not a new migration framework (the accounts database already
//! rules that out) or a second database (it would only move the
//! consistency problem from "one file, two owners" to "two files, one
//! foreign key that cannot be enforced across them"). Instead:
//!
//! - **`senken-identity` stays the file's single owner of `user_version`.**
//!   This crate's five tables (`workspaces`, `layouts`, `panes`, `layers`,
//!   `drawings`) are created by *that* crate's schema module, at its own
//!   v3/v5 (see `senken_identity`'s `schema` module doc comment) — even
//!   though that crate never queries them. This is the one deliberately
//!   unusual part of this design: `senken-identity`'s schema outgrows its
//!   own name a little, in exchange for there being exactly one place that
//!   ever decides what the file's version means.
//! - **This crate never opens its own connection to that file.**
//!   [`WorkspaceStore::new`] takes a `&senken_identity::IdentityStore` and
//!   calls [`senken_identity::IdentityStore::shared_connection`], which
//!   hands back an `Arc<Mutex<rusqlite::Connection>>` clone of the exact
//!   connection that store already uses. There is no `senken_workspace::open`
//!   that takes a path — that function does not exist, on purpose, because
//!   its existence would be an invitation to open a second connection.
//!
//! The trade this makes explicit: `senken-identity`'s public API now
//! returns a `rusqlite::Connection` type, which ties a sibling crate's
//! sharing mechanism to that concrete library rather than an abstraction
//! over it. Given both crates already depend on `rusqlite` directly (the
//! workspace pins one version for the whole tree) and neither is a
//! published library aimed at hiding that choice from its own callers, this
//! was judged a smaller cost than either alternative above. A crate name
//! honestly describing what *this* crate holds — workspaces, layouts,
//! panes, layers — was judged more valuable than folding this domain into
//! `senken-identity` itself, which would have kept one schema-version owner
//! but made that crate's own name stop describing what it holds.
//!
//! # What this crate does not do
//!
//! It does not validate that a stored `InstrumentId` refers to a real,
//! catalogued instrument (the job, once live data and the
//! catalog are wired to the UI) and it does not interpret an indicator
//! layer's `params` beyond checking it parses as JSON (the job).
//! Persistence and authorisation are this crate's whole job.

mod error;
mod id;
mod preset;
mod store;

pub use crate::error::WorkspaceError;
pub use crate::id::{DrawingId, LayerId, LayoutId, PaneId, WorkspaceId};
pub use crate::preset::{LayoutPreset, ParseLayoutPresetError};
pub use crate::store::{
    DrawingInput, DrawingKind, DrawingLineStyle, DrawingPoint, DrawingRecord, DrawingStyle,
    LayerInput, LayerKind, LayerRecord, LayoutDetail, LayoutSummary, PaneInput, PaneRecord,
    WorkspaceStore, WorkspaceSummary,
};

// Re-exported for convenience: every listing here returns
// `senken_identity::Page<T>`, the exact same paginated-result shape that
// crate's own guarded queries return — reused rather than
// re-declared, so a caller of both crates' listings is not asked to learn
// two names for one concept.
pub use senken_identity::Page;
