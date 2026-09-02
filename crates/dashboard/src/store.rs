//! [`DashboardWorkspaceStore`]: the guarded query API for dashboard
//! workspaces and their placed widgets.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};

use crate::error::DashboardError;
use crate::id::{DashboardWidgetId, DashboardWorkspaceId};

/// The name every auto-created default dashboard workspace is given
/// (opening the dashboard with no workspace creates a default one).
const DEFAULT_WORKSPACE_NAME: &str = "Default";

/// The column count a newly created workspace starts with.
const DEFAULT_COLUMNS: u32 = 12;

/// The narrowest a workspace's grid may ever be — a single-column grid can
/// still place widgets stacked one per row, so this is not zero.
const MIN_COLUMNS: u32 = 1;

/// A dashboard workspace row as returned by a guarded listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardWorkspaceSummary {
    /// The workspace's id.
    pub id: DashboardWorkspaceId,
    /// The account that owns this workspace.
    pub owner_id: UserId,
    /// The workspace's display name.
    pub name: String,
    /// How many grid columns this workspace's layout uses. A widget's
    /// `position_x + width` must never exceed this.
    pub columns: u32,
    /// Bumped by one on every successful
    /// [`DashboardWorkspaceStore::replace_layout`] call — the optimistic
    /// concurrency token a client must echo back as `expected_revision`.
    pub revision: u64,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the workspace's own fields or
    /// its widget layout.
    pub updated_at: i64,
}

/// One widget placed on a workspace's grid, as read back from
/// [`DashboardWorkspaceStore::get_layout`].
///
/// Stores a provider id and a widget type id, **never a component** — this
/// is what makes a placeholder possible: when `widget_type_id` is not in a
/// caller's effective [`crate::WidgetRegistry`] (a provider disabled,
/// failed to load, or is simply not installed in this build), the cell,
/// its size and its `config` are all still exactly here, unharmed. There is
/// deliberately no foreign key from `widget_type_id` to any registry — a
/// registry entry can disappear; a stored layout must still read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetRecord {
    /// This widget instance's id — stable across every move, resize and
    /// provider enable/disable.
    pub id: DashboardWidgetId,
    /// The plugin (or `"senken"` for a built-in) that contributes this
    /// widget's rendering.
    pub provider_id: String,
    /// `<provider_id>/<widget>` — looked up against a
    /// [`crate::WidgetRegistry`] to render this widget for real, or a
    /// placeholder when the lookup misses.
    pub widget_type_id: String,
    /// This widget's left edge, in grid columns from the workspace's own
    /// left edge — never pixels.
    pub position_x: u32,
    /// This widget's top edge, in grid rows.
    pub position_y: u32,
    /// This widget's width, in grid columns.
    pub width: u32,
    /// This widget's height, in grid rows.
    pub height: u32,
    /// Whether this widget is currently shown. A user-facing show/hide
    /// toggle, independent of whether its provider is available.
    pub visible: bool,
    /// This widget's configuration, as opaque JSON-object text. This crate
    /// does not interpret it beyond checking it parses as JSON — the
    /// widget's own provider owns what the fields inside mean.
    pub config: String,
    /// The version of `config`'s own shape, for the provider's own
    /// migration (never interpreted by this crate).
    pub config_schema_version: u32,
    /// Unix timestamp this widget was first placed.
    pub created_at: i64,
    /// Unix timestamp of the last change to this widget's placement,
    /// visibility or config.
    pub updated_at: i64,
}

/// A workspace's full grid, as read back by
/// [`DashboardWorkspaceStore::get_layout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardLayout {
    /// The workspace's own identity and grid width.
    pub workspace: DashboardWorkspaceSummary,
    /// Every widget placed on this workspace's grid, in no particular
    /// order — a caller lays them out from `position_x`/`position_y`
    /// directly, not from array order.
    pub widgets: Vec<WidgetRecord>,
}

/// One widget to write, supplied to
/// [`DashboardWorkspaceStore::replace_layout`] as part of a full-workspace
/// snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetPlacementInput {
    /// `None` for a newly added widget (a fresh id is generated); `Some` to
    /// update an existing widget in place, preserving its id, its
    /// `created_at`, and — critically — its identity for anything a client
    /// keyed off it. An id that does not already belong to this workspace
    /// is rejected with [`DashboardError::WidgetNotFound`] rather than
    /// silently adopted.
    pub id: Option<DashboardWidgetId>,
    /// The rendering provider — see [`WidgetRecord::provider_id`].
    pub provider_id: String,
    /// The widget type — see [`WidgetRecord::widget_type_id`].
    pub widget_type_id: String,
    /// See [`WidgetRecord::position_x`].
    pub position_x: u32,
    /// See [`WidgetRecord::position_y`].
    pub position_y: u32,
    /// See [`WidgetRecord::width`].
    pub width: u32,
    /// See [`WidgetRecord::height`].
    pub height: u32,
    /// See [`WidgetRecord::visible`].
    pub visible: bool,
    /// See [`WidgetRecord::config`].
    pub config: String,
    /// See [`WidgetRecord::config_schema_version`].
    pub config_schema_version: u32,
}

/// Guarded queries over dashboard workspaces and their placed widgets.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second
/// one — the same reasoning `senken-chart` documents for itself. Every
/// mutation and every listing goes through an [`AuthenticatedUser`] and
/// calls [`AuthenticatedUser::authorize`]: there is no method here that
/// reads or writes a row without that check running first.
#[derive(Debug)]
pub struct DashboardWorkspaceStore {
    conn: Arc<Mutex<Connection>>,
}

impl DashboardWorkspaceStore {
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

    /// Creates a new, named, empty workspace owned by `auth`.
    ///
    /// # Errors
    /// [`DashboardError::Identity`] if `auth` may not create a dashboard
    /// workspace; otherwise as [`DashboardError::Database`].
    pub fn create_workspace(
        &self,
        auth: &AuthenticatedUser,
        name: &str,
    ) -> Result<DashboardWorkspaceId, DashboardError> {
        auth.authorize(Action::Create, Resource::DashboardWorkspace)?;
        let conn = self.lock();
        insert_workspace(&conn, auth.user_id(), name)
    }

    /// Returns `auth`'s default workspace, creating one if `auth` owns none
    /// yet (opening the dashboard with no workspace creates a default one
    /// rather than an empty state). Calling this again for the same
    /// account returns the **same** workspace — it never creates a second
    /// one.
    ///
    /// # Errors
    /// [`DashboardError::Identity`] if `auth` may not view (or, on first
    /// use, create) a dashboard workspace; otherwise as
    /// [`DashboardError::Database`].
    pub fn get_or_create_default_workspace(
        &self,
        auth: &AuthenticatedUser,
    ) -> Result<DashboardWorkspaceId, DashboardError> {
        auth.authorize(Action::View, Resource::DashboardWorkspace)?;
        // Held for the whole check-then-create below, so a second call
        // racing this one cannot observe "no default yet" for both and
        // create two.
        let conn = self.lock();
        if let Some(found) = existing_default(&conn, auth.user_id())? {
            return Ok(found);
        }
        auth.authorize(Action::Create, Resource::DashboardWorkspace)?;
        insert_workspace(&conn, auth.user_id(), DEFAULT_WORKSPACE_NAME)
    }

    /// Lists workspaces visible to `auth`, scoped: the `WHERE` clause is
    /// chosen by `auth`'s [`senken_acl::decide`]d [`Scope`] for
    /// `(Action::View, Resource::DashboardWorkspace)`, and [`Page::total`]
    /// is counted under that same clause.
    ///
    /// # Errors
    /// [`DashboardError::Identity`] if `auth` may not view workspaces at
    /// all, or if `decide` returns a [`Scope`] variant this crate does not
    /// yet translate to SQL; otherwise as [`DashboardError::Database`].
    pub fn list_workspaces(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<DashboardWorkspaceSummary>, DashboardError> {
        let scope = auth.authorize(Action::View, Resource::DashboardWorkspace)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM dashboard_workspaces WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, columns, revision, created_at, updated_at
                     FROM dashboard_workspaces
                     WHERE owner_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_workspace)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM dashboard_workspaces", [], |row| {
                        row.get(0)
                    })?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, columns, revision, created_at, updated_at
                     FROM dashboard_workspaces
                     ORDER BY created_at ASC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_workspace)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — a future variant this crate
            // has not been taught to turn into a `WHERE` clause must fail
            // closed, never fall back to an unfiltered query.
            _ => return Err(DashboardError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Renames a workspace — a field-level edit, not a layout rewrite.
    ///
    /// # Errors
    /// [`DashboardError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`DashboardError::Identity`] if `auth` may not edit this
    /// workspace; otherwise as [`DashboardError::Database`].
    pub fn rename_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: DashboardWorkspaceId,
        new_name: &str,
    ) -> Result<(), DashboardError> {
        let scope = auth.authorize(Action::Edit, Resource::DashboardWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE dashboard_workspaces SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_name, now_unix(), workspace_id],
        )?;
        Ok(())
    }

    /// Deletes a workspace and, by `ON DELETE CASCADE`, every widget placed
    /// on it.
    ///
    /// Deleting a user's last workspace is not specially refused — the
    /// same choice `senken-chart`'s own `delete_workspace` already makes.
    /// Nothing reads a dashboard without going through
    /// [`get_or_create_default_workspace`](Self::get_or_create_default_workspace)
    /// first, and that call re-creates a default the moment none exists —
    /// so "no workspace" is never a state a caller can actually observe,
    /// only ever a transient one this store heals on the next open.
    ///
    /// # Errors
    /// [`DashboardError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`DashboardError::Identity`] if `auth` may not delete this
    /// workspace; otherwise as [`DashboardError::Database`].
    pub fn delete_workspace(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: DashboardWorkspaceId,
    ) -> Result<(), DashboardError> {
        let scope = auth.authorize(Action::Delete, Resource::DashboardWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "DELETE FROM dashboard_workspaces WHERE id = ?1",
            params![workspace_id],
        )?;
        Ok(())
    }

    /// Fetches one workspace's full grid.
    ///
    /// # Errors
    /// [`DashboardError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`DashboardError::Identity`] if `auth` may not view this
    /// workspace; otherwise as [`DashboardError::Database`].
    pub fn get_layout(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: DashboardWorkspaceId,
    ) -> Result<DashboardLayout, DashboardError> {
        let scope = auth.authorize(Action::View, Resource::DashboardWorkspace)?;
        let conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        let workspace = load_workspace(&conn, workspace_id)?;
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, widget_type_id, position_x, position_y, width, height,
                    visible, config, config_schema_version, created_at, updated_at
             FROM dashboard_widgets WHERE workspace_id = ?1",
        )?;
        let widgets = stmt
            .query_map(params![workspace_id], row_to_widget)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(DashboardLayout { workspace, widgets })
    }

    /// Replaces a workspace's entire widget grid in one transaction:
    /// add, move, resize and delete are all just different snapshots
    /// through this one call, which is what lets them share optimistic
    /// concurrency instead of each needing its own conflict rule.
    ///
    /// Every rectangle and every `config` is validated **before** any
    /// database work begins: grid bounds, zero-sized widgets and pairwise
    /// overlaps are all checked in Rust, since an ordinary SQLite
    /// constraint cannot reject two overlapping rectangles. `expected_revision`
    /// is then checked against the workspace's current
    /// [`DashboardWorkspaceSummary::revision`] inside the same lock that
    /// performs the write: a mismatch means another write already landed
    /// (most likely a second open tab), and this call is refused with
    /// [`DashboardError::RevisionConflict`] rather than silently
    /// overwriting it.
    ///
    /// Every widget the caller wants kept — moved, resized, or entirely
    /// unchanged — must be present in `widgets` with its own [`Some`] id;
    /// any existing widget whose id is *not* present is deleted. Deleting a
    /// widget is therefore always something the caller asked for by
    /// omission, never a side effect of anything else this call does.
    ///
    /// # Errors
    /// [`DashboardError::OutOfBounds`]/[`DashboardError::ZeroSizedWidget`]/
    /// [`DashboardError::OverlappingWidgets`]/[`DashboardError::EmptyWidgetIdentity`]/
    /// [`DashboardError::InvalidConfig`] if `widgets` fails validation;
    /// [`DashboardError::WorkspaceNotFound`] if `workspace_id` does not
    /// exist; [`DashboardError::Identity`] if `auth` may not edit this
    /// workspace; [`DashboardError::RevisionConflict`] if
    /// `expected_revision` is stale; [`DashboardError::WidgetNotFound`] if
    /// `widgets` names an id this workspace does not have; otherwise as
    /// [`DashboardError::Database`].
    pub fn replace_layout(
        &self,
        auth: &AuthenticatedUser,
        workspace_id: DashboardWorkspaceId,
        expected_revision: u64,
        columns: u32,
        widgets: &[WidgetPlacementInput],
    ) -> Result<u64, DashboardError> {
        let scope = auth.authorize(Action::Edit, Resource::DashboardWorkspace)?;
        validate_layout(columns, widgets)?;

        let mut conn = self.lock();
        let owner = workspace_owner(&conn, workspace_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let tx = conn.transaction()?;
        let current_revision = current_revision(&tx, workspace_id)?;
        if current_revision != expected_revision {
            return Err(DashboardError::RevisionConflict {
                expected: expected_revision,
                current: current_revision,
            });
        }

        let existing_ids = existing_widget_ids(&tx, workspace_id)?;
        delete_stale_widgets(&tx, &existing_ids, widgets)?;
        upsert_widgets(&tx, workspace_id, &existing_ids, widgets)?;

        let new_revision = expected_revision.saturating_add(1);
        tx.execute(
            "UPDATE dashboard_workspaces SET columns = ?1, revision = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                columns,
                i64::try_from(new_revision).unwrap_or(i64::MAX),
                now_unix(),
                workspace_id
            ],
        )?;
        tx.commit()?;
        Ok(new_revision)
    }
}

/// The workspace's current `revision`, for
/// [`DashboardWorkspaceStore::replace_layout`]'s optimistic-concurrency
/// check.
fn current_revision(
    tx: &Transaction<'_>,
    workspace_id: DashboardWorkspaceId,
) -> Result<u64, DashboardError> {
    let revision: i64 = tx.query_row(
        "SELECT revision FROM dashboard_workspaces WHERE id = ?1",
        params![workspace_id],
        |row| row.get(0),
    )?;
    Ok(u64::try_from(revision).unwrap_or(0))
}

/// Every widget id this workspace currently has, before
/// [`DashboardWorkspaceStore::replace_layout`] applies its incoming
/// snapshot.
fn existing_widget_ids(
    tx: &Transaction<'_>,
    workspace_id: DashboardWorkspaceId,
) -> Result<HashSet<DashboardWidgetId>, DashboardError> {
    let ids = tx
        .prepare("SELECT id FROM dashboard_widgets WHERE workspace_id = ?1")?
        .query_map(params![workspace_id], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(ids)
}

/// Deletes every existing widget whose id is not named in `widgets` — the
/// only way a widget is ever removed by
/// [`DashboardWorkspaceStore::replace_layout`]: an explicit omission by the
/// caller, never a side effect of anything else the call does.
fn delete_stale_widgets(
    tx: &Transaction<'_>,
    existing_ids: &HashSet<DashboardWidgetId>,
    widgets: &[WidgetPlacementInput],
) -> Result<(), DashboardError> {
    let kept_ids: HashSet<DashboardWidgetId> = widgets.iter().filter_map(|w| w.id).collect();
    for stale_id in existing_ids.difference(&kept_ids) {
        tx.execute(
            "DELETE FROM dashboard_widgets WHERE id = ?1",
            params![stale_id],
        )?;
    }
    Ok(())
}

/// Updates every widget already present (by id) in place, and inserts a
/// fresh row for every widget with no id — the write half of
/// [`DashboardWorkspaceStore::replace_layout`]'s snapshot diff.
fn upsert_widgets(
    tx: &Transaction<'_>,
    workspace_id: DashboardWorkspaceId,
    existing_ids: &HashSet<DashboardWidgetId>,
    widgets: &[WidgetPlacementInput],
) -> Result<(), DashboardError> {
    let now = now_unix();
    for widget in widgets {
        match widget.id {
            Some(id) => {
                if !existing_ids.contains(&id) {
                    return Err(DashboardError::WidgetNotFound);
                }
                tx.execute(
                    "UPDATE dashboard_widgets SET
                        provider_id = ?1, widget_type_id = ?2, position_x = ?3,
                        position_y = ?4, width = ?5, height = ?6, visible = ?7,
                        config = ?8, config_schema_version = ?9, updated_at = ?10
                     WHERE id = ?11",
                    params![
                        widget.provider_id,
                        widget.widget_type_id,
                        widget.position_x,
                        widget.position_y,
                        widget.width,
                        widget.height,
                        widget.visible,
                        widget.config,
                        widget.config_schema_version,
                        now,
                        id,
                    ],
                )?;
            }
            None => {
                tx.execute(
                    "INSERT INTO dashboard_widgets (
                        id, workspace_id, provider_id, widget_type_id, position_x,
                        position_y, width, height, visible, config,
                        config_schema_version, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)",
                    params![
                        DashboardWidgetId::new(),
                        workspace_id,
                        widget.provider_id,
                        widget.widget_type_id,
                        widget.position_x,
                        widget.position_y,
                        widget.width,
                        widget.height,
                        widget.visible,
                        widget.config,
                        widget.config_schema_version,
                        now,
                    ],
                )?;
            }
        }
    }
    Ok(())
}

/// Resolves `Scope` against a row's owner, the same check every method
/// above needs after it has already resolved a [`Scope`] from
/// [`AuthenticatedUser::authorize`] for a single-row operation.
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), DashboardError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        // Covers both "Own but not this row's owner" and any future
        // `Scope` variant this crate has not been taught to interpret —
        // failing closed either way.
        _ => Err(DashboardError::Identity(IdentityError::Forbidden)),
    }
}

/// Looks up a workspace's owner, or [`DashboardError::WorkspaceNotFound`].
fn workspace_owner(
    conn: &Connection,
    workspace_id: DashboardWorkspaceId,
) -> Result<UserId, DashboardError> {
    conn.query_row(
        "SELECT owner_id FROM dashboard_workspaces WHERE id = ?1",
        [workspace_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(DashboardError::WorkspaceNotFound)
}

/// Loads a workspace's own summary row, or
/// [`DashboardError::WorkspaceNotFound`].
fn load_workspace(
    conn: &Connection,
    workspace_id: DashboardWorkspaceId,
) -> Result<DashboardWorkspaceSummary, DashboardError> {
    conn.query_row(
        "SELECT id, owner_id, name, columns, revision, created_at, updated_at
         FROM dashboard_workspaces WHERE id = ?1",
        params![workspace_id],
        row_to_workspace,
    )
    .optional()?
    .ok_or(DashboardError::WorkspaceNotFound)
}

fn row_to_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<DashboardWorkspaceSummary> {
    let columns: i64 = row.get(3)?;
    let revision: i64 = row.get(4)?;
    Ok(DashboardWorkspaceSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        columns: u32::try_from(columns).unwrap_or(0),
        revision: u64::try_from(revision).unwrap_or(0),
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_widget(row: &rusqlite::Row<'_>) -> rusqlite::Result<WidgetRecord> {
    let position_x: i64 = row.get(3)?;
    let position_y: i64 = row.get(4)?;
    let width: i64 = row.get(5)?;
    let height: i64 = row.get(6)?;
    let config_schema_version: i64 = row.get(9)?;
    Ok(WidgetRecord {
        id: row.get(0)?,
        provider_id: row.get(1)?,
        widget_type_id: row.get(2)?,
        position_x: u32::try_from(position_x).unwrap_or(0),
        position_y: u32::try_from(position_y).unwrap_or(0),
        width: u32::try_from(width).unwrap_or(0),
        height: u32::try_from(height).unwrap_or(0),
        visible: row.get(7)?,
        config: row.get(8)?,
        config_schema_version: u32::try_from(config_schema_version).unwrap_or(0),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn insert_workspace(
    conn: &Connection,
    owner: UserId,
    name: &str,
) -> Result<DashboardWorkspaceId, DashboardError> {
    let workspace_id = DashboardWorkspaceId::new();
    let now = now_unix();
    conn.execute(
        "INSERT INTO dashboard_workspaces (id, owner_id, name, columns, revision, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)",
        params![workspace_id, owner, name, DEFAULT_COLUMNS, now],
    )?;
    Ok(workspace_id)
}

/// Finds `owner`'s oldest workspace, if any exist — the "does this account
/// already have a default" check
/// [`DashboardWorkspaceStore::get_or_create_default_workspace`] needs
/// before deciding whether to create one.
fn existing_default(
    conn: &Connection,
    owner: UserId,
) -> Result<Option<DashboardWorkspaceId>, DashboardError> {
    let found = conn
        .query_row(
            "SELECT id FROM dashboard_workspaces WHERE owner_id = ?1 ORDER BY created_at ASC LIMIT 1",
            [owner],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found)
}

/// Two axis-aligned grid rectangles overlap iff they overlap on both axes.
fn rectangles_overlap(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    ax < bx.saturating_add(bw)
        && bx < ax.saturating_add(aw)
        && ay < by.saturating_add(bh)
        && by < ay.saturating_add(ah)
}

/// Validates grid bounds, zero-sized rectangles, pairwise overlaps and
/// `config`/identity shape for a whole incoming snapshot — everything
/// [`DashboardWorkspaceStore::replace_layout`] must reject before it ever
/// opens a transaction, because plain SQLite constraints cannot express
/// "no two rows' rectangles overlap".
fn validate_layout(columns: u32, widgets: &[WidgetPlacementInput]) -> Result<(), DashboardError> {
    if columns < MIN_COLUMNS {
        return Err(DashboardError::OutOfBounds {
            x: 0,
            y: 0,
            width: 0,
            columns,
        });
    }
    for widget in widgets {
        if widget.provider_id.trim().is_empty() || widget.widget_type_id.trim().is_empty() {
            return Err(DashboardError::EmptyWidgetIdentity);
        }
        if widget.width == 0 || widget.height == 0 {
            return Err(DashboardError::ZeroSizedWidget {
                x: widget.position_x,
                y: widget.position_y,
            });
        }
        if widget.position_x.saturating_add(widget.width) > columns {
            return Err(DashboardError::OutOfBounds {
                x: widget.position_x,
                y: widget.position_y,
                width: widget.width,
                columns,
            });
        }
        let _: serde_json::Value = serde_json::from_str(&widget.config)
            .map_err(|e| DashboardError::InvalidConfig(e.to_string()))?;
    }
    for (i, a) in widgets.iter().enumerate() {
        for b in &widgets[i + 1..] {
            let a_rect = (a.position_x, a.position_y, a.width, a.height);
            let b_rect = (b.position_x, b.position_y, b.width, b.height);
            if rectangles_overlap(a_rect, b_rect) {
                return Err(DashboardError::OverlappingWidgets {
                    x1: a.position_x,
                    y1: a.position_y,
                    x2: b.position_x,
                    y2: b.position_y,
                });
            }
        }
    }
    Ok(())
}

/// The current time as a Unix timestamp, for `created_at`/`updated_at`.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityStore};
    use tempfile::TempDir;

    use super::{DashboardError, DashboardWorkspaceStore, WidgetPlacementInput};

    fn temp_stores() -> (TempDir, IdentityStore, DashboardWorkspaceStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let dashboard = DashboardWorkspaceStore::new(&identity);
        (dir, identity, dashboard)
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

    /// Creates an ordinary account with exactly the grants a real
    /// "Dashboard User" role would carry — View/Create/Edit/Delete on
    /// `DashboardWorkspace`, at `Scope::Own`.
    fn dashboard_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Dashboard User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::DashboardWorkspace, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    fn widget(
        id: Option<super::DashboardWidgetId>,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> WidgetPlacementInput {
        WidgetPlacementInput {
            id,
            provider_id: "senken".to_owned(),
            widget_type_id: "senken/equity".to_owned(),
            position_x: x,
            position_y: y,
            width: w,
            height: h,
            visible: true,
            config: "{}".to_owned(),
            config_schema_version: 1,
        }
    }

    // --- scope reaches the query, including the total -------------------

    #[test]
    fn two_users_cannot_see_each_others_workspaces_and_the_total_respects_scope_too() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice@example.com");
        let bob = dashboard_user(&identity, &admin, "bob@example.com");

        dashboard.create_workspace(&alice, "Alice's Dash").unwrap();
        dashboard.create_workspace(&bob, "Bob's Dash").unwrap();
        dashboard.create_workspace(&bob, "Bob's Second").unwrap();

        let alice_page = dashboard.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(alice_page.rows[0].name, "Alice's Dash");
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too — otherwise pagination leaks \
             how many workspaces exist"
        );

        let bob_page = dashboard.list_workspaces(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
        assert!(
            bob_page.rows.iter().all(|w| w.owner_id == bob.user_id()),
            "bob must never see alice's workspace"
        );
    }

    #[test]
    fn a_superadmin_sees_every_users_workspaces() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice2@example.com");
        let bob = dashboard_user(&identity, &admin, "bob2@example.com");
        dashboard.create_workspace(&alice, "Alice").unwrap();
        dashboard.create_workspace(&bob, "Bob").unwrap();

        let page = dashboard.list_workspaces(&admin, 50, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 2);
    }

    // --- default workspace on first open ---------------------------------

    #[test]
    fn opening_with_no_workspace_creates_a_default_and_a_second_open_does_not_duplicate_it() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice3@example.com");

        let first = dashboard.get_or_create_default_workspace(&alice).unwrap();
        let second = dashboard.get_or_create_default_workspace(&alice).unwrap();
        assert_eq!(
            first, second,
            "a second open must not create a second default"
        );

        let page = dashboard.list_workspaces(&alice, 50, 0).unwrap();
        assert_eq!(page.total, 1);
    }

    #[test]
    fn deleting_the_only_workspace_is_healed_by_the_next_default_open() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice4@example.com");

        let first = dashboard.get_or_create_default_workspace(&alice).unwrap();
        dashboard.delete_workspace(&alice, first).unwrap();
        assert_eq!(dashboard.list_workspaces(&alice, 50, 0).unwrap().total, 0);

        let healed = dashboard.get_or_create_default_workspace(&alice).unwrap();
        assert_ne!(healed, first, "the healed workspace is a fresh row");
        let healed_again = dashboard.get_or_create_default_workspace(&alice).unwrap();
        assert_eq!(healed, healed_again, "still no duplicate once healed");
    }

    // --- the grid: add, move, resize, delete, all through replace_layout --

    #[test]
    fn replace_layout_adds_moves_resizes_and_deletes_widgets_in_one_call() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice5@example.com");
        let ws = dashboard.create_workspace(&alice, "Grid").unwrap();

        // Add two widgets.
        let revision = dashboard
            .replace_layout(
                &alice,
                ws,
                0,
                12,
                &[widget(None, 0, 0, 6, 4), widget(None, 6, 0, 6, 4)],
            )
            .unwrap();
        assert_eq!(revision, 1);
        let layout = dashboard.get_layout(&alice, ws).unwrap();
        assert_eq!(layout.workspace.revision, 1);
        assert_eq!(layout.widgets.len(), 2);
        let first_id = layout.widgets[0].id;
        let second_id = layout.widgets[1].id;

        // Move + resize the first, delete the second by omission.
        let moved = widget(Some(first_id), 2, 1, 8, 5);
        let revision = dashboard
            .replace_layout(&alice, ws, revision, 12, &[moved])
            .unwrap();
        assert_eq!(revision, 2);

        let layout = dashboard.get_layout(&alice, ws).unwrap();
        assert_eq!(
            layout.widgets.len(),
            1,
            "the widget omitted from the snapshot must be deleted, and only that one"
        );
        let remaining = &layout.widgets[0];
        assert_eq!(remaining.id, first_id, "the surviving widget keeps its id");
        assert_ne!(remaining.id, second_id);
        assert_eq!(remaining.position_x, 2);
        assert_eq!(remaining.position_y, 1);
        assert_eq!(remaining.width, 8);
        assert_eq!(remaining.height, 5);
    }

    #[test]
    fn a_widget_stores_its_provider_id_and_config_not_a_component() {
        // A widget whose type no registry in this process knows about must
        // still round-trip exactly: geometry and config survive regardless
        // of whether anything can currently render it. This is what makes
        // a placeholder possible — see this crate's module docs.
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice6@example.com");
        let ws = dashboard.create_workspace(&alice, "Ghost").unwrap();

        let ghost = WidgetPlacementInput {
            provider_id: "long-gone-plugin".to_owned(),
            widget_type_id: "long-gone-plugin/thing".to_owned(),
            config: r#"{"symbol":"BTCUSDT"}"#.to_owned(),
            ..widget(None, 0, 0, 4, 3)
        };
        dashboard
            .replace_layout(&alice, ws, 0, 12, &[ghost])
            .unwrap();

        let registry = crate::WidgetRegistry::builtin();
        let layout = dashboard.get_layout(&alice, ws).unwrap();
        let stored = &layout.widgets[0];
        assert!(
            !registry.contains(&stored.widget_type_id),
            "this widget type must not be one this build's registry knows about"
        );
        assert_eq!(stored.provider_id, "long-gone-plugin");
        assert_eq!(stored.widget_type_id, "long-gone-plugin/thing");
        assert_eq!(stored.config, r#"{"symbol":"BTCUSDT"}"#);
        assert_eq!(stored.position_x, 0);
        assert_eq!(stored.width, 4);

        // Writing an unrelated new widget alongside it — the "disable does
        // not touch layout" property — must leave every field of the
        // ghost widget exactly as it was, since the caller resubmits it
        // verbatim as part of the same full-workspace snapshot.
        let new_revision = dashboard
            .replace_layout(
                &alice,
                ws,
                1,
                12,
                &[
                    WidgetPlacementInput {
                        id: Some(stored.id),
                        provider_id: stored.provider_id.clone(),
                        widget_type_id: stored.widget_type_id.clone(),
                        position_x: stored.position_x,
                        position_y: stored.position_y,
                        width: stored.width,
                        height: stored.height,
                        visible: stored.visible,
                        config: stored.config.clone(),
                        config_schema_version: stored.config_schema_version,
                    },
                    widget(None, 4, 0, 4, 3),
                ],
            )
            .unwrap();
        assert_eq!(new_revision, 2);
        let layout = dashboard.get_layout(&alice, ws).unwrap();
        let still_ghost = layout
            .widgets
            .iter()
            .find(|w| w.id == stored.id)
            .expect("the ghost widget must still exist, unharmed");
        assert_eq!(still_ghost.config, r#"{"symbol":"BTCUSDT"}"#);
        assert_eq!(still_ghost.widget_type_id, "long-gone-plugin/thing");
    }

    // --- optimistic concurrency: two tabs cannot silently overwrite ------

    #[test]
    fn replace_layout_rejects_a_stale_expected_revision() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice7@example.com");
        let ws = dashboard.create_workspace(&alice, "Contested").unwrap();

        // Two tabs both read revision 0.
        let revision_both_tabs_opened_at = 0;

        // The first tab saves and wins, moving to revision 1.
        dashboard
            .replace_layout(
                &alice,
                ws,
                revision_both_tabs_opened_at,
                12,
                &[widget(None, 0, 0, 4, 3)],
            )
            .unwrap();

        // The second tab, still holding the stale revision it opened with,
        // must be refused rather than silently overwriting the first save.
        let err = dashboard
            .replace_layout(
                &alice,
                ws,
                revision_both_tabs_opened_at,
                12,
                &[widget(None, 8, 0, 4, 3)],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            DashboardError::RevisionConflict {
                expected: 0,
                current: 1
            }
        ));

        // Tab A's widget must be exactly what survived — tab B's attempt
        // left no trace.
        let layout = dashboard.get_layout(&alice, ws).unwrap();
        assert_eq!(layout.widgets.len(), 1);
        assert_eq!(layout.widgets[0].position_x, 0);
    }

    // --- one transaction: a failure partway through changes nothing ------

    #[test]
    fn a_failure_partway_through_replace_layout_leaves_the_previous_layout_intact() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice8@example.com");
        let ws = dashboard.create_workspace(&alice, "Atomic").unwrap();

        let revision = dashboard
            .replace_layout(&alice, ws, 0, 12, &[widget(None, 0, 0, 4, 3)])
            .unwrap();
        let before = dashboard.get_layout(&alice, ws).unwrap();
        let real_id = before.widgets[0].id;

        // A snapshot that updates the real widget (position 0,0 -> 1,1)
        // *and* references a second, nonexistent widget id. The update to
        // the real widget executes first, inside the same transaction;
        // the second entry then fails with `WidgetNotFound` before the
        // transaction ever commits.
        let bogus_id = super::DashboardWidgetId::new();
        let moved_real = widget(Some(real_id), 1, 1, 4, 3);
        let bogus = widget(Some(bogus_id), 6, 0, 4, 3);
        let err = dashboard
            .replace_layout(&alice, ws, revision, 12, &[moved_real, bogus])
            .unwrap_err();
        assert!(matches!(err, DashboardError::WidgetNotFound));

        let after = dashboard.get_layout(&alice, ws).unwrap();
        assert_eq!(
            after, before,
            "the whole call must roll back — the real widget's earlier, \
             already-executed UPDATE must not have survived the abandoned \
             transaction, and the workspace's own revision must not have moved"
        );
    }

    // --- grid validation, done in Rust before any database work ----------

    #[test]
    fn replace_layout_rejects_overlapping_widgets() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice9@example.com");
        let ws = dashboard.create_workspace(&alice, "Collide").unwrap();

        // Adjacent, non-overlapping widgets succeed first — proving the
        // check is about overlap specifically, not merely "more than one
        // widget".
        dashboard
            .replace_layout(
                &alice,
                ws,
                0,
                12,
                &[widget(None, 0, 0, 6, 4), widget(None, 6, 0, 6, 4)],
            )
            .unwrap();

        // Now shift the second widget one column left so it overlaps the
        // first by exactly one column — this must be rejected.
        let err = dashboard
            .replace_layout(
                &alice,
                ws,
                1,
                12,
                &[widget(None, 0, 0, 6, 4), widget(None, 5, 0, 6, 4)],
            )
            .unwrap_err();
        assert!(matches!(err, DashboardError::OverlappingWidgets { .. }));
    }

    #[test]
    fn replace_layout_rejects_a_widget_that_does_not_fit_the_columns() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice10@example.com");
        let ws = dashboard.create_workspace(&alice, "Overflow").unwrap();

        let err = dashboard
            .replace_layout(&alice, ws, 0, 12, &[widget(None, 8, 0, 6, 4)])
            .unwrap_err();
        assert!(matches!(err, DashboardError::OutOfBounds { .. }));

        // Nothing must have been written.
        assert_eq!(dashboard.get_layout(&alice, ws).unwrap().widgets.len(), 0);
    }

    #[test]
    fn replace_layout_rejects_invalid_config_json() {
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice11@example.com");
        let ws = dashboard.create_workspace(&alice, "Bad Config").unwrap();

        let bad = WidgetPlacementInput {
            config: "not json".to_owned(),
            ..widget(None, 0, 0, 4, 3)
        };
        let err = dashboard
            .replace_layout(&alice, ws, 0, 12, &[bad])
            .unwrap_err();
        assert!(matches!(err, DashboardError::InvalidConfig(_)));
    }

    // --- geometry is grid cells, never pixels -----------------------------

    #[test]
    fn stored_geometry_round_trips_as_the_exact_integers_it_was_given() {
        // No pixel value is ever computed or stored — a caller applying
        // these integers to a container of any width gets back the same
        // grid placement regardless of how wide that container happens to
        // be at read time.
        let (_dir, identity, dashboard) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = dashboard_user(&identity, &admin, "alice12@example.com");
        let ws = dashboard.create_workspace(&alice, "Units").unwrap();

        dashboard
            .replace_layout(&alice, ws, 0, 24, &[widget(None, 17, 9, 3, 2)])
            .unwrap();
        let layout = dashboard.get_layout(&alice, ws).unwrap();
        assert_eq!(layout.workspace.columns, 24);
        let w = &layout.widgets[0];
        assert_eq!(
            (w.position_x, w.position_y, w.width, w.height),
            (17, 9, 3, 2)
        );
    }
}
