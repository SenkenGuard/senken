//! Dashboard persistence: a user-owned workspace and the widgets placed on
//! its grid, queried and mutated through the exact guarded-query pattern
//! `senken-identity`/`senken-chart` established — every read and write
//! here takes a [`senken_identity::AuthenticatedUser`], calls
//! [`senken_identity::AuthenticatedUser::authorize`] before touching a
//! row, and turns the [`senken_acl::Scope`] that comes back into a `WHERE`
//! clause, including in every listing's total, never a post-fetch filter.
//!
//! # Why a dashboard is its own aggregate, not a second chart workspace
//!
//! A chart workspace holds layouts, panes and pane items — a fixed-role
//! grid a chart's own preset dictates. A dashboard workspace holds
//! arbitrary widgets a user places, moves and resizes freely. The two
//! share nothing but the word "workspace" and a user id to key off, so
//! `senken-chart`'s tables stay `chart_*` and this crate's stay
//! `dashboard_*` — see `senken_acl::Resource::ChartWorkspace`'s own docs
//! for the same reasoning applied to the authorisation side.
//!
//! # Structure
//!
//! A [`DashboardWorkspaceStore`] holds dashboard workspaces, each with a
//! grid width ([`DashboardWorkspaceSummary::columns`]) and zero or more
//! placed widgets ([`WidgetRecord`]). [`DashboardWorkspaceStore::replace_layout`]
//! writes a workspace's whole widget grid in one transaction, guarded by
//! optimistic concurrency ([`DashboardWorkspaceSummary::revision`]) so two
//! open tabs on the same workspace cannot silently overwrite each other.
//!
//! A placed widget stores a provider id and a widget type id, **never a
//! component and never a copy of that widget type's own metadata** — see
//! [`WidgetRecord`]'s own docs for why that is what makes a placeholder
//! possible when a widget's provider is no longer available.
//! [`WidgetRegistry`] is the separate, non-persisted catalog a caller
//! cross-references a stored `widget_type_id` against to decide whether to
//! render a widget for real or as a placeholder — this crate never makes
//! that decision itself, so a registry that changes (a plugin disabled, a
//! widget renamed) never has to migrate a single stored row.
//!
//! Every geometry field — a widget's position and size — is stored as
//! whole grid columns and rows, never pixels: a pixel value bakes in
//! whatever screen size happened to be open at save time, and reads back
//! wrong the moment the window is a different size.
//!
//! # One database, one schema-version owner
//!
//! Dashboard workspaces reference `users(id)`, so the natural home for
//! `dashboard_workspaces`/`dashboard_widgets` is the same SQLite file
//! `senken-identity` already owns at `.data/accounts/`, created by that
//! crate's own schema module rather than by a second connection to the
//! same file stamping its own `PRAGMA user_version` — see
//! `senken_identity`'s `schema` module doc comment, and `senken-chart`'s
//! own module docs for the full reasoning this crate reuses verbatim.
//! [`DashboardWorkspaceStore::new`] takes a `&senken_identity::IdentityStore`
//! and shares its connection; there is no `senken_dashboard::open` that
//! takes a path.
//!
//! # What this crate does not do
//!
//! It does not render anything, does not know what a widget's `config`
//! fields mean (a widget's own provider owns that), and does not decide
//! whether a widget renders for real or as a placeholder — see
//! [`WidgetRegistry`]'s own docs. Persistence, grid-geometry validation and
//! authorisation are this crate's whole job.

mod error;
mod id;
mod registry;
mod store;

pub use crate::error::DashboardError;
pub use crate::id::{DashboardWidgetId, DashboardWorkspaceId};
pub use crate::registry::{
    BUILTIN_PROVIDER_ID, DataSource, GridSize, WidgetDefinition, WidgetRegistry,
};
pub use crate::store::{
    DashboardLayout, DashboardWorkspaceStore, DashboardWorkspaceSummary, WidgetPlacementInput,
    WidgetRecord,
};

// Re-exported for convenience: every listing here returns
// `senken_identity::Page<T>`, the exact same paginated-result shape
// `senken-chart`'s own guarded queries return — reused rather than
// re-declared, so a caller of both crates' listings is not asked to learn
// two names for one concept.
pub use senken_identity::Page;
