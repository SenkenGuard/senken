//! [`ChartWorkspaceStore`]: the guarded query API for chart workspaces,
//! layouts, panes and pane items.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use senken_acl::{Action, Resource, Scope};
use senken_core::UnixNanos;
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};
use senken_indicators::LabelAnchor;
use senken_marketdata::InstrumentId;
use senken_series::BarSpec;

use crate::error::ChartError;
use crate::id::{ChartLayoutId, ChartPaneId, ChartWorkspaceId, PaneItemId};
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

/// Where a pane item sits within its pane's grid slot. Orthogonal to what
/// the item draws ([`ItemSource`]) — an indicator can be placed as an
/// overlay or in its own sub-pane without changing what kind of thing it
/// is, the same way moving a window between two monitors does not change
/// what program is running in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The pane's main price plot.
    Main,
    /// A sub-pane beneath the main one, identified by index. This build
    /// only ever creates index `0` (there has only ever been one sub-pane
    /// region per pane), but the shape does not bake that limit in.
    Sub(u8),
}

/// What a pane item draws, and where its data comes from. Replaces the
/// old `LayerKind`/`DrawingKind` split across two tables: a layer's
/// `overlay_instrument` becomes [`Referenced`](Self::Referenced), its two
/// indicator kinds collapse into [`Computed`](Self::Computed) now that
/// placement lives on [`Slot`] instead of being baked into the kind tag,
/// and a drawing becomes [`Anchored`](Self::Anchored).
#[derive(Debug, Clone, PartialEq)]
pub enum ItemSource {
    /// A built-in indicator, computed from the pane's own bars.
    Computed {
        /// The indicator's name (e.g. `"EMA"`).
        name: String,
        /// The indicator's parameters, as a JSON object — this crate does
        /// not interpret it beyond checking it parses as JSON, never its
        /// shape.
        params: String,
    },
    /// A second instrument plotted on the same pane as the main one.
    Referenced {
        /// The overlaid instrument.
        instrument: InstrumentId,
    },
    /// A user-drawn chart annotation.
    Anchored {
        /// What kind of drawing this is, and its geometry.
        kind: DrawingKind,
        /// The instrument this annotation was drawn against.
        ///
        /// A drawing's anchors are prices and instants on one particular
        /// market: a trend line across Bitcoin's range says nothing about
        /// Ethereum, and drawing it over Ethereum's candles because the
        /// same pane happens to be showing them is a claim nobody made.
        /// A pane keeps its annotations per instrument and shows the ones
        /// that belong to whatever it is currently displaying.
        ///
        /// `None` for a drawing stored before this field existed: its
        /// instrument is genuinely unknown, and guessing one would be
        /// worse than saying so. A reader deciding what to draw treats
        /// `None` as "belongs to no instrument in particular".
        instrument: Option<InstrumentId>,
    },
}

/// One user-drawn chart annotation's kind and geometry. `HorizontalLine`/
/// `TrendLine`/`Rectangle` are the original three; `Ray`/`FibRetracement`/
/// `TextNote` extend the persisted shape so 015's own tool/anchor mapping
/// (`Ray` → `Drawable::Segment { extend: Forward }`, `FibRetracement` →
/// `Drawable::Level × 6`, `TextNote` → `Drawable::Label`) has somewhere to
/// round-trip through — this crate does not render any of them, only
/// stores and returns their geometry, the same as it always has for the
/// first three.
#[derive(Debug, Clone, PartialEq)]
pub enum DrawingKind {
    /// A single price level, drawn across the pane's full width.
    HorizontalLine {
        /// The line's price level — a coordinate the user set by clicking
        /// the chart, not an order price.
        price: f64,
    },
    /// A straight line between two (time, price) points, unextended.
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
    /// A straight line through two points, extended forever past the
    /// second one.
    Ray {
        /// The ray's origin.
        start: DrawingPoint,
        /// The ray's second anchor; the ray extends past this point.
        end: DrawingPoint,
    },
    /// A Fibonacci retracement between a high and a low.
    FibRetracement {
        /// One end of the retracement range.
        start: DrawingPoint,
        /// The other end.
        end: DrawingPoint,
    },
    /// Free text anchored to one chart point.
    TextNote {
        /// Where the note is anchored.
        at: DrawingPoint,
        /// The note's text.
        text: String,
        /// The label's position relative to `at`, reusing
        /// `senken_indicators::LabelAnchor` rather than a second
        /// three-value enum of the same shape.
        anchor: LabelAnchor,
    },
}

/// One (time, price) anchor for a multi-point [`DrawingKind`].
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
/// object rather than per-segment. Persisted as JSON text inside a pane
/// item's own `style` column — [`Self::to_json`]/[`Self::from_json`] are
/// this crate's typed view onto that one shared encoding, not a second
/// column-level one.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawingStyle {
    /// The drawing's colour, as `#rrggbb`.
    pub color: String,
    /// Stroke width in pixels, 1 through 4.
    pub width: u8,
    /// The line's dash pattern.
    pub line_style: DrawingLineStyle,
}

impl DrawingStyle {
    /// Encodes this style as the JSON object text a pane item's `style`
    /// column holds.
    #[must_use]
    pub fn to_json(&self) -> String {
        let line_style = match self.line_style {
            DrawingLineStyle::Solid => "solid",
            DrawingLineStyle::Dashed => "dashed",
            DrawingLineStyle::Dotted => "dotted",
        };
        serde_json::json!({
            "color": self.color,
            "width": self.width,
            "line_style": line_style,
        })
        .to_string()
    }

    /// Decodes a pane item's `style` JSON text back into a style, or
    /// [`ChartError::InvalidDrawingStyle`] if it is not valid JSON, is
    /// missing a field, or holds a `width`/`color` no chart could render.
    ///
    /// # Errors
    /// [`ChartError::InvalidDrawingStyle`] as described above.
    pub fn from_json(text: &str) -> Result<Self, ChartError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| ChartError::InvalidDrawingStyle(e.to_string()))?;
        let invalid = || ChartError::InvalidDrawingStyle(format!("malformed style: {text}"));
        let color = value
            .get("color")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(invalid)?
            .to_owned();
        let width = value
            .get("width")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(invalid)?;
        let width = u8::try_from(width).map_err(|_| invalid())?;
        let line_style = match value.get("line_style").and_then(serde_json::Value::as_str) {
            Some("solid") => DrawingLineStyle::Solid,
            Some("dashed") => DrawingLineStyle::Dashed,
            Some("dotted") => DrawingLineStyle::Dotted,
            _ => return Err(invalid()),
        };
        let style = Self {
            color,
            width,
            line_style,
        };
        validate_drawing_style(&style)?;
        Ok(style)
    }
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

/// One pane's main instrument, timeframe, item stack and settings, as read
/// back from [`ChartWorkspaceStore::get_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct PaneRecord {
    /// The pane's id.
    pub id: ChartPaneId,
    /// This pane's slot within its layout's grid.
    pub position: u32,
    /// The pane's main instrument ("a pane holds one main instrument plus items").
    pub instrument: InstrumentId,
    /// The main instrument's bar timeframe.
    pub timeframe: BarSpec,
    /// This pane's items — computed indicators, referenced overlay
    /// instruments, and anchored drawings alike — in stacking order.
    pub items: Vec<PaneItemRecord>,
    /// Display settings for this pane's chart — candle colours, precision,
    /// scales, canvas, status line — as opaque JSON-object text. This crate
    /// does not interpret it: the chart front end owns what these settings
    /// mean.
    pub settings: String,
}

/// One pane item, as read back from [`ChartWorkspaceStore::get_layout`] —
/// a computed indicator, a referenced overlay instrument, or an anchored
/// drawing, unified: one id space, one `position`/`visible`/`style`
/// encoding, one select/hit-test/edit/delete/persist path.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneItemRecord {
    /// The item's id.
    pub id: PaneItemId,
    /// This item's stacking order within its pane.
    pub position: u32,
    /// Where this item is placed within the pane's grid slot.
    pub slot: Slot,
    /// Whether the item is currently shown. Applies uniformly — an
    /// anchored drawing could never express this before schema v8.
    pub visible: bool,
    /// How this item is drawn, as opaque JSON-object text. This crate
    /// checks only that it parses as JSON for a `Computed`/`Referenced`
    /// item; for an `Anchored` item it additionally validates the
    /// `color`/`width`/`line_style` shape [`DrawingStyle`] names — see
    /// [`DrawingStyle::to_json`]/[`DrawingStyle::from_json`].
    pub style: String,
    /// What this item draws, and where its data comes from.
    pub source: ItemSource,
}

/// A layout's identity and grid arrangement, without its panes — returned
/// by [`ChartWorkspaceStore::list_layouts`]; see [`ChartLayoutDetail`] for
/// the full nested structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartLayoutSummary {
    /// The layout's id.
    pub id: ChartLayoutId,
    /// The workspace this layout belongs to.
    pub workspace_id: ChartWorkspaceId,
    /// The layout's display name.
    pub name: String,
    /// Which pane-grid arrangement this layout uses.
    pub preset: LayoutPreset,
    /// This layout's tab order within its workspace.
    pub position: u32,
}

/// A layout together with its panes and their items — the full nested
/// structure of chart workspace → layout → panes → items, minus the
/// workspace itself.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartLayoutDetail {
    /// The layout's identity and grid arrangement.
    pub layout: ChartLayoutSummary,
    /// The layout's panes, in grid-slot order.
    pub panes: Vec<PaneRecord>,
}

/// A chart workspace row as returned by a guarded listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartWorkspaceSummary {
    /// The workspace's id.
    pub id: ChartWorkspaceId,
    /// The account that owns this workspace.
    pub owner_id: UserId,
    /// The workspace's display name.
    pub name: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the workspace's own fields
    /// (its name or its `settings`) — a layout's own `updated_at` moves
    /// independently.
    pub updated_at: i64,
    /// Display settings for the workspace as a whole, as opaque
    /// JSON-object text — this crate does not interpret it beyond checking
    /// it parses as a JSON object, the same non-interpretation rule
    /// [`PaneInput::settings`] already documents for a pane's own settings.
    pub settings: String,
}

/// One pane to write, supplied to
/// [`ChartWorkspaceStore::replace_layout`].
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
    /// This pane's items.
    pub items: Vec<PaneItemInput>,
    /// Display settings for this pane's chart, as opaque JSON-object text —
    /// see [`PaneRecord::settings`]. [`ChartWorkspaceStore::replace_layout`]
    /// checks only that this parses as JSON.
    pub settings: String,
}

/// One pane item to write, supplied as part of a [`PaneInput`] or directly
/// to [`ChartWorkspaceStore::update_pane_item`].
#[derive(Debug, Clone, PartialEq)]
pub struct PaneItemInput {
    /// This item's stacking order within its pane. Must be unique among a
    /// pane's own items — see [`PaneRecord::items`]. Only
    /// [`ChartWorkspaceStore::replace_layout`] (a structural rewrite) ever
    /// changes it; [`ChartWorkspaceStore::update_pane_item`] ignores this
    /// field entirely — see that method's own docs for why.
    pub position: u32,
    /// Where this item is placed within the pane's grid slot.
    pub slot: Slot,
    /// Whether the item starts out visible.
    pub visible: bool,
    /// How this item is drawn, as opaque JSON-object text — see
    /// [`PaneItemRecord::style`].
    pub style: String,
    /// What this item draws, and where its data comes from.
    pub source: ItemSource,
}

/// Guarded queries over chart workspaces, layouts, panes and pane items.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second one —
/// see this crate's module docs for why. Every mutation and every listing
/// goes through an [`AuthenticatedUser`] and calls
/// [`AuthenticatedUser::authorize`] exactly the way `senken-identity`'s own
/// guarded queries do: there is no method here that reads
/// or writes a row without that check running first.
#[derive(Debug)]
pub struct ChartWorkspaceStore {
    conn: Arc<Mutex<Connection>>,
}

impl ChartWorkspaceStore {
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
    /// Requires `auth` to hold `Action::Create` on both
    /// `Resource::ChartWorkspace` and `Resource::ChartLayout` — creating a
    /// workspace always creates its first layout too.
    ///
    /// # Errors
    /// [`ChartError::Identity`] (wrapping
    /// [`IdentityError::PasswordNotSet`] or
    /// [`IdentityError::Forbidden`](senken_identity::IdentityError::Forbidden))
    /// if `auth` may not create a workspace or a layout; otherwise as
    /// [`ChartError::Database`].
    pub fn create_workspace(
        &self,
        auth: &AuthenticatedUser,
        name: &str,
    ) -> Result<ChartWorkspaceId, ChartError> {
        auth.authorize(Action::Create, Resource::ChartWorkspace)?;
        auth.authorize(Action::Create, Resource::ChartLayout)?;
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
    /// Requires `auth` to hold `Action::View` on `Resource::ChartWorkspace`
    /// always, and additionally `Action::Create` on both
    /// `Resource::ChartWorkspace` and `Resource::ChartLayout` the first
    /// time (when there is nothing yet to view).
    ///
    /// # Errors
    /// [`ChartError::Identity`] if `auth` may not view (or, on first
    /// use, create) a workspace/layout; otherwise as
    /// [`ChartError::Database`].
    pub fn get_or_create_default_workspace(
        &self,
        auth: &AuthenticatedUser,
    ) -> Result<(ChartWorkspaceId, ChartLayoutId), ChartError> {
        auth.authorize(Action::View, Resource::ChartWorkspace)?;
        // Held for the whole check-then-create below, so a second call
        // racing this one on another thread cannot observe "no default
        // yet" for both and create two.
        let mut conn = self.lock();
        if let Some(found) = existing_default(&conn, auth.user_id())? {
            return Ok(found);
        }

        auth.authorize(Action::Create, Resource::ChartWorkspace)?;
        auth.authorize(Action::Create, Resource::ChartLayout)?;
        let tx = conn.transaction()?;
        let workspace_id = insert_default_workspace(&tx, auth.user_id(), DEFAULT_WORKSPACE_NAME)?;
        let layout_id: ChartLayoutId = tx.query_row(
            "SELECT id FROM chart_layouts WHERE workspace_id = ?1 ORDER BY position LIMIT 1",
            params![workspace_id],
            |row| row.get(0),
        )?;
        tx.commit()?;
        Ok((workspace_id, layout_id))
    }

    /// Lists workspaces visible to `auth`, scoped: the
    /// `WHERE` clause is chosen by `auth`'s [`senken_acl::decide`]d
    /// [`Scope`] for `(Action::View, Resource::ChartWorkspace)`, and
    /// [`Page::total`] is counted under that same clause.
    ///
    /// # Errors
    /// [`ChartError::Identity`] if `auth` may not view workspaces at
    /// all, or if `decide` returns a [`Scope`] variant this crate does not
    /// yet translate to SQL; otherwise as [`ChartError::Database`].
    pub fn list_workspaces(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<ChartWorkspaceSummary>, ChartError> {
        let scope = auth.authorize(Action::View, Resource::ChartWorkspace)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM chart_workspaces WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, created_at, updated_at, settings FROM chart_workspaces
                     WHERE owner_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_workspace)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM chart_workspaces", [], |row| {
                        row.get(0)
                    })?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, created_at, updated_at, settings FROM chart_workspaces
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
            _ => return Err(ChartError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Renames a workspace — a field-level edit, not a full
    /// layout rewrite.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::ChartWorkspace`,
    /// with the returned [`Scope`] applied against this workspace's owner:
    /// an actor scoped to `Own` may only rename a workspace they themselves
    /// own, regardless of whether they can *see* someone else's (which
    /// `Action::View` governs separately).
    ///
    /// # Errors
    /// [`ChartError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`ChartError::Identity`] if `auth` may not edit
    /// workspaces at all, or may not reach this particular one; otherwise
    /// as [`ChartError::Database`].
    pub fn rename_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: ChartWorkspaceId,
        new_name: &str,
    ) -> Result<(), ChartError> {
        let scope = auth.authorize(Action::Edit, Resource::ChartWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE chart_workspaces SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_name, now_unix(), workspace_id],
        )?;
        Ok(())
    }

    /// Replaces a workspace's own display settings — a field-level edit,
    /// the same shape as [`rename_workspace`](Self::rename_workspace) but
    /// for `settings` rather than `name`. This crate does not interpret
    /// `settings` beyond checking it is a JSON object; the chart front end
    /// owns everything inside it.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::ChartWorkspace`,
    /// scoped against this workspace's owner the same way
    /// [`rename_workspace`](Self::rename_workspace) is.
    ///
    /// # Errors
    /// [`ChartError::InvalidWorkspaceSettings`] if `settings` is not a JSON
    /// object; [`ChartError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`ChartError::Identity`] if `auth` may not edit this
    /// workspace; otherwise as [`ChartError::Database`].
    pub fn update_workspace_settings(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: ChartWorkspaceId,
        settings: &str,
    ) -> Result<(), ChartError> {
        validate_workspace_settings(settings)?;
        let scope = auth.authorize(Action::Edit, Resource::ChartWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE chart_workspaces SET settings = ?1, updated_at = ?2 WHERE id = ?3",
            params![settings, now_unix(), workspace_id],
        )?;
        Ok(())
    }

    /// Deletes a workspace and, by `ON DELETE CASCADE`, every layout, pane
    /// and pane item it owns.
    ///
    /// Requires `auth` to hold `Action::Delete` on
    /// `Resource::ChartWorkspace`, scoped against this workspace's owner
    /// the same way [`rename_workspace`](Self::rename_workspace) is.
    ///
    /// # Errors
    /// [`ChartError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`ChartError::Identity`] if `auth` may not delete this
    /// workspace; otherwise as [`ChartError::Database`].
    pub fn delete_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: ChartWorkspaceId,
    ) -> Result<(), ChartError> {
        let scope = auth.authorize(Action::Delete, Resource::ChartWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "DELETE FROM chart_workspaces WHERE id = ?1",
            params![workspace_id],
        )?;
        Ok(())
    }

    /// Lists a workspace's layouts (its "tabs"), in tab order.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::ChartLayout`,
    /// scoped against the *workspace's* owner the same way
    /// [`rename_workspace`](Self::rename_workspace) resolves
    /// `Resource::ChartWorkspace`'s scope — a layout has no owner column of
    /// its own, so ownership is always read through the workspace it
    /// belongs to.
    ///
    /// # Errors
    /// [`ChartError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`ChartError::Identity`] if `auth` may not view this
    /// workspace's layouts; [`ChartError::CorruptPreset`] if a stored
    /// preset no longer parses; otherwise as [`ChartError::Database`].
    pub fn list_layouts(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: ChartWorkspaceId,
    ) -> Result<Vec<ChartLayoutSummary>, ChartError> {
        let scope = auth.authorize(Action::View, Resource::ChartLayout)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, name, preset, position FROM chart_layouts
             WHERE workspace_id = ?1 ORDER BY position ASC",
        )?;
        let rows = stmt
            .query_map(params![workspace_id], |row| {
                Ok((
                    row.get::<_, ChartLayoutId>(0)?,
                    row.get::<_, ChartWorkspaceId>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, workspace_id, name, preset, position)| {
                Ok(ChartLayoutSummary {
                    id,
                    workspace_id,
                    name,
                    preset: preset
                        .parse()
                        .map_err(|_| ChartError::CorruptPreset(preset.clone()))?,
                    position,
                })
            })
            .collect()
    }

    /// Fetches one layout with its full nested pane/item structure.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::ChartLayout`,
    /// scoped against the layout's owning workspace.
    ///
    /// # Errors
    /// [`ChartError::LayoutNotFound`] if `layout_id` does not exist;
    /// [`ChartError::Identity`] if `auth` may not view this layout;
    /// [`ChartError::CorruptPreset`]/[`ChartError::CorruptInstrumentId`]/
    /// [`ChartError::CorruptTimeframe`]/[`ChartError::CorruptPaneItem`]/
    /// [`ChartError::CorruptDrawing`] if a stored row no longer parses;
    /// otherwise as [`ChartError::Database`].
    pub fn get_layout(
        &self,
        auth: &AuthenticatedUser,
        layout_id: ChartLayoutId,
    ) -> Result<ChartLayoutDetail, ChartError> {
        let scope = auth.authorize(Action::View, Resource::ChartLayout)?;
        let conn = self.lock();
        let (owner, summary) = load_layout_summary(&conn, layout_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        let panes = load_panes_with_items(&conn, layout_id)?;
        Ok(ChartLayoutDetail {
            layout: summary,
            panes,
        })
    }

    /// Replaces a layout's entire pane/item structure in one transaction
    /// ("a layout change must be transactional"). Either every
    /// pane and item in `panes` is written, or none of them are — a
    /// failure partway through (a duplicate `position` violating a `UNIQUE`
    /// constraint, for instance) rolls the whole change back, leaving the
    /// layout exactly as it was before this call.
    ///
    /// `panes.len()` must equal `preset.pane_count()`.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::ChartLayout`,
    /// scoped against the layout's owning workspace.
    ///
    /// # Errors
    /// [`ChartError::LayoutNotFound`] if `layout_id` does not exist;
    /// [`ChartError::Identity`] if `auth` may not edit this layout;
    /// [`ChartError::PaneCountMismatch`] if `panes.len()` does not match
    /// `preset`; [`ChartError::InvalidIndicatorParams`] if a computed
    /// item's `params` is not valid JSON; [`ChartError::InvalidDrawingStyle`]
    /// if an anchored item's style is malformed; otherwise as
    /// [`ChartError::Database`] (including any constraint violation,
    /// already rolled back by the time it is returned).
    pub fn replace_layout(
        &self,
        auth: &AuthenticatedUser,
        layout_id: ChartLayoutId,
        preset: LayoutPreset,
        panes: &[PaneInput],
    ) -> Result<(), ChartError> {
        let scope = auth.authorize(Action::Edit, Resource::ChartLayout)?;
        if panes.len() != preset.pane_count() {
            return Err(ChartError::PaneCountMismatch {
                preset,
                expected: preset.pane_count(),
                actual: panes.len(),
            });
        }
        for pane in panes {
            validate_pane_settings(&pane.settings)?;
            for item in &pane.items {
                validate_pane_item(item)?;
            }
        }

        let mut conn = self.lock();
        let owner = layout_owner(&conn, layout_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE chart_layouts SET preset = ?1, updated_at = ?2 WHERE id = ?3",
            params![preset.to_string(), now_unix(), layout_id],
        )?;
        // Keep every row whose grid position survives. Chart panes and
        // items are keyed by these ids in the client, so replacing a
        // structural layout must not turn an unrelated parameter edit into a
        // wholesale remount. The transaction still ensures observers see
        // either the old complete layout or the new complete layout.
        let retained_positions = panes
            .iter()
            .map(|pane| pane.position)
            .collect::<HashSet<_>>();
        let existing_panes = tx
            .prepare("SELECT id, position FROM chart_panes WHERE layout_id = ?1")?
            .query_map(params![layout_id], |row| {
                Ok((row.get::<_, ChartPaneId>(0)?, row.get::<_, u32>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (pane_id, position) in existing_panes {
            if !retained_positions.contains(&position) {
                tx.execute("DELETE FROM chart_panes WHERE id = ?1", params![pane_id])?;
            }
        }
        let mut written_positions = HashSet::with_capacity(panes.len());
        for pane in panes {
            if !written_positions.insert(pane.position) {
                // Keep invalid duplicate positions subject to SQLite's own
                // constraint so this transaction has the same all-or-nothing
                // failure semantics as every other structural constraint.
                tx.execute(
                    "INSERT INTO chart_panes (id, layout_id, position, instrument, timeframe, settings)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        ChartPaneId::new(),
                        layout_id,
                        pane.position,
                        pane.instrument.as_str(),
                        pane.timeframe.to_string(),
                        pane.settings
                    ],
                )?;
            }
            let pane_id = tx
                .query_row(
                    "SELECT id FROM chart_panes WHERE layout_id = ?1 AND position = ?2",
                    params![layout_id, pane.position],
                    |row| row.get::<_, ChartPaneId>(0),
                )
                .optional()?;
            let pane_id = if let Some(pane_id) = pane_id {
                tx.execute(
                    "UPDATE chart_panes SET instrument = ?1, timeframe = ?2, settings = ?3 WHERE id = ?4",
                    params![
                        pane.instrument.as_str(),
                        pane.timeframe.to_string(),
                        pane.settings,
                        pane_id
                    ],
                )?;
                pane_id
            } else {
                let pane_id = ChartPaneId::new();
                tx.execute(
                    "INSERT INTO chart_panes (id, layout_id, position, instrument, timeframe, settings)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        pane_id,
                        layout_id,
                        pane.position,
                        pane.instrument.as_str(),
                        pane.timeframe.to_string(),
                        pane.settings
                    ],
                )?;
                pane_id
            };
            replace_pane_items(&tx, pane_id, &pane.items)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Updates one pane item without replacing its pane or its neighbours
    /// ("double-buffer, never blank" is the client's job on top of this —
    /// this call itself is a plain in-place update).
    ///
    /// `item.position` is ignored — this call never reorders an item
    /// relative to its neighbours, only `replace_layout` does that
    /// structurally; a stray position value round-tripped from a `GET`
    /// response (which — see `crate` consumers' own DTO layer — is
    /// renumbered per family, not the item's true stored global position)
    /// must never collide with another item's position.
    ///
    /// The new `item.source`'s family (computed/referenced vs. anchored)
    /// must match the existing item's — this is what keeps
    /// `/api/layers/{id}` and `/api/drawings/{id}` meaningful now that both
    /// update the same underlying row kind.
    ///
    /// # Errors
    /// [`ChartError::PaneItemNotFound`] if `item_id` does not exist;
    /// [`ChartError::ItemSourceMismatch`] if `item.source`'s family differs
    /// from the stored item's; [`ChartError::Identity`] if `auth` lacks
    /// permission; otherwise as [`ChartError::Database`].
    pub fn update_pane_item(
        &self,
        auth: &AuthenticatedUser,
        item_id: PaneItemId,
        item: &PaneItemInput,
    ) -> Result<(), ChartError> {
        validate_pane_item(item)?;
        let scope = auth.authorize(Action::Edit, Resource::ChartLayout)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        let existing_kind: String = tx
            .query_row(
                "SELECT source_kind FROM chart_pane_items WHERE id = ?1",
                params![item_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(ChartError::PaneItemNotFound)?;
        if item_source_family(&existing_kind) != item_source_family(source_kind_of(&item.source)) {
            return Err(ChartError::ItemSourceMismatch);
        }

        let (slot, slot_index) = encode_slot(item.slot);
        let (
            source_kind,
            instrument,
            indicator_name,
            indicator_params,
            tool_kind,
            price,
            time1,
            price1,
            time2,
            price2,
            label_text,
            label_anchor,
        ) = encode_item_source(&item.source);
        let changed = match scope {
            Scope::All => tx.execute(
                "UPDATE chart_pane_items SET slot = ?1, slot_index = ?2, visible = ?3,
                 style = ?4, source_kind = ?5, instrument = ?6, indicator_name = ?7, indicator_params = ?8,
                 tool_kind = ?9, price = ?10, time1 = ?11, price1 = ?12, time2 = ?13, price2 = ?14,
                 label_text = ?15, label_anchor = ?16 WHERE id = ?17",
                params![
                    slot, slot_index, item.visible, item.style,
                    source_kind, instrument, indicator_name, indicator_params,
                    tool_kind, price, time1, price1, time2, price2, label_text, label_anchor,
                    item_id
                ],
            )?,
            Scope::Own => tx.execute(
                "UPDATE chart_pane_items SET slot = ?1, slot_index = ?2, visible = ?3,
                 style = ?4, source_kind = ?5, instrument = ?6, indicator_name = ?7, indicator_params = ?8,
                 tool_kind = ?9, price = ?10, time1 = ?11, price1 = ?12, time2 = ?13, price2 = ?14,
                 label_text = ?15, label_anchor = ?16 WHERE id = ?17 AND pane_id IN
                 (SELECT p.id FROM chart_panes p JOIN chart_layouts l ON l.id = p.layout_id
                  JOIN chart_workspaces w ON w.id = l.workspace_id WHERE w.owner_id = ?18)",
                params![
                    slot, slot_index, item.visible, item.style,
                    source_kind, instrument, indicator_name, indicator_params,
                    tool_kind, price, time1, price1, time2, price2, label_text, label_anchor,
                    item_id, auth.user_id()
                ],
            )?,
            _ => 0,
        };
        if changed == 0 {
            return Err(ChartError::Identity(IdentityError::Forbidden));
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes one pane item without replacing its pane or its neighbours.
    ///
    /// # Errors
    /// Returns an error when the user lacks permission or the item is absent.
    pub fn delete_pane_item(
        &self,
        auth: &AuthenticatedUser,
        item_id: PaneItemId,
    ) -> Result<(), ChartError> {
        let scope = auth.authorize(Action::Edit, Resource::ChartLayout)?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let changed = match scope {
            Scope::All => tx.execute(
                "DELETE FROM chart_pane_items WHERE id = ?1",
                params![item_id],
            )?,
            Scope::Own => tx.execute(
                "DELETE FROM chart_pane_items WHERE id = ?1 AND pane_id IN
                 (SELECT p.id FROM chart_panes p JOIN chart_layouts l ON l.id = p.layout_id
                  JOIN chart_workspaces w ON w.id = l.workspace_id WHERE w.owner_id = ?2)",
                params![item_id, auth.user_id()],
            )?,
            _ => 0,
        };
        if changed == 0 {
            return Err(ChartError::Identity(IdentityError::Forbidden));
        }
        tx.commit()?;
        Ok(())
    }
}

/// Resolves `Scope` against a row's owner, the same check every method
/// above needs after it has already resolved a [`Scope`] from
/// [`AuthenticatedUser::authorize`] for a single-row operation (as opposed
/// to [`ChartWorkspaceStore::list_workspaces`], which applies its `Scope`
/// straight to a `WHERE` clause instead).
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), ChartError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        // Covers both "Own but not this row's owner" and any future
        // `Scope` variant this crate has not been taught to interpret —
        // failing closed either way, the same discipline `list_workspaces`
        // applies to its own `match`.
        _ => Err(ChartError::Identity(IdentityError::Forbidden)),
    }
}

/// Looks up a workspace's owner, or [`ChartError::WorkspaceNotFound`].
fn workspace_owner(
    conn: &Connection,
    workspace_id: ChartWorkspaceId,
) -> Result<UserId, ChartError> {
    conn.query_row(
        "SELECT owner_id FROM chart_workspaces WHERE id = ?1",
        [workspace_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(ChartError::WorkspaceNotFound)
}

/// Looks up the owner of the workspace a layout belongs to, or
/// [`ChartError::LayoutNotFound`].
fn layout_owner(conn: &Connection, layout_id: ChartLayoutId) -> Result<UserId, ChartError> {
    conn.query_row(
        "SELECT w.owner_id FROM chart_layouts l
         JOIN chart_workspaces w ON w.id = l.workspace_id
         WHERE l.id = ?1",
        [layout_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(ChartError::LayoutNotFound)
}

/// Loads a layout's owner (via its workspace) and its own summary row in
/// one round trip, for [`ChartWorkspaceStore::get_layout`].
fn load_layout_summary(
    conn: &Connection,
    layout_id: ChartLayoutId,
) -> Result<(UserId, ChartLayoutSummary), ChartError> {
    let row: Option<(UserId, ChartWorkspaceId, String, String, u32)> = conn
        .query_row(
            "SELECT w.owner_id, l.workspace_id, l.name, l.preset, l.position
             FROM chart_layouts l JOIN chart_workspaces w ON w.id = l.workspace_id
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
    let (owner, workspace_id, name, preset, position) = row.ok_or(ChartError::LayoutNotFound)?;
    let preset = preset
        .parse()
        .map_err(|_| ChartError::CorruptPreset(preset.clone()))?;
    Ok((
        owner,
        ChartLayoutSummary {
            id: layout_id,
            workspace_id,
            name,
            preset,
            position,
        },
    ))
}

/// One `chart_pane_items` row's raw columns, in table order (id through
/// `label_anchor`), decoded by [`decode_pane_item`].
type PaneItemRow = (
    PaneItemId,
    u32,
    String,
    Option<i64>,
    bool,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<String>,
    Option<String>,
);

/// Loads every pane of `layout_id`, each with its own items, in grid-slot
/// and stacking order respectively.
fn load_panes_with_items(
    conn: &Connection,
    layout_id: ChartLayoutId,
) -> Result<Vec<PaneRecord>, ChartError> {
    let mut pane_stmt = conn.prepare(
        "SELECT id, position, instrument, timeframe, settings FROM chart_panes
         WHERE layout_id = ?1 ORDER BY position ASC",
    )?;
    let pane_rows = pane_stmt
        .query_map(params![layout_id], |row| {
            Ok((
                row.get::<_, ChartPaneId>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut item_stmt = conn.prepare(
        "SELECT id, position, slot, slot_index, visible, style, source_kind, instrument,
                indicator_name, indicator_params, tool_kind, price, time1, price1, time2, price2,
                label_text, label_anchor
         FROM chart_pane_items WHERE pane_id = ?1 ORDER BY position ASC",
    )?;

    let mut panes = Vec::with_capacity(pane_rows.len());
    for (pane_id, position, instrument, timeframe, settings) in pane_rows {
        let instrument = InstrumentId::parse(&instrument)
            .map_err(|e| ChartError::CorruptInstrumentId(e.to_string()))?;
        let timeframe: BarSpec =
            timeframe
                .parse()
                .map_err(|e: senken_series::ParseBarSpecError| {
                    ChartError::CorruptTimeframe(e.to_string())
                })?;

        let item_rows: Vec<PaneItemRow> = item_stmt
            .query_map(params![pane_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                    row.get(17)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let items = item_rows
            .into_iter()
            .map(decode_pane_item)
            .collect::<Result<Vec<_>, _>>()?;

        panes.push(PaneRecord {
            id: pane_id,
            position,
            instrument,
            timeframe,
            items,
            settings,
        });
    }
    Ok(panes)
}

/// Decodes one raw [`PaneItemRow`] into a [`PaneItemRecord`].
fn decode_pane_item(row: PaneItemRow) -> Result<PaneItemRecord, ChartError> {
    let (
        id,
        position,
        slot,
        slot_index,
        visible,
        style,
        source_kind,
        instrument,
        indicator_name,
        indicator_params,
        tool_kind,
        price,
        time1,
        price1,
        time2,
        price2,
        label_text,
        label_anchor,
    ) = row;
    let slot = decode_slot(&slot, slot_index)?;
    let source = decode_item_source(
        &source_kind,
        instrument,
        indicator_name,
        indicator_params,
        tool_kind,
        price,
        time1,
        price1,
        time2,
        price2,
        label_text,
        label_anchor,
    )?;
    Ok(PaneItemRecord {
        id,
        position,
        slot,
        visible,
        style,
        source,
    })
}

/// Encodes a [`Slot`] as `chart_pane_items`' `(slot, slot_index)` columns.
fn encode_slot(slot: Slot) -> (&'static str, Option<i64>) {
    match slot {
        Slot::Main => ("main", None),
        Slot::Sub(index) => ("sub", Some(i64::from(index))),
    }
}

/// Decodes `chart_pane_items`' `(slot, slot_index)` columns back into a
/// [`Slot`].
fn decode_slot(slot: &str, slot_index: Option<i64>) -> Result<Slot, ChartError> {
    match slot {
        "main" => Ok(Slot::Main),
        "sub" => {
            let index = slot_index.ok_or_else(|| {
                ChartError::CorruptPaneItem("slot `sub` row is missing its slot_index".to_owned())
            })?;
            let index = u8::try_from(index).map_err(|_| {
                ChartError::CorruptPaneItem(format!("slot_index out of range: {index}"))
            })?;
            Ok(Slot::Sub(index))
        }
        other => Err(ChartError::CorruptPaneItem(format!(
            "unknown slot `{other}`"
        ))),
    }
}

/// `chart_pane_items`' source-dependent columns, in the exact order
/// [`encode_item_source`]/[`decode_item_source`] use: `source_kind`,
/// `instrument`, `indicator_name`, `indicator_params`, `tool_kind`,
/// `price`, `time1`, `price1`, `time2`, `price2`, `label_text`,
/// `label_anchor`.
#[allow(clippy::type_complexity)]
type ItemSourceColumns = (
    &'static str,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<String>,
    Option<String>,
);

/// The `source_kind` token for a given [`ItemSource`], without allocating
/// the rest of its columns — used by [`ChartWorkspaceStore::update_pane_item`]
/// to compare against a stored row's family before writing.
fn source_kind_of(source: &ItemSource) -> &'static str {
    match source {
        ItemSource::Computed { .. } => "computed",
        ItemSource::Referenced { .. } => "referenced",
        ItemSource::Anchored { .. } => "anchored",
    }
}

/// Whether a `source_kind` token belongs to the "layer-like" family
/// (`computed`/`referenced`) or the "drawing-like" one (`anchored`) — the
/// invariant `update_pane_item` enforces so `/api/layers/{id}` can never
/// silently turn into a drawing or vice versa.
fn item_source_family(source_kind: &str) -> u8 {
    match source_kind {
        "anchored" => 1,
        _ => 0,
    }
}

/// Encodes an [`ItemSource`] as [`ItemSourceColumns`].
fn encode_item_source(source: &ItemSource) -> ItemSourceColumns {
    match source {
        ItemSource::Computed { name, params } => (
            "computed",
            None,
            Some(name.clone()),
            Some(params.clone()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        ItemSource::Referenced { instrument } => (
            "referenced",
            Some(instrument.as_str().to_owned()),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        ItemSource::Anchored { kind, instrument } => {
            let (tool_kind, price, time1, price1, time2, price2, label_text, label_anchor) =
                encode_drawing_kind(kind);
            (
                "anchored",
                // The same column a `referenced` item stores its overlay
                // instrument in: one nullable text column, two source kinds
                // that each name an instrument, no second column to keep in
                // step with it.
                instrument.as_ref().map(|id| id.as_str().to_owned()),
                None,
                None,
                Some(tool_kind.to_owned()),
                price,
                time1,
                price1,
                time2,
                price2,
                label_text,
                label_anchor,
            )
        }
    }
}

/// Decodes `chart_pane_items`' source-dependent columns back into an
/// [`ItemSource`].
#[allow(clippy::too_many_arguments)]
fn decode_item_source(
    source_kind: &str,
    instrument: Option<String>,
    indicator_name: Option<String>,
    indicator_params: Option<String>,
    tool_kind: Option<String>,
    price: Option<f64>,
    time1: Option<i64>,
    price1: Option<f64>,
    time2: Option<i64>,
    price2: Option<f64>,
    label_text: Option<String>,
    label_anchor: Option<String>,
) -> Result<ItemSource, ChartError> {
    match source_kind {
        "computed" => {
            let name = indicator_name.ok_or_else(|| {
                ChartError::CorruptPaneItem(
                    "computed item row is missing its indicator_name".to_owned(),
                )
            })?;
            let params = indicator_params.ok_or_else(|| {
                ChartError::CorruptPaneItem(
                    "computed item row is missing its indicator_params".to_owned(),
                )
            })?;
            Ok(ItemSource::Computed { name, params })
        }
        "referenced" => {
            let instrument = instrument.ok_or_else(|| {
                ChartError::CorruptPaneItem(
                    "referenced item row is missing its instrument".to_owned(),
                )
            })?;
            let instrument = InstrumentId::parse(&instrument)
                .map_err(|e| ChartError::CorruptInstrumentId(e.to_string()))?;
            Ok(ItemSource::Referenced { instrument })
        }
        "anchored" => {
            let tool_kind = tool_kind.ok_or_else(|| {
                ChartError::CorruptPaneItem("anchored item row is missing its tool_kind".to_owned())
            })?;
            let kind = decode_drawing_kind(
                &tool_kind,
                price,
                time1,
                price1,
                time2,
                price2,
                label_text,
                label_anchor,
            )?;
            // A row written before this field existed has no instrument at
            // all, which is a real answer rather than a corrupt one — unlike
            // a `referenced` row, where a missing instrument means the row
            // cannot say what it draws.
            let instrument = instrument
                .map(|text| InstrumentId::parse(&text))
                .transpose()
                .map_err(|e| ChartError::CorruptInstrumentId(e.to_string()))?;
            Ok(ItemSource::Anchored { kind, instrument })
        }
        other => Err(ChartError::CorruptPaneItem(format!(
            "unknown source_kind `{other}`"
        ))),
    }
}

/// `chart_pane_items`' six geometry columns for an [`ItemSource::Anchored`]
/// item, in the order [`encode_drawing_kind`]/[`decode_drawing_kind`] use:
/// `tool_kind`, `price`, `time1`, `price1`, `time2`, `price2`, `label_text`,
/// `label_anchor`.
#[allow(clippy::type_complexity)]
type DrawingGeometryColumns = (
    &'static str,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<i64>,
    Option<f64>,
    Option<String>,
    Option<String>,
);

/// Encodes a [`DrawingKind`] as [`DrawingGeometryColumns`].
fn encode_drawing_kind(kind: &DrawingKind) -> DrawingGeometryColumns {
    match kind {
        DrawingKind::HorizontalLine { price } => (
            "horizontal_line",
            Some(*price),
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        DrawingKind::TrendLine { start, end } => (
            "trend_line",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
            None,
            None,
        ),
        DrawingKind::Rectangle { start, end } => (
            "rectangle",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
            None,
            None,
        ),
        DrawingKind::Ray { start, end } => (
            "ray",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
            None,
            None,
        ),
        DrawingKind::FibRetracement { start, end } => (
            "fib_retracement",
            None,
            Some(start.time.as_nanos()),
            Some(start.price),
            Some(end.time.as_nanos()),
            Some(end.price),
            None,
            None,
        ),
        DrawingKind::TextNote { at, text, anchor } => (
            "text_note",
            None,
            Some(at.time.as_nanos()),
            Some(at.price),
            None,
            None,
            Some(text.clone()),
            Some(encode_label_anchor(*anchor).to_owned()),
        ),
    }
}

/// Decodes `chart_pane_items`' geometry columns back into a
/// [`DrawingKind`].
#[allow(clippy::too_many_arguments)]
fn decode_drawing_kind(
    kind: &str,
    price: Option<f64>,
    time1: Option<i64>,
    price1: Option<f64>,
    time2: Option<i64>,
    price2: Option<f64>,
    label_text: Option<String>,
    label_anchor: Option<String>,
) -> Result<DrawingKind, ChartError> {
    let two_points = |kind: &str| -> Result<(DrawingPoint, DrawingPoint), ChartError> {
        let missing = || {
            ChartError::CorruptDrawing(format!(
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
        Ok((start, end))
    };
    match kind {
        "horizontal_line" => {
            let price = price.ok_or_else(|| {
                ChartError::CorruptDrawing(
                    "horizontal_line drawing row is missing its price".to_owned(),
                )
            })?;
            Ok(DrawingKind::HorizontalLine { price })
        }
        "trend_line" => {
            let (start, end) = two_points(kind)?;
            Ok(DrawingKind::TrendLine { start, end })
        }
        "rectangle" => {
            let (start, end) = two_points(kind)?;
            Ok(DrawingKind::Rectangle { start, end })
        }
        "ray" => {
            let (start, end) = two_points(kind)?;
            Ok(DrawingKind::Ray { start, end })
        }
        "fib_retracement" => {
            let (start, end) = two_points(kind)?;
            Ok(DrawingKind::FibRetracement { start, end })
        }
        "text_note" => {
            let missing = || {
                ChartError::CorruptDrawing("text_note drawing row is missing a field".to_owned())
            };
            let at = DrawingPoint {
                time: UnixNanos::from_nanos(time1.ok_or_else(missing)?),
                price: price1.ok_or_else(missing)?,
            };
            let text = label_text.ok_or_else(missing)?;
            let anchor = decode_label_anchor(&label_anchor.ok_or_else(missing)?)?;
            Ok(DrawingKind::TextNote { at, text, anchor })
        }
        other => Err(ChartError::CorruptDrawing(format!(
            "unknown drawing kind `{other}`"
        ))),
    }
}

/// Encodes a [`LabelAnchor`] as the token `chart_pane_items.label_anchor`
/// holds — this crate's own encoding, since `senken_indicators::LabelAnchor`
/// carries no `Display`/`FromStr` of its own.
fn encode_label_anchor(anchor: LabelAnchor) -> &'static str {
    match anchor {
        LabelAnchor::Above => "above",
        LabelAnchor::Below => "below",
        LabelAnchor::Center => "center",
    }
}

/// Decodes `chart_pane_items.label_anchor` back into a [`LabelAnchor`].
fn decode_label_anchor(text: &str) -> Result<LabelAnchor, ChartError> {
    match text {
        "above" => Ok(LabelAnchor::Above),
        "below" => Ok(LabelAnchor::Below),
        "center" => Ok(LabelAnchor::Center),
        other => Err(ChartError::CorruptDrawing(format!(
            "unknown label anchor `{other}`"
        ))),
    }
}

/// Inserts one pane item row for `pane_id` inside an already-open
/// transaction.
fn insert_pane_item(
    tx: &Transaction<'_>,
    pane_id: ChartPaneId,
    item: &PaneItemInput,
) -> Result<(), ChartError> {
    let (slot, slot_index) = encode_slot(item.slot);
    let (
        source_kind,
        instrument,
        indicator_name,
        indicator_params,
        tool_kind,
        price,
        time1,
        price1,
        time2,
        price2,
        label_text,
        label_anchor,
    ) = encode_item_source(&item.source);
    tx.execute(
        "INSERT INTO chart_pane_items (
            id, pane_id, position, slot, slot_index, visible, style,
            source_kind, instrument, indicator_name, indicator_params,
            tool_kind, price, time1, price1, time2, price2, label_text, label_anchor
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            PaneItemId::new(),
            pane_id,
            item.position,
            slot,
            slot_index,
            item.visible,
            item.style,
            source_kind,
            instrument,
            indicator_name,
            indicator_params,
            tool_kind,
            price,
            time1,
            price1,
            time2,
            price2,
            label_text,
            label_anchor,
        ],
    )?;
    Ok(())
}

/// Reconciles a pane's items by their stable stacking positions — the
/// same diff-by-position strategy `replace_layout` itself uses for panes,
/// generalised from the old, separate `replace_layers`/`replace_drawings`
/// now that both are one table.
fn replace_pane_items(
    tx: &Transaction<'_>,
    pane_id: ChartPaneId,
    items: &[PaneItemInput],
) -> Result<(), ChartError> {
    let positions = items
        .iter()
        .map(|item| item.position)
        .collect::<HashSet<_>>();
    let existing = tx
        .prepare("SELECT id, position FROM chart_pane_items WHERE pane_id = ?1")?
        .query_map(params![pane_id], |row| {
            Ok((row.get::<_, PaneItemId>(0)?, row.get::<_, u32>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, position) in existing {
        if !positions.contains(&position) {
            tx.execute("DELETE FROM chart_pane_items WHERE id = ?1", params![id])?;
        }
    }
    for item in items {
        let (slot, slot_index) = encode_slot(item.slot);
        let (
            source_kind,
            instrument,
            indicator_name,
            indicator_params,
            tool_kind,
            price,
            time1,
            price1,
            time2,
            price2,
            label_text,
            label_anchor,
        ) = encode_item_source(&item.source);
        let changed = tx.execute(
            "UPDATE chart_pane_items SET slot = ?1, slot_index = ?2, visible = ?3, style = ?4,
             source_kind = ?5, instrument = ?6, indicator_name = ?7, indicator_params = ?8,
             tool_kind = ?9, price = ?10, time1 = ?11, price1 = ?12, time2 = ?13, price2 = ?14,
             label_text = ?15, label_anchor = ?16
             WHERE pane_id = ?17 AND position = ?18",
            params![
                slot,
                slot_index,
                item.visible,
                item.style,
                source_kind,
                instrument,
                indicator_name,
                indicator_params,
                tool_kind,
                price,
                time1,
                price1,
                time2,
                price2,
                label_text,
                label_anchor,
                pane_id,
                item.position
            ],
        )?;
        if changed == 0 {
            insert_pane_item(tx, pane_id, item)?;
        }
    }
    Ok(())
}

/// Checks a pane item's `style` (and, for a computed item, its `params`)
/// parse as expected before either is ever written.
fn validate_pane_item(item: &PaneItemInput) -> Result<(), ChartError> {
    match &item.source {
        ItemSource::Computed { params, .. } => {
            serde_json::from_str::<serde_json::Value>(params)
                .map_err(|e| ChartError::InvalidIndicatorParams(e.to_string()))?;
            serde_json::from_str::<serde_json::Value>(&item.style)
                .map_err(|e| ChartError::InvalidIndicatorParams(e.to_string()))?;
        }
        ItemSource::Referenced { .. } => {
            serde_json::from_str::<serde_json::Value>(&item.style)
                .map_err(|e| ChartError::InvalidIndicatorParams(e.to_string()))?;
        }
        ItemSource::Anchored { .. } => {
            DrawingStyle::from_json(&item.style)?;
        }
    }
    Ok(())
}

/// Checks a pane's `settings` parses as JSON before it is ever written,
/// for the same reason [`validate_pane_item`] does for an item's own
/// `style`/`params`: this crate does not interpret a pane's display
/// settings at all (the chart front end's job), but a value that cannot
/// even be parsed back is worth refusing at the door.
fn validate_pane_settings(settings: &str) -> Result<(), ChartError> {
    serde_json::from_str::<serde_json::Value>(settings)
        .map_err(|e| ChartError::InvalidPaneSettings(e.to_string()))?;
    Ok(())
}

/// Checks a workspace's `settings` parses as a JSON **object** before it is
/// ever written — stricter than [`validate_pane_settings`] (which only
/// requires valid JSON) because a workspace's settings are meant to be
/// extended key by key over time, the same shape every reader of
/// [`ChartWorkspaceSummary::settings`] can then rely on without checking
/// what kind of JSON value it got back.
fn validate_workspace_settings(settings: &str) -> Result<(), ChartError> {
    let invalid = |msg: String| ChartError::InvalidWorkspaceSettings(msg);
    let value: serde_json::Value =
        serde_json::from_str(settings).map_err(|e| invalid(e.to_string()))?;
    if !value.is_object() {
        return Err(invalid(format!("expected a JSON object, got: {settings}")));
    }
    Ok(())
}

/// Checks a drawing's style before it is ever written: `width` must be one
/// of the four `lightweight-charts` stroke widths, and `color` must be a
/// plain `#rrggbb` hex string — the same vocabulary this project's own
/// `layer-style.ts` already uses for a plot's colour, so a client never has
/// to translate between two colour formats.
fn validate_drawing_style(style: &DrawingStyle) -> Result<(), ChartError> {
    if !(1..=4).contains(&style.width) {
        return Err(ChartError::InvalidDrawingStyle(format!(
            "width must be 1-4, got {}",
            style.width
        )));
    }
    let valid_color = style.color.len() == 7
        && style.color.starts_with('#')
        && style.color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid_color {
        return Err(ChartError::InvalidDrawingStyle(format!(
            "color must be `#rrggbb`, got `{}`",
            style.color
        )));
    }
    Ok(())
}

fn row_to_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChartWorkspaceSummary> {
    Ok(ChartWorkspaceSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        settings: row.get(5)?,
    })
}

/// The default single-pane layout every freshly created workspace starts
/// with. Shared by [`ChartWorkspaceStore::create_workspace`] and
/// [`ChartWorkspaceStore::get_or_create_default_workspace`] so the two
/// entry points cannot drift on what "a workable default chart" means.
fn insert_default_workspace(
    tx: &Transaction<'_>,
    owner: UserId,
    name: &str,
) -> Result<ChartWorkspaceId, ChartError> {
    let workspace_id = ChartWorkspaceId::new();
    let now = now_unix();
    tx.execute(
        "INSERT INTO chart_workspaces (id, owner_id, name, created_at, updated_at, settings) VALUES (?1, ?2, ?3, ?4, ?4, '{}')",
        params![workspace_id, owner, name, now],
    )?;

    let layout_id = ChartLayoutId::new();
    tx.execute(
        "INSERT INTO chart_layouts (id, workspace_id, name, preset, position, created_at, updated_at)
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
        "INSERT INTO chart_panes (id, layout_id, position, instrument, timeframe) VALUES (?1, ?2, 0, ?3, ?4)",
        params![
            ChartPaneId::new(),
            layout_id,
            default_instrument().as_str(),
            default_timeframe().to_string(),
        ],
    )?;

    Ok(workspace_id)
}

/// Finds `owner`'s oldest workspace and its first layout, if any exist —
/// the "does this account already have a default" check
/// [`ChartWorkspaceStore::get_or_create_default_workspace`] needs before
/// deciding whether to create one.
fn existing_default(
    conn: &Connection,
    owner: UserId,
) -> Result<Option<(ChartWorkspaceId, ChartLayoutId)>, ChartError> {
    let found = conn
        .query_row(
            "SELECT w.id, l.id FROM chart_workspaces w
             JOIN chart_layouts l ON l.workspace_id = w.id
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
    use senken_indicators::LabelAnchor;
    use senken_series::BarUnit;
    use tempfile::TempDir;

    use super::{
        ChartError, ChartWorkspaceStore, DrawingKind, DrawingLineStyle, DrawingPoint, DrawingStyle,
        ItemSource, LayoutPreset, PaneInput, PaneItemInput, Slot,
    };
    use senken_marketdata::InstrumentId;

    fn temp_stores() -> (TempDir, IdentityStore, ChartWorkspaceStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let workspace = ChartWorkspaceStore::new(&identity);
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
    /// User" role would carry — View/Create/Edit/Delete on
    /// `ChartWorkspace`/`ChartLayout`, at `Scope::Own` — since a freshly
    /// created account otherwise holds no grants at all and every guarded
    /// method here would refuse it outright.
    fn charts_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Charts User", Some("a very long password"))
            .unwrap();
        for resource in [Resource::ChartWorkspace, Resource::ChartLayout] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    fn empty_style() -> String {
        "{}".to_owned()
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
            matches!(err, ChartError::Identity(IdentityError::Forbidden)),
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
            ChartError::Identity(IdentityError::Forbidden)
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

        // Two panes sharing `position = 0` violates `chart_panes`' `UNIQUE
        // (layout_id, position)` constraint on the second insert, partway
        // through the transaction.
        let broken_panes = vec![
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(15, BarUnit::Minute),
                items: vec![PaneItemInput {
                    position: 0,
                    slot: Slot::Main,
                    visible: true,
                    style: empty_style(),
                    source: ItemSource::Referenced {
                        instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                    },
                }],
                settings: empty_style(),
            },
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                items: vec![],
                settings: empty_style(),
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
        assert!(matches!(err, ChartError::Database(_)));

        let after = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(
            before, after,
            "a failed layout change must leave the previous layout intact"
        );
    }

    #[test]
    fn a_successful_layout_change_replaces_the_pane_and_item_structure() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice5@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![
            PaneInput {
                position: 0,
                instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(15, BarUnit::Minute),
                items: vec![
                    PaneItemInput {
                        position: 0,
                        slot: Slot::Sub(0),
                        visible: true,
                        style: empty_style(),
                        source: ItemSource::Computed {
                            name: "RSI".to_owned(),
                            params: r#"{"period":14}"#.to_owned(),
                        },
                    },
                    PaneItemInput {
                        position: 1,
                        slot: Slot::Main,
                        visible: true,
                        style: DrawingStyle {
                            color: "#f2f2ef".to_owned(),
                            width: 2,
                            line_style: DrawingLineStyle::Dashed,
                        }
                        .to_json(),
                        source: ItemSource::Anchored {
                            kind: DrawingKind::HorizontalLine { price: 2450.5 },
                            instrument: None,
                        },
                    },
                ],
                settings: empty_style(),
            },
            PaneInput {
                position: 1,
                instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                items: vec![],
                settings: empty_style(),
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
        assert_eq!(detail.panes[0].items.len(), 2);
        assert_eq!(detail.panes[0].items[0].slot, Slot::Sub(0));
        assert!(matches!(
            &detail.panes[0].items[0].source,
            ItemSource::Computed { name, .. } if name == "RSI"
        ));
        assert_eq!(
            detail.panes[0].items[1].source,
            ItemSource::Anchored {
                kind: DrawingKind::HorizontalLine { price: 2450.5 },
                instrument: None,
            }
        );
        assert_eq!(
            detail.panes[1].items.len(),
            0,
            "a pane's items must not leak into its sibling"
        );
    }

    #[test]
    fn a_ray_a_fib_retracement_and_a_text_note_round_trip_through_replace_layout_and_get_layout() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice-new-tools@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let start = DrawingPoint {
            time: senken_core::UnixNanos::from_secs(1_700_000_000).unwrap(),
            price: 100.25,
        };
        let end = DrawingPoint {
            time: senken_core::UnixNanos::from_secs(1_700_003_600).unwrap(),
            price: 101.75,
        };
        let style = DrawingStyle {
            color: "#7aa7e8".to_owned(),
            width: 1,
            line_style: DrawingLineStyle::Solid,
        }
        .to_json();
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![
                PaneItemInput {
                    position: 0,
                    slot: Slot::Main,
                    visible: true,
                    style: style.clone(),
                    source: ItemSource::Anchored {
                        kind: DrawingKind::Ray { start, end },
                        instrument: None,
                    },
                },
                PaneItemInput {
                    position: 1,
                    slot: Slot::Main,
                    visible: true,
                    style: style.clone(),
                    source: ItemSource::Anchored {
                        kind: DrawingKind::FibRetracement { start, end },
                        instrument: None,
                    },
                },
                PaneItemInput {
                    position: 2,
                    slot: Slot::Main,
                    visible: true,
                    style,
                    source: ItemSource::Anchored {
                        kind: DrawingKind::TextNote {
                            at: start,
                            text: "Breakout level".to_owned(),
                            anchor: LabelAnchor::Above,
                        },
                        instrument: None,
                    },
                },
            ],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(detail.panes[0].items.len(), 3);
        assert_eq!(
            detail.panes[0].items[0].source,
            ItemSource::Anchored {
                kind: DrawingKind::Ray { start, end },
                instrument: None,
            }
        );
        assert_eq!(
            detail.panes[0].items[1].source,
            ItemSource::Anchored {
                kind: DrawingKind::FibRetracement { start, end },
                instrument: None,
            }
        );
        assert_eq!(
            detail.panes[0].items[2].source,
            ItemSource::Anchored {
                kind: DrawingKind::TextNote {
                    at: start,
                    text: "Breakout level".to_owned(),
                    anchor: LabelAnchor::Above,
                },
                instrument: None,
            }
        );
    }

    // A drawing's anchors are prices and instants on one particular market.
    // Storing which market that was is what lets a pane show a reader only
    // the annotations that belong to whatever it is currently displaying,
    // instead of painting Bitcoin's trend line over Ethereum's candles.
    #[test]
    fn an_anchored_item_remembers_the_instrument_it_was_drawn_against() {
        let (_dir, identity, workspace) = temp_stores();
        let alice = admin_auth(&identity);
        let (_, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let drawn_on = InstrumentId::parse("binance-spot:ETHUSDT").unwrap();

        workspace
            .replace_layout(
                &alice,
                layout_id,
                LayoutPreset::One,
                &[PaneInput {
                    position: 0,
                    instrument: drawn_on.clone(),
                    timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                    items: vec![PaneItemInput {
                        position: 0,
                        slot: Slot::Main,
                        visible: true,
                        style: DrawingStyle {
                            color: "#f2f2ef".to_owned(),
                            width: 1,
                            line_style: DrawingLineStyle::Solid,
                        }
                        .to_json(),
                        source: ItemSource::Anchored {
                            kind: DrawingKind::HorizontalLine { price: 2450.5 },
                            instrument: Some(drawn_on.clone()),
                        },
                    }],
                    settings: empty_style(),
                }],
            )
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        let ItemSource::Anchored { instrument, .. } = &detail.panes[0].items[0].source else {
            panic!("the item written as a drawing must read back as one");
        };
        assert_eq!(instrument.as_ref(), Some(&drawn_on));

        // The pane's own instrument can change without touching what its
        // drawings were drawn against — that separation is the whole point.
        workspace
            .replace_layout(
                &alice,
                layout_id,
                LayoutPreset::One,
                &[PaneInput {
                    position: 0,
                    instrument: InstrumentId::parse("binance-spot:SOLUSDT").unwrap(),
                    timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                    items: vec![PaneItemInput {
                        position: 0,
                        slot: Slot::Main,
                        visible: true,
                        style: DrawingStyle {
                            color: "#f2f2ef".to_owned(),
                            width: 1,
                            line_style: DrawingLineStyle::Solid,
                        }
                        .to_json(),
                        source: ItemSource::Anchored {
                            kind: DrawingKind::HorizontalLine { price: 2450.5 },
                            instrument: Some(drawn_on.clone()),
                        },
                    }],
                    settings: empty_style(),
                }],
            )
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        let ItemSource::Anchored { instrument, .. } = &detail.panes[0].items[0].source else {
            panic!("the item written as a drawing must read back as one");
        };
        assert_eq!(instrument.as_ref(), Some(&drawn_on));
    }

    // Every drawing stored before this field existed has no instrument, and
    // that is a real answer rather than a corrupt row: a reader shows such a
    // drawing whatever the pane is displaying, because nothing ever recorded
    // what it was drawn against.
    #[test]
    fn a_drawing_with_no_recorded_instrument_reads_back_as_none_rather_than_failing() {
        let (_dir, identity, workspace) = temp_stores();
        let alice = admin_auth(&identity);
        let (_, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        workspace
            .replace_layout(
                &alice,
                layout_id,
                LayoutPreset::One,
                &[PaneInput {
                    position: 0,
                    instrument: InstrumentId::parse("binance-spot:ETHUSDT").unwrap(),
                    timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                    items: vec![PaneItemInput {
                        position: 0,
                        slot: Slot::Main,
                        visible: true,
                        style: DrawingStyle {
                            color: "#f2f2ef".to_owned(),
                            width: 1,
                            line_style: DrawingLineStyle::Solid,
                        }
                        .to_json(),
                        source: ItemSource::Anchored {
                            kind: DrawingKind::HorizontalLine { price: 1.0 },
                            instrument: None,
                        },
                    }],
                    settings: empty_style(),
                }],
            )
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(
            detail.panes[0].items[0].source,
            ItemSource::Anchored {
                kind: DrawingKind::HorizontalLine { price: 1.0 },
                instrument: None,
            }
        );
    }

    #[test]
    fn visible_applies_to_an_anchored_drawing_and_survives_reload() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "alice-visible-drawing@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();

        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("binance-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: false,
                style: DrawingStyle {
                    color: "#ffffff".to_owned(),
                    width: 1,
                    line_style: DrawingLineStyle::Solid,
                }
                .to_json(),
                source: ItemSource::Anchored {
                    kind: DrawingKind::HorizontalLine { price: 1.0 },
                    instrument: None,
                },
            }],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();

        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert!(
            !detail.panes[0].items[0].visible,
            "a drawing must be able to be hidden — it never could be before schema v8"
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
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: DrawingStyle {
                    color: "#f2f2ef".to_owned(),
                    width: 9,
                    line_style: DrawingLineStyle::Solid,
                }
                .to_json(),
                source: ItemSource::Anchored {
                    kind: DrawingKind::HorizontalLine { price: 1.0 },
                    instrument: None,
                },
            }],
            settings: empty_style(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, ChartError::InvalidDrawingStyle(_)));

        // The rejected call must not have written anything — the pane still
        // has zero items, not a half-applied one.
        let detail = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(detail.panes[0].items.len(), 0);
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
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: DrawingStyle {
                    color: "rgba(0,0,0,1)".to_owned(),
                    width: 1,
                    line_style: DrawingLineStyle::Solid,
                }
                .to_json(),
                source: ItemSource::Anchored {
                    kind: DrawingKind::HorizontalLine { price: 1.0 },
                    instrument: None,
                },
            }],
            settings: empty_style(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, ChartError::InvalidDrawingStyle(_)));
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
            items: vec![],
            settings: empty_style(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::TwoHorizontal, &one_pane)
            .unwrap_err();
        assert!(matches!(
            err,
            ChartError::PaneCountMismatch {
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
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: empty_style(),
                source: ItemSource::Computed {
                    name: "EMA".to_owned(),
                    params: "not json".to_owned(),
                },
            }],
            settings: empty_style(),
        }];

        let err = workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap_err();
        assert!(matches!(err, ChartError::InvalidIndicatorParams(_)));
    }

    #[test]
    fn replace_layout_keeps_pane_and_item_ids_when_positions_survive() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "stable-ids@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("okx-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: r##"{"color":"#ffffff"}"##.to_owned(),
                source: ItemSource::Computed {
                    name: "EMA".to_owned(),
                    params: r#"{"period":20}"#.to_owned(),
                },
            }],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();
        let before = workspace.get_layout(&alice, layout_id).unwrap();
        let pane_id = before.panes[0].id;
        let item_id = before.panes[0].items[0].id;

        let mut changed = panes;
        changed[0].items[0].source = ItemSource::Computed {
            name: "EMA".to_owned(),
            params: r#"{"period":50}"#.to_owned(),
        };
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &changed)
            .unwrap();
        let after = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(after.panes[0].id, pane_id);
        assert_eq!(after.panes[0].items[0].id, item_id);
    }

    #[test]
    fn replace_layout_keeps_a_panes_id_when_a_sibling_is_added() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "stable-structure@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let before = workspace.get_layout(&alice, layout_id).unwrap();
        let original = before.panes[0].id;
        let panes = vec![
            PaneInput {
                position: 0,
                instrument: before.panes[0].instrument.clone(),
                timeframe: before.panes[0].timeframe,
                items: vec![],
                settings: empty_style(),
            },
            PaneInput {
                position: 1,
                instrument: InstrumentId::parse("okx-spot:ETHUSDT").unwrap(),
                timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
                items: vec![],
                settings: empty_style(),
            },
        ];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::TwoHorizontal, &panes)
            .unwrap();
        assert_eq!(
            workspace.get_layout(&alice, layout_id).unwrap().panes[0].id,
            original
        );
    }

    #[test]
    fn updating_one_pane_item_leaves_its_neighbour_unchanged() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "one-item@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("okx-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![
                PaneItemInput {
                    position: 0,
                    slot: Slot::Main,
                    visible: true,
                    style: empty_style(),
                    source: ItemSource::Computed {
                        name: "EMA".to_owned(),
                        params: r#"{"period":20}"#.to_owned(),
                    },
                },
                PaneItemInput {
                    position: 1,
                    slot: Slot::Main,
                    visible: true,
                    style: empty_style(),
                    source: ItemSource::Computed {
                        name: "SMA".to_owned(),
                        params: r#"{"period":50}"#.to_owned(),
                    },
                },
            ],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();
        let before = workspace.get_layout(&alice, layout_id).unwrap();
        let target = before.panes[0].items[0].id;
        let neighbour = before.panes[0].items[1].clone();
        workspace
            .update_pane_item(
                &alice,
                target,
                &PaneItemInput {
                    position: 0,
                    slot: Slot::Main,
                    visible: false,
                    style: r##"{"color":"#abcdef"}"##.to_owned(),
                    source: ItemSource::Computed {
                        name: "EMA".to_owned(),
                        params: r#"{"period":10}"#.to_owned(),
                    },
                },
            )
            .unwrap();
        let after = workspace.get_layout(&alice, layout_id).unwrap();
        assert_eq!(after.panes[0].items[1], neighbour);
        assert!(!after.panes[0].items[0].visible);
    }

    #[test]
    fn a_user_cannot_update_another_users_pane_item() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "item-alice@example.com");
        let bob = charts_user(&identity, &admin, "item-bob@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("okx-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: empty_style(),
                source: ItemSource::Computed {
                    name: "EMA".to_owned(),
                    params: r#"{"period":20}"#.to_owned(),
                },
            }],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();
        let item_id = workspace.get_layout(&alice, layout_id).unwrap().panes[0].items[0].id;
        let error = workspace
            .update_pane_item(&bob, item_id, &panes[0].items[0])
            .unwrap_err();
        assert!(matches!(
            error,
            ChartError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn updating_a_drawing_through_the_indicator_family_is_refused() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "family-guard@example.com");
        let (_workspace_id, layout_id) = workspace.get_or_create_default_workspace(&alice).unwrap();
        let panes = vec![PaneInput {
            position: 0,
            instrument: InstrumentId::parse("okx-spot:BTCUSDT").unwrap(),
            timeframe: senken_series::BarSpec::new(1, BarUnit::Hour),
            items: vec![PaneItemInput {
                position: 0,
                slot: Slot::Main,
                visible: true,
                style: DrawingStyle {
                    color: "#ffffff".to_owned(),
                    width: 1,
                    line_style: DrawingLineStyle::Solid,
                }
                .to_json(),
                source: ItemSource::Anchored {
                    kind: DrawingKind::HorizontalLine { price: 1.0 },
                    instrument: None,
                },
            }],
            settings: empty_style(),
        }];
        workspace
            .replace_layout(&alice, layout_id, LayoutPreset::One, &panes)
            .unwrap();
        let item_id = workspace.get_layout(&alice, layout_id).unwrap().panes[0].items[0].id;

        let error = workspace
            .update_pane_item(
                &alice,
                item_id,
                &PaneItemInput {
                    position: 0,
                    slot: Slot::Main,
                    visible: true,
                    style: empty_style(),
                    source: ItemSource::Computed {
                        name: "EMA".to_owned(),
                        params: r#"{"period":20}"#.to_owned(),
                    },
                },
            )
            .unwrap_err();
        assert!(matches!(error, ChartError::ItemSourceMismatch));
    }

    #[test]
    fn a_valid_json_object_round_trips_through_workspace_settings() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "settings-round-trip@example.com");
        let workspace_id = workspace
            .create_workspace(&alice, "Alice's Charts")
            .unwrap();

        let page = workspace.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(
            page.rows[0].settings, "{}",
            "a freshly created workspace must start with an empty settings object"
        );

        let new_settings = r#"{"theme":"dark","gridVisible":false}"#;
        workspace
            .update_workspace_settings(&alice, workspace_id, new_settings)
            .unwrap();

        let page = workspace.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(page.rows[0].settings, new_settings);
    }

    #[test]
    fn non_object_json_is_refused_as_workspace_settings() {
        let (_dir, identity, workspace) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = charts_user(&identity, &admin, "settings-non-object@example.com");
        let workspace_id = workspace
            .create_workspace(&alice, "Alice's Charts")
            .unwrap();

        for invalid in ["[1,2,3]", "\"a string\"", "42", "null", "not json at all"] {
            let error = workspace
                .update_workspace_settings(&alice, workspace_id, invalid)
                .unwrap_err();
            assert!(
                matches!(error, ChartError::InvalidWorkspaceSettings(_)),
                "expected InvalidWorkspaceSettings for {invalid:?}, got {error:?}"
            );
        }

        // Refused before the row is ever touched.
        let page = workspace.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(page.rows[0].settings, "{}");
    }
}
