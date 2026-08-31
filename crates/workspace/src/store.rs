//! [`WorkspaceStore`]: the guarded query API for workspaces, layouts,
//! panes and layers.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use senken_acl::{Action, Resource, Scope};
use senken_core::UnixNanos;
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};
use senken_marketdata::InstrumentId;
use senken_series::BarSpec;

use crate::error::WorkspaceError;
use crate::id::{DrawingId, LayerId, LayoutId, PaneId, WorkspaceId};
use crate::preset::LayoutPreset;

/// The name every auto-created default workspace and its one layout are
/// given ("opening charts with no workspace creates a default one").
const DEFAULT_WORKSPACE_NAME: &str = "Default";
const DEFAULT_LAYOUT_NAME: &str = "Main";

/// The placeholder chart every freshly created workspace's first layout
/// starts with, so it is never an empty state: BTC/USDT at one hour, on the
/// one source this build has a live price feed for — a new user's first
/// chart must be able to show a live price, which Binance's spot listing
/// (this default's previous choice) cannot.
fn default_instrument() -> InstrumentId {
    InstrumentId::parse("okx-spot:BTCUSDT").expect("fixed, valid instrument id")
}

fn default_timeframe() -> BarSpec {
    BarSpec::new(1, senken_series::BarUnit::Hour)
}

/// One layer attached to a pane: either an overlay instrument, or an
/// indicator placed either as an overlay or in its own sub-pane. This crate does not interpret `name`/`params` — that is the
/// indicator engine's job — beyond checking `params` is valid
/// JSON before persisting it, so a corrupt value is refused at the door
/// rather than discovered by whatever reads it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerKind {
    /// A second instrument plotted on the same pane as the main one.
    OverlayInstrument {
        /// The overlaid instrument.
        instrument: InstrumentId,
    },
    /// An indicator drawn on top of the pane's main price plot (e.g. an
    /// EMA or Bollinger Bands).
    IndicatorOverlay {
        /// The indicator's name (e.g. `"EMA"`).
        name: String,
        /// The indicator's parameters, as a JSON object — this crate
        /// validates only that it parses as JSON, never its shape.
        params: String,
    },
    /// An indicator drawn in its own pane beneath the main one (e.g. RSI or
    /// MACD).
    IndicatorSubPane {
        /// The indicator's name.
        name: String,
        /// The indicator's parameters, as a JSON object.
        params: String,
    },
}

/// One user-drawn chart annotation, with identity — the minimum viable set
/// a TradingView-style drawing tool needs: a horizontal price line, a
/// trend line between two points, and a rectangle zone between two
/// corners. A fourth kind needs the same create/hit-test/select/edit/
/// delete/persist treatment as these three, not just a new variant here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawingKind {
    /// A single price level, drawn across the pane's full width.
    HorizontalLine {
        /// The line's price level — a coordinate the user set by clicking
        /// the chart, not an order price.
        price: f64,
    },
    /// A straight line between two (time, price) points.
    TrendLine {
        /// The line's first point.
        start: DrawingPoint,
        /// The line's second point.
        end: DrawingPoint,
    },
    /// A filled zone between two (time, price) corners.
    Rectangle {
        /// One corner.
        start: DrawingPoint,
        /// The opposite corner.
        end: DrawingPoint,
    },
}

/// One (time, price) anchor for a [`DrawingKind::TrendLine`] or
/// [`DrawingKind::Rectangle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingPoint {
    /// When this anchor falls.
    pub time: UnixNanos,
    /// The anchor's price — like [`DrawingKind::HorizontalLine`]'s own
    /// price, a clicked coordinate rather than an order price.
    pub price: f64,
}

/// A drawing's colour, width and line style — the three properties a
/// TradingView-style "edit object" panel offers, applied to the whole
/// object rather than per-segment.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingStyle {
    /// The drawing's colour, as `#rrggbb`.
    pub color: String,
    /// Stroke width in pixels, 1 through 4.
    pub width: u8,
    /// The line's dash pattern.
    pub line_style: DrawingLineStyle,
}

/// A drawing's line style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingLineStyle {
    /// An unbroken line.
    Solid,
    /// A dashed line.
    Dashed,
    /// A dotted line.
    Dotted,
}

/// One pane's main instrument, timeframe, layer stack and drawings, as read
/// back from [`WorkspaceStore::get_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct PaneRecord {
    /// The pane's id.
    pub id: PaneId,
    /// This pane's slot within its layout's grid.
    pub position: u32,
    /// The pane's main instrument ("a pane holds one main instrument plus layers").
    pub instrument: InstrumentId,
    /// The main instrument's bar timeframe.
    pub timeframe: BarSpec,
    /// This pane's layers, in stacking order.
    pub layers: Vec<LayerRecord>,
    /// This pane's drawings, in stacking order.
    pub drawings: Vec<DrawingRecord>,
    /// Display settings for this pane's chart — candle colours, precision,
    /// scales, canvas, status line — as opaque JSON-object text. This crate
    /// does not interpret it, the same way [`LayerKind::IndicatorOverlay`]'s
    /// `params` is only checked for being valid JSON, never for its shape:
    /// the chart front end owns what these settings mean.
    pub settings: String,
}

/// One layer, as read back from [`WorkspaceStore::get_layout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerRecord {
    /// The layer's id.
    pub id: LayerId,
    /// This layer's stacking order within its pane.
    pub position: u32,
    /// What kind of layer this is.
    pub kind: LayerKind,
    /// Whether the layer is currently shown.
    pub visible: bool,
    /// How this layer is drawn — colour, line style, width, per-plot
    /// visibility — as opaque JSON-object text. Kept apart from an
    /// indicator's `params`: changing a period recomputes the series,
    /// changing a colour does not. This crate checks only that it parses as
    /// JSON; the chart front end owns what the keys mean.
    pub style: String,
}

/// One drawing, as read back from [`WorkspaceStore::get_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingRecord {
    /// The drawing's id.
    pub id: DrawingId,
    /// This drawing's stacking order within its pane.
    pub position: u32,
    /// What kind of drawing this is, and its geometry.
    pub kind: DrawingKind,
    /// The drawing's colour/width/line-style.
    pub style: DrawingStyle,
}

/// A layout's identity and grid arrangement, without its panes — returned
/// by [`WorkspaceStore::list_layouts`]; see [`LayoutDetail`] for the full
/// nested structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutSummary {
    /// The layout's id.
    pub id: LayoutId,
    /// The workspace this layout belongs to.
    pub workspace_id: WorkspaceId,
    /// The layout's display name.
    pub name: String,
    /// Which pane-grid arrangement this layout uses.
    pub preset: LayoutPreset,
    /// This layout's tab order within its workspace.
    pub position: u32,
}

/// A layout together with its panes and their layers — the full nested
/// structure of workspace → layout → panes → layers,
/// minus the workspace itself.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutDetail {
    /// The layout's identity and grid arrangement.
    pub layout: LayoutSummary,
    /// The layout's panes, in grid-slot order.
    pub panes: Vec<PaneRecord>,
}

/// A workspace row as returned by a guarded listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSummary {
    /// The workspace's id.
    pub id: WorkspaceId,
    /// The account that owns this workspace.
    pub owner_id: UserId,
    /// The workspace's display name.
    pub name: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the workspace's own fields
    /// (its name) — a layout's own `updated_at` moves independently.
    pub updated_at: i64,
}

/// One pane to write, supplied to
/// [`WorkspaceStore::replace_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct PaneInput {
    /// This pane's slot within the layout's grid. Must be unique among the
    /// panes in one call — a duplicate is a SQLite constraint violation
    /// that rolls the whole call back (see `replace_layout`'s docs).
    pub position: u32,
    /// The pane's main instrument.
    pub instrument: InstrumentId,
    /// The main instrument's bar timeframe.
    pub timeframe: BarSpec,
    /// This pane's layers.
    pub layers: Vec<LayerInput>,
    /// This pane's drawings.
    pub drawings: Vec<DrawingInput>,
    /// Display settings for this pane's chart, as opaque JSON-object text —
    /// see [`PaneRecord::settings`]. [`WorkspaceStore::replace_layout`]
    /// checks only that this parses as JSON.
    pub settings: String,
}

/// One layer to write, supplied as part of a [`PaneInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerInput {
    /// This layer's stacking order within its pane. Must be unique among a
    /// pane's own layers.
    pub position: u32,
    /// What kind of layer this is.
    pub kind: LayerKind,
    /// Whether the layer starts out visible.
    pub visible: bool,
    /// How this layer is drawn, as opaque JSON-object text — see
    /// [`LayerRecord::style`]. Checked only for being valid JSON.
    pub style: String,
}

/// One drawing to write, supplied as part of a [`PaneInput`].
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingInput {
    /// This drawing's stacking order within its pane. Must be unique among
    /// a pane's own drawings.
    pub position: u32,
    /// What kind of drawing this is, and its geometry.
    pub kind: DrawingKind,
    /// The drawing's colour/width/line-style.
    pub style: DrawingStyle,
}

/// Guarded queries over workspaces, layouts, panes and layers.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second one —
/// see this crate's module docs for why. Every mutation and every listing
/// goes through an [`AuthenticatedUser`] and calls
/// [`AuthenticatedUser::authorize`] exactly the way `senken-identity`'s own
/// guarded queries do: there is no method here that reads
/// or writes a row without that check running first.
#[derive(Debug)]
pub struct WorkspaceStore {
    conn: Arc<Mutex<Connection>>,
}

impl WorkspaceStore {
    /// Builds a store sharing `identity`'s own database connection.
    #[must_use]
    pub fn new(identity: &IdentityStore) -> Self {
        Self {
            conn: identity.shared_connection(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Creates a new, named workspace owned by `auth`, seeded with one
    /// default layout (never an empty state).
    ///
    /// Requires `auth` to hold `Action::Create` on both `Resource::Workspace`
    /// and `Resource::Layout` — creating a workspace always creates its
    /// first layout too.
    ///
    /// # Errors
    /// [`WorkspaceError::Identity`] (wrapping
    /// [`IdentityError::PasswordNotSet`] or
    /// [`IdentityError::Forbidden`](senken_identity::IdentityError::Forbidden))
    /// if `auth` may not create a workspace or a layout; otherwise as
    /// [`WorkspaceError::Database`].
    pub fn create_workspace(
        &self,
        auth: &AuthenticatedUser,
        name: &str,
    ) -> Result<WorkspaceId, WorkspaceError> {
        auth.authorize(Action::Create, Resource::Workspace)?;
        auth.authorize(Action::Create, Resource::Layout)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let workspace_id = insert_default_workspace(&tx, auth.user_id(), name)?;
        tx.commit()?;
        Ok(workspace_id)
    }

    /// Returns `auth`'s default workspace and its one layout, creating both
    /// if `auth` owns no workspace yet (opening charts with no workspace creates a default one rather than an empty state).
    /// Calling this again for the same account returns the **same**
    /// workspace and layout — it never creates a second one.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Workspace`
    /// always, and additionally `Action::Create` on both
    /// `Resource::Workspace` and `Resource::Layout` the first time (when
    /// there is nothing yet to view).
    ///
    /// # Errors
    /// [`WorkspaceError::Identity`] if `auth` may not view (or, on first
    /// use, create) a workspace/layout; otherwise as
    /// [`WorkspaceError::Database`].
    pub fn get_or_create_default_workspace(
        &self,
        auth: &AuthenticatedUser,
    ) -> Result<(WorkspaceId, LayoutId), WorkspaceError> {
        auth.authorize(Action::View, Resource::Workspace)?;
        // Held for the whole check-then-create below, so a second call
        // racing this one on another thread cannot observe "no default
        // yet" for both and create two.
        let mut conn = self.lock();
        if let Some(found) = existing_default(&conn, auth.user_id())? {
            return Ok(found);
        }

        auth.authorize(Action::Create, Resource::Workspace)?;
        auth.authorize(Action::Create, Resource::Layout)?;
        let tx = conn.transaction()?;
        let workspace_id = insert_default_workspace(&tx, auth.user_id(), DEFAULT_WORKSPACE_NAME)?;
        let layout_id: LayoutId = tx.query_row(
            "SELECT id FROM layouts WHERE workspace_id = ?1 ORDER BY position LIMIT 1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok((workspace_id, layout_id))
    }

    /// Lists workspaces visible to `auth`, scoped: the
    /// `WHERE` clause is chosen by `auth`'s [`senken_acl::decide`]d
    /// [`Scope`] for `(Action::View, Resource::Workspace)`, and
    /// [`Page::total`] is counted under that same clause.
    ///
    /// # Errors
    /// [`WorkspaceError::Identity`] if `auth` may not view workspaces at
    /// all, or if `decide` returns a [`Scope`] variant this crate does not
    /// yet translate to SQL; otherwise as [`WorkspaceError::Database`].
    pub fn list_workspaces(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<WorkspaceSummary>, WorkspaceError> {
        let scope = auth.authorize(Action::View, Resource::Workspace)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM workspaces WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, created_at, updated_at FROM workspaces
                     WHERE owner_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_workspace)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, created_at, updated_at FROM workspaces
                     ORDER BY created_at ASC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_workspace)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — a future
            // variant this crate has not been taught to turn into a
            // `WHERE` clause must fail closed, never fall back to an
            // unfiltered query.
            _ => return Err(WorkspaceError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Renames a workspace — a field-level edit, not a full
    /// layout rewrite.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Workspace`, with
    /// the returned [`Scope`] applied against this workspace's owner: an
    /// actor scoped to `Own` may only rename a workspace they themselves
    /// own, regardless of whether they can *see* someone else's (which
    /// `Action::View` governs separately).
    ///
    /// # Errors
    /// [`WorkspaceError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`WorkspaceError::Identity`] if `auth` may not edit
    /// workspaces at all, or may not reach this particular one; otherwise
    /// as [`WorkspaceError::Database`].
    pub fn rename_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: WorkspaceId,
        new_name: &str,
    ) -> Result<(), WorkspaceError> {
        let scope = auth.authorize(Action::Edit, Resource::Workspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE workspaces SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_name, now_unix(), workspace_id],
        )?;
        Ok(())
    }

    /// Deletes a workspace and, by `ON DELETE CASCADE`, every layout, pane
    /// and layer it owns.
    ///
    /// Requires `auth` to hold `Action::Delete` on `Resource::Workspace`,
    /// scoped against this workspace's owner the same way
    /// [`rename_workspace`](Self::rename_workspace) is.
    ///
    /// # Errors
    /// [`WorkspaceError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`WorkspaceError::Identity`] if `auth` may not delete this
    /// workspace; otherwise as [`WorkspaceError::Database`].
    pub fn delete_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: WorkspaceId,
    ) -> Result<(), WorkspaceError> {
        let scope = auth.authorize(Action::Delete, Resource::Workspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![workspace_id],
        )?;
        Ok(())
    }

    /// Lists a workspace's layouts (its "tabs"), in tab order.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Layout`, scoped
    /// against the *workspace's* owner the same way
    /// [`rename_workspace`](Self::rename_workspace) resolves
    /// `Resource::Workspace`'s scope — a layout has no owner column of its
    /// own, so ownership is always read through the workspace it belongs
    /// to.
    ///
    /// # Errors
    /// [`WorkspaceError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`WorkspaceError::Identity`] if `auth` may not view this
    /// workspace's layouts; [`WorkspaceError::CorruptPreset`] if a stored
    /// preset no longer parses; otherwise as [`WorkspaceError::Database`].
    pub fn list_layouts(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<LayoutSummary>, WorkspaceError> {
        let scope = auth.authorize(Action::View, Resource::Layout)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, name, preset, position FROM layouts
             WHERE workspace_id = ?1 ORDER BY position ASC",
        )?;
        let rows = stmt
            .query_map(params![workspace_id], |row| {
                Ok((
                    row.get::<_, LayoutId>(0)?,
                    row.get::<_, WorkspaceId>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, workspace_id, name, preset, position)| {
                Ok(LayoutSummary {
                    id,
                    workspace_id,
                    name,
                    preset: preset
                        .parse()
                        .map_err(|_| WorkspaceError::CorruptPreset(preset.clone()))?,
                    position,
                })
            })
            .collect()
    }

    /// Fetches one layout with its full nested pane/layer structure.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Layout`, scoped
    /// against the layout's owning workspace.
    ///
    /// # Errors
    /// [`WorkspaceError::LayoutNotFound`] if `layout_id` does not exist;
    /// [`WorkspaceError::Identity`] if `auth` may not view this layout;
    /// [`WorkspaceError::CorruptPreset`]/[`WorkspaceError::CorruptInstrumentId`]/
    /// [`WorkspaceError::CorruptTimeframe`]/[`WorkspaceError::CorruptLayer`]
    /// if a stored row no longer parses; otherwise as
    /// [`WorkspaceError::Database`].
    pub fn get_layout(
        &self,
        auth: &AuthenticatedUser,
        layout_id: LayoutId,
    ) -> Result<LayoutDetail, WorkspaceError> {
        let scope = auth.authorize(Action::View, Resource::Layout)?;
        let conn = self.lock();
        let (owner, summary) = load_layout_summary(&conn, layout_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        let panes = load_panes_with_layers(&conn, layout_id)?;
        Ok(LayoutDetail {
            layout: summary,
            panes,
        })
    }

    /// Replaces a layout's entire pane/layer structure in one transaction
    /// ("a layout change must be transactional"). Either every
    /// pane and layer in `panes` is written, or none of them are — a
    /// failure partway through (a duplicate `position` violating a `UNIQUE`
    /// constraint, for instance) rolls the whole change back, leaving the
    /// layout exactly as it was before this call.
    ///
    /// `panes.len()` must equal `preset.pane_count()`.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Layout`, scoped
    /// against the layout's owning workspace.
    ///
    /// # Errors
    /// [`WorkspaceError::LayoutNotFound`] if `layout_id` does not exist;
    /// [`WorkspaceError::Identity`] if `auth` may not edit this layout;
    /// [`WorkspaceError::PaneCountMismatch`] if `panes.len()` does not match
    /// `preset`; [`WorkspaceError::InvalidIndicatorParams`] if an indicator
    /// layer's `params` is not valid JSON; [`WorkspaceError::InvalidDrawingStyle`]
    /// if a drawing's `width` is outside `1..=4` or its `color` is not
    /// `#rrggbb`; otherwise as [`WorkspaceError::Database`] (including any
    /// constraint violation, already rolled back by the time it is
    /// returned).
    pub fn replace_layout(
        &self,
        auth: &AuthenticatedUser,
        layout_id: LayoutId,
        preset: LayoutPreset,
        panes: &[PaneInput],
    ) -> Result<(), WorkspaceError> {
        let scope = auth.authorize(Action::Edit, Resource::Layout)?;
        if panes.len() != preset.pane_count() {
            return Err(WorkspaceError::PaneCountMismatch {
                preset,
                expected: preset.pane_count(),
                actual: panes.len(),
            });
        }
        for pane in panes {
            validate_pane_settings(&pane.settings)?;
            for layer in &pane.layers {
                validate_layer_params(&layer.kind)?;
            }
            for drawing in &pane.drawings {
                validate_drawing_style(&drawing.style)?;
            }
        }

        let mut conn = self.lock();
        let owner = layout_owner(&conn, layout_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE layouts SET preset = ?1, updated_at = ?2 WHERE id = ?3",
            params![preset.to_string(), now_unix(), layout_id],
        )?;
        // Panes are deleted and re-inserted wholesale rather than diffed —
        // `ON DELETE CASCADE` takes their layers and drawings with them —
        // which is why this whole block must be one transaction: a caller
        // observing a half-deleted layout would see fewer panes than either
        // the old or the new state ever had.
        tx.execute("DELETE FROM panes WHERE layout_id = ?1", params![layout_id])?;
        for pane in panes {
            let pane_id = PaneId::new();
            tx.execute(
                "INSERT INTO panes (id, layout_id, position, instrument, timeframe, settings)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    pane_id,
                    layout_id,
                    pane.position,
                    pane.instrument.as_str(),
                    pane.timeframe.to_string(),
                    pane.settings,
                ],
            )?;
            for layer in &pane.layers {
                insert_layer(&tx, pane_id, layer)?;
            }
            for drawing in &pane.drawings {
                insert_drawing(&tx, pane_id, drawing)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// Resolves `Scope` against a row's owner, the same check every method
/// above needs after it has already resolved a [`Scope`] from
/// [`AuthenticatedUser::authorize`] for a single-row operation (as opposed
/// to [`WorkspaceStore::list_workspaces`], which applies its `Scope`
/// straight to a `WHERE` clause instead).
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), WorkspaceError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        // Covers both "Own but not this row's owner" and any future
        // `Scope` variant this crate has not been taught to interpret —
        // failing closed either way, the same discipline `list_workspaces`
        // applies to its own `match`.
        _ => Err(WorkspaceError::Identity(IdentityError::Forbidden)),
    }
}

/// Looks up a workspace's owner, or [`WorkspaceError::WorkspaceNotFound`].
fn workspace_owner(conn: &Connection, workspace_id: WorkspaceId) -> Result<UserId, WorkspaceError> {
    conn.query_row(
        "SELECT owner_id FROM workspaces WHERE id = ?1",
        [workspace_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(WorkspaceError::WorkspaceNotFound)
}

/// Looks up the owner of the workspace a layout belongs to, or
/// [`WorkspaceError::LayoutNotFound`].
fn layout_owner(conn: &Connection, layout_id: LayoutId) -> Result<UserId, WorkspaceError> {
    conn.query_row(
        "SELECT w.owner_id FROM layouts l
         JOIN workspaces w ON w.id = l.workspace_id
         WHERE l.id = ?1",
        [layout_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(WorkspaceError::LayoutNotFound)
}

/// Loads a layout's owner (via its workspace) and its own summary row in
/// one round trip, for [`WorkspaceStore::get_layout`].
fn load_layout_summary(
    conn: &Connection,
    layout_id: LayoutId,
) -> Result<(UserId, LayoutSummary), WorkspaceError> {
    let row: Option<(UserId, WorkspaceId, String, String, u32)> = conn
        .query_row(
            "SELECT w.owner_id, l.workspace_id, l.name, l.preset, l.position
             FROM layouts l JOIN workspaces w ON w.id = l.workspace_id
             WHERE l.id = ?1",
            [layout_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    let (owner, workspace_id, name, preset, position) =
        row.ok_or(WorkspaceError::LayoutNotFound)?;
    let preset = preset
        .parse()
        .map_err(|_| WorkspaceError::CorruptPreset(preset.clone()))?;
    Ok((
        owner,
        LayoutSummary {
            id: layout_id,
            workspace_id,
            name,
            preset,
            position,
        },
    ))
}

/// Loads every pane of `layout_id`, each with its own layers and drawings,
/// in grid-slot and stacking order respectively.
fn load_panes_with_layers(
    conn: &Connection,
    layout_id: LayoutId,
) -> Result<Vec<PaneRecord>, WorkspaceError> {
    let mut pane_stmt = conn.prepare(
        "SELECT id, position, instrument, timeframe, settings FROM panes
         WHERE layout_id = ?1 ORDER BY position ASC",
    )?;
    let pane_rows = pane_stmt
        .query_map(params![layout_id], |row| {
            Ok((
                row.get::<_, PaneId>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut layer_stmt = conn.prepare(
        "SELECT id, position, kind, instrument, indicator_name, indicator_params, visible, style
         FROM layers WHERE pane_id = ?1 ORDER BY position ASC",
    )?;
    let mut drawing_stmt = conn.prepare(
        "SELECT id, position, kind, price, time1, price1, time2, price2, color, width, line_style
         FROM drawings WHERE pane_id = ?1 ORDER BY position ASC",
    )?;

    let mut panes = Vec::with_capacity(pane_rows.len());
    for (pane_id, position, instrument, timeframe, settings) in pane_rows {
        let instrument = InstrumentId::parse(&instrument)
            .map_err(|e| WorkspaceError::CorruptInstrumentId(e.to_string()))?;
        let timeframe: BarSpec =
            timeframe
                .parse()
                .map_err(|e: senken_series::ParseBarSpecError| {
                    WorkspaceError::CorruptTimeframe(e.to_string())
                })?;

        let layer_rows = layer_stmt
            .query_map(params![pane_id], |row| {
                Ok((
                    row.get::<_, LayerId>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut layers = Vec::with_capacity(layer_rows.len());
        for (id, position, kind, instrument_col, name, layer_params, visible, style) in layer_rows {
            layers.push(LayerRecord {
                id,
                position,
                kind: decode_layer_kind(&kind, instrument_col, name, layer_params)?,
                visible,
                style,
            });
        }

        let drawing_rows = drawing_stmt
            .query_map(params![pane_id], |row| {
                Ok((
                    row.get::<_, DrawingId>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<f64>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, String>(10)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut drawings = Vec::with_capacity(drawing_rows.len());
        for (id, position, kind, price, time1, price1, time2, price2, color, width, line_style) in
            drawing_rows
        {
            drawings.push(DrawingRecord {
                id,
                position,
                kind: decode_drawing_kind(&kind, price, time1, price1, time2, price2)?,
                style: decode_drawing_style(color, width, &line_style)?,
            });
        }

        panes.push(PaneRecord {
            id: pane_id,
            position,
            instrument,
            timeframe,
            layers,
            drawings,
            settings,
        });
    }
    Ok(panes)
}

/// Inserts one layer row for `pane_id` inside an already-open transaction.
fn insert_layer(
    tx: &Transaction<'_>,
    pane_id: PaneId,
    layer: &LayerInput,
) -> Result<(), WorkspaceError> {
    let (kind, instrument, indicator_name, indicator_params) = encode_layer_kind(&layer.kind);
    tx.execute(
        "INSERT INTO layers (id, pane_id, position, kind, instrument, indicator_name, indicator_params, visible, style)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            LayerId::new(),
            pane_id,
            layer.position,
            kind,
            instrument,
            indicator_name,
            indicator_params,
            layer.visible,
            layer.style,
        ],
    )?;
    Ok(())
}

/// Checks an indicator layer's `params` parses as JSON before it is ever
/// written — this crate does not interpret the value beyond that
/// R4 owns the indicator engine), but a value that cannot even be parsed
/// back is worth refusing at the door.
fn validate_layer_params(kind: &LayerKind) -> Result<(), WorkspaceError> {
    let params = match kind {
        LayerKind::OverlayInstrument { .. } => return Ok(()),
        LayerKind::IndicatorOverlay { params, .. } | LayerKind::IndicatorSubPane { params, .. } => {
            params
        }
    };
    serde_json::from_str::<serde_json::Value>(params)
        .map_err(|e| WorkspaceError::InvalidIndicatorParams(e.to_string()))?;
    Ok(())
}

/// Checks a pane's `settings` parses as JSON before it is ever written,
/// for the same reason [`validate_layer_params`] does for an indicator
/// layer's `params`: this crate does not interpret a pane's display
/// settings at all (the chart front end's job), but a value that
/// cannot even be parsed back is worth refusing at the door.
fn validate_pane_settings(settings: &str) -> Result<(), WorkspaceError> {
    serde_json::from_str::<serde_json::Value>(settings)
        .map_err(|e| WorkspaceError::InvalidPaneSettings(e.to_string()))?;
    Ok(())
}

/// Encodes a [`LayerKind`] as `layers`' four kind-dependent columns.
fn encode_layer_kind(kind: &LayerKind) -> (&'static str, Option<&str>, Option<&str>, Option<&str>) {
    match kind {
        LayerKind::OverlayInstrument { instrument } => {
            ("overlay_instrument", Some(instrument.as_str()), None, None)
        }
        LayerKind::IndicatorOverlay { name, params } => (
            "indicator_overlay",
            None,
            Some(name.as_str()),
            Some(params.as_str()),
        ),
        LayerKind::IndicatorSubPane { name, params } => (
            "indicator_sub_pane",
            None,
            Some(name.as_str()),
            Some(params.as_str()),
        ),
    }
}

/// Decodes `layers`' four kind-dependent columns back into a [`LayerKind`].
fn decode_layer_kind(
    kind: &str,
    instrument: Option<String>,
    name: Option<String>,
    params: Option<String>,
) -> Result<LayerKind, WorkspaceError> {
    match kind {
        "overlay_instrument" => {
            let instrument = instrument.ok_or_else(|| {
                WorkspaceError::CorruptLayer(
                    "overlay_instrument layer row is missing its instrument".to_owned(),
                )
            })?;
            let instrument = InstrumentId::parse(&instrument)
                .map_err(|e| WorkspaceError::CorruptInstrumentId(e.to_string()))?;
            Ok(LayerKind::OverlayInstrument { instrument })
        }
        "indicator_overlay" | "indicator_sub_pane" => {
            let name = name.ok_or_else(|| {
                WorkspaceError::CorruptLayer(format!("{kind} layer row is missing its name"))
            })?;
            let params = params.ok_or_else(|| {
                WorkspaceError::CorruptLayer(format!("{kind} layer row is missing its params"))
            })?;
            Ok(if kind == "indicator_overlay" {
                LayerKind::IndicatorOverlay { name, params }
            } else {
                LayerKind::IndicatorSubPane { name, params }
            })
        }
        other => Err(WorkspaceError::CorruptLayer(format!(
            "unknown layer kind `{other}`"
        ))),
    }
}

/// Inserts one drawing row for `pane_id` inside an already-open
/// transaction.
fn insert_drawing(
    tx: &Transaction<'_>,
    pane_id: PaneId,
    drawing: &DrawingInput,
) -> Result<(), WorkspaceError> {
    let (kind, price, time1, price1, time2, price2) = encode_drawing_kind(&drawing.kind);
    let (color, width, line_style) = encode_drawing_style(&drawing.style);
    tx.execute(
        "INSERT INTO drawings (id, pane_id, position, kind, price, time1, price1, time2, price2, color, width, line_style)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            DrawingId::new(),
            pane_id,
            drawing.position,
            kind,
            price,
            time1,
            price1,
            time2,
            price2,
            color,
            width,
            line_style,
        ],
    )?;
    Ok(())
}

/// Checks a drawing's style before it is ever written: `width` must be one
/// of the four `lightweight-charts` stroke widths, and `color` must be a
/// plain `#rrggbb` hex string — the same vocabulary this project's own
/// `layer-style.ts` already uses for a plot's colour, so a client never has
/// to translate between two colour formats.
fn validate_drawing_style(style: &DrawingStyle) -> Result<(), WorkspaceError> {
    if !(1..=4).contains(&style.width) {
        return Err(WorkspaceError::InvalidDrawingStyle(format!(
            "width must be 1-4, got {}",
            style.width
        )));
    }
    let valid_color = style.color.len() == 7
        && style.color.starts_with('#')
        && style.color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid_color {
        return Err(WorkspaceError::InvalidDrawingStyle(format!(
            "color must be `#rrggbb`, got `{}`",
            style.color
        )));
    }
    Ok(())
}

/// `drawings`' five kind-dependent columns: the `kind` tag, then `price`,
/// `time1`, `price1`, `time2`, `price2` in that order.
type DrawingGeometryColumns = (
    &'static str,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
);

/// Encodes a [`DrawingKind`] as [`DrawingGeometryColumns`].
fn encode_drawing_kind(kind: &DrawingKind) -> DrawingGeometryColumns {
    match kind {
        DrawingKind::HorizontalLine { price } => {
            ("horizontal_line", Some(*price), None, None, None, None)
        }
        DrawingKind::TrendLine { start, end } => (
            "trend_line",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
        ),
        DrawingKind::Rectangle { start, end } => (
            "rectangle",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
        ),
    }
}

/// Decodes `drawings`' five kind-dependent columns back into a
/// [`DrawingKind`].
fn decode_drawing_kind(
    kind: &str,
    price: Option<f64>,
    time1: Option<i64>,
    price1: Option<f64>,
    time2: Option<i64>,
    price2: Option<f64>,
) -> Result<DrawingKind, WorkspaceError> {
    match kind {
        "horizontal_line" => {
            let price = price.ok_or_else(|| {
                WorkspaceError::CorruptDrawing(
                    "horizontal_line drawing row is missing its price".to_owned(),
                )
            })?;
            Ok(DrawingKind::HorizontalLine { price })
        }
        "trend_line" | "rectangle" => {
            let missing = || {
                WorkspaceError::CorruptDrawing(format!(
                    "{kind} drawing row is missing one of its two points"
                ))
            };
            let start = DrawingPoint {
                time: UnixNanos::from_nanos(time1.ok_or_else(missing)?),
                price: price1.ok_or_else(missing)?,
            };
            let end = DrawingPoint {
                time: UnixNanos::from_nanos(time2.ok_or_else(missing)?),
                price: price2.ok_or_else(missing)?,
            };
            Ok(if kind == "trend_line" {
                DrawingKind::TrendLine { start, end }
            } else {
                DrawingKind::Rectangle { start, end }
            })
        }
        other => Err(WorkspaceError::CorruptDrawing(format!(
            "unknown drawing kind `{other}`"
        ))),
    }
}

/// Encodes a [`DrawingStyle`] as `drawings`' three style columns.
fn encode_drawing_style(style: &DrawingStyle) -> (&str, u32, &'static str) {
    let line_style = match style.line_style {
        DrawingLineStyle::Solid => "solid",
        DrawingLineStyle::Dashed => "dashed",
        DrawingLineStyle::Dotted => "dotted",
    };
    (style.color.as_str(), u32::from(style.width), line_style)
}

/// Decodes `drawings`' three style columns back into a [`DrawingStyle`].
fn decode_drawing_style(
    color: String,
    width: u32,
    line_style: &str,
) -> Result<DrawingStyle, WorkspaceError> {
    let line_style = match line_style {
        "solid" => DrawingLineStyle::Solid,
        "dashed" => DrawingLineStyle::Dashed,
        "dotted" => DrawingLineStyle::Dotted,
        other => {
            return Err(WorkspaceError::CorruptDrawing(format!(
                "unknown drawing line style `{other}`"
            )));
        }
    };
    let width = u8::try_from(width).map_err(|_| {
        WorkspaceError::CorruptDrawing(format!("drawing width out of range: {width}"))
    })?;
    Ok(DrawingStyle {
        color,
        width,
        line_style,
    })
}

fn row_to_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceSummary> {
    Ok(WorkspaceSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

/// The default single-pane layout every freshly created workspace starts
/// with. Shared by [`WorkspaceStore::create_workspace`] and
/// [`WorkspaceStore::get_or_create_default_workspace`] so the two entry
/// points cannot drift on what "a workable default chart" means.
fn insert_default_workspace(
    tx: &Transaction<'_>,
    owner: UserId,
    name: &str,
) -> Result<WorkspaceId, WorkspaceError> {
    let workspace_id = WorkspaceId::new();
    let now = now_unix();
    tx.execute(
        "INSERT INTO workspaces (id, owner_id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
        params![workspace_id, owner, name, now],
    )?;

    let layout_id = LayoutId::new();
    tx.execute(
        "INSERT INTO layouts (id, workspace_id, name, preset, position, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
        params![
            layout_id,
            workspace_id,
            DEFAULT_LAYOUT_NAME,
            LayoutPreset::One.to_string(),
            now,
        ],
    )?;

    tx.execute(
        "INSERT INTO panes (id, layout_id, position, instrument, timeframe) VALUES (?1, ?2, 0, ?3, ?4)",
        params![
            PaneId::new(),
            layout_id,
            default_instrument().as_str(),
            default_timeframe().to_string(),
        ],
    )?;

    Ok(workspace_id)
}

/// Finds `owner`'s oldest workspace and its first layout, if any exist —
/// the "does this account already have a default" check
/// [`WorkspaceStore::get_or_create_default_workspace`] needs before
/// deciding whether to create one.
fn existing_default(
    conn: &Connection,
    owner: UserId,
) -> Result<Option<(WorkspaceId, LayoutId)>, WorkspaceError> {
    let found = conn
        .query_row(
            "SELECT w.id, l.id FROM workspaces w
             JOIN layouts l ON l.workspace_id = w.id
             WHERE w.owner_id = ?1
             ORDER BY w.created_at ASC, l.position ASC
             LIMIT 1",
            [owner],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(found)
}

/// The current time as a Unix timestamp, for `created_at`/`updated_at`.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore};
    use senken_series::BarUnit;
    use tempfile::TempDir;

    use super::{
        DrawingInput, DrawingKind, DrawingLineStyle, DrawingPoint, DrawingStyle, LayerInput,
        LayerKind, LayoutPreset, PaneInput, WorkspaceError, WorkspaceStore,
    };
    use senken_marketdata::InstrumentId;

    fn temp_stores() -> (TempDir, IdentityStore, WorkspaceStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let workspace = WorkspaceStore::new(&identity);
        (dir, identity, workspace)
    }

    const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";

    /// Sets the seeded default admin's password, logs in, and resolves the
    /// session — the seeded `Superadmin` role holds every `(Action,
    /// Resource)` pair at `Scope::All`, so this actor can always proceed.
    fn admin_auth(identity: &IdentityStore) -> AuthenticatedUser {
        identity
            .set_password(
                senken_identity::DEFAULT_ADMIN_EMAIL,
                ADMIN_TEST_PASSWORD,
                None,
            )
            .unwrap();
        let (_uid, token) = identity
            .login(senken_identity::DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    /// Creates an ordinary account with exactly the grants a real "Charts
    /// User" role would carry — View/Create/Edit/Delete on Workspace and
    /// Layout, at `Scope::Own` — since a freshly created account otherwise
    /// holds no grants at all and every guarded method here
    /// would refuse it outright.
    fn charts_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Charts User", Some("a very long password"))
            .unwrap();
        for resource in [Resource::Workspace, Resource::Layout] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    // --- B6/B7: scope reaches the query, including the total -----------

    #[test]
    fn two_users_cannot_see_each_others_workspaces_and_the_total_respects_scope_too() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice@example.com");
        let bob = charts_user(&identity, &admin, "bob@example.com");

        workspace
            .create_workspace(&alice, "Alice's Charts")
            .unwrap();
        workspace.create_workspace(&bob, "Bob's Charts").unwrap();
        workspace.create_workspace(&bob, "Bob's Second").unwrap();

        let alice_page = workspace.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(alice_page.rows[0].name, "Alice's Charts");
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too — otherwise pagination leaks \
             how many workspaces exist"
        );

        let bob_page = workspace.list_workspaces(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
        assert!(
            bob_page.rows.iter().all(|w| w.owner_id == bob.user_id()),
            "bob must never see alice's workspace"
        );
    }

    #[test]
    fn a_superadmin_sees_every_users_workspaces() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice2@example.com");
        let bob = charts_user(&identity, &admin, "bob2@example.com");
        workspace.create_workspace(&alice, "Alice").unwrap();
        workspace.create_workspace(&bob, "Bob").unwrap();

        let page = workspace.list_workspaces(&admin, 50, 0).unwrap();
        assert_eq!(
            page.total, 2,
            "the superadmin must see both users' workspaces"
        );
        assert_eq!(page.rows.len(), 2);
    }

    // --- B1: default workspace on first open ----------------------------

    #[test]
    fn opening_charts_with_no_workspace_creates_a_default_one_and_a_second_open_does_not_duplicate_it()
     {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice3@example.com");

        let (first_workspace, first_layout) =
            workspace.get_or_create_default_workspace(&alice).unwrap();
        let (second_workspace, second_layout) =
            workspace.get_or_create_default_workspace(&alice).unwrap();

        assert_eq!(
            first_workspace, second_workspace,
            "must return the same workspace"
        );
        assert_eq!(first_layout, second_layout, "must return the same layout");

        let page = workspace.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(
            page.total, 1,
            "opening twice must not create a second workspace"
        );

        let detail = workspace.get_layout(&alice, first_layout).unwrap();
        assert_eq!(
            detail.panes.len(),
            1,
            "the default layout is never an empty state"
        );
    }

    // --- headless caller refused by the store itself --------------------

    #[test]
    fn a_headless_caller_without_the_create_workspace_grant_is_refused_by_the_store_itself() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let user_id = identity
            .create_user(
                &admin,
                "powerless@example.com",
                "Powerless",
                Some("a very long password"),
            )
            .unwrap();
        let (_uid, token) = identity
            .login("powerless@example.com", "a very long password")
            .unwrap();
        let powerless = identity.resolve_session(token.reveal()).unwrap().unwrap();
        let _ = user_id;

        let err = workspace
            .create_workspace(&powerless, "Sneaky")
            .unwrap_err();
        assert!(
            matches!(err, WorkspaceError::Identity(IdentityError::Forbidden)),
            "no HTTP layer is involved in this test at all — the store must refuse this itself"
        );
    }

    #[test]
    fn a_headless_caller_without_the_view_workspace_grant_cannot_list_via_the_store() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        identity
            .create_user(
                &admin,
                "powerless2@example.com",
                "Powerless Two",
                Some("a very long password"),
            )
            .unwrap();
        let (_uid, token) = identity
            .login("powerless2@example.com", "a very long password")
            .unwrap();
        let powerless = identity.resolve_session(token.reveal()).unwrap().unwrap();

        let err = workspace.list_workspaces(&powerless, 10, 0).unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::Identity(IdentityError::Forbidden)
        ));
    }

    // --- B9: a layout change is transactional ---------------------------

    #[test]
    fn a_layout_change_is_transactional_a_failure_part_way_leaves_the_previous_layout_intact() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice4@example.com");

        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let before = workspace.get_layout(&alice, layout_id).unwrap();

        // Two panes sharing `position = 0` violates `panes`' `UNIQUE
        // (layout_id, position)` constraint on the second insert, partway
        // through the transaction.
        let broken_panes = vec![
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(15, BarUnit::Minute),
                layers: vec![LayerInput {
                    position: 0,
                    kind: LayerKind::OverlayInstrument {
                        instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                    },
                    visible: true,
                    style: "{}".to_owned(),
                }],
                drawings: vec![],
                settings: "{}".to_owned(),
            },
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                layers: vec![],
                drawings: vec![],
                settings: "{}".to_owned(),
            },
        ];

        let err = workspace
            .replace_layout(
                &alice,
                layout_id,
                LayoutPreset::TwoHorizontal,
                &broken_panes,
            )
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::Database(_)));

        let after = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(
            before, after,
            "a failed layout change must leave the previous layout intact"
        );
    }

    #[test]
    fn a_successful_layout_change_replaces_the_pane_and_layer_structure() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice5@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(15, BarUnit::Minute),
                layers: vec![LayerInput {
                    position: 0,
                    kind: LayerKind::IndicatorSubPane {
                        name: "RSI".to_owned(),
                        params: r#"{"period":14}"#.to_owned(),
                    },
                    visible: true,
                    style: "{}".to_owned(),
                }],
                drawings: vec![DrawingInput {
                    position: 0,
                    kind: DrawingKind::HorizontalLine { price: 2450.5 },
                    style: DrawingStyle {
                        color: "#f2f2ef".to_owned(),
                        width: 2,
                        line_style: DrawingLineStyle::Dashed,
                    },
                }],
                settings: "{}".to_owned(),
            },
            PaneInput {
                position: 1,
                instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                layers: vec![],
                drawings: vec![],
                settings: "{}".to_owned(),
            },
        ];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::TwoHorizontal, &panes)
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(detail.layout.preset, LayoutPreset::TwoHorizontal);
        assert_eq!(detail.panes.len(), 2);
        assert_eq!(
            detail.panes[0].instrument,
            InstrumentId::parse("binance-spot:ETHUSDT").unwrap()
        );
        assert_eq!(detail.panes[0].layers.len(), 1);
        assert!(matches!(
            &detail.panes[0].layers[0].kind,
            LayerKind::IndicatorSubPane { name, .. } if name == "RSI"
        ));
        assert_eq!(detail.panes[0].drawings.len(), 1);
        assert_eq!(
            detail.panes[0].drawings[0].kind,
            DrawingKind::HorizontalLine { price: 2450.5 }
        );
        assert_eq!(
            detail.panes[0].drawings[0].style,
            DrawingStyle {
                color: "#f2f2ef".to_owned(),
                width: 2,
                line_style: DrawingLineStyle::Dashed,
            }
        );
        assert_eq!(
            detail.panes[1].drawings.len(),
            0,
            "a pane's drawings must not leak into its sibling"
        );
    }

    #[test]
    fn a_trend_line_and_a_rectangle_round_trip_through_replace_layout_and_get_layout() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice-drawings@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let start = DrawingPoint {
            time: senken_core::UnixNanos::from_secs(1_700_000_000).unwrap(),
            price: 100.25,
        };
        let end = DrawingPoint {
            time: senken_core::UnixNanos::from_secs(1_700_003_600).unwrap(),
            price: 101.75,
        };
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            layers: vec![],
            drawings: vec![
                DrawingInput {
                    position: 0,
                    kind: DrawingKind::TrendLine { start, end },
                    style: DrawingStyle {
                        color: "#7aa7e8".to_owned(),
                        width: 1,
                        line_style: DrawingLineStyle::Solid,
                    },
                },
                DrawingInput {
                    position: 1,
                    kind: DrawingKind::Rectangle { start, end },
                    style: DrawingStyle {
                        color: "#e8836f".to_owned(),
                        width: 3,
                        line_style: DrawingLineStyle::Dotted,
                    },
                },
            ],
            settings: "{}".to_owned(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(detail.panes[0].drawings.len(), 2);
        assert_eq!(
            detail.panes[0].drawings[0].kind,
            DrawingKind::TrendLine { start, end }
        );
        assert_eq!(
            detail.panes[0].drawings[1].kind,
            DrawingKind::Rectangle { start, end }
        );
    }

    #[test]
    fn replace_layout_rejects_a_drawing_width_outside_one_to_four() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice-bad-width@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            layers: vec![],
            drawings: vec![DrawingInput {
                position: 0,
                kind: DrawingKind::HorizontalLine { price: 1.0 },
                style: DrawingStyle {
                    color: "#f2f2ef".to_owned(),
                    width: 9,
                    line_style: DrawingLineStyle::Solid,
                },
            }],
            settings: "{}".to_owned(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidDrawingStyle(_)));

        // The rejected call must not have written anything — the pane still
        // has zero drawings, not a half-applied one.
        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(detail.panes[0].drawings.len(), 0);
    }

    #[test]
    fn replace_layout_rejects_a_drawing_color_that_is_not_hex() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice-bad-color@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            layers: vec![],
            drawings: vec![DrawingInput {
                position: 0,
                kind: DrawingKind::HorizontalLine { price: 1.0 },
                style: DrawingStyle {
                    color: "rgba(0,0,0,1)".to_owned(),
                    width: 1,
                    line_style: DrawingLineStyle::Solid,
                },
            }],
            settings: "{}".to_owned(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidDrawingStyle(_)));
    }

    #[test]
    fn replace_layout_rejects_a_pane_count_that_does_not_match_the_preset() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice6@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let one_pane = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            layers: vec![],
            drawings: vec![],
            settings: "{}".to_owned(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::TwoHorizontal, &one_pane)
            .unwrap_err();
        assert!(matches!(
            err,
            WorkspaceError::PaneCountMismatch {
                expected: 2,
                actual: 1,
                ..
            }
        ));
    }

    #[test]
    fn replace_layout_rejects_indicator_params_that_are_not_valid_json() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice7@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            layers: vec![LayerInput {
                position: 0,
                kind: LayerKind::IndicatorOverlay {
                    name: "EMA".to_owned(),
                    params: "not json".to_owned(),
                },
                visible: true,
                style: "{}".to_owned(),
            }],
            drawings: vec![],
            settings: "{}".to_owned(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, WorkspaceError::InvalidIndicatorParams(_)));
    }
}
