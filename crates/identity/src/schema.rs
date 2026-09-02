//! The accounts database's schema and connection setup.
//!
//! Schema history:
//! - **v1**: `users`, `roles`, `role_grants`, `user_roles`,
//!   `user_grants`, `sessions`.
//! - **v2** (closing the coordination gap Q2 and Q7 each reasonably left to the other — see the plan's "Coordination gap" section): `plugin_permissions`, `role_plugin_grants`,
//!   `user_plugin_grants`, exactly the shapes B14 fixed. `plugin_permissions`
//!   gets a real writer in this crate ([`crate::IdentityStore::load_plugin_permissions`]/
//!   [`crate::IdentityStore::save_plugin_permissions`], paired with
//!   `senken_plugin::reconcile_plugin_permissions`'s pure reconciliation);
//!   the two grant-junction tables are created here but populated by
//!   whichever milestone builds admin role/grant assignment,
//!   the same way `role_grants`/`user_grants` are written by this crate but
//!   only ever populated through `IdentityStore::create_role`/`grant_direct`.
//! - **v3**: `workspaces`, `layouts`, `panes`, `layers` — the
//!   chart persistence tables. Workspaces reference
//!   `users(id)`, so they live in this same file rather than a second SQLite
//!   database; this crate stays the single owner of the file's
//!   `user_version` sequence for exactly that reason, rather than a second
//!   crate racing it with one of its own. Unlike v1/v2, this crate never
//!   queries these four tables itself — `senken-chart` does, sharing
//!   this connection via [`crate::IdentityStore::shared_connection`] rather
//!   than opening a second one to the same file. See that crate's module
//!   docs for the full reasoning behind putting the tables here instead of
//!   giving `senken-chart` its own database.
//! - **v4**: `alerts` — one row per standalone alert. Alerts reference
//!   `users(id)` too, so the same
//!   single-schema-owner reasoning v3 already established for
//!   `senken-chart` applies verbatim here: this crate creates the
//!   table and owns `user_version`, but `senken-alerts` is the only crate
//!   that ever queries it, sharing this connection via
//!   [`crate::IdentityStore::shared_connection`] rather than opening a
//!   second one. See that crate's module docs for the full reasoning.
//! - **v5**: `drawings` — one row per chart drawing object (horizontal
//!   line, trend line, rectangle), owned by a pane the same way `layers`
//!   already is. Same reasoning as v3/v4: created here, queried only by
//!   `senken-chart`.
//! - **v7**: `layers.style` — one JSON-object-text column holding an
//!   indicator layer's plot styling (colour, line style, width, per-plot
//!   visibility), kept apart from `params`: an input change recomputes the
//!   series, a colour change does not.
//! - **v6**: `panes.settings` — one JSON-object-text column holding a
//!   pane's display settings (candle colours, precision, scales, canvas,
//!   status line). Added to the existing `panes` table rather than a new
//!   one, since a pane's settings are 1:1 with the pane itself, the same
//!   relationship `instrument`/`timeframe` already have. This crate does
//!   not interpret the column's contents, matching how it already treats
//!   `layers.indicator_params`.
//! - **v8**: two changes landed together because the second only becomes
//!   cheap while the first is already rewriting every row.
//!
//!   1. `layers` and `drawings` collapse into one `chart_pane_items` table.
//!      They were always the same concept — a positioned, orderable,
//!      show/hideable thing attached to a pane — encoded twice: `layers`
//!      had `visible` and no drawing ever could be hidden, `layers.kind`
//!      baked an overlay-vs-sub-pane *placement* into the same tag as its
//!      *source*, and `drawings` had its own three style columns instead of
//!      `layers`' one JSON one. A `layers` row becomes `source_kind =
//!      'computed'` (an indicator) or `'referenced'` (an overlay
//!      instrument); its old `overlay`/`sub_pane` placement moves out of
//!      the kind tag and into `slot`. A `drawings` row becomes
//!      `source_kind = 'anchored'`, its three style columns folded into
//!      the one shared `style` JSON column every other item already used,
//!      and `visible` defaults to `1` — every drawing has always rendered
//!      unconditionally, so this is the fact that was already true, now
//!      finally expressible. Positions are renumbered contiguously per
//!      pane, layers first then drawings, since the two source tables each
//!      had their own independent `(pane_id, position)` uniqueness and the
//!      merged table has one.
//!   2. `workspaces`/`layouts`/`panes` rename to `chart_workspaces`/
//!      `chart_layouts`/`chart_panes` (`chart_pane_items` is named that way
//!      from birth). A table is prefixed when it is part of a domain
//!      aggregate, and the prefix is the owning crate's name — that rule
//!      did not change. What changed is that a *dashboard* aggregate is
//!      coming with its own `dashboard_workspaces`, and at that point
//!      "workspace" stops naming one aggregate and starts being a common
//!      noun for two of them: `workspace_panes` would no longer answer
//!      "which workspace's pane". `chart_*` still does. `senken_acl::Resource::ChartWorkspace`/`ChartLayout` rename for the same
//!      reason, and every existing `role_grants`/`user_grants` row still
//!      holding the old `workspace`/`layout` token is rewritten to
//!      `chart_workspace`/`chart_layout` in the same migration step — an
//!      account's grants surviving an upgrade is not optional. `ALTER
//!      TABLE … RENAME TO` is cheap in SQLite and keeps every foreign key,
//!      including `chart_pane_items`' own, correctly pointed; only the
//!      indexes need re-creating under their new `<table>_<column>` names,
//!      since SQLite has no `ALTER INDEX RENAME`.
//! - **v9**: `chart_workspaces.settings` (a workspace-level counterpart to
//!   the pane-level `chart_panes.settings` v6 already added — display
//!   preferences that belong to the whole workspace rather than one pane),
//!   plus two new user-owned domains: `watchlist_groups`/
//!   `watchlist_members` and `notes`. Same single-schema-owner reasoning as
//!   v3/v4/v5: created here because they reference `users(id)`, queried
//!   only by `senken-watchlist`/`senken-notes`, which share this connection
//!   via [`crate::IdentityStore::shared_connection`] rather than opening
//!   their own.
//! - **v10**: `dashboard_workspaces`/`dashboard_widgets` — a dashboard
//!   workspace and the widgets placed on its grid. Same single-schema-owner
//!   reasoning as v3/v4/v5/v9: created here because they reference
//!   `users(id)`, queried only by `senken-dashboard`, which shares this
//!   connection the same way `senken-chart`/`senken-notes` do. A dashboard
//!   workspace is its own aggregate, not a row in `chart_workspaces` — see
//!   `senken-chart`'s own module docs for why the two never share a table.
//!   `dashboard_widgets` has no foreign key from `widget_type_id` to any
//!   registry on purpose: a registry entry can disappear (a plugin
//!   disabled, or simply an older build) while a stored layout must still
//!   read back — see `senken-dashboard`'s own module docs.
//! - **v11**: `users.display_zone` — the IANA zone id a user has chosen for
//!   how times are shown to them, validated as a `senken_core::IanaZone`
//!   before this crate ever writes it. Nullable with no default (unlike
//!   `panes.settings`/`chart_workspaces.settings`'s `'{}'`): there is no
//!   meaningful placeholder zone to invent for an account that has never
//!   chosen one, so a fresh column reads back exactly as "not yet chosen"
//!   (`NULL`, decoded as `Option::None`) rather than a guessed value
//!   quietly standing in for one. The browser's own detected zone is a
//!   client-side suggestion for that case only — see
//!   `packages/web/src/lib/time/zone.ts` — never something this crate
//!   invents server-side.
//! - **v12**: `indicator_registry_entries` — one row per published
//!   indicator-lang source, for `senken-indicator-registry`. Same
//!   single-schema-owner reasoning as v3/v4/v5/v9/v10: created here
//!   because it references `users(id)`, queried only by that crate, which
//!   shares this connection the same way `senken-chart`/`senken-notes` do.
//!   `owner_id` **is** a published indicator's namespace — see that
//!   crate's own module docs for why an account id, not a self-chosen
//!   display handle, is what closes publisher impersonation — so
//!   `UNIQUE (owner_id, name)` is what lets two different authors publish
//!   the same `name` without colliding while making it structurally
//!   impossible to hold a second row for a name an account already owns.
//!   `source` is indicator-lang source text, never a compiled artifact —
//!   this registry's whole point is that a binary is never what is stored
//!   or served. `language_version` records the *publishing* host's own
//!   compiler version at the moment of that publish, so an installing host
//!   — potentially a different build, months apart — can refuse an entry
//!   newer than itself with a message naming both versions, rather than
//!   failing to load with none.
//! - **v13**: `registry_handles` — one row per account that has claimed a
//!   human-readable registry handle, for `senken-indicator-registry`. Same
//!   single-schema-owner reasoning as v3/v4/v5/v9/v10/v12: created here
//!   because it references `users(id)`, queried only by that crate, which
//!   shares this connection the same way `senken-chart`/`senken-notes` do.
//!   `owner_id` is the primary key — one handle per account — and
//!   `handle` is `UNIQUE`, which is what makes a handle a pointer at
//!   exactly one account rather than a second, unenforced naming scheme
//!   sitting next to the real one. The account id (`owner_id`, a
//!   published indicator's `namespace`) stays the canonical,
//!   storage-level identity; a handle is only ever a human-facing address
//!   that resolves to it — see that crate's own module docs for why both
//!   are needed.

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::error::IdentityError;

/// The `user_version` this build of the crate creates and expects. Bump
/// this and extend the schema (or add a migration step) when the shape
/// changes — there is deliberately no migration crate, not schema
/// evolution itself.
const SCHEMA_VERSION: i32 = 13;

/// `CREATE TABLE` statements for every table assigned to this
/// milestone: users, roles and the grants attached to either, plus
/// sessions. Column names, types and nullability match B14 exactly —
/// notably `users.password_hash` is nullable, which *is* the B4 first-run
/// fence (state, not a flag that can drift out of sync with it), and
/// `sessions` stores `token_hash`, never the token itself.
const SCHEMA_SQL: &str = r"
CREATE TABLE users (
    id             TEXT PRIMARY KEY,
    email          TEXT NOT NULL UNIQUE,
    display_name   TEXT NOT NULL,
    password_hash  TEXT,
    created_at     INTEGER NOT NULL,
    disabled       INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE roles (
    id           TEXT PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE,
    description  TEXT NOT NULL DEFAULT '',
    builtin      INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE role_grants (
    role_id   TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    action    TEXT NOT NULL,
    resource  TEXT NOT NULL,
    scope     TEXT NOT NULL,
    PRIMARY KEY (role_id, action, resource)
) STRICT;

CREATE TABLE user_roles (
    user_id  TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id  TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, role_id)
) STRICT;

-- Direct grants, per the product brief referenced in a user
-- may hold a grant with no role attached at all.
CREATE TABLE user_grants (
    user_id   TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    action    TEXT NOT NULL,
    resource  TEXT NOT NULL,
    scope     TEXT NOT NULL,
    PRIMARY KEY (user_id, action, resource)
) STRICT;

CREATE TABLE sessions (
    token_hash    BLOB PRIMARY KEY,
    user_id       TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at    INTEGER NOT NULL,
    last_seen_at  INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL
) STRICT;

CREATE INDEX sessions_user_id ON sessions(user_id);
";

/// `CREATE TABLE` statements added in schema v2: the plugin
/// permission tables B14 fixed but that Q2 and Q7 each left to the other
/// (see this plan's "Coordination gap" section). Column names and
/// nullability match B14 exactly.
const SCHEMA_SQL_V2: &str = r"
-- One row per plugin-declared permission this database has ever seen,
-- across every plugin. `name` (e.g.
-- `mychart.dashboard:view`) is globally unique by construction — it embeds
-- the owning plugin's namespace — so it, not `id`, is what
-- `IdentityStore::save_plugin_permissions` upserts on.
CREATE TABLE plugin_permissions (
    id             TEXT PRIMARY KEY,
    plugin_id      TEXT NOT NULL,
    name           TEXT NOT NULL UNIQUE,
    registered_at  INTEGER NOT NULL,
    orphaned       INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX plugin_permissions_plugin_id ON plugin_permissions(plugin_id);

-- Populated by admin action, not by this crate's
-- reconciliation path — a plugin may register a permission but never grant
-- one, so nothing in this crate ever writes to this table.
CREATE TABLE role_plugin_grants (
    role_id               TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    plugin_permission_id  TEXT NOT NULL REFERENCES plugin_permissions(id) ON DELETE CASCADE,
    PRIMARY KEY (role_id, plugin_permission_id)
) STRICT;

-- Direct plugin-permission grants, mirroring `user_grants` for core
-- permissions. Same admin-action-only note as `role_plugin_grants`.
CREATE TABLE user_plugin_grants (
    user_id               TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plugin_permission_id  TEXT NOT NULL REFERENCES plugin_permissions(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, plugin_permission_id)
) STRICT;
";

/// `CREATE TABLE` statements added in schema v3: workspace →
/// layout → panes → layers. Queried and mutated entirely by
/// `senken-chart`, not this crate — see this module's doc comment.
///
/// A pane holds one main instrument (`instrument`/`timeframe`) plus zero or
/// more layers; a layer is either an overlay instrument or an indicator in
/// `overlay`/`sub_pane` placement, encoded in `layers.kind` as
/// `overlay_instrument` / `indicator_overlay` / `indicator_sub_pane`.
/// `position` columns are the caller-assigned ordering within their parent
/// (tab order, grid slot, stacking order) and are unique within it, so two
/// panes cannot silently occupy the same grid slot.
const SCHEMA_SQL_V3: &str = r"
CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;

CREATE INDEX workspaces_owner_id ON workspaces(owner_id);

CREATE TABLE layouts (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    preset        TEXT NOT NULL,
    position      INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    UNIQUE (workspace_id, position)
) STRICT;

CREATE INDEX layouts_workspace_id ON layouts(workspace_id);

CREATE TABLE panes (
    id          TEXT PRIMARY KEY,
    layout_id   TEXT NOT NULL REFERENCES layouts(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    instrument  TEXT NOT NULL,
    timeframe   TEXT NOT NULL,
    UNIQUE (layout_id, position)
) STRICT;

CREATE INDEX panes_layout_id ON panes(layout_id);

-- `instrument` is set iff `kind = 'overlay_instrument'`; `indicator_name`/
-- `indicator_params` are set iff `kind` is one of the two `indicator_*`
-- values. Not enforced by a `CHECK` constraint: the only writer is
-- `senken-chart`, which enforces it in Rust before this table is ever
-- touched (`senken_chart::store::ItemSource`'s three variants each carry
-- exactly the fields their own case needs, so there is nothing to check
-- twice).
CREATE TABLE layers (
    id                TEXT PRIMARY KEY,
    pane_id           TEXT NOT NULL REFERENCES panes(id) ON DELETE CASCADE,
    position          INTEGER NOT NULL,
    kind              TEXT NOT NULL,
    instrument        TEXT,
    indicator_name    TEXT,
    indicator_params  TEXT,
    visible           INTEGER NOT NULL DEFAULT 1,
    UNIQUE (pane_id, position)
) STRICT;

CREATE INDEX layers_pane_id ON layers(pane_id);
";

/// `CREATE TABLE` statements added in schema v4: `alerts`, one
/// row per standalone `(series key, indicator spec, condition)` alert.
/// Queried and mutated entirely by `senken-alerts`, not this
/// crate — see this module's doc comment.
///
/// `instrument`/`timeframe` are an alert's series key, encoded the same way
/// `panes.instrument`/`panes.timeframe` already are in v3.
/// `indicator_name`/`indicator_params` name one of `senken-indicators`' ten
/// built-ins the same way `layers.indicator_name`/`layers.indicator_params`
/// do. `condition_field`/`condition_comparator`/`condition_threshold` are
/// `senken-alerts`' own addition: which of an indicator's own numbers to
/// read, how to compare it, and the threshold — on the indicator side of
/// the `f64` boundary, so `condition_threshold` is `REAL`, not a
/// scaled integer. `last_fired_at`/`last_fired_value`/`fire_count` are the
/// engine's own bookkeeping ("firing is recording that it fired" — no notification of any kind is ever sent).
const SCHEMA_SQL_V4: &str = r"
CREATE TABLE alerts (
    id                    TEXT PRIMARY KEY,
    owner_id              TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    instrument            TEXT NOT NULL,
    timeframe             TEXT NOT NULL,
    indicator_name        TEXT NOT NULL,
    indicator_params      TEXT NOT NULL,
    condition_field       TEXT NOT NULL,
    condition_comparator  TEXT NOT NULL,
    condition_threshold   REAL NOT NULL,
    enabled               INTEGER NOT NULL DEFAULT 1,
    created_at            INTEGER NOT NULL,
    updated_at            INTEGER NOT NULL,
    last_fired_at         INTEGER,
    last_fired_value      REAL,
    fire_count            INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE INDEX alerts_owner_id ON alerts(owner_id);
CREATE INDEX alerts_enabled ON alerts(enabled);
";

/// `CREATE TABLE` statements added in schema v5: `drawings`, one row per
/// chart drawing object attached to a pane. Queried and mutated entirely by
/// `senken-chart`, not this crate — see this module's doc comment.
///
/// `kind` is one of `horizontal_line` / `trend_line` / `rectangle`. A
/// horizontal line uses only `price`; a trend line or rectangle uses both
/// `(time1, price1)` and `(time2, price2)` as its two anchors and leaves
/// `price` `NULL`. `time1`/`time2` are Unix nanoseconds, the same unit
/// `senken_core::UnixNanos` uses everywhere else in this project. A
/// drawing's price is a coordinate the user set by clicking a chart, not an
/// order price — the same reasoning that already gives `alerts.
/// condition_threshold` above a `REAL` column rather than a scaled
/// integer — so `price`/`price1`/`price2` are `REAL` too. `color`/`width`/
/// `line_style` are the one drawing-wide style every object carries
/// (colour, width, line style), editable after the fact the same way a
/// price line's own `title` is set once and never touched again by this
/// schema.
const SCHEMA_SQL_V5: &str = r"
CREATE TABLE drawings (
    id          TEXT PRIMARY KEY,
    pane_id     TEXT NOT NULL REFERENCES panes(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    price       REAL,
    time1       INTEGER,
    price1      REAL,
    time2       INTEGER,
    price2      REAL,
    color       TEXT NOT NULL,
    width       INTEGER NOT NULL,
    line_style  TEXT NOT NULL,
    UNIQUE (pane_id, position)
) STRICT;

CREATE INDEX drawings_pane_id ON drawings(pane_id);
";

/// `ALTER TABLE` statement added in schema v6: `panes.settings`, a
/// JSON-object-text column for a pane's display settings — see this
/// module's doc comment. Defaults existing rows to an empty settings
/// object rather than `NULL`, so every reader (`senken-chart`'s own
/// `PaneRecord::settings`) can treat the column as a plain, always-present
/// string with no `Option` to thread through call sites that predate this
/// column.
const SCHEMA_SQL_V6: &str = r"
ALTER TABLE panes ADD COLUMN settings TEXT NOT NULL DEFAULT '{}';
";

/// `ALTER TABLE` statement added in schema v7: `layers.style`, a
/// JSON-object-text column for an indicator layer's plot styling — the
/// colour, line style, width and per-plot visibility a chart draws it
/// with. Separate from the layer's `params`, which are the indicator's own
/// inputs: changing a period recomputes the series, changing a colour does
/// not. Defaults existing rows to an empty object so every reader can treat
/// it as an always-present string.
const SCHEMA_SQL_V7: &str = r"
ALTER TABLE layers ADD COLUMN style TEXT NOT NULL DEFAULT '{}';
";

/// Statements added in schema v8 — see this module's doc comment for the
/// full reasoning. Wrapped in its own explicit `BEGIN`/`COMMIT` (unlike
/// every simpler step above it): `execute_batch` runs each statement in a
/// script separately and does not roll one back on its own, and this step
/// is complex enough — table renames, a data-carrying merge, two drops —
/// that a failure partway through must leave a v7 database exactly as it
/// was, never a half-renamed one.
///
/// Order matters: the three renames run first so `chart_pane_items`' own
/// foreign key can name `chart_panes` directly; `layers`/`drawings` are
/// merged into it next (`layers` first, `drawings` after, per pane — see
/// the module doc comment on why positions are renumbered rather than
/// copied); the two old tables are dropped once every row has a new home;
/// and the grant-token rewrite runs last since `role_grants`/`user_grants`
/// are untouched by anything above it.
const SCHEMA_SQL_V8: &str = r"
BEGIN IMMEDIATE;

ALTER TABLE workspaces RENAME TO chart_workspaces;
DROP INDEX workspaces_owner_id;
CREATE INDEX chart_workspaces_owner_id ON chart_workspaces(owner_id);

ALTER TABLE layouts RENAME TO chart_layouts;
DROP INDEX layouts_workspace_id;
CREATE INDEX chart_layouts_workspace_id ON chart_layouts(workspace_id);

ALTER TABLE panes RENAME TO chart_panes;
DROP INDEX panes_layout_id;
CREATE INDEX chart_panes_layout_id ON chart_panes(layout_id);

CREATE TABLE chart_pane_items (
    id                TEXT PRIMARY KEY,
    pane_id           TEXT NOT NULL REFERENCES chart_panes(id) ON DELETE CASCADE,
    position          INTEGER NOT NULL,
    slot              TEXT NOT NULL,
    slot_index        INTEGER,
    visible           INTEGER NOT NULL DEFAULT 1,
    style             TEXT NOT NULL DEFAULT '{}',
    source_kind       TEXT NOT NULL,
    instrument        TEXT,
    indicator_name    TEXT,
    indicator_params  TEXT,
    tool_kind         TEXT,
    price             REAL,
    time1             INTEGER,
    price1            REAL,
    time2             INTEGER,
    price2            REAL,
    label_text        TEXT,
    label_anchor      TEXT,
    UNIQUE (pane_id, position)
) STRICT;

CREATE INDEX chart_pane_items_pane_id ON chart_pane_items(pane_id);

-- `layers` rows first: `indicator_sub_pane` becomes `slot = 'sub'`
-- (index 0 — this build has never had more than one sub-pane slot per
-- pane), everything else `slot = 'main'`. `overlay_instrument` becomes
-- `source_kind = 'referenced'`; the two indicator kinds collapse into
-- `'computed'` now that placement lives on `slot` rather than the kind tag.
INSERT INTO chart_pane_items (
    id, pane_id, position, slot, slot_index, visible, style,
    source_kind, instrument, indicator_name, indicator_params
)
SELECT
    id, pane_id,
    ROW_NUMBER() OVER (PARTITION BY pane_id ORDER BY position) - 1,
    CASE WHEN kind = 'indicator_sub_pane' THEN 'sub' ELSE 'main' END,
    CASE WHEN kind = 'indicator_sub_pane' THEN 0 ELSE NULL END,
    visible, style,
    CASE WHEN kind = 'overlay_instrument' THEN 'referenced' ELSE 'computed' END,
    instrument, indicator_name, indicator_params
FROM layers;

-- `drawings` rows land after that pane's own migrated layers (the offset
-- subquery reads only what the insert above already committed), so a
-- drawing keeps stacking visually above indicators the same way it always
-- has. `drawings` never had a `visible` column at all — every drawing has
-- always rendered unconditionally, so `1` is not a default so much as the
-- fact that was already true. Its three style columns fold into the one
-- JSON `style` column every other item already used.
INSERT INTO chart_pane_items (
    id, pane_id, position, slot, slot_index, visible, style,
    source_kind, tool_kind, price, time1, price1, time2, price2
)
SELECT
    d.id, d.pane_id,
    (SELECT COALESCE(MAX(position), -1) + 1 FROM chart_pane_items WHERE pane_id = d.pane_id)
      + ROW_NUMBER() OVER (PARTITION BY d.pane_id ORDER BY d.position) - 1,
    'main', NULL, 1,
    json_object('color', d.color, 'width', d.width, 'line_style', d.line_style),
    'anchored', d.kind, d.price, d.time1, d.price1, d.time2, d.price2
FROM drawings d;

DROP TABLE layers;
DROP TABLE drawings;

-- `senken_acl::Resource::ChartWorkspace`/`ChartLayout` (formerly
-- `Workspace`/`Layout`) changed their stored token to match; every grant
-- referencing the old token is rewritten so an upgrade never silently
-- revokes a user's existing chart permissions.
UPDATE role_grants SET resource = 'chart_workspace' WHERE resource = 'workspace';
UPDATE role_grants SET resource = 'chart_layout' WHERE resource = 'layout';
UPDATE user_grants SET resource = 'chart_workspace' WHERE resource = 'workspace';
UPDATE user_grants SET resource = 'chart_layout' WHERE resource = 'layout';

COMMIT;
";

/// Statements added in schema v9: `chart_workspaces.settings` (opaque
/// JSON-object text, the same shape and same non-interpretation rule as
/// `chart_panes.settings` from v6 — see `senken-chart`'s own
/// `ChartWorkspaceStore::update_workspace_settings`), plus the tables for
/// two new user-owned domains, `senken-watchlist` and `senken-notes`.
///
/// `watchlist_members` carries no `owner_id` of its own — a member's owner
/// is read through its group, the same way a `chart_pane`'s owner is read
/// through its workspace — so deleting a group cascades its members
/// without a second, redundant ownership column to keep in sync.
/// `(group_id, instrument)` is unique so adding an instrument a group
/// already holds is a lookup, not a constraint violation the caller has to
/// handle.
///
/// `ALTER TABLE … ADD COLUMN … DEFAULT '{}'` is allowed here (unlike a
/// non-constant default) because `'{}'` is a literal, not an expression —
/// SQLite only refuses `ADD COLUMN` defaults it cannot fold into every
/// existing row at add-time.
///
/// Wrapped in one transaction, like v8 and unlike the smaller steps before
/// it. `user_version` is only stamped once every step has run, so a batch
/// that failed halfway would leave some of these tables created against a
/// database still calling itself v8 — and the next start would re-run this
/// step and die on a table that already exists, needing a hand to repair.
/// Atomic, it either all lands or none of it does.
const SCHEMA_SQL_V9: &str = r"
BEGIN IMMEDIATE;

ALTER TABLE chart_workspaces ADD COLUMN settings TEXT NOT NULL DEFAULT '{}';

CREATE TABLE watchlist_groups (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    position    INTEGER NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX watchlist_groups_owner_id ON watchlist_groups(owner_id);

CREATE TABLE watchlist_members (
    id          TEXT PRIMARY KEY,
    group_id    TEXT NOT NULL REFERENCES watchlist_groups(id) ON DELETE CASCADE,
    instrument  TEXT NOT NULL,
    position    INTEGER NOT NULL,
    UNIQUE (group_id, instrument)
) STRICT;
CREATE INDEX watchlist_members_group_id ON watchlist_members(group_id);

CREATE TABLE notes (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX notes_owner_id ON notes(owner_id);

COMMIT;
";

/// Statements added in schema v10: `dashboard_workspaces`/
/// `dashboard_widgets`, for `senken-dashboard`.
///
/// `columns` and `revision` both default to values a fresh row always
/// supplies explicitly (`senken_dashboard::DashboardWorkspaceStore` never
/// relies on either default) — kept anyway so a hand-inspected row without
/// them is still valid `STRICT` data, the same defensive default this
/// schema already gives `dashboard_widgets.visible`.
///
/// `widget_type_id` intentionally has no foreign key: a registry entry can
/// disappear (a plugin disabled, or simply an older build) while a stored
/// layout must still read back — see `senken-dashboard`'s own module docs.
/// Collision-rectangle and grid-bounds validation is `senken-dashboard`'s
/// job in Rust, not a constraint here — plain SQL cannot express "no two
/// rows' rectangles overlap".
const SCHEMA_SQL_V10: &str = r"
CREATE TABLE dashboard_workspaces (
    id          TEXT PRIMARY KEY,
    owner_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    columns     INTEGER NOT NULL DEFAULT 12,
    revision    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
) STRICT;
CREATE INDEX dashboard_workspaces_owner_id ON dashboard_workspaces(owner_id);

CREATE TABLE dashboard_widgets (
    id                     TEXT PRIMARY KEY,
    workspace_id           TEXT NOT NULL
        REFERENCES dashboard_workspaces(id) ON DELETE CASCADE,
    provider_id            TEXT NOT NULL,
    widget_type_id         TEXT NOT NULL,
    position_x             INTEGER NOT NULL,
    position_y             INTEGER NOT NULL,
    width                  INTEGER NOT NULL,
    height                 INTEGER NOT NULL,
    visible                INTEGER NOT NULL DEFAULT 1,
    config                 TEXT NOT NULL,
    config_schema_version  INTEGER NOT NULL,
    created_at             INTEGER NOT NULL,
    updated_at             INTEGER NOT NULL
) STRICT;
CREATE INDEX dashboard_widgets_workspace_id ON dashboard_widgets(workspace_id);
";

/// `ALTER TABLE` statement added in schema v11: `users.display_zone` — see
/// this module's doc comment. No `DEFAULT`, unlike v6/v7/v9's JSON-object
/// columns: `NULL` already means exactly what this column needs an existing
/// row to mean ("this account has not chosen a display zone"), so there is
/// no placeholder value to fabricate for a row written before this column
/// existed.
const SCHEMA_SQL_V11: &str = r"
ALTER TABLE users ADD COLUMN display_zone TEXT;
";

/// `CREATE TABLE` statement added in schema v12: `indicator_registry_entries`,
/// for `senken-indicator-registry`. See this module's doc comment.
const SCHEMA_SQL_V12: &str = r"
CREATE TABLE indicator_registry_entries (
    id                TEXT PRIMARY KEY,
    owner_id          TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    source            TEXT NOT NULL,
    language_version  TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL,
    UNIQUE (owner_id, name)
) STRICT;

CREATE INDEX indicator_registry_entries_owner_id ON indicator_registry_entries(owner_id);
CREATE INDEX indicator_registry_entries_name ON indicator_registry_entries(name);
";

/// `CREATE TABLE` statement added in schema v13: `registry_handles`, for
/// `senken-indicator-registry`. See this module's doc comment. `handle`'s
/// `UNIQUE` constraint is the actual guard against two accounts holding the
/// same handle -- the only one that also closes the race an
/// application-level check-then-insert cannot, which is why this is a
/// column constraint and not left to that crate alone.
const SCHEMA_SQL_V13: &str = r"
CREATE TABLE registry_handles (
    owner_id    TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    handle      TEXT NOT NULL UNIQUE,
    created_at  INTEGER NOT NULL
) STRICT;
";

/// Opens (creating if absent) the SQLite database at `path`, applies the
/// the pragmas this database requires, and creates or checks the schema.
///
/// # Errors
/// [`IdentityError::Database`] if SQLite cannot open or configure the file;
/// [`IdentityError::SchemaVersionMismatch`] if an existing file's
/// `user_version` is neither `0` nor [`SCHEMA_VERSION`].
pub(crate) fn open(path: &Path) -> Result<Connection, IdentityError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(path)?;

    // WAL + NORMAL synchronous: readers do not block the
    // writer, and a crash loses at most the last commit rather than
    // corrupting the file — an acceptable trade for an accounts database
    // that is not the system of record for money movement.
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // A single connection is shared behind a mutex (see `IdentityStore`),
    // so contention is between this process and nothing else — five
    // seconds is generous headroom for a slow disk, not a real wait.
    conn.busy_timeout(Duration::from_secs(5))?;
    // `ON DELETE CASCADE` above only takes effect with this on; SQLite
    // defaults it off for backwards compatibility with pre-3.6.19 files.
    conn.pragma_update(None, "foreign_keys", true)?;

    ensure_schema(&conn)?;
    Ok(conn)
}

/// Creates the schema on a fresh database, or confirms an existing one
/// matches what this crate expects — migrating an older database in place
/// by applying each version's SQL in turn (v1 adds v2's plugin permission
/// tables, v2 adds v3's workspace tables, v3 adds v4's alerts table, v4 adds
/// v5's drawings table, v5 adds v6's `panes.settings` column, v6 adds v7's
/// `layers.style` column, v7 merges `layers`/`drawings` into
/// `chart_pane_items` and renames the chart tables to `chart_*`, v8 adds
/// v9's `chart_workspaces.settings` column plus the watchlist and notes
/// tables, v9 adds v10's `dashboard_workspaces`/`dashboard_widgets`
/// tables, v10 adds v11's `users.display_zone` column, v11 adds v12's
/// `indicator_registry_entries` table, v12 adds v13's `registry_handles`
/// table), since there is no
/// migration crate but not migrating by hand.
/// A database newer than this crate knows about is reported, never guessed
/// at.
fn ensure_schema(conn: &Connection) -> Result<(), IdentityError> {
    let found: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    if found > SCHEMA_VERSION {
        return Err(IdentityError::SchemaVersionMismatch {
            found,
            expected: SCHEMA_VERSION,
        });
    }

    let mut version = found;
    if version == 0 {
        conn.execute_batch(SCHEMA_SQL)?;
        version = 1;
    }
    if version == 1 {
        conn.execute_batch(SCHEMA_SQL_V2)?;
        version = 2;
    }
    if version == 2 {
        conn.execute_batch(SCHEMA_SQL_V3)?;
        version = 3;
    }
    if version == 3 {
        conn.execute_batch(SCHEMA_SQL_V4)?;
        version = 4;
    }
    if version == 4 {
        conn.execute_batch(SCHEMA_SQL_V5)?;
        version = 5;
    }
    if version == 5 {
        conn.execute_batch(SCHEMA_SQL_V6)?;
        version = 6;
    }
    if version == 6 {
        conn.execute_batch(SCHEMA_SQL_V7)?;
        version = 7;
    }
    if version == 7 {
        conn.execute_batch(SCHEMA_SQL_V8)?;
        version = 8;
    }
    if version == 8 {
        conn.execute_batch(SCHEMA_SQL_V9)?;
        version = 9;
    }
    if version == 9 {
        conn.execute_batch(SCHEMA_SQL_V10)?;
        version = 10;
    }
    if version == 10 {
        conn.execute_batch(SCHEMA_SQL_V11)?;
        version = 11;
    }
    if version == 11 {
        conn.execute_batch(SCHEMA_SQL_V12)?;
        version = 12;
    }
    if version == 12 {
        conn.execute_batch(SCHEMA_SQL_V13)?;
        version = 13;
    }
    debug_assert_eq!(
        version, SCHEMA_VERSION,
        "every step from `found` to SCHEMA_VERSION must be applied above"
    );
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;

    if found == 0 {
        tracing::info!(schema_version = SCHEMA_VERSION, "accounts schema created");
    } else {
        tracing::info!(
            from = found,
            to = SCHEMA_VERSION,
            "accounts schema migrated"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::open;
    use crate::error::IdentityError;

    #[test]
    fn opening_a_fresh_path_creates_every_table_this_crate_owns() {
        let dir = TempDir::new().unwrap();
        let conn = open(&dir.path().join("accounts.db")).unwrap();

        for table in [
            "users",
            "roles",
            "role_grants",
            "user_roles",
            "user_grants",
            "sessions",
            "plugin_permissions",
            "role_plugin_grants",
            "user_plugin_grants",
            "chart_workspaces",
            "chart_layouts",
            "chart_panes",
            "chart_pane_items",
            "alerts",
            "watchlist_groups",
            "watchlist_members",
            "notes",
            "dashboard_workspaces",
            "dashboard_widgets",
            "indicator_registry_entries",
            "registry_handles",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(exists, "table `{table}` was not created");
        }
        for table in ["workspaces", "layouts", "panes", "layers", "drawings"] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(
                !exists,
                "pre-v8 table `{table}` must not survive on a fresh database"
            );
        }
    }

    #[test]
    fn a_v1_database_is_migrated_in_place_to_the_current_version_without_losing_its_v1_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // Simulate a database written by the pre-Q4 (v1-only) schema:
            // the real v1 SQL, `user_version` left at 1, and no plugin
            // permission or workspace tables at all.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(
            version,
            super::SCHEMA_VERSION,
            "the database must end up on the current version, not just the next one"
        );

        for table in [
            "plugin_permissions",
            "role_plugin_grants",
            "user_plugin_grants",
            "chart_workspaces",
            "chart_layouts",
            "chart_panes",
            "chart_pane_items",
            "alerts",
            "watchlist_groups",
            "watchlist_members",
            "notes",
            "dashboard_workspaces",
            "dashboard_widgets",
            "indicator_registry_entries",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(exists, "migration must add `{table}`");
        }
        // The v1 tables must still be there — this is a migration, not a
        // rebuild.
        let users_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'users'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(users_exists);
    }

    #[test]
    fn a_v2_database_is_migrated_in_place_to_v3_without_losing_its_v2_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // Simulate a database written by the pre-R1 (v2) schema: v1 +
            // v2 SQL, `user_version` left at 2, and no workspace tables.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.pragma_update(None, "user_version", 2).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        for table in ["chart_workspaces", "chart_layouts", "chart_panes", "alerts"] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(exists, "migration must add `{table}`");
        }
        let plugin_permissions_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'plugin_permissions'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            plugin_permissions_exists,
            "the v2 tables must still be there — this is a migration, not a rebuild"
        );
    }

    #[test]
    fn a_v3_database_is_migrated_in_place_to_v4_without_losing_its_v3_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // Simulate a database written by the pre-R6 (v3) schema: v1 + v2
            // + v3 SQL, `user_version` left at 3, and no `alerts` table.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.pragma_update(None, "user_version", 3).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let alerts_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'alerts'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(alerts_exists, "migration must add `alerts`");

        let workspaces_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chart_workspaces'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            workspaces_exists,
            "the v3 tables must still be there (renamed to chart_* by v8) — this is a migration, not a rebuild"
        );
    }

    #[test]
    fn a_v4_database_is_migrated_in_place_to_v5_without_losing_its_v4_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // Simulate a database written by the pre-drawings (v4) schema:
            // v1 + v2 + v3 + v4 SQL, `user_version` left at 4, and no
            // `drawings` table.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
            conn.pragma_update(None, "user_version", 4).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let pane_items_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chart_pane_items'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            pane_items_exists,
            "migration must add `drawings` (v5) and later fold it, with `layers`, into `chart_pane_items` (v8)"
        );

        let alerts_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'alerts'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            alerts_exists,
            "the v4 tables must still be there — this is a migration, not a rebuild"
        );
    }

    #[test]
    fn a_v5_database_is_migrated_in_place_to_v6_and_an_existing_pane_defaults_to_empty_settings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // Simulate a database written by the pre-pane-settings (v5)
            // schema: v1 + v2 + v3 + v4 + v5 SQL, `user_version` left at 5,
            // and one pane row written before `settings` existed.
            let conn = rusqlite::Connection::open(&path).unwrap();
            // This raw pre-migration connection inserts a pane with no
            // matching layout below (unlike a real write path, which always
            // goes through a real layout) — foreign keys are off here
            // purely so that insert is accepted; it plays no other role in
            // what this test checks.
            conn.pragma_update(None, "foreign_keys", false).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
            conn.execute(
                "INSERT INTO panes (id, layout_id, position, instrument, timeframe)
                 VALUES ('pane-1', 'layout-1', 0, 'binance-spot:BTCUSDT', '1h')",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 5).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let settings: String = conn
            .query_row(
                "SELECT settings FROM chart_panes WHERE id = 'pane-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            settings, "{}",
            "a pane written before this column existed must default to an empty settings object, not NULL"
        );

        let pane_items_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chart_pane_items'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            pane_items_exists,
            "the v5 tables must still be there (renamed/merged by v8) — this is a migration, not a rebuild"
        );
    }

    /// Builds a genuine v7 database at `path` with the exact v1..v7 SQL
    /// this crate has actually shipped (not an invented approximation of
    /// the schema), then writes a realistic layout by hand the same way
    /// `senken-chart`'s own `replace_layout` would have: three layers (an
    /// EMA overlay, an RSI sub-pane, an overlaid instrument) and two
    /// drawings (a horizontal line, a trend line) on one pane, plus a role
    /// grant and a direct user grant still holding the pre-rename
    /// `workspace`/`layout` tokens — exactly what a real account
    /// accumulated before this crate ever knew about
    /// `chart_workspace`/`chart_layout`. Shared by every
    /// `a_v7_database_*` test below so each one migrates its own fresh
    /// copy rather than one test's assertions depending on another's.
    fn seed_v7_layout_fixture(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(super::SCHEMA_SQL).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();

        conn.execute(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-1', 'v7@example.com', 'V7 User', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO roles (id, name, builtin) VALUES ('role-1', 'Charts User', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO role_grants (role_id, action, resource, scope) VALUES ('role-1', 'view', 'workspace', 'own'), ('role-1', 'edit', 'layout', 'own')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO user_grants (user_id, action, resource, scope) VALUES ('user-1', 'create', 'workspace', 'own')",
            [],
        ).unwrap();

        conn.execute(
            "INSERT INTO workspaces (id, owner_id, name, created_at, updated_at) VALUES ('ws-1', 'user-1', 'My Charts', 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO layouts (id, workspace_id, name, preset, position, created_at, updated_at) VALUES ('layout-1', 'ws-1', 'Main', '1', 0, 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO panes (id, layout_id, position, instrument, timeframe) VALUES ('pane-1', 'layout-1', 0, 'okx-spot:BTCUSDT', '1h')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO layers (id, pane_id, position, kind, instrument, indicator_name, indicator_params, visible, style)
             VALUES ('layer-ema', 'pane-1', 0, 'indicator_overlay', NULL, 'EMA', '{\"period\":20}', 1, '{\"color\":\"#ffaa00\"}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO layers (id, pane_id, position, kind, instrument, indicator_name, indicator_params, visible, style)
             VALUES ('layer-rsi', 'pane-1', 1, 'indicator_sub_pane', NULL, 'RSI', '{\"period\":14}', 1, '{}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO layers (id, pane_id, position, kind, instrument, indicator_name, indicator_params, visible, style)
             VALUES ('layer-overlay', 'pane-1', 2, 'overlay_instrument', 'okx-spot:ETHUSDT', NULL, NULL, 0, '{}')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO drawings (id, pane_id, position, kind, price, time1, price1, time2, price2, color, width, line_style)
             VALUES ('drawing-hline', 'pane-1', 0, 'horizontal_line', 2450.5, NULL, NULL, NULL, NULL, '#f2f2ef', 2, 'dashed')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO drawings (id, pane_id, position, kind, price, time1, price1, time2, price2, color, width, line_style)
             VALUES ('drawing-trend', 'pane-1', 1, 'trend_line', NULL, 1700000000000000000, 100.0, 1700003600000000000, 101.5, '#7aa7e8', 1, 'solid')",
            [],
        ).unwrap();
        conn.pragma_update(None, "user_version", 7).unwrap();
    }

    #[test]
    fn a_v7_database_migrates_to_v8_without_losing_a_single_row() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);

        let (before_layer_count, before_drawing_count): (i64, i64) = {
            let conn = rusqlite::Connection::open(&path).unwrap();
            (
                conn.query_row("SELECT COUNT(*) FROM layers", [], |row| row.get(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM drawings", [], |row| row.get(0))
                    .unwrap(),
            )
        };
        assert_eq!((before_layer_count, before_drawing_count), (3, 2));

        let conn = open(&path).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |row| row.get::<_, i32>(0))
                .unwrap(),
            super::SCHEMA_VERSION
        );

        // No row was lost: three migrated layers plus two migrated
        // drawings is exactly five pane items — counted, not assumed.
        let after_item_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chart_pane_items", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            after_item_count,
            before_layer_count + before_drawing_count,
            "every layer and drawing row must land as exactly one pane item"
        );

        // Every id survived, in insertion order, indicator/overlay items
        // before drawings.
        let mut stmt = conn
            .prepare("SELECT id FROM chart_pane_items WHERE pane_id = 'pane-1' ORDER BY position")
            .unwrap();
        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            ids,
            vec![
                "layer-ema",
                "layer-rsi",
                "layer-overlay",
                "drawing-hline",
                "drawing-trend",
            ]
        );
    }

    #[test]
    fn renaming_workspaces_layouts_and_panes_to_chart_preserves_every_row_count() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);

        let (workspaces_before, layouts_before, panes_before): (i64, i64, i64) = {
            let conn = rusqlite::Connection::open(&path).unwrap();
            (
                conn.query_row("SELECT COUNT(*) FROM workspaces", [], |row| row.get(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM layouts", [], |row| row.get(0))
                    .unwrap(),
                conn.query_row("SELECT COUNT(*) FROM panes", [], |row| row.get(0))
                    .unwrap(),
            )
        };
        assert_eq!((workspaces_before, layouts_before, panes_before), (1, 1, 1));

        let conn = open(&path).unwrap();
        let workspaces_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM chart_workspaces", [], |row| {
                row.get(0)
            })
            .unwrap();
        let layouts_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM chart_layouts", [], |row| row.get(0))
            .unwrap();
        let panes_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM chart_panes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            (workspaces_after, layouts_after, panes_after),
            (workspaces_before, layouts_before, panes_before),
            "a table rename must not add or drop a single row"
        );
        // And the actual row, not just the count, is still reachable under
        // its original id.
        let workspace_name: String = conn
            .query_row(
                "SELECT name FROM chart_workspaces WHERE id = 'ws-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(workspace_name, "My Charts");
    }

    #[test]
    fn a_migrated_v7_layer_carries_the_right_slot_and_source_kind() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);
        let conn = open(&path).unwrap();

        // Placement moved from the kind tag to `slot`: the sub-pane
        // indicator is the only `sub` row, everything else is `main`.
        let (rsi_slot, rsi_source): (String, String) = conn
            .query_row(
                "SELECT slot, source_kind FROM chart_pane_items WHERE id = 'layer-rsi'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (rsi_slot.as_str(), rsi_source.as_str()),
            ("sub", "computed")
        );

        let (overlay_slot, overlay_source, overlay_instrument): (String, String, String) = conn
            .query_row(
                "SELECT slot, source_kind, instrument FROM chart_pane_items WHERE id = 'layer-overlay'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(overlay_slot, "main");
        assert_eq!(overlay_source, "referenced");
        assert_eq!(overlay_instrument, "okx-spot:ETHUSDT");
    }

    #[test]
    fn a_migrated_v7_drawing_defaults_to_visible_while_a_hidden_layer_stays_hidden() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);
        let conn = open(&path).unwrap();

        // `visible` now applies uniformly: the layer that was explicitly
        // hidden stays hidden, and — the property that could not be
        // expressed before this migration — a drawing that had no
        // `visible` column at all defaults to shown.
        let overlay_visible: bool = conn
            .query_row(
                "SELECT visible FROM chart_pane_items WHERE id = 'layer-overlay'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!overlay_visible, "the layer's own visible=0 must survive");
        let (hline_visible, trend_visible): (bool, bool) = conn
            .query_row(
                "SELECT
                    (SELECT visible FROM chart_pane_items WHERE id = 'drawing-hline'),
                    (SELECT visible FROM chart_pane_items WHERE id = 'drawing-trend')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(
            hline_visible && trend_visible,
            "a migrated drawing must default to visible — it could never be hidden before v8"
        );
    }

    #[test]
    fn a_migrated_v7_drawings_geometry_and_style_columns_survive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);
        let conn = open(&path).unwrap();

        // A drawing's geometry and its three former style columns survive,
        // folded into the shared JSON `style` column.
        let (tool_kind, price, style): (String, f64, String) = conn
            .query_row(
                "SELECT tool_kind, price, style FROM chart_pane_items WHERE id = 'drawing-hline'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(tool_kind, "horizontal_line");
        assert!((price - 2450.5).abs() < f64::EPSILON);
        assert!(style.contains(r##""color":"#f2f2ef""##));
        assert!(style.contains(r#""width":2"#));
        assert!(style.contains(r#""line_style":"dashed""#));

        let (t1_kind, t1_time1, t1_price1, t1_time2, t1_price2): (String, i64, f64, i64, f64) = conn
            .query_row(
                "SELECT tool_kind, time1, price1, time2, price2 FROM chart_pane_items WHERE id = 'drawing-trend'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(t1_kind, "trend_line");
        assert_eq!(t1_time1, 1_700_000_000_000_000_000);
        assert!((t1_price1 - 100.0).abs() < f64::EPSILON);
        assert_eq!(t1_time2, 1_700_003_600_000_000_000);
        assert!((t1_price2 - 101.5).abs() < f64::EPSILON);
    }

    /// Builds a genuine v8 database at `path` (the real v1..v8 SQL this
    /// crate has actually shipped) with one pre-existing chart workspace,
    /// so the v9 migration tests below can assert that row survives the
    /// `ALTER TABLE … ADD COLUMN` untouched.
    fn seed_v8_workspace_fixture(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(super::SCHEMA_SQL).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V8).unwrap();

        conn.execute(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-v8', 'v8@example.com', 'V8 User', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chart_workspaces (id, owner_id, name, created_at, updated_at) VALUES ('ws-v8', 'user-v8', 'Pre-existing', 0, 0)",
            [],
        ).unwrap();
        conn.pragma_update(None, "user_version", 8).unwrap();
    }

    #[test]
    fn a_v8_database_migrates_to_v9_with_the_three_new_tables_and_settings_column() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v8_workspace_fixture(&path);

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        // The database must end up on the current version, not stop at the
        // v9 this migration step itself targets — see the v1 test's own
        // comment for why this is `super::SCHEMA_VERSION`, not a literal.
        assert_eq!(version, super::SCHEMA_VERSION);

        for table in ["watchlist_groups", "watchlist_members", "notes"] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(
                exists,
                "table `{table}` was not created by the v9 migration"
            );
        }

        let column_exists: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('chart_workspaces') WHERE name = 'settings'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            column_exists,
            "chart_workspaces.settings must exist after the v9 migration"
        );
    }

    #[test]
    fn a_pre_existing_chart_workspace_survives_the_v9_migration_with_empty_settings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v8_workspace_fixture(&path);

        let conn = open(&path).unwrap();
        let (name, settings): (String, String) = conn
            .query_row(
                "SELECT name, settings FROM chart_workspaces WHERE id = 'ws-v8'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            name, "Pre-existing",
            "the migration must not touch existing columns"
        );
        assert_eq!(
            settings, "{}",
            "a workspace written before this column existed must default to an empty settings object, not NULL"
        );
    }

    #[test]
    fn a_v9_database_can_insert_into_every_new_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        let conn = open(&path).unwrap();

        conn.execute(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-v9', 'v9@example.com', 'V9 User', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO watchlist_groups (id, owner_id, name, position, created_at, updated_at) VALUES ('grp-1', 'user-v9', 'Majors', 0, 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO watchlist_members (id, group_id, instrument, position) VALUES ('mem-1', 'grp-1', 'okx-spot:BTCUSDT', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, owner_id, title, body, created_at, updated_at) VALUES ('note-1', 'user-v9', 'Title', 'Body', 0, 0)",
            [],
        ).unwrap();

        // Cascade: deleting the owning group must remove its member row,
        // the same `ON DELETE CASCADE` `chart_workspaces` already relies on.
        conn.execute("DELETE FROM watchlist_groups WHERE id = 'grp-1'", [])
            .unwrap();
        let member_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM watchlist_members", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            member_count, 0,
            "a watchlist member must be cascade-deleted with its group"
        );
    }

    /// Builds a genuine v9 database at `path` (the real v1..v9 SQL this
    /// crate has actually shipped) with one pre-existing chart workspace,
    /// so the v10 migration test below can assert that row survives
    /// untouched.
    fn seed_v9_workspace_fixture(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(super::SCHEMA_SQL).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V8).unwrap();
        conn.execute_batch(super::SCHEMA_SQL_V9).unwrap();

        conn.execute(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-v9', 'v9@example.com', 'V9 User', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO chart_workspaces (id, owner_id, name, created_at, updated_at) VALUES ('ws-v9', 'user-v9', 'Pre-existing', 0, 0)",
            [],
        ).unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();
    }

    #[test]
    fn a_v9_database_migrates_to_v10_with_the_dashboard_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v9_workspace_fixture(&path);

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        for table in ["dashboard_workspaces", "dashboard_widgets"] {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(
                exists,
                "table `{table}` was not created by the v10 migration"
            );
        }

        // The migration must not touch a pre-existing chart workspace —
        // this is an added table, not a rewrite of anything that already
        // existed.
        let name: String = conn
            .query_row(
                "SELECT name FROM chart_workspaces WHERE id = 'ws-v9'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "Pre-existing");
    }

    #[test]
    fn a_v10_database_can_insert_into_every_new_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        let conn = open(&path).unwrap();

        conn.execute(
            "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-v10', 'v10@example.com', 'V10 User', 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO dashboard_workspaces (id, owner_id, name, columns, revision, created_at, updated_at)
             VALUES ('dash-1', 'user-v10', 'Default', 12, 0, 0, 0)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO dashboard_widgets (
                id, workspace_id, provider_id, widget_type_id, position_x, position_y,
                width, height, visible, config, config_schema_version, created_at, updated_at
             ) VALUES ('widget-1', 'dash-1', 'senken', 'senken/equity', 0, 0, 6, 4, 1, '{}', 1, 0, 0)",
            [],
        ).unwrap();

        // Cascade: deleting the owning workspace must remove its widget
        // row, the same `ON DELETE CASCADE` `chart_workspaces` already
        // relies on for its own child tables.
        conn.execute("DELETE FROM dashboard_workspaces WHERE id = 'dash-1'", [])
            .unwrap();
        let widget_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dashboard_widgets", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            widget_count, 0,
            "a dashboard widget must be cascade-deleted with its workspace"
        );
    }

    #[test]
    fn a_v10_database_is_migrated_to_v11_and_an_existing_user_reads_back_with_no_zone_chosen() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            // A user row written before `display_zone` existed at all.
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V8).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V9).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V10).unwrap();
            conn.execute(
                "INSERT INTO users (id, email, display_name, created_at) VALUES ('user-v10', 'v10-zone@example.com', 'V10 User', 0)",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 10).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        // Migration must not fabricate a zone for an account that never
        // chose one — the column must exist and read back `NULL`, never an
        // empty string or a guessed default.
        let zone: Option<String> = conn
            .query_row(
                "SELECT display_zone FROM users WHERE id = 'user-v10'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            zone, None,
            "a user written before this column existed must read back with no zone chosen, not a guessed default"
        );
    }

    #[test]
    fn a_v11_database_is_migrated_to_v12_and_gains_the_registry_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V8).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V9).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V10).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V11).unwrap();
            conn.pragma_update(None, "user_version", 11).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'indicator_registry_entries'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(exists, "migration must add `indicator_registry_entries`");
    }

    #[test]
    fn a_v12_database_is_migrated_to_v13_and_gains_the_registry_handles_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(super::SCHEMA_SQL).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V2).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V3).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V4).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V5).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V6).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V7).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V8).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V9).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V10).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V11).unwrap();
            conn.execute_batch(super::SCHEMA_SQL_V12).unwrap();
            conn.pragma_update(None, "user_version", 12).unwrap();
        }

        let conn = open(&path).unwrap();
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, super::SCHEMA_VERSION);

        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'registry_handles'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(exists, "migration must add `registry_handles`");
    }

    #[test]
    fn a_v7_accounts_grants_survive_the_resource_token_rewrite() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        seed_v7_layout_fixture(&path);
        let conn = open(&path).unwrap();

        // Authorisation survives: the grants this account already held
        // under the old `workspace`/`layout` tokens are rewritten, not
        // dropped — an upgrade must never silently revoke access.
        let role_grant_resources: Vec<String> = conn
            .prepare("SELECT resource FROM role_grants WHERE role_id = 'role-1' ORDER BY resource")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            role_grant_resources,
            vec!["chart_layout", "chart_workspace"]
        );
        let user_grant_resource: String = conn
            .query_row(
                "SELECT resource FROM user_grants WHERE user_id = 'user-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(user_grant_resource, "chart_workspace");
    }

    #[test]
    fn opening_the_same_path_twice_does_not_fail_or_duplicate_tables() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        drop(open(&path).unwrap());
        drop(open(&path).unwrap());
    }

    #[test]
    fn a_database_with_a_newer_schema_version_is_reported_not_guessed_at() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            let conn = open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }

        let err = open(&path).unwrap_err();
        // Against the constant, not a literal: every schema bump would
        // otherwise break this test for a reason that has nothing to do
        // with the behaviour it is checking.
        assert!(matches!(
            err,
            IdentityError::SchemaVersionMismatch {
                found: 99,
                expected
            } if expected == super::SCHEMA_VERSION
        ));
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let dir = TempDir::new().unwrap();
        let conn = open(&dir.path().join("accounts.db")).unwrap();
        let err = conn
            .execute(
                "INSERT INTO user_roles (user_id, role_id) VALUES ('missing-user', 'missing-role')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("FOREIGN KEY"));
    }
}
