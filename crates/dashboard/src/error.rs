//! Everything that can go wrong in the dashboard store.

/// Why a dashboard-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DashboardError {
    /// The underlying SQLite call failed — including a constraint violation
    /// partway through [`crate::DashboardWorkspaceStore::replace_layout`]'s
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

    /// No dashboard workspace exists for the given id.
    #[error("no dashboard workspace found for that id")]
    WorkspaceNotFound,

    /// [`crate::DashboardWorkspaceStore::replace_layout`] named a widget id
    /// in its snapshot that either does not exist or belongs to a different
    /// workspace. A client only ever names a widget id it already read back
    /// from this same workspace — this is not a validation error a
    /// well-behaved client can trigger honestly, but a stale or crossed-wire
    /// snapshot must still fail rather than silently adopt the row.
    #[error("no widget found for that id in this workspace")]
    WidgetNotFound,

    /// [`crate::DashboardWorkspaceStore::replace_layout`] was called with an
    /// `expected_revision` that no longer matches the workspace's current
    /// one — another write (another browser tab, most likely) landed first.
    /// The caller's snapshot is discarded; the previous layout is left
    /// exactly as it was.
    #[error("workspace revision {expected} does not match the current revision {current}")]
    RevisionConflict {
        /// The revision the caller's snapshot was built against.
        expected: u64,
        /// The workspace's actual current revision.
        current: u64,
    },

    /// [`crate::DashboardWorkspaceStore::replace_layout`] was called with a
    /// `columns` of `0`, or a widget rectangle that does not fit within
    /// `0..columns` horizontally. A grid with no width, or a widget wider
    /// than its own grid, could never be rendered.
    #[error("widget at ({x}, {y}) with width {width} does not fit within {columns} columns")]
    OutOfBounds {
        /// The offending widget's column.
        x: u32,
        /// The offending widget's row.
        y: u32,
        /// The offending widget's width, in grid columns.
        width: u32,
        /// The workspace's own column count.
        columns: u32,
    },

    /// [`crate::DashboardWorkspaceStore::replace_layout`] was called with a
    /// widget whose `width` or `height` was `0` — a rectangle with no area
    /// could never be rendered, and could never usefully collide or not
    /// collide with anything either.
    #[error("widget at ({x}, {y}) has zero width or height")]
    ZeroSizedWidget {
        /// The offending widget's column.
        x: u32,
        /// The offending widget's row.
        y: u32,
    },

    /// [`crate::DashboardWorkspaceStore::replace_layout`] was called with
    /// two widgets whose rectangles overlap. SQLite has no constraint that
    /// can reject two overlapping rectangles, so this is checked in Rust
    /// before the transaction ever opens.
    #[error("widgets at ({x1}, {y1}) and ({x2}, {y2}) overlap")]
    OverlappingWidgets {
        /// The first widget's column.
        x1: u32,
        /// The first widget's row.
        y1: u32,
        /// The second widget's column.
        x2: u32,
        /// The second widget's row.
        y2: u32,
    },

    /// A widget's `config` was not valid JSON. This crate does not
    /// interpret a widget's configuration beyond checking it parses — a
    /// widget's own provider owns what the fields inside mean.
    #[error("widget config is not valid JSON: {0}")]
    InvalidConfig(String),

    /// A `provider_id` or `widget_type_id` was empty.
    #[error("widget provider_id and widget_type_id must not be empty")]
    EmptyWidgetIdentity,
}
