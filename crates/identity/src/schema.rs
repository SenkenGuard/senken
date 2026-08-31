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
//!   queries these four tables itself — `senken-workspace` does, sharing
//!   this connection via [`crate::IdentityStore::shared_connection`] rather
//!   than opening a second one to the same file. See that crate's module
//!   docs for the full reasoning behind putting the tables here instead of
//!   giving `senken-workspace` its own database.
//! - **v4**: `alerts` — one row per standalone alert. Alerts reference
//!   `users(id)` too, so the same
//!   single-schema-owner reasoning v3 already established for
//!   `senken-workspace` applies verbatim here: this crate creates the
//!   table and owns `user_version`, but `senken-alerts` is the only crate
//!   that ever queries it, sharing this connection via
//!   [`crate::IdentityStore::shared_connection`] rather than opening a
//!   second one. See that crate's module docs for the full reasoning.
//! - **v5**: `drawings` — one row per chart drawing object (horizontal
//!   line, trend line, rectangle), owned by a pane the same way `layers`
//!   already is. Same reasoning as v3/v4: created here, queried only by
//!   `senken-workspace`.
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

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::error::IdentityError;

/// The `user_version` this build of the crate creates and expects. Bump
/// this and extend the schema (or add a migration step) when the shape
/// changes — there is deliberately no migration crate, not schema
/// evolution itself.
const SCHEMA_VERSION: i32 = 7;

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
/// `senken-workspace`, not this crate — see this module's doc comment.
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
-- `senken-workspace`, which enforces it in Rust before this table is ever
-- touched (`senken_workspace::LayerKind`'s three variants each carry
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
/// `senken-workspace`, not this crate — see this module's doc comment.
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
/// object rather than `NULL`, so every reader (`senken-workspace`'s own
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
/// v5's drawings table, v5 adds v6's `panes.settings` column), since there
/// is no migration crate but not migrating by hand. A database newer than
/// this crate knows about is reported, never guessed at.
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
            "workspaces",
            "layouts",
            "panes",
            "layers",
            "alerts",
            "drawings",
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
            "workspaces",
            "layouts",
            "panes",
            "layers",
            "alerts",
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

        for table in ["workspaces", "layouts", "panes", "layers", "alerts"] {
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
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'workspaces'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            workspaces_exists,
            "the v3 tables must still be there — this is a migration, not a rebuild"
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

        let drawings_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'drawings'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(drawings_exists, "migration must add `drawings`");

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
                "SELECT settings FROM panes WHERE id = 'pane-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            settings, "{}",
            "a pane written before this column existed must default to an empty settings object, not NULL"
        );

        let drawings_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'drawings'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(
            drawings_exists,
            "the v5 tables must still be there — this is a migration, not a rebuild"
        );
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
