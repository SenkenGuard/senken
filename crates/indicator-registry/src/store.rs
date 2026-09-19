//! [`RegistryStore`]: publish, search, install, and revoke a published
//! indicator's source.
//!
//! Follows the same guarded-query shape `senken_notes`/`senken_dashboard`
//! establish — an [`AuthenticatedUser`] and [`AuthenticatedUser::authorize`]
//! in front of every write — with one deliberate difference: **a published
//! indicator is public by design.** The whole point of a registry entry is
//! that other users search and install it, so [`RegistryStore::search`],
//! [`RegistryStore::get`] and [`RegistryStore::install`] take no
//! [`AuthenticatedUser`] at all and apply no [`Scope`] — matching how this
//! workspace already treats market data as global with no owner to check a
//! grant against. [`RegistryStore::publish`], [`RegistryStore::delete`] (an
//! account must exist to author or revoke something) and
//! [`RegistryStore::list_mine`] (an author's own view of what they have
//! published) go through a permission check, and `list_mine`'s total obeys
//! [`Scope`] the same way every other listing in this workspace does.
//!
//! # This module is not wired to anything right now
//!
//! Publishing to a public registry, and the handle a human would address
//! one by, are both out of scope for the MVP a trader actually asked for
//! (write Rust, compile it, use it on your own charts). Nothing in
//! `senken-api` mounts [`RegistryStore::publish`]/[`Self::install`]
//! any more, and no source published here is validated by a compiler —
//! `publish` and `install` stay compiled and tested for whenever a
//! registry is designed again, but neither runs as a request from a real
//! user today. `senken-identity`'s own `registry_handles` table is
//! untouched by that change: it still exists, still migrates forward, and
//! `publish`'s own handle gate still checks it — there is simply no
//! surface left that can ever populate it.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};

use crate::error::RegistryError;
use crate::id::IndicatorEntryId;
use crate::version;

/// A published indicator, without its source — what [`RegistryStore::search`]
/// and [`RegistryStore::list_mine`] return. See [`IndicatorEntry`] for the
/// full row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndicatorSummary {
    /// This entry's id.
    pub id: IndicatorEntryId,
    /// The publishing account's id, which **is** this indicator's
    /// namespace — see this module's docs for why an account id, rather
    /// than a self-chosen display handle, is what closes impersonation.
    /// The qualified name a user searches for is `{namespace}/{name}`.
    pub namespace: UserId,
    /// The indicator's name within its namespace. Two authors may use the
    /// same name in their own namespaces without colliding, since the
    /// stored uniqueness is `(namespace, name)`, never `name` alone.
    pub name: String,
    /// The indicator language version this entry was last published
    /// against.
    pub language_version: String,
    /// Unix timestamp this entry was first published.
    pub created_at: i64,
    /// Unix timestamp of the last successful publish to this entry.
    pub updated_at: i64,
}

/// A published indicator, source included — returned by
/// [`RegistryStore::get`], never by a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndicatorEntry {
    /// This entry's id.
    pub id: IndicatorEntryId,
    /// See [`IndicatorSummary::namespace`].
    pub namespace: UserId,
    /// See [`IndicatorSummary::name`].
    pub name: String,
    /// The source exactly as published — never a compiled artifact. See
    /// this crate's README for why source, not a binary, is what this
    /// registry ever stores or serves.
    pub source: String,
    /// See [`IndicatorSummary::language_version`].
    pub language_version: String,
    /// Unix timestamp this entry was first published.
    pub created_at: i64,
    /// Unix timestamp of the last successful publish to this entry.
    pub updated_at: i64,
}

/// The result of a successful install: the source that was fetched, and
/// the metadata that came with it.
///
/// Carries no compiled artifact: this crate has no compiler of its own to
/// produce one (see this module's own docs for why installing does not run
/// one today), so there is nothing honest to put in that field until
/// installing is redesigned around whatever actually turns this source
/// into something runnable next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledIndicator {
    /// See [`IndicatorSummary::namespace`].
    pub namespace: UserId,
    /// See [`IndicatorSummary::name`].
    pub name: String,
    /// The source exactly as published.
    pub source: String,
    /// The language version this component was published against, already
    /// checked against [`version::HOST_LANGUAGE_VERSION`] by the time this
    /// value exists.
    pub language_version: String,
}

/// Guarded and public queries over the indicator registry.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second
/// one — the same reasoning `senken-notes`/`senken-dashboard` document for
/// themselves.
#[derive(Debug)]
pub struct RegistryStore {
    conn: Arc<Mutex<Connection>>,
}

impl RegistryStore {
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

    /// Publishes `source` under `namespace/name`, creating a new entry or
    /// replacing an existing one's source in place — a registry entry has
    /// exactly one, current, source, not a version history (out of this
    /// registry's stated scope; see this crate's README).
    ///
    /// `namespace` must equal `auth.user_id()` — `ensure_owns_namespace`
    /// is the check that closes author impersonation, the whole reason a
    /// registry entry is namespaced by account rather than by a free-text
    /// name a publisher chooses. `namespace` must also already hold a
    /// handle in `registry_handles` — a registry entry addressable only by
    /// raw account id is not meaningfully published — but nothing in this
    /// build can ever claim one (see this module's own docs), so this
    /// check can no longer pass for anyone. `source` itself is stored
    /// as-is: this crate has no compiler to validate it against any more.
    ///
    /// # Errors
    /// [`RegistryError::Identity`] if `auth` may not publish at all;
    /// [`RegistryError::ForeignNamespace`] if `namespace` is not `auth`'s
    /// own; [`RegistryError::HandleNotSet`] always, for the reason above;
    /// [`RegistryError::InvalidName`] for an empty name or one containing
    /// `/` (which would make `{namespace}/{name}` ambiguous to parse back
    /// apart); otherwise as [`RegistryError::Database`].
    pub fn publish(
        &self,
        auth: &AuthenticatedUser,
        namespace: UserId,
        name: &str,
        source: &str,
    ) -> Result<IndicatorEntryId, RegistryError> {
        ensure_owns_namespace(auth, namespace)?;
        validate_name(name)?;

        let conn = self.lock();
        let existing_id: Option<IndicatorEntryId> = conn
            .query_row(
                "SELECT id FROM indicator_registry_entries WHERE owner_id = ?1 AND name = ?2",
                params![namespace, name],
                |row| row.get(0),
            )
            .optional()?;

        // Checked before the handle gate below, not after: an actor with
        // no grant at all must see `Forbidden`, never a hint about what
        // else they would need to fix if they *could* publish.
        if existing_id.is_some() {
            auth.authorize(Action::Edit, Resource::IndicatorRegistry)?;
        } else {
            auth.authorize(Action::Create, Resource::IndicatorRegistry)?;
        }

        // Read through the same held connection as the existing-row check
        // above, not via `ensure_handle_chosen`/`self.lock()` -- this
        // crate's `Mutex` is not reentrant, and a second `self.lock()`
        // call while `conn` is still held would deadlock.
        let handle_chosen: bool = conn
            .query_row(
                "SELECT 1 FROM registry_handles WHERE owner_id = ?1",
                params![namespace],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !handle_chosen {
            return Err(RegistryError::HandleNotSet);
        }

        let now = now_unix();
        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE indicator_registry_entries
                 SET source = ?1, language_version = ?2, updated_at = ?3
                 WHERE id = ?4",
                params![source, version::HOST_LANGUAGE_VERSION, now, id],
            )?;
            Ok(id)
        } else {
            let id = IndicatorEntryId::new();
            conn.execute(
                "INSERT INTO indicator_registry_entries
                 (id, owner_id, name, source, language_version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    id,
                    namespace,
                    name,
                    source,
                    version::HOST_LANGUAGE_VERSION,
                    now
                ],
            )?;
            Ok(id)
        }
    }

    /// Searches every published indicator across every namespace — the
    /// public catalog, open to an anonymous caller the same way market
    /// data is: there is no owner-scoped hiding to apply to a directory of
    /// things every account is meant to be able to find. `query`, when
    /// given, matches indicators whose name contains it (case-insensitive).
    ///
    /// # Errors
    /// [`RegistryError::Database`] on a storage failure.
    pub fn search(
        &self,
        query: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Page<IndicatorSummary>, RegistryError> {
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = if let Some(query) = query.filter(|q| !q.is_empty()) {
            let pattern = format!("%{}%", escape_like(query));
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM indicator_registry_entries
                 WHERE name LIKE ?1 ESCAPE '\\'",
                [&pattern],
                |row| row.get(0),
            )?;
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, name, language_version, created_at, updated_at
                 FROM indicator_registry_entries
                 WHERE name LIKE ?1 ESCAPE '\\'
                 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3",
            )?;
            let rows = stmt
                .query_map(params![pattern, limit, offset], row_to_summary)?
                .collect::<Result<Vec<_>, _>>()?;
            (total, rows)
        } else {
            let total: i64 = conn.query_row(
                "SELECT COUNT(*) FROM indicator_registry_entries",
                [],
                |row| row.get(0),
            )?;
            let mut stmt = conn.prepare(
                "SELECT id, owner_id, name, language_version, created_at, updated_at
                 FROM indicator_registry_entries
                 ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = stmt
                .query_map(params![limit, offset], row_to_summary)?
                .collect::<Result<Vec<_>, _>>()?;
            (total, rows)
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Lists the calling account's own view of the registry: every
    /// indicator they have published (`Scope::Own`), or, for an actor
    /// granted `Scope::All`, every published indicator regardless of
    /// author — the same scoped-listing shape `senken_notes::list_notes`
    /// uses, including `total` respecting the same scope as `rows`.
    ///
    /// # Errors
    /// [`RegistryError::Identity`] if `auth` may not view registry entries
    /// at all, or if the resolved [`Scope`] is one this crate does not
    /// translate to SQL; otherwise as [`RegistryError::Database`].
    pub fn list_mine(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<IndicatorSummary>, RegistryError> {
        let scope = auth.authorize(Action::View, Resource::IndicatorRegistry)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM indicator_registry_entries WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, language_version, created_at, updated_at
                     FROM indicator_registry_entries
                     WHERE owner_id = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM indicator_registry_entries",
                    [],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, language_version, created_at, updated_at
                     FROM indicator_registry_entries
                     ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — a future variant this crate
            // has not been taught to turn into a `WHERE` clause must fail
            // closed, never fall back to an unfiltered query.
            _ => return Err(RegistryError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Reads one published indicator in full, source included — public,
    /// like [`search`](Self::search).
    ///
    /// # Errors
    /// [`RegistryError::NotFound`] if no entry exists at `namespace/name`;
    /// otherwise as [`RegistryError::Database`].
    pub fn get(&self, namespace: UserId, name: &str) -> Result<IndicatorEntry, RegistryError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, owner_id, name, source, language_version, created_at, updated_at
             FROM indicator_registry_entries WHERE owner_id = ?1 AND name = ?2",
            params![namespace, name],
            row_to_entry,
        )
        .optional()?
        .ok_or(RegistryError::NotFound)
    }

    /// Installs one published indicator: fetches its current source and
    /// checks its recorded language version against
    /// [`version::HOST_LANGUAGE_VERSION`]. Requires no account: see this
    /// crate's README for why publishing and installing sit on opposite
    /// sides of that line.
    ///
    /// Does not compile the source — see this module's own docs for why
    /// this crate has nothing to compile it with any more.
    ///
    /// # Errors
    /// [`RegistryError::NotFound`] if no entry exists at `namespace/name`;
    /// [`RegistryError::LanguageVersionTooNew`] naming both versions if
    /// this host is too old for it; otherwise as [`RegistryError::Database`].
    pub fn install(
        &self,
        namespace: UserId,
        name: &str,
    ) -> Result<InstalledIndicator, RegistryError> {
        let entry = self.get(namespace, name)?;
        version::ensure_host_supports(&entry.language_version)?;
        Ok(InstalledIndicator {
            namespace: entry.namespace,
            name: entry.name,
            source: entry.source,
            language_version: entry.language_version,
        })
    }

    /// Permanently removes `namespace`'s own `name` entry from the
    /// registry.
    ///
    /// Reuses this module's own `ensure_owns_namespace` check for exactly
    /// the reason `publish` does: revoking is an identity fact, never a
    /// permission level, so an
    /// actor granted `Scope::All` over `IndicatorRegistry` still may not
    /// delete another author's entry through this call — only their own
    /// namespace is ever accepted. This does not reach anyone who already
    /// installed the entry: [`install`](Self::install) copies the
    /// compiled bytes to the installing machine, so there is no live
    /// reference left here for a delete to invalidate.
    ///
    /// # Errors
    /// [`RegistryError::Identity`] if `auth` may not delete registry
    /// entries at all; [`RegistryError::ForeignNamespace`] if `namespace`
    /// is not `auth`'s own; [`RegistryError::NotFound`] if no entry exists
    /// at `namespace/name`; otherwise as [`RegistryError::Database`].
    pub fn delete(
        &self,
        auth: &AuthenticatedUser,
        namespace: UserId,
        name: &str,
    ) -> Result<(), RegistryError> {
        ensure_owns_namespace(auth, namespace)?;
        auth.authorize(Action::Delete, Resource::IndicatorRegistry)?;

        let conn = self.lock();
        let deleted = conn.execute(
            "DELETE FROM indicator_registry_entries WHERE owner_id = ?1 AND name = ?2",
            params![namespace, name],
        )?;
        if deleted == 0 {
            return Err(RegistryError::NotFound);
        }
        Ok(())
    }
}

/// Closes author impersonation: a publish request must name the caller's
/// own account as its namespace, never anyone else's — see this module's
/// doc comment and [`RegistryStore::publish`].
fn ensure_owns_namespace(auth: &AuthenticatedUser, namespace: UserId) -> Result<(), RegistryError> {
    if namespace == auth.user_id() {
        Ok(())
    } else {
        Err(RegistryError::ForeignNamespace)
    }
}

/// Rejects an empty name, or one containing `/` — the qualified-name
/// separator between namespace and name, which a name containing one would
/// make ambiguous to parse back apart.
fn validate_name(name: &str) -> Result<(), RegistryError> {
    if name.is_empty() || name.contains('/') {
        Err(RegistryError::InvalidName(name.to_owned()))
    } else {
        Ok(())
    }
}

/// Escapes `%`, `_` and the escape character itself for a `LIKE ... ESCAPE
/// '\\'` pattern, so a search query containing any of them is matched
/// literally rather than as a SQL wildcard.
fn escape_like(raw: &str) -> String {
    raw.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndicatorSummary> {
    Ok(IndicatorSummary {
        id: row.get(0)?,
        namespace: row.get(1)?,
        name: row.get(2)?,
        language_version: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndicatorEntry> {
    Ok(IndicatorEntry {
        id: row.get(0)?,
        namespace: row.get(1)?,
        name: row.get(2)?,
        source: row.get(3)?,
        language_version: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// The current time as a Unix timestamp, for `created_at`/`updated_at`.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, UserId};
    use tempfile::TempDir;

    use super::{RegistryError, RegistryStore};

    /// A minimal source string every publish test uses — this crate never
    /// validates it (see this module's own docs for why), so any non-empty
    /// text proves the same thing a real program would.
    const VALID_SOURCE: &str = "let fast = ema(close, 5)\nplot fast\n";

    fn temp_stores() -> (TempDir, IdentityStore, RegistryStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let registry = RegistryStore::new(&identity);
        (dir, identity, registry)
    }

    /// Inserts a `registry_handles` row directly through SQL, standing in
    /// for the removed `RegistryStore::set_handle` — this crate's tests
    /// still need a way to satisfy `publish`'s handle gate to exercise
    /// everything downstream of it, even though nothing in production
    /// code can populate this table any more (see this module's own
    /// docs).
    fn claim_handle_for_test(identity: &IdentityStore, user_id: UserId, handle: &str) {
        identity
            .shared_connection()
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO registry_handles (owner_id, handle, created_at) VALUES (?1, ?2, 0)",
                rusqlite::params![user_id, handle],
            )
            .unwrap();
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

    /// Creates an ordinary account with exactly the grants a real
    /// "Indicator Author" role would carry — View/Create/Edit/Delete on
    /// `IndicatorRegistry`, at `Scope::Own` — and claims a registry handle
    /// derived from `email`'s local part via [`claim_handle_for_test`], so
    /// this fixture already satisfies [`RegistryStore::publish`]'s handle
    /// gate, which nothing in production code can satisfy any more.
    fn author(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(
                admin,
                email,
                "Indicator Author",
                Some("a very long password"),
            )
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::IndicatorRegistry, Scope::Own),
                )
                .unwrap();
        }
        let local_part = email.split('@').next().unwrap();
        claim_handle_for_test(identity, user_id, local_part);
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn publishing_then_installing_from_a_different_account_round_trips_the_source() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice@example.com");

        registry
            .publish(&alice, alice.user_id(), "rsi-cross", VALID_SOURCE)
            .unwrap();

        // "a different account": bob never published this, and installing
        // takes no account at all.
        let installed = registry.install(alice.user_id(), "rsi-cross").unwrap();
        assert_eq!(installed.name, "rsi-cross");
        assert_eq!(installed.namespace, alice.user_id());
        assert_eq!(
            installed.source, VALID_SOURCE,
            "install must fetch exactly the source that was published"
        );
    }

    #[test]
    fn two_authors_may_publish_the_same_name_in_their_own_namespaces() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice2@example.com");
        let bob = author(&identity, &admin, "bob2@example.com");

        registry
            .publish(&alice, alice.user_id(), "macd-plus", VALID_SOURCE)
            .unwrap();
        registry
            .publish(&bob, bob.user_id(), "macd-plus", VALID_SOURCE)
            .unwrap();

        let alices = registry.get(alice.user_id(), "macd-plus").unwrap();
        let bobs = registry.get(bob.user_id(), "macd-plus").unwrap();
        assert_ne!(alices.id, bobs.id, "each namespace's entry is its own row");

        let page = registry.search(Some("macd-plus"), 50, 0).unwrap();
        assert_eq!(
            page.total, 2,
            "both authors' entries must be visible in the public catalog"
        );
    }

    #[test]
    fn publishing_into_another_authors_namespace_is_refused() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice3@example.com");
        let bob = author(&identity, &admin, "bob3@example.com");

        let error = registry
            .publish(&alice, bob.user_id(), "hijack", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(error, RegistryError::ForeignNamespace));

        // Nothing was written under bob's namespace as a side effect of
        // the refused attempt.
        let error = registry.get(bob.user_id(), "hijack").unwrap_err();
        assert!(matches!(error, RegistryError::NotFound));
    }

    #[test]
    fn an_indicator_that_needs_a_newer_language_version_is_refused_with_a_named_message() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice5@example.com");
        registry
            .publish(&alice, alice.user_id(), "future", VALID_SOURCE)
            .unwrap();

        // Simulate a row published by a hypothetical future host build —
        // this crate is the table's only writer, so the only way to get a
        // too-new version into it for this test is to reach past the
        // guarded API, exactly the way `senken-notes`' own tests seed
        // pre-migration rows directly through SQL.
        identity
            .shared_connection()
            .lock()
            .unwrap()
            .execute(
                "UPDATE indicator_registry_entries SET language_version = '999.0.0' WHERE name = 'future'",
                [],
            )
            .unwrap();

        let error = registry.install(alice.user_id(), "future").unwrap_err();
        match error {
            RegistryError::LanguageVersionTooNew { required, host } => {
                assert_eq!(required, "999.0.0");
                assert_eq!(host, super::version::HOST_LANGUAGE_VERSION);
            }
            other => panic!("expected LanguageVersionTooNew, got {other:?}"),
        }
    }

    #[test]
    fn republishing_the_same_name_updates_the_source_in_place_not_a_second_row() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice6@example.com");

        let first_id = registry
            .publish(&alice, alice.user_id(), "iterating", VALID_SOURCE)
            .unwrap();
        let second_source = "let slow = ema(close, 50)\nplot slow\n";
        let second_id = registry
            .publish(&alice, alice.user_id(), "iterating", second_source)
            .unwrap();

        assert_eq!(first_id, second_id);
        let entry = registry.get(alice.user_id(), "iterating").unwrap();
        assert_eq!(entry.source, second_source);

        let page = registry.search(Some("iterating"), 50, 0).unwrap();
        assert_eq!(page.total, 1);
    }

    #[test]
    fn a_second_authors_entries_are_invisible_and_not_counted_in_list_mines_total() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice7@example.com");
        let bob = author(&identity, &admin, "bob7@example.com");

        registry
            .publish(&alice, alice.user_id(), "alices-own", VALID_SOURCE)
            .unwrap();
        registry
            .publish(&bob, bob.user_id(), "bobs-one", VALID_SOURCE)
            .unwrap();
        registry
            .publish(&bob, bob.user_id(), "bobs-two", VALID_SOURCE)
            .unwrap();

        let alice_page = registry.list_mine(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too -- otherwise pagination leaks how many entries exist"
        );

        let bob_page = registry.list_mine(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
    }

    #[test]
    fn a_superadmin_sees_every_authors_entries_in_list_mine() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice8@example.com");
        let bob = author(&identity, &admin, "bob8@example.com");
        registry
            .publish(&alice, alice.user_id(), "a", VALID_SOURCE)
            .unwrap();
        registry
            .publish(&bob, bob.user_id(), "b", VALID_SOURCE)
            .unwrap();

        let page = registry.list_mine(&admin, 50, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn an_actor_with_no_grant_cannot_publish() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let user_id = identity
            .create_user(
                &admin,
                "ungranted@example.com",
                "No Grants",
                Some("a very long password"),
            )
            .unwrap();
        let (_uid, token) = identity
            .login("ungranted@example.com", "a very long password")
            .unwrap();
        let ungranted = identity.resolve_session(token.reveal()).unwrap().unwrap();
        let _ = user_id;

        let error = registry
            .publish(&ungranted, ungranted.user_id(), "nope", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(
            error,
            RegistryError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn searching_and_installing_need_no_account_at_all() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice9@example.com");
        registry
            .publish(&alice, alice.user_id(), "public-one", VALID_SOURCE)
            .unwrap();

        // Neither call below takes an `AuthenticatedUser` at all -- the
        // type signature itself is the proof that no account is required.
        let page = registry.search(None, 50, 0).unwrap();
        assert_eq!(page.total, 1);
        registry.install(alice.user_id(), "public-one").unwrap();
    }

    #[test]
    fn an_empty_or_slash_containing_name_is_rejected() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice10@example.com");

        let error = registry
            .publish(&alice, alice.user_id(), "", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(error, RegistryError::InvalidName(_)));

        let error = registry
            .publish(&alice, alice.user_id(), "has/slash", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(error, RegistryError::InvalidName(_)));
    }

    /// Same grants `author` gives, but deliberately claims no handle — for
    /// tests specifically about the handle gate.
    fn author_with_no_handle(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(
                admin,
                email,
                "Indicator Author",
                Some("a very long password"),
            )
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::IndicatorRegistry, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn publishing_with_full_grants_but_no_handle_chosen_is_refused_and_told_to_choose_one() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author_with_no_handle(&identity, &admin, "alice11@example.com");

        let error = registry
            .publish(&alice, alice.user_id(), "no-handle-yet", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(error, RegistryError::HandleNotSet));

        let error = registry.get(alice.user_id(), "no-handle-yet").unwrap_err();
        assert!(
            matches!(error, RegistryError::NotFound),
            "a refused publish must not leave a partial row behind"
        );
    }

    #[test]
    fn an_actor_with_no_grant_and_no_handle_is_told_forbidden_not_handle_not_set() {
        // Authorisation is checked before the handle gate: an actor who
        // could never publish at all must not learn anything about what
        // else stands between them and publishing.
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let user_id = identity
            .create_user(
                &admin,
                "nogrant-nohandle@example.com",
                "No Grants",
                Some("a very long password"),
            )
            .unwrap();
        let (_uid, token) = identity
            .login("nogrant-nohandle@example.com", "a very long password")
            .unwrap();
        let ungranted = identity.resolve_session(token.reveal()).unwrap().unwrap();
        let _ = user_id;

        let error = registry
            .publish(&ungranted, ungranted.user_id(), "nope", VALID_SOURCE)
            .unwrap_err();
        assert!(matches!(
            error,
            RegistryError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn a_handle_row_claimed_directly_through_sql_satisfies_the_publish_gate() {
        // `RegistryStore` has no method left that can ever write a
        // `registry_handles` row (`set_handle` was removed along with the
        // rest of the account-handle feature) — this proves `publish`'s
        // own gate still reads that table correctly, via the one way
        // left to populate it.
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author_with_no_handle(&identity, &admin, "alice12@example.com");

        claim_handle_for_test(&identity, alice.user_id(), "alice12");

        registry
            .publish(&alice, alice.user_id(), "now-addressable", VALID_SOURCE)
            .unwrap();
    }

    #[test]
    fn an_author_can_delete_their_own_published_indicator() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice16@example.com");
        registry
            .publish(&alice, alice.user_id(), "revocable", VALID_SOURCE)
            .unwrap();

        registry
            .delete(&alice, alice.user_id(), "revocable")
            .unwrap();

        let error = registry.get(alice.user_id(), "revocable").unwrap_err();
        assert!(matches!(error, RegistryError::NotFound));
    }

    #[test]
    fn deleting_a_nonexistent_entry_reports_not_found() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice17@example.com");

        let error = registry
            .delete(&alice, alice.user_id(), "never-published")
            .unwrap_err();
        assert!(matches!(error, RegistryError::NotFound));
    }

    #[test]
    fn deleting_another_authors_indicator_is_refused_and_leaves_it_installable() {
        let (_dir, identity, registry) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = author(&identity, &admin, "alice18@example.com");
        let bob = author(&identity, &admin, "bob18@example.com");
        registry
            .publish(&alice, alice.user_id(), "alices-only", VALID_SOURCE)
            .unwrap();

        // Bob holds a full `Delete` grant on `IndicatorRegistry` at
        // `Scope::Own` -- exactly what `ensure_owns_namespace` must catch
        // regardless of, since a grant is not what decides whose
        // namespace this is.
        let error = registry
            .delete(&bob, alice.user_id(), "alices-only")
            .unwrap_err();
        assert!(matches!(error, RegistryError::ForeignNamespace));

        // Untouched by the refused attempt: still there, still
        // installable.
        registry
            .install(alice.user_id(), "alices-only")
            .expect("a refused delete by a different account must not remove the entry");
    }
}
