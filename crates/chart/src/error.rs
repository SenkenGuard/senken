//! Everything that can go wrong in the chart store.

use crate::preset::LayoutPreset;

/// Why a chart-store operation failed. Named `ChartError` (not
/// `WorkspaceError`) to match this crate's own rename — see the crate's
/// module docs.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ChartError {
    /// The underlying SQLite call failed — including a constraint violation
    /// partway through [`ChartWorkspaceStore::replace_layout`](crate::ChartWorkspaceStore::replace_layout)'s
    /// transaction, which this variant also carries: the transaction is
    /// rolled back before this ever reaches the caller.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The permission check itself — `senken_identity::AuthenticatedUser::authorize`'s
    /// own error, reused rather than re-declared so a caller sees the exact
    /// same [`PasswordNotSet`](senken_identity::IdentityError::PasswordNotSet)/
    /// [`Forbidden`](senken_identity::IdentityError::Forbidden) this crate's
    /// own guarded queries and `senken-identity`'s produce for the same
    /// reasons.
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),

    /// No chart workspace exists for the given id.
    #[error("no workspace found for that id")]
    WorkspaceNotFound,

    /// No layout exists for the given id.
    #[error("no layout found for that id")]
    LayoutNotFound,

    /// No pane item exists for the given id.
    #[error("no pane item found for that id")]
    PaneItemNotFound,

    /// [`ChartWorkspaceStore::replace_layout`](crate::ChartWorkspaceStore::replace_layout)
    /// was called with a `panes` list whose length does not match
    /// `preset`'s [`pane_count`](LayoutPreset::pane_count) — e.g. three
    /// panes for a `2h` preset, which the UI can never render.
    #[error("layout preset {preset} requires exactly {expected} panes, got {actual}")]
    PaneCountMismatch {
        /// The preset the caller supplied.
        preset: LayoutPreset,
        /// How many panes that preset requires.
        expected: usize,
        /// How many the caller actually supplied.
        actual: usize,
    },

    /// An indicator item's `params` was not valid JSON. This crate does
    /// not interpret indicator parameters (that is the indicator engine's
    /// job) — it only refuses to persist a value that could not possibly be
    /// read back as anything.
    #[error("indicator item parameters are not valid JSON: {0}")]
    InvalidIndicatorParams(String),

    /// A pane's `settings` was not valid JSON. This crate does not
    /// interpret a pane's display settings at all (the chart front end's
    /// job) — it only refuses to persist a value that could not possibly
    /// be read back as anything, the same as
    /// [`InvalidIndicatorParams`](Self::InvalidIndicatorParams).
    #[error("pane settings are not valid JSON: {0}")]
    InvalidPaneSettings(String),

    /// A `chart_panes.instrument` or a referenced item's `instrument`
    /// column held text that no longer parses as a
    /// `senken_marketdata::InstrumentId` — the database was written by an
    /// incompatible version of this crate, or edited by hand.
    #[error("stored instrument id is corrupt: {0}")]
    CorruptInstrumentId(String),

    /// A `chart_panes.timeframe` column held text that no longer parses as
    /// a `senken_series::BarSpec`, for the same reason as
    /// [`CorruptInstrumentId`](Self::CorruptInstrumentId).
    #[error("stored timeframe is corrupt: {0}")]
    CorruptTimeframe(String),

    /// A `chart_layouts.preset` column held text that does not match any
    /// [`LayoutPreset`], for the same reason as
    /// [`CorruptInstrumentId`](Self::CorruptInstrumentId).
    #[error("stored layout preset is corrupt: {0}")]
    CorruptPreset(String),

    /// A `chart_pane_items` row's `slot` column held a value other than
    /// `main`/`sub`, or the columns a given `source_kind` requires were
    /// unexpectedly `NULL` — either way the database was written by an
    /// incompatible version of this crate, or edited by hand.
    #[error("stored pane item is corrupt: {0}")]
    CorruptPaneItem(String),

    /// An anchored item's `width` was outside `1..=4` or its `color` was
    /// not a plain `#rrggbb` hex string. This crate does not interpret a
    /// drawing's geometry beyond its own kind-specific shapes, but a style
    /// no chart could ever render is refused at the door the same way
    /// [`InvalidIndicatorParams`](Self::InvalidIndicatorParams) refuses
    /// unparsable JSON.
    #[error("invalid drawing style: {0}")]
    InvalidDrawingStyle(String),

    /// A `chart_pane_items.tool_kind` or `.style` column held a value this
    /// build does not recognise, or the columns a given anchored kind
    /// requires were unexpectedly `NULL` — for the same reason as
    /// [`CorruptPaneItem`](Self::CorruptPaneItem).
    #[error("stored drawing is corrupt: {0}")]
    CorruptDrawing(String),

    /// A `PATCH` tried to change a pane item's `source_kind` family — e.g.
    /// turning a computed indicator into an anchored drawing through
    /// `update_pane_item`. Each of `/api/layers/{id}` and
    /// `/api/drawings/{id}` only ever updates items of its own family;
    /// this is the invariant that keeps a client's `layers[]`/`drawings[]`
    /// id namespaces meaningful even though both now live in one table.
    #[error("cannot change a pane item's source kind through an update")]
    ItemSourceMismatch,
}
