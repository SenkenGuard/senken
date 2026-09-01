//! Chart persistence: chart workspace → layout
//! → panes → pane items, as SQLite records with an owner rather
//! than JSON blobs, queried and mutated through the exact
//! guarded-query pattern `senken-identity` established
//! : every read and write here takes a
//! [`senken_identity::AuthenticatedUser`], calls
//! [`senken_identity::AuthenticatedUser::authorize`] before touching a row,
//! and turns the [`senken_acl::Scope`] that comes back into a `WHERE`
//! clause — including in every listing's total, never a post-fetch filter.
//!
//! # Why this crate carries the `chart` name
//!
//! A dashboard is coming with its own workspace concept
//! (`dashboard_workspaces`). At that point "workspace" stops naming one
//! aggregate and starts being a common noun for two of them —
//! `workspace_panes` would no longer answer "which workspace's pane". This
//! crate, its tables (`chart_workspaces`/`chart_layouts`/`chart_panes`/
//! `chart_pane_items`) and its `Resource::ChartWorkspace`/`ChartLayout`
//! authorisation resources are all named `chart_*`/`Chart*` for the same
//! reason: the prefix names the crate that owns the table, and this crate
//! is the one that owns chart persistence specifically, not workspaces in
//! general.
//!
//! # Structure
//!
//! A [`ChartWorkspaceStore`] holds chart workspaces, each with one or more
//! layouts ("tabs"). A layout has a [`LayoutPreset`] (which pane-grid
//! arrangement it uses) and a fixed number of panes matching it. A pane
//! ([`PaneRecord`]) has one main instrument and timeframe, plus zero or
//! more items ([`PaneItemRecord`]). [`ChartWorkspaceStore::replace_layout`]
//! writes a layout's whole pane/item structure in one transaction, so a
//! failure partway through — a duplicate grid position, say — can never
//! leave a layout with fewer panes than either its old or its new state
//! ever had.
//!
//! A pane item is one of three things ([`ItemSource`]): a computed
//! indicator, a referenced overlay instrument, or an anchored drawing
//! ([`DrawingKind`]: a horizontal line, a trend line, a rectangle, a ray, a
//! Fibonacci retracement, or a text note, each with its own geometry
//! ([`DrawingPoint`]) and colour/width/line-style ([`DrawingStyle`])).
//! Where an item is placed within its pane's grid slot ([`Slot`]) is
//! orthogonal to what it draws — one table, one id space, one
//! select/hit-test/edit/delete/persist path for all three, with `visible`
//! applying uniformly. Strategy layers and everything else this crate
//! marks out of scope are still not modelled here.
//!
//! # One database, one schema-version owner
//!
//! Chart workspaces reference `users(id)`, so the natural home for these
//! four tables is the same SQLite file `senken-identity` already owns at
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
//!   This crate's four tables (`chart_workspaces`, `chart_layouts`,
//!   `chart_panes`, `chart_pane_items`) are created by *that* crate's
//!   schema module, at its own v3/v5/v8 (see `senken_identity`'s `schema`
//!   module doc comment) — even though that crate never queries them.
//!   This is the one deliberately unusual part of this design:
//!   `senken-identity`'s schema outgrows its own name a little, in
//!   exchange for there being exactly one place that ever decides what the
//!   file's version means.
//! - **This crate never opens its own connection to that file.**
//!   [`ChartWorkspaceStore::new`] takes a `&senken_identity::IdentityStore`
//!   and calls [`senken_identity::IdentityStore::shared_connection`],
//!   which hands back an `Arc<Mutex<rusqlite::Connection>>` clone of the
//!   exact connection that store already uses. There is no
//!   `senken_chart::open` that takes a path — that function does not
//!   exist, on purpose, because its existence would be an invitation to
//!   open a second connection.
//!
//! The trade this makes explicit: `senken-identity`'s public API now
//! returns a `rusqlite::Connection` type, which ties a sibling crate's
//! sharing mechanism to that concrete library rather than an abstraction
//! over it. Given both crates already depend on `rusqlite` directly (the
//! workspace pins one version for the whole tree) and neither is a
//! published library aimed at hiding that choice from its own callers, this
//! was judged a smaller cost than either alternative above. A crate name
//! honestly describing what *this* crate holds — chart workspaces, layouts,
//! panes, pane items — was judged more valuable than folding this domain
//! into `senken-identity` itself, which would have kept one schema-version
//! owner but made that crate's own name stop describing what it holds.
//!
//! # What this crate does not do
//!
//! It does not validate that a stored `InstrumentId` refers to a real,
//! catalogued instrument (the job, once live data and the
//! catalog are wired to the UI) and it does not interpret a computed
//! item's `params` beyond checking it parses as JSON (the job). It does
//! not render anything — `senken-indicators` owns the `Drawable`/
//! `ToolDescriptor` output contract an anchored item is eventually drawn
//! through; this crate only stores and returns its geometry. Persistence
//! and authorisation are this crate's whole job.

mod error;
mod id;
mod preset;
mod store;

pub use crate::error::ChartError;
pub use crate::id::{ChartLayoutId, ChartPaneId, ChartWorkspaceId, PaneItemId};
pub use crate::preset::{LayoutPreset, ParseLayoutPresetError};
pub use crate::store::{
    ChartLayoutDetail, ChartLayoutSummary, ChartWorkspaceStore, ChartWorkspaceSummary, DrawingKind,
    DrawingLineStyle, DrawingPoint, DrawingStyle, ItemSource, PaneInput, PaneItemInput,
    PaneItemRecord, PaneRecord, Slot,
};

// Re-exported for convenience: every listing here returns
// `senken_identity::Page<T>`, the exact same paginated-result shape that
// crate's own guarded queries return — reused rather than
// re-declared, so a caller of both crates' listings is not asked to learn
// two names for one concept.
pub use senken_identity::Page;
