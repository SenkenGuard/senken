//! [`NoteStore`]: the guarded query API for notes.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};

use crate::error::NoteError;
use crate::id::NoteId;

/// A note row as returned by a guarded listing — never the body, so a
/// listing's payload does not grow with how much a user has written. See
/// [`Note`] for the full row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteSummary {
    /// The note's id.
    pub id: NoteId,
    /// The account that owns this note.
    pub owner_id: UserId,
    /// The note's title.
    pub title: String,
    /// Unix timestamp of the last change to the note's title or body.
    pub updated_at: i64,
}

/// A full note row, body included — returned only by
/// [`NoteStore::get_note`], never by a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The note's id.
    pub id: NoteId,
    /// The account that owns this note.
    pub owner_id: UserId,
    /// The note's title.
    pub title: String,
    /// The note's body.
    pub body: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the note's title or body.
    pub updated_at: i64,
}

/// Guarded queries over notes.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second one —
/// see this crate's module docs, which follow `senken-chart`'s reasoning
/// verbatim. Every mutation and every listing goes through an
/// [`AuthenticatedUser`] and calls [`AuthenticatedUser::authorize`] exactly
/// the way `senken-identity`'s and `senken-chart`'s own guarded queries do:
/// there is no method here that reads or writes a row without that check
/// running first.
#[derive(Debug)]
pub struct NoteStore {
    conn: Arc<Mutex<Connection>>,
}

impl NoteStore {
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

    /// Lists notes visible to `auth`, scoped: the `WHERE` clause is chosen
    /// by `auth`'s [`senken_acl::decide`]d [`Scope`] for `(Action::View,
    /// Resource::Note)`, and [`Page::total`] is counted under that same
    /// clause. Ordered by most-recently-updated first.
    ///
    /// # Errors
    /// [`NoteError::Identity`] if `auth` may not view notes at all, or if
    /// `decide` returns a [`Scope`] variant this crate does not yet
    /// translate to SQL; otherwise as [`NoteError::Database`].
    pub fn list_notes(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<NoteSummary>, NoteError> {
        let scope = auth.authorize(Action::View, Resource::Note)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM notes WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, title, updated_at FROM notes
                     WHERE owner_id = ?1 ORDER BY updated_at DESC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, title, updated_at FROM notes
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
            _ => return Err(NoteError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Reads one note in full, body included.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Note`, with the
    /// returned [`Scope`] applied against this note's owner.
    ///
    /// # Errors
    /// [`NoteError::NoteNotFound`] if `id` does not exist;
    /// [`NoteError::Identity`] if `auth` may not view notes at all, or may
    /// not reach this particular one; otherwise as [`NoteError::Database`].
    pub fn get_note(&self, auth: &AuthenticatedUser, id: NoteId) -> Result<Note, NoteError> {
        let scope = auth.authorize(Action::View, Resource::Note)?;
        let conn = self.lock();
        let note = conn
            .query_row(
                "SELECT id, owner_id, title, body, created_at, updated_at FROM notes WHERE id = ?1",
                [id],
                row_to_note,
            )
            .optional()?
            .ok_or(NoteError::NoteNotFound)?;
        ensure_scope_allows(scope, note.owner_id, auth.user_id())?;
        Ok(note)
    }

    /// Creates a new note owned by `auth`.
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::Note`.
    ///
    /// # Errors
    /// [`NoteError::Identity`] if `auth` may not create a note; otherwise
    /// as [`NoteError::Database`].
    pub fn create_note(
        &self,
        auth: &AuthenticatedUser,
        title: &str,
        body: &str,
    ) -> Result<NoteId, NoteError> {
        auth.authorize(Action::Create, Resource::Note)?;
        let conn = self.lock();
        let id = NoteId::new();
        let now = now_unix();
        conn.execute(
            "INSERT INTO notes (id, owner_id, title, body, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, auth.user_id(), title, body, now],
        )?;
        Ok(id)
    }

    /// Replaces a note's title and body.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Note`, with the
    /// returned [`Scope`] applied against this note's owner: an actor
    /// scoped to `Own` may only edit a note they themselves own.
    ///
    /// # Errors
    /// [`NoteError::NoteNotFound`] if `id` does not exist;
    /// [`NoteError::Identity`] if `auth` may not edit notes at all, or may
    /// not reach this particular one; otherwise as [`NoteError::Database`].
    pub fn update_note(
        &self,
        auth: &AuthenticatedUser,
        id: NoteId,
        title: &str,
        body: &str,
    ) -> Result<(), NoteError> {
        let scope = auth.authorize(Action::Edit, Resource::Note)?;
        let conn = self.lock();
        let owner = note_owner(&conn, id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3 WHERE id = ?4",
            params![title, body, now_unix(), id],
        )?;
        Ok(())
    }

    /// Deletes a note.
    ///
    /// Requires `auth` to hold `Action::Delete` on `Resource::Note`, scoped
    /// against this note's owner the same way
    /// [`update_note`](Self::update_note) is.
    ///
    /// # Errors
    /// [`NoteError::NoteNotFound`] if `id` does not exist;
    /// [`NoteError::Identity`] if `auth` may not delete this note;
    /// otherwise as [`NoteError::Database`].
    pub fn delete_note(&self, auth: &AuthenticatedUser, id: NoteId) -> Result<(), NoteError> {
        let scope = auth.authorize(Action::Delete, Resource::Note)?;
        let conn = self.lock();
        let owner = note_owner(&conn, id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }
}

/// Resolves `Scope` against a row's owner, the same check every
/// single-row method above needs after it has already resolved a [`Scope`]
/// from [`AuthenticatedUser::authorize`] — copied from `senken-chart`'s own
/// helper of the same name and shape.
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), NoteError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        // Covers both "Own but not this row's owner" and any future
        // `Scope` variant this crate has not been taught to interpret —
        // failing closed either way, the same discipline `list_notes`
        // applies to its own `match`.
        _ => Err(NoteError::Identity(IdentityError::Forbidden)),
    }
}

/// Looks up a note's owner, or [`NoteError::NoteNotFound`].
fn note_owner(conn: &Connection, id: NoteId) -> Result<UserId, NoteError> {
    conn.query_row("SELECT owner_id FROM notes WHERE id = ?1", [id], |row| {
        row.get(0)
    })
    .optional()?
    .ok_or(NoteError::NoteNotFound)
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteSummary> {
    Ok(NoteSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        title: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// The current time as a Unix timestamp, for `created_at`/`updated_at`.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore};
    use tempfile::TempDir;

    use super::{NoteError, NoteStore};

    fn temp_stores() -> (TempDir, IdentityStore, NoteStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let notes = NoteStore::new(&identity);
        (dir, identity, notes)
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

    /// Creates an ordinary account with exactly the grants a real "Notes
    /// User" role would carry — View/Create/Edit/Delete on `Note`, at
    /// `Scope::Own` — since a freshly created account otherwise holds no
    /// grants at all and every guarded method here would refuse it
    /// outright.
    fn notes_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Notes User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Note, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn a_created_note_is_listed_back_and_readable_in_full() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice@example.com");

        let note_id = notes
            .create_note(&alice, "Trade journal", "Bought the dip.")
            .unwrap();

        let page = notes.list_notes(&alice, 50, 0).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, note_id);
        assert_eq!(page.rows[0].title, "Trade journal");

        let note = notes.get_note(&alice, note_id).unwrap();
        assert_eq!(note.title, "Trade journal");
        assert_eq!(note.body, "Bought the dip.");
    }

    #[test]
    fn a_second_users_note_is_invisible_and_not_counted_in_the_total() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice2@example.com");
        let bob = notes_user(&identity, &admin, "bob2@example.com");

        notes.create_note(&alice, "Alice's note", "").unwrap();
        notes.create_note(&bob, "Bob's note", "").unwrap();
        notes.create_note(&bob, "Bob's second note", "").unwrap();

        let alice_page = notes.list_notes(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(alice_page.rows[0].title, "Alice's note");
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too — otherwise pagination leaks \
             how many notes exist"
        );

        let bob_page = notes.list_notes(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
    }

    #[test]
    fn alice_cannot_read_bobs_note_even_though_she_knows_its_id() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice3@example.com");
        let bob = notes_user(&identity, &admin, "bob3@example.com");
        let bobs_note = notes
            .create_note(&bob, "Bob's private note", "shh")
            .unwrap();

        let error = notes.get_note(&alice, bobs_note).unwrap_err();
        assert!(matches!(
            error,
            NoteError::Identity(IdentityError::Forbidden)
        ));

        let error = notes
            .update_note(&alice, bobs_note, "Hijacked", "")
            .unwrap_err();
        assert!(matches!(
            error,
            NoteError::Identity(IdentityError::Forbidden)
        ));

        let error = notes.delete_note(&alice, bobs_note).unwrap_err();
        assert!(matches!(
            error,
            NoteError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn a_superadmin_sees_every_users_notes() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice4@example.com");
        let bob = notes_user(&identity, &admin, "bob4@example.com");
        notes.create_note(&alice, "Alice", "").unwrap();
        notes.create_note(&bob, "Bob", "").unwrap();

        let page = notes.list_notes(&admin, 50, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn an_actor_with_no_grant_is_denied() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        identity
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

        let error = notes.create_note(&ungranted, "Nope", "").unwrap_err();
        assert!(matches!(
            error,
            NoteError::Identity(IdentityError::Forbidden)
        ));

        let error = notes.list_notes(&ungranted, 50, 0).unwrap_err();
        assert!(matches!(
            error,
            NoteError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn updating_a_note_changes_title_body_and_updated_at_without_touching_created_at() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice5@example.com");
        let note_id = notes.create_note(&alice, "Draft", "v1").unwrap();
        let original = notes.get_note(&alice, note_id).unwrap();

        notes.update_note(&alice, note_id, "Final", "v2").unwrap();

        let updated = notes.get_note(&alice, note_id).unwrap();
        assert_eq!(updated.title, "Final");
        assert_eq!(updated.body, "v2");
        assert_eq!(updated.created_at, original.created_at);
    }

    #[test]
    fn deleting_a_note_removes_it() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice6@example.com");
        let note_id = notes.create_note(&alice, "Temp", "").unwrap();

        notes.delete_note(&alice, note_id).unwrap();

        let error = notes.get_note(&alice, note_id).unwrap_err();
        assert!(matches!(error, NoteError::NoteNotFound));
    }

    #[test]
    fn listing_orders_by_most_recently_updated_first() {
        let (_dir, identity, notes) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = notes_user(&identity, &admin, "alice7@example.com");
        let first = notes.create_note(&alice, "First", "").unwrap();
        let second = notes.create_note(&alice, "Second", "").unwrap();

        // Touch the first note again so it becomes the most recently
        // updated, even though it was created first. `update_note` stamps
        // `updated_at` from the real clock, which has only one-second
        // resolution — nudging the column forward by hand keeps this test
        // deterministic instead of depending on a real sleep spanning a
        // second boundary.
        notes
            .update_note(&alice, first, "First (edited)", "")
            .unwrap();
        identity
            .shared_connection()
            .lock()
            .unwrap()
            .execute(
                "UPDATE notes SET updated_at = updated_at + 1000 WHERE id = ?1",
                [first],
            )
            .unwrap();

        let page = notes.list_notes(&alice, 50, 0).unwrap();
        assert_eq!(page.rows[0].id, first);
        assert_eq!(page.rows[1].id, second);
    }
}
