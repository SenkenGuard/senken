//! [`WatchlistStore`]: the guarded query API for watchlist groups and
//! their membership.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};
use senken_marketdata::InstrumentId;

use crate::error::WatchlistError;
use crate::id::{WatchlistGroupId, WatchlistMemberId};

/// A watchlist group row as returned by a guarded listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchlistGroupSummary {
    /// The group's id.
    pub id: WatchlistGroupId,
    /// The account that owns this group.
    pub owner_id: UserId,
    /// The group's display name.
    pub name: String,
    /// This group's display order among its owner's other groups.
    pub position: u32,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the group's own fields (its
    /// name or its position) — a member's own row moves independently.
    pub updated_at: i64,
}

/// One instrument's membership in a watchlist group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchlistMember {
    /// The member's id.
    pub id: WatchlistMemberId,
    /// The group this membership belongs to.
    pub group_id: WatchlistGroupId,
    /// The watched instrument.
    pub instrument: InstrumentId,
    /// This member's display order within its group.
    pub position: u32,
}

/// Guarded queries over watchlist groups and their membership.
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
pub struct WatchlistStore {
    conn: Arc<Mutex<Connection>>,
}

impl WatchlistStore {
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

    /// Lists watchlist groups visible to `auth`, scoped: the `WHERE` clause
    /// is chosen by `auth`'s [`senken_acl::decide`]d [`Scope`] for
    /// `(Action::View, Resource::Watchlist)`, and [`Page::total`] is
    /// counted under that same clause. Ordered by display position, then
    /// creation time.
    ///
    /// # Errors
    /// [`WatchlistError::Identity`] if `auth` may not view watchlists at
    /// all, or if `decide` returns a [`Scope`] variant this crate does not
    /// yet translate to SQL; otherwise as [`WatchlistError::Database`].
    pub fn list_groups(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<WatchlistGroupSummary>, WatchlistError> {
        let scope = auth.authorize(Action::View, Resource::Watchlist)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM watchlist_groups WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, position, created_at, updated_at
                     FROM watchlist_groups
                     WHERE owner_id = ?1 ORDER BY position ASC, created_at ASC LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_group)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM watchlist_groups", [], |row| {
                        row.get(0)
                    })?;
                let mut stmt = conn.prepare(
                    "SELECT id, owner_id, name, position, created_at, updated_at
                     FROM watchlist_groups
                     ORDER BY position ASC, created_at ASC LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_group)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — a future variant this crate
            // has not been taught to turn into a `WHERE` clause must fail
            // closed, never fall back to an unfiltered query.
            _ => return Err(WatchlistError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Creates a new, named watchlist group owned by `auth`, appended after
    /// its owner's other groups (`position` = current max + 1, or `0` for
    /// the first one).
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::Watchlist`.
    ///
    /// # Errors
    /// [`WatchlistError::Identity`] if `auth` may not create a watchlist
    /// group; otherwise as [`WatchlistError::Database`].
    pub fn create_group(
        &self,
        auth: &AuthenticatedUser,
        name: &str,
    ) -> Result<WatchlistGroupId, WatchlistError> {
        auth.authorize(Action::Create, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = auth.user_id();
        let position = next_position(&conn, "watchlist_groups", "owner_id", owner)?;
        let group_id = WatchlistGroupId::new();
        let now = now_unix();
        conn.execute(
            "INSERT INTO watchlist_groups (id, owner_id, name, position, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![group_id, owner, name, position, now],
        )?;
        Ok(group_id)
    }

    /// Renames a watchlist group — a field-level edit.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Watchlist`, with
    /// the returned [`Scope`] applied against this group's owner: an actor
    /// scoped to `Own` may only rename a group they themselves own.
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if `group_id` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not edit watchlists at
    /// all, or may not reach this particular group; otherwise as
    /// [`WatchlistError::Database`].
    pub fn rename_group(
        &self,
        auth: &AuthenticatedUser,
        group_id: WatchlistGroupId,
        name: &str,
    ) -> Result<(), WatchlistError> {
        let scope = auth.authorize(Action::Edit, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = group_owner(&conn, group_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "UPDATE watchlist_groups SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, now_unix(), group_id],
        )?;
        Ok(())
    }

    /// Deletes a watchlist group and, by `ON DELETE CASCADE`, every member
    /// it holds.
    ///
    /// Requires `auth` to hold `Action::Delete` on `Resource::Watchlist`,
    /// scoped against this group's owner the same way
    /// [`rename_group`](Self::rename_group) is.
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if `group_id` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not delete this group;
    /// otherwise as [`WatchlistError::Database`].
    pub fn delete_group(
        &self,
        auth: &AuthenticatedUser,
        group_id: WatchlistGroupId,
    ) -> Result<(), WatchlistError> {
        let scope = auth.authorize(Action::Delete, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = group_owner(&conn, group_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "DELETE FROM watchlist_groups WHERE id = ?1",
            params![group_id],
        )?;
        Ok(())
    }

    /// Rewrites the display order of `auth`'s watchlist groups: `ids[0]`
    /// becomes position `0`, `ids[1]` position `1`, and so on, all in one
    /// transaction.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Watchlist`; each
    /// id in `ids` is checked against this scope individually before any
    /// write happens, so a caller cannot smuggle another owner's group id
    /// into their own reorder call.
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if any id in `ids` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not edit watchlists at
    /// all, or may not reach any one of the groups named in `ids`;
    /// otherwise as [`WatchlistError::Database`].
    pub fn reorder_groups(
        &self,
        auth: &AuthenticatedUser,
        ids: &[WatchlistGroupId],
    ) -> Result<(), WatchlistError> {
        let scope = auth.authorize(Action::Edit, Resource::Watchlist)?;
        let mut conn = self.lock();
        for &id in ids {
            let owner = group_owner(&conn, id)?;
            ensure_scope_allows(scope, owner, auth.user_id())?;
        }

        let tx = conn.transaction()?;
        for (index, &id) in ids.iter().enumerate() {
            let position = index_to_position(index);
            tx.execute(
                "UPDATE watchlist_groups SET position = ?1, updated_at = ?2 WHERE id = ?3",
                params![position, now_unix(), id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Lists a group's members in display order.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Watchlist`, with
    /// the returned [`Scope`] applied against the group's owner (a
    /// member's own row has no owner column — see this crate's module
    /// docs).
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if `group_id` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not view this group;
    /// [`WatchlistError::CorruptInstrument`] if a stored member's
    /// instrument text no longer parses; otherwise as
    /// [`WatchlistError::Database`].
    pub fn list_members(
        &self,
        auth: &AuthenticatedUser,
        group_id: WatchlistGroupId,
    ) -> Result<Vec<WatchlistMember>, WatchlistError> {
        let scope = auth.authorize(Action::View, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = group_owner(&conn, group_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let rows: Vec<(WatchlistMemberId, WatchlistGroupId, String, u32)> = conn
            .prepare(
                "SELECT id, group_id, instrument, position FROM watchlist_members
                 WHERE group_id = ?1 ORDER BY position ASC",
            )?
            .query_map(params![group_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(id, group_id, instrument, position)| {
                let instrument = InstrumentId::parse(&instrument)
                    .map_err(|e| WatchlistError::CorruptInstrument(e.to_string()))?;
                Ok(WatchlistMember {
                    id,
                    group_id,
                    instrument,
                    position,
                })
            })
            .collect()
    }

    /// Adds an instrument to a group, appended after its existing members.
    /// Adding an instrument the group already holds is **idempotent**: the
    /// existing member is returned rather than a duplicate being created or
    /// an error raised, since a watchlist behaves like a set of instruments
    /// rather than a strictly ordered log a second insert could
    /// meaningfully conflict with.
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::Watchlist`,
    /// scoped against the group's owner.
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if `group_id` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not add to this group;
    /// otherwise as [`WatchlistError::Database`].
    pub fn add_member(
        &self,
        auth: &AuthenticatedUser,
        group_id: WatchlistGroupId,
        instrument: &InstrumentId,
    ) -> Result<WatchlistMemberId, WatchlistError> {
        let scope = auth.authorize(Action::Create, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = group_owner(&conn, group_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let instrument_text = instrument.as_str();
        let existing = conn
            .query_row(
                "SELECT id FROM watchlist_members WHERE group_id = ?1 AND instrument = ?2",
                params![group_id, instrument_text],
                |row| row.get::<_, WatchlistMemberId>(0),
            )
            .optional()?;
        if let Some(member_id) = existing {
            return Ok(member_id);
        }

        let position = next_position(&conn, "watchlist_members", "group_id", group_id)?;
        let member_id = WatchlistMemberId::new();
        conn.execute(
            "INSERT INTO watchlist_members (id, group_id, instrument, position)
             VALUES (?1, ?2, ?3, ?4)",
            params![member_id, group_id, instrument_text, position],
        )?;
        Ok(member_id)
    }

    /// Removes one member from its group.
    ///
    /// Requires `auth` to hold `Action::Delete` on `Resource::Watchlist`,
    /// scoped against the owner of the member's group.
    ///
    /// # Errors
    /// [`WatchlistError::MemberNotFound`] if `member_id` does not exist;
    /// [`WatchlistError::Identity`] if `auth` may not remove this member;
    /// otherwise as [`WatchlistError::Database`].
    pub fn remove_member(
        &self,
        auth: &AuthenticatedUser,
        member_id: WatchlistMemberId,
    ) -> Result<(), WatchlistError> {
        let scope = auth.authorize(Action::Delete, Resource::Watchlist)?;
        let conn = self.lock();
        let owner = member_owner(&conn, member_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;
        conn.execute(
            "DELETE FROM watchlist_members WHERE id = ?1",
            params![member_id],
        )?;
        Ok(())
    }

    /// Rewrites the display order of one group's members: `ids[0]` becomes
    /// position `0`, `ids[1]` position `1`, and so on, all in one
    /// transaction.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Watchlist`,
    /// scoped against the group's owner. Every id in `ids` must already
    /// belong to `group_id` — an id from a different group (whether or not
    /// the caller owns it) is refused rather than silently moved.
    ///
    /// # Errors
    /// [`WatchlistError::GroupNotFound`] if `group_id` does not exist;
    /// [`WatchlistError::MemberNotFound`] if any id in `ids` is not a
    /// member of `group_id`; [`WatchlistError::Identity`] if `auth` may not
    /// edit this group; otherwise as [`WatchlistError::Database`].
    pub fn reorder_members(
        &self,
        auth: &AuthenticatedUser,
        group_id: WatchlistGroupId,
        ids: &[WatchlistMemberId],
    ) -> Result<(), WatchlistError> {
        let scope = auth.authorize(Action::Edit, Resource::Watchlist)?;
        let mut conn = self.lock();
        let owner = group_owner(&conn, group_id)?;
        ensure_scope_allows(scope, owner, auth.user_id())?;

        let tx = conn.transaction()?;
        for (index, &member_id) in ids.iter().enumerate() {
            let position = index_to_position(index);
            let updated = tx.execute(
                "UPDATE watchlist_members SET position = ?1 WHERE id = ?2 AND group_id = ?3",
                params![position, member_id, group_id],
            )?;
            if updated == 0 {
                // Dropping `tx` without committing rolls every position
                // written so far back — a partial reorder is worse than no
                // reorder at all.
                return Err(WatchlistError::MemberNotFound);
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// Resolves `Scope` against a row's owner, the same check every
/// single-row method above needs after it has already resolved a [`Scope`]
/// from [`AuthenticatedUser::authorize`] — copied from `senken-chart`'s own
/// helper of the same name and shape.
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), WatchlistError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        // Covers both "Own but not this row's owner" and any future
        // `Scope` variant this crate has not been taught to interpret —
        // failing closed either way, the same discipline `list_groups`
        // applies to its own `match`.
        _ => Err(WatchlistError::Identity(IdentityError::Forbidden)),
    }
}

/// Looks up a group's owner, or [`WatchlistError::GroupNotFound`].
fn group_owner(conn: &Connection, group_id: WatchlistGroupId) -> Result<UserId, WatchlistError> {
    conn.query_row(
        "SELECT owner_id FROM watchlist_groups WHERE id = ?1",
        [group_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(WatchlistError::GroupNotFound)
}

/// Looks up the owner of the group a member belongs to, or
/// [`WatchlistError::MemberNotFound`] — a member row carries no `owner_id`
/// of its own, the same relationship a chart pane has to its workspace.
fn member_owner(conn: &Connection, member_id: WatchlistMemberId) -> Result<UserId, WatchlistError> {
    conn.query_row(
        "SELECT g.owner_id FROM watchlist_members m
         JOIN watchlist_groups g ON g.id = m.group_id
         WHERE m.id = ?1",
        [member_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or(WatchlistError::MemberNotFound)
}

/// The position one more row appended to `table` (filtered to
/// `scope_column = scope_value`) should take: one past the current
/// maximum, or `0` for the first row. Shared by
/// [`WatchlistStore::create_group`] and [`WatchlistStore::add_member`] so
/// "append" means the same thing in both places.
fn next_position(
    conn: &Connection,
    table: &'static str,
    scope_column: &'static str,
    scope_value: impl rusqlite::ToSql,
) -> Result<u32, WatchlistError> {
    let next: i64 = conn.query_row(
        &format!("SELECT COALESCE(MAX(position), -1) + 1 FROM {table} WHERE {scope_column} = ?1"),
        [scope_value],
        |row| row.get(0),
    )?;
    // A `u32` position wrapping would need over four billion rows under one
    // owner/group — saturating here rather than erroring keeps `create_group`/
    // `add_member` infallible on the one axis that will never actually be
    // hit in practice.
    Ok(u32::try_from(next).unwrap_or(u32::MAX))
}

/// Converts a 0-based `Vec` index from a reorder call into the `u32`
/// position it is written as. `ids.len()` is bounded by what a caller can
/// fit in one request, nowhere near `u32::MAX`.
fn index_to_position(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

fn row_to_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<WatchlistGroupSummary> {
    Ok(WatchlistGroupSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        name: row.get(2)?,
        position: row.get(3)?,
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
    use senken_marketdata::InstrumentId;
    use tempfile::TempDir;

    use super::{WatchlistError, WatchlistStore};

    fn temp_stores() -> (TempDir, IdentityStore, WatchlistStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let watchlist = WatchlistStore::new(&identity);
        (dir, identity, watchlist)
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
    /// "Watchlist User" role would carry — View/Create/Edit/Delete on
    /// `Watchlist`, at `Scope::Own` — since a freshly created account
    /// otherwise holds no grants at all and every guarded method here would
    /// refuse it outright.
    fn watchlist_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Watchlist User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Watchlist, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn a_created_group_is_listed_back() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice@example.com");

        let group_id = watchlist.create_group(&alice, "Majors").unwrap();

        let page = watchlist.list_groups(&alice, 50, 0).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].id, group_id);
        assert_eq!(page.rows[0].name, "Majors");
        assert_eq!(page.rows[0].position, 0);
    }

    #[test]
    fn a_second_users_group_is_invisible_and_not_counted_in_the_total() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice2@example.com");
        let bob = watchlist_user(&identity, &admin, "bob2@example.com");

        watchlist.create_group(&alice, "Alice's Majors").unwrap();
        watchlist.create_group(&bob, "Bob's Majors").unwrap();
        watchlist.create_group(&bob, "Bob's Alts").unwrap();

        let alice_page = watchlist.list_groups(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(alice_page.rows[0].name, "Alice's Majors");
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too — otherwise pagination leaks \
             how many groups exist"
        );

        let bob_page = watchlist.list_groups(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
    }

    #[test]
    fn a_superadmin_sees_every_users_groups() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice3@example.com");
        let bob = watchlist_user(&identity, &admin, "bob3@example.com");
        watchlist.create_group(&alice, "Alice").unwrap();
        watchlist.create_group(&bob, "Bob").unwrap();

        let page = watchlist.list_groups(&admin, 50, 0).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn an_actor_with_no_grant_is_denied() {
        let (_dir, identity, watchlist) = temp_stores();
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
        assert_eq!(ungranted.user_id(), user_id);

        let error = watchlist.create_group(&ungranted, "Nope").unwrap_err();
        assert!(matches!(
            error,
            WatchlistError::Identity(IdentityError::Forbidden)
        ));

        let error = watchlist.list_groups(&ungranted, 50, 0).unwrap_err();
        assert!(matches!(
            error,
            WatchlistError::Identity(IdentityError::Forbidden)
        ));
    }

    #[test]
    fn members_round_trip_in_order() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice4@example.com");
        let group_id = watchlist.create_group(&alice, "Majors").unwrap();

        let btc = InstrumentId::parse("okx-spot:BTCUSDT").unwrap();
        let eth = InstrumentId::parse("okx-spot:ETHUSDT").unwrap();
        let sol = InstrumentId::parse("okx-spot:SOLUSDT").unwrap();
        watchlist.add_member(&alice, group_id, &btc).unwrap();
        watchlist.add_member(&alice, group_id, &eth).unwrap();
        watchlist.add_member(&alice, group_id, &sol).unwrap();

        let members = watchlist.list_members(&alice, group_id).unwrap();
        assert_eq!(
            members
                .iter()
                .map(|m| m.instrument.clone())
                .collect::<Vec<_>>(),
            vec![btc, eth, sol]
        );
        assert_eq!(
            members.iter().map(|m| m.position).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn adding_a_duplicate_instrument_does_not_create_a_second_row() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice5@example.com");
        let group_id = watchlist.create_group(&alice, "Majors").unwrap();
        let btc = InstrumentId::parse("okx-spot:BTCUSDT").unwrap();

        let first = watchlist.add_member(&alice, group_id, &btc).unwrap();
        let second = watchlist.add_member(&alice, group_id, &btc).unwrap();
        assert_eq!(
            first, second,
            "adding an existing instrument must return the same member"
        );

        let members = watchlist.list_members(&alice, group_id).unwrap();
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn deleting_a_group_cascades_its_members() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice6@example.com");
        let group_id = watchlist.create_group(&alice, "Majors").unwrap();
        let btc = InstrumentId::parse("okx-spot:BTCUSDT").unwrap();
        let member_id = watchlist.add_member(&alice, group_id, &btc).unwrap();

        watchlist.delete_group(&alice, group_id).unwrap();

        let error = watchlist.list_members(&alice, group_id).unwrap_err();
        assert!(matches!(error, WatchlistError::GroupNotFound));

        // The member row itself is gone too, not merely unreachable through
        // its (now-deleted) group.
        let error = watchlist.remove_member(&alice, member_id).unwrap_err();
        assert!(matches!(error, WatchlistError::MemberNotFound));
    }

    #[test]
    fn reordering_groups_persists() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice7@example.com");
        let first = watchlist.create_group(&alice, "First").unwrap();
        let second = watchlist.create_group(&alice, "Second").unwrap();

        watchlist.reorder_groups(&alice, &[second, first]).unwrap();

        let page = watchlist.list_groups(&alice, 50, 0).unwrap();
        assert_eq!(page.rows[0].id, second);
        assert_eq!(page.rows[0].position, 0);
        assert_eq!(page.rows[1].id, first);
        assert_eq!(page.rows[1].position, 1);
    }

    #[test]
    fn reordering_members_persists() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice8@example.com");
        let group_id = watchlist.create_group(&alice, "Majors").unwrap();
        let btc = InstrumentId::parse("okx-spot:BTCUSDT").unwrap();
        let eth = InstrumentId::parse("okx-spot:ETHUSDT").unwrap();
        let btc_id = watchlist.add_member(&alice, group_id, &btc).unwrap();
        let eth_id = watchlist.add_member(&alice, group_id, &eth).unwrap();

        watchlist
            .reorder_members(&alice, group_id, &[eth_id, btc_id])
            .unwrap();

        let members = watchlist.list_members(&alice, group_id).unwrap();
        assert_eq!(members[0].id, eth_id);
        assert_eq!(members[1].id, btc_id);
    }

    #[test]
    fn a_second_users_group_cannot_be_reached_through_own_scope() {
        let (_dir, identity, watchlist) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = watchlist_user(&identity, &admin, "alice9@example.com");
        let bob = watchlist_user(&identity, &admin, "bob9@example.com");
        let bobs_group = watchlist.create_group(&bob, "Bob's Majors").unwrap();

        let error = watchlist
            .rename_group(&alice, bobs_group, "Hijacked")
            .unwrap_err();
        assert!(matches!(
            error,
            WatchlistError::Identity(IdentityError::Forbidden)
        ));

        let error = watchlist.delete_group(&alice, bobs_group).unwrap_err();
        assert!(matches!(
            error,
            WatchlistError::Identity(IdentityError::Forbidden)
        ));
    }
}
