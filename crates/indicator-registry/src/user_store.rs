//! [`UserIndicatorStore`]: the guarded query API for indicators a user
//! wrote and compiled themselves.
//!
//! Follows the same guarded-query shape `senken_notes::NoteStore` and
//! `senken_chart`'s stores use — every read and write goes through
//! [`senken_identity::AuthenticatedUser::authorize`] first, and a scoped
//! listing's `WHERE` clause (and therefore its count) is chosen by the
//! resolved [`Scope`], never applied after the fact.
//!
//! [`get`](UserIndicatorStore::get) and
//! [`update_source`](UserIndicatorStore::update_source) are the two
//! exceptions to "`Scope::All` widens what an actor may reach": they treat
//! an indicator's source the way
//! `senken_trade::TradeAccountStore::settings_for`/`replace_settings` treat
//! broker credentials — as the author's own work, readable and writable
//! only by its owner, whatever a wider role grants. A caller who is not
//! the owner is told the indicator does not exist rather than that they
//! may not see it: a "forbidden" answer would itself confirm that *some*
//! indicator lives at that id, which is exactly the existence leak
//! `senken_trade`'s `UnknownAccount` (not a `Forbidden`) is written to
//! avoid, and is why this store's [`UserIndicatorError::NotFound`] does
//! the same job here.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_core::UnixNanos;
use senken_identity::{AuthenticatedUser, IdentityStore, UserId};

use crate::id::UserIndicatorId;
use crate::user_error::UserIndicatorError;

/// A user indicator row as returned by a listing — source and compiled
/// bytes never leave this shape, only [`UserIndicator::source`]/`wasm` do
/// (via [`UserIndicatorStore::get`], which additionally enforces
/// ownership regardless of scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIndicatorSummary {
    /// The indicator's id.
    pub id: UserIndicatorId,
    /// The account that owns this indicator.
    pub owner_id: UserId,
    /// The catalog slug this indicator compiles to (`my/<slug>`).
    pub slug: String,
    /// The display title, as typed by its author.
    pub title: String,
    /// `true` when the most recent compile attempt succeeded and `wasm`
    /// is loadable.
    pub compiled: bool,
    /// The most recent compile's error message, if its last attempt
    /// failed. `None` while `compiled` is `true`, or before any attempt.
    pub compile_error: Option<String>,
    /// When the row (source, or compile outcome) was last changed.
    pub updated_at: UnixNanos,
}

/// A full user indicator row: everything in [`UserIndicatorSummary`], plus
/// the source and the compiled artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIndicator {
    /// The indicator's id.
    pub id: UserIndicatorId,
    /// The account that owns this indicator.
    pub owner_id: UserId,
    /// The catalog slug this indicator compiles to (`my/<slug>`).
    pub slug: String,
    /// The display title, as typed by its author.
    pub title: String,
    /// `true` when the most recent compile attempt succeeded and `wasm`
    /// is loadable.
    pub compiled: bool,
    /// The most recent compile's error message, if its last attempt
    /// failed.
    pub compile_error: Option<String>,
    /// When the row was last changed.
    pub updated_at: UnixNanos,
    /// The Rust source as last saved.
    pub source: String,
    /// The most recently *successfully* compiled component, if any. Set
    /// once a compile has ever succeeded and left untouched by a later
    /// failed attempt — see [`UserIndicatorStore::record_compile`].
    pub wasm: Option<Vec<u8>>,
    /// The `senken-plugin-api` version the current `wasm` was built
    /// against, if any.
    pub api_version: Option<String>,
}

/// The outcome of one compile attempt, as
/// [`UserIndicatorStore::record_compile`] persists it.
#[derive(Debug, Clone)]
pub enum CompileOutcome {
    /// The source compiled. Replaces the stored artifact and clears any
    /// previous error — `wasm` and `compile_error` are never both
    /// non-`NULL` after this.
    Success {
        /// The compiled WebAssembly component.
        wasm: Vec<u8>,
        /// The SDK version it was built against.
        api_version: String,
    },
    /// The source failed to compile. The previous `wasm` (if any) is left
    /// exactly as it was — a typo must never take a working indicator off
    /// a chart that already uses it.
    Failure {
        /// A product-facing compile error message.
        message: String,
    },
}

/// Guarded queries over user-authored indicators.
///
/// Shares `senken-identity`'s own SQLite connection rather than opening a
/// second one — see `senken_notes::NoteStore`'s module docs for the full
/// reasoning, made identically here.
#[derive(Debug)]
pub struct UserIndicatorStore {
    conn: Arc<Mutex<Connection>>,
}

impl UserIndicatorStore {
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

    /// Lists indicators visible to `auth`: the caller's own at
    /// `Scope::Own`, everyone's at `Scope::All` — the same scoping every
    /// other guarded listing in this workspace applies, before any
    /// content (never applied by filtering afterward).
    ///
    /// # Errors
    /// [`UserIndicatorError::Identity`] if `auth` may not view indicators
    /// at all; otherwise as [`UserIndicatorError::Database`].
    pub fn list(
        &self,
        auth: &AuthenticatedUser,
    ) -> Result<Vec<UserIndicatorSummary>, UserIndicatorError> {
        let scope = auth.authorize(Action::View, Resource::UserIndicator)?;
        let conn = self.lock();
        let sql = "SELECT id, owner_id, slug, title, wasm IS NOT NULL, compile_error, updated_at
                    FROM user_indicators";
        let rows = match scope {
            Scope::Own => {
                let mut stmt = conn.prepare(&format!(
                    "{sql} WHERE owner_id = ?1 ORDER BY updated_at DESC"
                ))?;
                stmt.query_map(params![auth.user_id()], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?
            }
            Scope::All => {
                let mut stmt = conn.prepare(&format!("{sql} ORDER BY updated_at DESC"))?;
                stmt.query_map([], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?
            }
            // `Scope` is `#[non_exhaustive]` — a future variant this crate
            // has not been taught to turn into a `WHERE` clause must fail
            // closed, never fall back to an unfiltered query.
            _ => {
                return Err(UserIndicatorError::Identity(
                    senken_identity::IdentityError::Forbidden,
                ));
            }
        };
        Ok(rows)
    }

    /// Reads one indicator in full, source and compiled bytes included —
    /// for its own owner only, whatever `auth`'s resolved scope says. See
    /// this module's docs for why.
    ///
    /// # Errors
    /// [`UserIndicatorError::NotFound`] if `id` does not exist, or exists
    /// but is not `auth`'s own; [`UserIndicatorError::Identity`] if `auth`
    /// may not view indicators at all.
    pub fn get(
        &self,
        auth: &AuthenticatedUser,
        id: UserIndicatorId,
    ) -> Result<UserIndicator, UserIndicatorError> {
        auth.authorize(Action::View, Resource::UserIndicator)?;
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT id, owner_id, slug, title, wasm, api_version, compile_error, updated_at, source
                 FROM user_indicators WHERE id = ?1",
                params![id],
                row_to_full,
            )
            .optional()?
            .ok_or(UserIndicatorError::NotFound)?;
        if row.owner_id != auth.user_id() {
            return Err(UserIndicatorError::NotFound);
        }
        Ok(row)
    }

    /// Creates a new indicator owned by `auth`, uncompiled (`wasm` and
    /// `compile_error` both `NULL`).
    ///
    /// The catalog slug is derived from `title` (lower-cased,
    /// non-alphanumeric runs collapsed to a single `-`), which is what
    /// `user_indicators`'s `UNIQUE (owner_id, slug)` guards.
    ///
    /// # Errors
    /// [`UserIndicatorError::DuplicateSlug`] if `title` slugifies to one
    /// this account already has; [`UserIndicatorError::Identity`] if
    /// `auth` may not create indicators.
    pub fn create(
        &self,
        auth: &AuthenticatedUser,
        title: &str,
        source: &str,
    ) -> Result<UserIndicatorId, UserIndicatorError> {
        auth.authorize(Action::Create, Resource::UserIndicator)?;
        let conn = self.lock();
        let id = UserIndicatorId::new();
        let slug = slugify(title);
        let now = now().as_nanos();
        conn.execute(
            "INSERT INTO user_indicators (id, owner_id, slug, title, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, auth.user_id(), slug, title, source, now],
        )
        .map_err(|error| duplicate_or_database(error, &slug))?;
        Ok(id)
    }

    /// Replaces an indicator's source (and, if given, its title) — for its
    /// own owner only, the same as [`get`](Self::get) and for the same
    /// reason.
    ///
    /// # Errors
    /// [`UserIndicatorError::NotFound`] if `id` does not exist or is not
    /// `auth`'s own; [`UserIndicatorError::DuplicateSlug`] if a new title
    /// slugifies to one this account already uses on another indicator.
    pub fn update_source(
        &self,
        auth: &AuthenticatedUser,
        id: UserIndicatorId,
        title: Option<&str>,
        source: &str,
    ) -> Result<(), UserIndicatorError> {
        auth.authorize(Action::Edit, Resource::UserIndicator)?;
        let conn = self.lock();
        let owner: UserId = conn
            .query_row(
                "SELECT owner_id FROM user_indicators WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(UserIndicatorError::NotFound)?;
        if owner != auth.user_id() {
            return Err(UserIndicatorError::NotFound);
        }
        let now = now().as_nanos();
        if let Some(title) = title {
            let slug = slugify(title);
            conn.execute(
                "UPDATE user_indicators SET title = ?1, slug = ?2, source = ?3, updated_at = ?4 WHERE id = ?5",
                params![title, slug, source, now, id],
            )
            .map_err(|error| duplicate_or_database(error, &slug))?;
        } else {
            conn.execute(
                "UPDATE user_indicators SET source = ?1, updated_at = ?2 WHERE id = ?3",
                params![source, now, id],
            )?;
        }
        Ok(())
    }

    /// Records one compile attempt's outcome — for its own owner only.
    ///
    /// A [`CompileOutcome::Failure`] leaves the previously-compiled `wasm`
    /// and `api_version` untouched: an indicator already placed on a
    /// chart keeps working after a typo in a later edit.
    ///
    /// # Errors
    /// [`UserIndicatorError::NotFound`] if `id` does not exist or is not
    /// `auth`'s own.
    pub fn record_compile(
        &self,
        auth: &AuthenticatedUser,
        id: UserIndicatorId,
        outcome: CompileOutcome,
    ) -> Result<(), UserIndicatorError> {
        auth.authorize(Action::Edit, Resource::UserIndicator)?;
        let conn = self.lock();
        let owner: UserId = conn
            .query_row(
                "SELECT owner_id FROM user_indicators WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(UserIndicatorError::NotFound)?;
        if owner != auth.user_id() {
            return Err(UserIndicatorError::NotFound);
        }
        let now = now().as_nanos();
        match outcome {
            CompileOutcome::Success { wasm, api_version } => {
                conn.execute(
                    "UPDATE user_indicators
                     SET wasm = ?1, api_version = ?2, compile_error = NULL, compiled_at = ?3, updated_at = ?3
                     WHERE id = ?4",
                    params![wasm, api_version, now, id],
                )?;
            }
            CompileOutcome::Failure { message } => {
                conn.execute(
                    "UPDATE user_indicators SET compile_error = ?1, updated_at = ?2 WHERE id = ?3",
                    params![message, now, id],
                )?;
            }
        }
        Ok(())
    }

    /// Deletes an indicator. Unlike [`get`](Self::get)/
    /// [`update_source`](Self::update_source), this follows the ordinary
    /// scope rule (`Scope::All` may delete any account's) — removing a
    /// row leaks nothing the way reading its source would.
    ///
    /// Deleting a row here has no cascading effect anywhere else: it does
    /// not touch a chart that references this indicator by catalog name,
    /// the same way removing a builtin from a future catalog would not
    /// rewrite a saved layout.
    ///
    /// # Errors
    /// [`UserIndicatorError::NotFound`] if `id` does not exist or is out
    /// of `auth`'s scope.
    pub fn delete(
        &self,
        auth: &AuthenticatedUser,
        id: UserIndicatorId,
    ) -> Result<(), UserIndicatorError> {
        let scope = auth.authorize(Action::Delete, Resource::UserIndicator)?;
        let conn = self.lock();
        let owner: UserId = conn
            .query_row(
                "SELECT owner_id FROM user_indicators WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(UserIndicatorError::NotFound)?;
        match scope {
            Scope::Own if owner == auth.user_id() => {}
            Scope::All => {}
            _ => return Err(UserIndicatorError::NotFound),
        }
        conn.execute("DELETE FROM user_indicators WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Every indicator that has ever compiled successfully, across every
    /// account, for loading back into the runtime's own per-owner
    /// catalogs when the server starts.
    ///
    /// Deliberately takes no [`AuthenticatedUser`] and runs no
    /// [`senken_acl`] check, unlike every other method here: this is the
    /// runtime composing its own startup state, not a request an account
    /// made, and there is no caller to scope the rows against — the same
    /// reasoning `Runtime::build`'s own indicator-plugin-directory scan
    /// already applies to files it finds on disk. Never call this from an
    /// HTTP handler.
    ///
    /// # Errors
    /// As [`UserIndicatorError::Database`].
    pub fn all_compiled_for_startup(
        &self,
    ) -> Result<Vec<CompiledUserIndicator>, UserIndicatorError> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT owner_id, slug, wasm FROM user_indicators WHERE wasm IS NOT NULL")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(CompiledUserIndicator {
                    owner_id: row.get(0)?,
                    slug: row.get(1)?,
                    wasm: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// One row [`UserIndicatorStore::all_compiled_for_startup`] returns:
/// enough to reload a compiled indicator into its owner's runtime catalog
/// without a second query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledUserIndicator {
    /// The account this indicator belongs to.
    pub owner_id: UserId,
    /// The indicator's slug — combined with the `my/` prefix by the
    /// caller to get the catalog name (`senken_runtime::user_indicators`
    /// is the one place that prefix is applied, so it stays defined in
    /// exactly one place).
    pub slug: String,
    /// The most recently successfully compiled component.
    pub wasm: Vec<u8>,
}

/// Lower-cases `title` and collapses every run of characters that are not
/// ASCII letters or digits into a single `-`, trimming any at the ends —
/// the catalog slug `user_indicators.slug` stores and `my/<slug>` is built
/// from.
fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }
    if slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "indicator".to_owned()
    } else {
        slug
    }
}

/// Translates a `UNIQUE (owner_id, slug)` violation into
/// [`UserIndicatorError::DuplicateSlug`], the same way
/// `RegistryStore::set_handle` translates its own constraint violation —
/// never a raw SQLite message reaching a caller.
fn duplicate_or_database(error: rusqlite::Error, slug: &str) -> UserIndicatorError {
    match &error {
        rusqlite::Error::SqliteFailure(sqlite_error, _)
            if sqlite_error.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            UserIndicatorError::DuplicateSlug(slug.to_owned())
        }
        _ => UserIndicatorError::Database(error),
    }
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserIndicatorSummary> {
    Ok(UserIndicatorSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        slug: row.get(2)?,
        title: row.get(3)?,
        compiled: row.get(4)?,
        compile_error: row.get(5)?,
        updated_at: UnixNanos::from_nanos(row.get(6)?),
    })
}

fn row_to_full(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserIndicator> {
    let wasm: Option<Vec<u8>> = row.get(4)?;
    Ok(UserIndicator {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        slug: row.get(2)?,
        title: row.get(3)?,
        compiled: wasm.is_some(),
        wasm,
        api_version: row.get(5)?,
        compile_error: row.get(6)?,
        updated_at: UnixNanos::from_nanos(row.get(7)?),
        source: row.get(8)?,
    })
}

/// The current time, for `created_at`/`updated_at`/`compiled_at`.
fn now() -> UnixNanos {
    let secs = time::OffsetDateTime::now_utc().unix_timestamp();
    UnixNanos::from_secs(secs).unwrap_or(UnixNanos::EPOCH)
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityStore};
    use tempfile::TempDir;

    use super::{CompileOutcome, UserIndicatorError, UserIndicatorStore};

    fn temp_stores() -> (TempDir, IdentityStore, UserIndicatorStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let store = UserIndicatorStore::new(&identity);
        (dir, identity, store)
    }

    const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";

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

    /// Creates an ordinary account with exactly the grants a real "User
    /// Indicators" role would carry — View/Create/Edit/Delete on
    /// `UserIndicator`, at `Scope::Own` — mirroring
    /// `senken_notes::store::tests::notes_user`.
    fn indicator_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Indicator User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::UserIndicator, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn a_user_sees_only_their_own_indicators_including_the_count() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice@example.com");
        let bob = indicator_user(&identity, &admin, "bob@example.com");

        store.create(&alice, "Alice's SMA", "// source").unwrap();
        store.create(&bob, "Bob's EMA", "// source").unwrap();
        store.create(&bob, "Bob's RSI", "// source").unwrap();

        let alice_list = store.list(&alice).unwrap();
        assert_eq!(alice_list.len(), 1);
        assert_eq!(alice_list[0].title, "Alice's SMA");

        let bob_list = store.list(&bob).unwrap();
        assert_eq!(bob_list.len(), 2);
    }

    #[test]
    fn another_users_indicator_is_not_found_not_forbidden() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice2@example.com");
        let bob = indicator_user(&identity, &admin, "bob2@example.com");

        let alice_id = store.create(&alice, "Alice's SMA", "// source").unwrap();

        let error = store.get(&bob, alice_id).unwrap_err();
        // Not `Identity(Forbidden)`: an answer that distinguishes "yours"
        // from "someone else's" would itself confirm an indicator exists
        // at this id, which is exactly what must not leak.
        assert!(matches!(error, UserIndicatorError::NotFound));

        let error = store
            .update_source(&bob, alice_id, None, "// evil")
            .unwrap_err();
        assert!(matches!(error, UserIndicatorError::NotFound));
    }

    #[test]
    fn a_failed_compile_keeps_the_previous_wasm() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice3@example.com");

        let id = store.create(&alice, "My SMA", "// v1").unwrap();
        store
            .record_compile(
                &alice,
                id,
                CompileOutcome::Success {
                    wasm: vec![0, 1, 2, 3],
                    api_version: "0.1.0".to_owned(),
                },
            )
            .unwrap();

        store
            .update_source(&alice, id, None, "// v2, has a typo")
            .unwrap();
        store
            .record_compile(
                &alice,
                id,
                CompileOutcome::Failure {
                    message: "expected `;`".to_owned(),
                },
            )
            .unwrap();

        let indicator = store.get(&alice, id).unwrap();
        // The old, working component is still there and still loadable —
        // a typo in a later edit must not take a chart's plot away.
        assert_eq!(indicator.wasm, Some(vec![0, 1, 2, 3]));
        assert!(indicator.compiled);
        // The failure is still reported, alongside the kept `wasm`: the
        // user sees the error in the panel without losing the plot.
        assert_eq!(indicator.compile_error.as_deref(), Some("expected `;`"));
        assert_eq!(indicator.source, "// v2, has a typo");
    }

    #[test]
    fn a_compile_failure_before_any_success_records_the_error_with_no_wasm() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice4@example.com");

        let id = store.create(&alice, "My SMA", "// broken").unwrap();
        store
            .record_compile(
                &alice,
                id,
                CompileOutcome::Failure {
                    message: "expected `;`".to_owned(),
                },
            )
            .unwrap();

        let indicator = store.get(&alice, id).unwrap();
        assert_eq!(indicator.wasm, None);
        assert!(!indicator.compiled);
        assert_eq!(indicator.compile_error.as_deref(), Some("expected `;`"));
    }

    #[test]
    fn deleting_cascades_nothing_else() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice5@example.com");

        let deleted_id = store.create(&alice, "Gone", "// source").unwrap();
        let kept_id = store.create(&alice, "Kept", "// source").unwrap();
        store.delete(&alice, deleted_id).unwrap();

        let error = store.get(&alice, deleted_id).unwrap_err();
        assert!(matches!(error, UserIndicatorError::NotFound));

        // Deleting one row touches only that row: a sibling indicator
        // owned by the same account, and the account itself, are
        // untouched.
        let kept = store.get(&alice, kept_id).unwrap();
        assert_eq!(kept.title, "Kept");
        assert_eq!(store.list(&alice).unwrap().len(), 1);
    }

    #[test]
    fn creating_a_second_indicator_with_the_same_slug_is_rejected() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice6@example.com");

        store.create(&alice, "My SMA", "// v1").unwrap();
        let error = store.create(&alice, "My SMA", "// v2").unwrap_err();
        assert!(matches!(error, UserIndicatorError::DuplicateSlug(_)));
    }

    #[test]
    fn all_compiled_for_startup_lists_only_successfully_compiled_rows_across_every_owner() {
        let (_dir, identity, store) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = indicator_user(&identity, &admin, "alice7@example.com");
        let bob = indicator_user(&identity, &admin, "bob7@example.com");

        let compiled_id = store.create(&alice, "Compiled", "// ok").unwrap();
        store
            .record_compile(
                &alice,
                compiled_id,
                CompileOutcome::Success {
                    wasm: vec![9, 9, 9],
                    api_version: "0.1.0".to_owned(),
                },
            )
            .unwrap();
        // Never compiled at all: must not appear.
        store.create(&bob, "Never compiled", "// broken").unwrap();

        let rows = store.all_compiled_for_startup().unwrap();
        assert_eq!(rows.len(), 1, "only the one row with a compiled wasm");
        assert_eq!(rows[0].owner_id, alice.user_id());
        assert_eq!(rows[0].slug, "compiled");
        assert_eq!(rows[0].wasm, vec![9, 9, 9]);
    }
}
