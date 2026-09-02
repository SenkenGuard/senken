//! The identity store's public surface: users, roles, sessions, and the
//! guarded queries that are the only way to read any of it back.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Grant, PluginPermissionName, PluginPermissionRecord, Resource, Scope};
use senken_core::IanaZone;

use crate::actor::{AuthenticatedUser, decode_grant, encode_grant, load_actor, resource_to_sql};
use crate::error::IdentityError;
use crate::id::{PluginPermissionId, RoleId, UserId};
use crate::password::{check_password_len, hash_password, verify_dummy, verify_password};
use crate::token::{RawSessionToken, TokenHash};

/// The email the default superadmin is seeded with on first run. Anyone who knows this is not a secret — the account has no
/// password until someone sets one, and every endpoint but "set password"
/// is refused until they do.
pub const DEFAULT_ADMIN_EMAIL: &str = "admin@mail.com";

/// The built-in role the default superadmin holds, granted every
/// `(Action, Resource)` pair this build of `senken-acl` knows about at
/// [`Scope::All`].
const SUPERADMIN_ROLE_NAME: &str = "Superadmin";

/// Every `Action` this crate knows how to seed the superadmin role with.
/// Not a `match` (there is nothing to be exhaustive *over* — this is a
/// list of values, not a check that every case is handled), so `Action`
/// staying `#[non_exhaustive]` does not force an update here; a new action
/// simply is not granted to the seeded role until someone adds it.
const ALL_ACTIONS: [Action; 5] = [
    Action::View,
    Action::Create,
    Action::Edit,
    Action::Delete,
    Action::Share,
];

/// Every `Resource` this crate knows about, for the same seeding purpose as
/// [`ALL_ACTIONS`].
const ALL_RESOURCES: [Resource; 16] = [
    Resource::ChartWorkspace,
    Resource::ChartLayout,
    Resource::DashboardWorkspace,
    Resource::Alert,
    Resource::Strategy,
    Resource::Account,
    Resource::Order,
    Resource::Adapter,
    Resource::User,
    Resource::Role,
    Resource::Indicator,
    Resource::IndicatorRegistry,
    Resource::Watchlist,
    Resource::Note,
    Resource::Storage,
    Resource::WidgetPlugin,
];

/// Idle session lifetime: 30 days, refreshed on every use.
const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;

/// One page of a guarded query's results, plus the total row count *under
/// the same scope*: counting after applying a narrower filter
/// than the one shown to the caller would leak existence through the total,
/// the exact leak B6 exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// The rows for this page.
    pub rows: Vec<T>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// A user row as returned by a guarded listing — never the full row (no
/// password hash leaves this module).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserSummary {
    /// The user's id.
    pub id: UserId,
    /// The user's email.
    pub email: String,
    /// The user's display name.
    pub display_name: String,
    /// `true` if the account is disabled.
    pub disabled: bool,
    /// `true` once the account has set a password (`false` means the account is still behind the first-run fence).
    pub password_set: bool,
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserSummary> {
    Ok(UserSummary {
        id: row.get(0)?,
        email: row.get(1)?,
        display_name: row.get(2)?,
        disabled: row.get(3)?,
        password_set: row.get(4)?,
    })
}

/// A role row as returned by a guarded listing, including the
/// grants it carries — the client's Users & Roles section (`packages/web`'s
/// `access-section.svelte`, built disabled against this exact shape) needs
/// the grant matrix, not just the role's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSummary {
    /// The role's id.
    pub id: RoleId,
    /// The role's name.
    pub name: String,
    /// A human-readable description.
    pub description: String,
    /// `true` for a role seeded by this crate (e.g. `Superadmin`) rather
    /// than created by an admin.
    pub builtin: bool,
    /// The grants this role carries.
    pub grants: Vec<Grant>,
}

/// The accounts database (SQLite at `.data/accounts/`;).
///
/// A single [`Connection`] behind a [`Mutex`] rather than a pool: SQLite
/// serialises writers regardless, this is a per-machine desktop-scale
/// database, and a pool would add a dependency and a failure mode
/// (checkout timeouts) to solve contention this workload does not have.
#[derive(Debug)]
pub struct IdentityStore {
    conn: Arc<Mutex<Connection>>,
}

impl IdentityStore {
    /// Opens (creating if absent) the accounts database at `path`, and
    /// seeds the default superadmin if the database has no
    /// users yet.
    ///
    /// `path` is the database *file*, not the `accounts/` directory the
    /// application reserves — joining this crate's default filename with an
    /// application's `.data` root is left to the caller (wiring this store
    /// into `senken-runtime` is, not this crate).
    ///
    /// # Errors
    /// See [`IdentityError`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, IdentityError> {
        let conn = crate::schema::open(path.as_ref())?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.seed_default_admin()?;
        store.backfill_superadmin_resource_grants()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Returns a handle to the **same** SQLite connection this store uses.
    ///
    /// Workspaces reference users, so `senken-chart`'s
    /// tables live in this same file, under `.data/accounts/`,
    /// rather than a second database — but two crates each opening their
    /// own connection and independently stamping `user_version` on one file
    /// is exactly the mess this arrangement avoids. This crate stays
    /// the file's single owner of that sequence (see `schema.rs`'s v3 doc
    /// comment); `senken-chart` calls this instead of
    /// [`open`](Self::open) so it never opens a second connection to the
    /// same file or manages a schema version of its own. See that crate's
    /// module docs for the full reasoning.
    #[must_use]
    pub fn shared_connection(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    /// The accounts database's own file path on disk, when it is backed by
    /// a real file (always true here — this crate never opens an
    /// in-memory connection). Exists for `senken-api`'s storage-usage
    /// report: an admin reclaiming disk space needs this database's own
    /// size too, and `senken-identity`, not its caller, is the one crate
    /// that actually knows where its file lives.
    #[must_use]
    pub fn db_path(&self) -> Option<std::path::PathBuf> {
        self.lock().path().map(std::path::PathBuf::from)
    }

    /// Creates the built-in `Superadmin` role and the `admin@mail.com`
    /// account with no password, iff the database has no users at all.
    /// Idempotent, so calling [`open`](Self::open) on an already-seeded
    /// database is a no-op.
    fn seed_default_admin(&self) -> Result<(), IdentityError> {
        let conn = self.lock();
        let user_count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
        if user_count > 0 {
            return Ok(());
        }

        let role_id = RoleId::new();
        conn.execute(
            "INSERT INTO roles (id, name, description, builtin) VALUES (?1, ?2, ?3, 1)",
            params![
                role_id,
                SUPERADMIN_ROLE_NAME,
                "Full access to every resource, seeded on first run"
            ],
        )?;
        for action in ALL_ACTIONS {
            for resource in ALL_RESOURCES {
                let (a, r, s) = encode_grant(Grant::new(action, resource, Scope::All))?;
                conn.execute(
                    "INSERT INTO role_grants (role_id, action, resource, scope) VALUES (?1, ?2, ?3, ?4)",
                    params![role_id, a, r, s],
                )?;
            }
        }

        let user_id = UserId::new();
        conn.execute(
            "INSERT INTO users (id, email, display_name, password_hash, created_at, disabled)
             VALUES (?1, ?2, ?3, NULL, ?4, 0)",
            params![user_id, DEFAULT_ADMIN_EMAIL, "Administrator", now_unix()],
        )?;
        conn.execute(
            "INSERT INTO user_roles (user_id, role_id) VALUES (?1, ?2)",
            params![user_id, role_id],
        )?;
        tracing::info!(
            email = DEFAULT_ADMIN_EMAIL,
            "seeded default superadmin with no password"
        );
        Ok(())
    }

    /// Grants the built-in `Superadmin` role full access to any `Resource`
    /// it has never held a grant on at all, on every `open()`.
    ///
    /// [`seed_default_admin`](Self::seed_default_admin) only ever runs its
    /// `ALL_RESOURCES` loop once, on a database with zero users — an
    /// **existing** install never sees it again, so a `Resource` variant
    /// added after that install's first run would otherwise 403 its own
    /// superadmin forever. This is that catch-up, and it is deliberately
    /// narrower than "re-seed everything": the rule is "the role has *no*
    /// row for this resource, at any action" — a resource the role already
    /// has *some* grant on (even a deliberately reduced one, e.g. an
    /// operator who removed `Delete` on `Alert`) is left completely alone.
    /// Idempotent: once every `ALL_RESOURCES` entry has at least one grant
    /// row, every later `open()` finds nothing left to do.
    fn backfill_superadmin_resource_grants(&self) -> Result<(), IdentityError> {
        let conn = self.lock();
        let role_id: Option<RoleId> = conn
            .query_row(
                "SELECT id FROM roles WHERE name = ?1 AND builtin = 1",
                [SUPERADMIN_ROLE_NAME],
                |row| row.get(0),
            )
            .optional()?;
        // No built-in Superadmin role exists — nothing to backfill onto.
        let Some(role_id) = role_id else {
            return Ok(());
        };

        for resource in ALL_RESOURCES {
            let resource_token = resource_to_sql(resource);
            let has_any_grant: bool = conn
                .query_row(
                    "SELECT 1 FROM role_grants WHERE role_id = ?1 AND resource = ?2 LIMIT 1",
                    params![role_id, resource_token],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if has_any_grant {
                continue;
            }
            for action in ALL_ACTIONS {
                let (a, r, s) = encode_grant(Grant::new(action, resource, Scope::All))?;
                conn.execute(
                    "INSERT INTO role_grants (role_id, action, resource, scope) VALUES (?1, ?2, ?3, ?4)",
                    params![role_id, a, r, s],
                )?;
            }
            tracing::info!(
                resource = resource_token,
                "backfilled superadmin grant for a resource newly added to this crate"
            );
        }
        Ok(())
    }

    /// Creates a user. `initial_password` is hashed immediately if given;
    /// `None` leaves `password_hash` `NULL`, i.e. the account is created
    /// behind the same B4 fence the default admin uses.
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::User`
    /// : this is one of the four mutations that, until now, took no
    /// [`AuthenticatedUser`] at all and so could not be authorised by
    /// anything but `senken-api`'s router-level guard — a check a headless
    /// caller (a backtest, a CLI, a test) has no HTTP layer to inherit,
    /// which is exactly the gap this exists to close: authorisation
    /// belongs in this crate, not the transport in front of it.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// create a user, [`IdentityError::PasswordTooShort`] if
    /// `initial_password` is given and too short, [`IdentityError::EmailTaken`]
    /// if `email` is already registered, or otherwise as
    /// [`IdentityError::Database`].
    pub fn create_user(
        &self,
        auth: &AuthenticatedUser,
        email: &str,
        display_name: &str,
        initial_password: Option<&str>,
    ) -> Result<UserId, IdentityError> {
        auth.authorize(Action::Create, Resource::User)?;
        if let Some(password) = initial_password {
            check_password_len(password)?;
        }
        let conn = self.lock();
        let taken: bool = conn
            .query_row("SELECT 1 FROM users WHERE email = ?1", [email], |_| Ok(()))
            .optional()?
            .is_some();
        if taken {
            return Err(IdentityError::EmailTaken(email.to_owned()));
        }

        let password_hash = initial_password.map(hash_password).transpose()?;
        let id = UserId::new();
        conn.execute(
            "INSERT INTO users (id, email, display_name, password_hash, created_at, disabled)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            params![id, email, display_name, password_hash, now_unix()],
        )?;
        Ok(id)
    }

    /// Creates a role with the given grants.
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::Role` (plan
    /// 004 Q9.3 — see [`create_user`](Self::create_user)'s doc for why this
    /// crate, not just `senken-api`, must be the one to check it).
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// create a role, or otherwise as [`IdentityError::Database`].
    pub fn create_role(
        &self,
        auth: &AuthenticatedUser,
        name: &str,
        description: &str,
        grants: &[Grant],
    ) -> Result<RoleId, IdentityError> {
        auth.authorize(Action::Create, Resource::Role)?;
        let conn = self.lock();
        let id = RoleId::new();
        conn.execute(
            "INSERT INTO roles (id, name, description, builtin) VALUES (?1, ?2, ?3, 0)",
            params![id, name, description],
        )?;
        for grant in grants {
            let (a, r, s) = encode_grant(*grant)?;
            conn.execute(
                "INSERT INTO role_grants (role_id, action, resource, scope) VALUES (?1, ?2, ?3, ?4)",
                params![id, a, r, s],
            )?;
        }
        Ok(id)
    }

    /// Assigns `role_id` to `user_id`, invalidating that user's other
    /// sessions (sessions rotate on privilege change, the same rule as a password change).
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::User` (see [`create_user`](Self::create_user)'s doc): assigning a
    /// role is a change to the target user's own record, the same category
    /// [`grant_direct`](Self::grant_direct) is.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit users, or otherwise as [`IdentityError::Database`].
    pub fn assign_role(
        &self,
        auth: &AuthenticatedUser,
        user_id: UserId,
        role_id: RoleId,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::User)?;
        let conn = self.lock();
        conn.execute(
            "INSERT OR IGNORE INTO user_roles (user_id, role_id) VALUES (?1, ?2)",
            params![user_id, role_id],
        )?;
        delete_all_sessions_for(&conn, user_id)?;
        Ok(())
    }

    /// Attaches a grant to `user_id` directly, independent of any role,
    /// invalidating that user's other sessions for the same
    /// reason as [`assign_role`](Self::assign_role).
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::User` (see [`create_user`](Self::create_user)'s doc).
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit users, or otherwise as [`IdentityError::Database`].
    pub fn grant_direct(
        &self,
        auth: &AuthenticatedUser,
        user_id: UserId,
        grant: Grant,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::User)?;
        let conn = self.lock();
        let (a, r, s) = encode_grant(grant)?;
        conn.execute(
            "INSERT OR REPLACE INTO user_grants (user_id, action, resource, scope) VALUES (?1, ?2, ?3, ?4)",
            params![user_id, a, r, s],
        )?;
        delete_all_sessions_for(&conn, user_id)?;
        Ok(())
    }

    /// Authenticates `email`/`password` and, on success, mints a session.
    ///
    /// Deliberately returns the same [`IdentityError::InvalidCredentials`]
    /// whether the email does not exist, the account has no password set
    /// yet, or the password is wrong — and runs a full
    /// Argon2 verify against a dummy hash on every path that is not a real
    /// password check, so the three cases cost the same wall-clock time.
    ///
    /// # Errors
    /// [`IdentityError::InvalidCredentials`] for any authentication
    /// failure; otherwise as [`IdentityError::Database`] or
    /// [`IdentityError::Hashing`].
    pub fn login(
        &self,
        email: &str,
        password: &str,
    ) -> Result<(UserId, RawSessionToken), IdentityError> {
        let conn = self.lock();
        let row: Option<(UserId, Option<String>, bool)> = conn
            .query_row(
                "SELECT id, password_hash, disabled FROM users WHERE email = ?1",
                [email],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;

        let Some((user_id, password_hash, disabled)) = row else {
            verify_dummy(password);
            return Err(IdentityError::InvalidCredentials);
        };

        // No password set yet (the first-run fence): login is not the mechanism that
        // clears it, so this must read exactly like "wrong password" —
        // including paying the same Argon2 cost.
        let Some(hash) = password_hash else {
            verify_dummy(password);
            return Err(IdentityError::InvalidCredentials);
        };

        let matches = verify_password(password, &hash);
        if !matches || disabled {
            return Err(IdentityError::InvalidCredentials);
        }

        let raw = RawSessionToken::generate();
        let token_hash = TokenHash::of(raw.reveal());
        let now = now_unix();
        conn.execute(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
             VALUES (?1, ?2, ?3, ?3, ?4)",
            params![token_hash, user_id, now, now + SESSION_TTL_SECONDS],
        )?;
        Ok((user_id, raw))
    }

    /// Resolves a raw session token into the authenticated user behind it,
    /// refreshing the session's idle timer on success ("30 days idle, refreshed on use").
    ///
    /// Returns `Ok(None)` — not an error — for a token that is missing,
    /// expired, or belongs to a disabled account: from the caller's point
    /// of view all three mean "this request is unauthenticated," and
    /// distinguishing them would tell a client information about a session
    /// it does not hold.
    ///
    /// # Errors
    /// As [`IdentityError::Database`] or [`IdentityError::CorruptGrant`] if
    /// a stored grant cannot be decoded.
    pub fn resolve_session(&self, token: &str) -> Result<Option<AuthenticatedUser>, IdentityError> {
        let conn = self.lock();
        let presented = TokenHash::of(token);

        let row: Option<(TokenHash, UserId, i64)> = conn
            .query_row(
                "SELECT token_hash, user_id, expires_at FROM sessions WHERE token_hash = ?1",
                [presented],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((stored, user_id, expires_at)) = row else {
            return Ok(None);
        };
        // Defense in depth: the row was already found by an
        // indexed lookup on `presented`, so this can only ever agree — but
        // the accept decision below is made from `stored.ct_eq`, not from
        // "a row came back", so nothing here ever rests on a plain `==`
        // over token-derived bytes.
        if !stored.ct_eq(&presented) {
            return Ok(None);
        }

        let now = now_unix();
        if now >= expires_at {
            return Ok(None);
        }

        let (password_hash, disabled): (Option<String>, bool) = conn.query_row(
            "SELECT password_hash, disabled FROM users WHERE id = ?1",
            [user_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if disabled {
            return Ok(None);
        }

        conn.execute(
            "UPDATE sessions SET last_seen_at = ?1, expires_at = ?2 WHERE token_hash = ?3",
            params![now, now + SESSION_TTL_SECONDS, presented],
        )?;

        let loaded = load_actor(&conn, user_id)?;
        Ok(Some(AuthenticatedUser::new(
            user_id,
            loaded.actor,
            password_hash.is_some(),
            loaded.role_names,
            loaded.effective_grants,
        )))
    }

    /// Deletes the session identified by `token`. Not an error if it did
    /// not exist — logging out twice is not a failure.
    ///
    /// # Errors
    /// As [`IdentityError::Database`].
    pub fn logout(&self, token: &str) -> Result<(), IdentityError> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM sessions WHERE token_hash = ?1",
            params![TokenHash::of(token)],
        )?;
        Ok(())
    }

    /// Sets `email`'s password, whether that account has one yet or not —
    /// this is the one operation the B4 fence exempts, so it takes an
    /// email rather than an [`AuthenticatedUser`] (there is nothing to
    /// authorise: a fenced account cannot pass any other check, and
    /// setting your own password never needs a grant).
    ///
    /// `current_token`, when given, names the session making this call so
    /// it survives; every *other* session for the account is deleted
    /// ("setting or changing a password invalidates every other session"). Passing `None` is the first-run case: there is no
    /// session yet, so every session the account might somehow already
    /// have is invalidated.
    ///
    /// This does **not** check whether `email` is currently fenced — it is
    /// the operation that clears the fence, so it must work identically
    /// whether the account already has a password or not. A caller that
    /// must not let this overwrite an *already-set* password without proof
    /// of identity (the HTTP layer, for the anonymous first-run
    /// case) is responsible for checking [`is_fenced`](Self::is_fenced)
    /// itself before calling this with no session.
    ///
    /// # Errors
    /// [`IdentityError::PasswordTooShort`], [`IdentityError::UserNotFound`],
    /// or as [`IdentityError::Database`]/[`IdentityError::Hashing`].
    pub fn set_password(
        &self,
        email: &str,
        new_password: &str,
        current_token: Option<&str>,
    ) -> Result<(), IdentityError> {
        check_password_len(new_password)?;
        let conn = self.lock();
        let user_id: UserId = conn
            .query_row("SELECT id FROM users WHERE email = ?1", [email], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or(IdentityError::UserNotFound)?;

        let hash = hash_password(new_password)?;
        apply_password(&conn, user_id, &hash, current_token)?;
        Ok(())
    }

    /// Sets the password of the account behind `user_id`, keeping only the
    /// session named by `current_token` alive.
    ///
    /// This is [`set_password`](Self::set_password)'s counterpart for an
    /// **already-authenticated** caller changing their own password: the
    /// HTTP layer only ever has a `user_id` in this case (from
    /// an already-resolved [`AuthenticatedUser::user_id`]), never an email
    /// it would otherwise have to trust from the request body — a self
    /// password change needs no [`senken_acl`] grant, exactly like
    /// [`set_password`](Self::set_password) needs none for the first-run
    /// case, so this performs no permission check either.
    ///
    /// Unlike [`set_password`](Self::set_password), `current_token` is
    /// required, not optional: a caller of this method always has a live
    /// session (that is where `user_id` came from), so there is no
    /// first-run case here to leave every session invalidated for.
    ///
    /// # Errors
    /// [`IdentityError::PasswordTooShort`], [`IdentityError::UserNotFound`]
    /// if `user_id` no longer exists, or as
    /// [`IdentityError::Database`]/[`IdentityError::Hashing`].
    pub fn set_password_for(
        &self,
        user_id: UserId,
        new_password: &str,
        current_token: &str,
    ) -> Result<(), IdentityError> {
        check_password_len(new_password)?;
        let hash = hash_password(new_password)?;
        let conn = self.lock();
        apply_password(&conn, user_id, &hash, Some(current_token))
    }

    /// `true` if `email`'s account has not set a password yet (the fence, `users.password_hash IS NULL`).
    ///
    /// Exists so the HTTP layer can decide whether an
    /// *unauthenticated* `set_password` call may proceed at all: without
    /// this, nothing would stop an anonymous caller who merely knows an
    /// email from overwriting that account's already-set password, since
    /// [`set_password`](Self::set_password) itself does not check the
    /// fence (it is the operation that clears it). A caller building a
    /// user-facing response from this should give the same answer for "no
    /// such email" and "not fenced" — this method distinguishes them
    /// ([`IdentityError::UserNotFound`] vs `Ok(false)`) so that decision is
    /// the caller's to make, not baked in here.
    ///
    /// # Errors
    /// [`IdentityError::UserNotFound`] if no account has this email;
    /// otherwise as [`IdentityError::Database`].
    pub fn is_fenced(&self, email: &str) -> Result<bool, IdentityError> {
        let conn = self.lock();
        let password_hash: Option<Option<String>> = conn
            .query_row(
                "SELECT password_hash FROM users WHERE email = ?1",
                [email],
                |row| row.get(0),
            )
            .optional()?;
        password_hash
            .map(|hash| hash.is_none())
            .ok_or(IdentityError::UserNotFound)
    }

    /// The basic profile of exactly the account behind `user_id`.
    ///
    /// This is deliberately **not** a [`senken_acl`]-guarded query like
    /// [`list_users`](Self::list_users): viewing your own identity is not
    /// something a role or grant can be missing for, the same reasoning
    /// [`set_password`](Self::set_password) documents for changing your own
    /// password. It is safe only because every caller in this crate's
    /// intended use (`GET /api/me`) supplies a `user_id` it
    /// already obtained by resolving a real session
    /// ([`AuthenticatedUser::user_id`]) — never one taken from a request
    /// parameter naming someone else. The guarded equivalent for looking up
    /// *another* user's profile is [`list_users`](Self::list_users).
    ///
    /// # Errors
    /// [`IdentityError::UserNotFound`] if `user_id` no longer exists;
    /// otherwise as [`IdentityError::Database`].
    pub fn get_own_profile(&self, user_id: UserId) -> Result<UserSummary, IdentityError> {
        let conn = self.lock();
        conn.query_row(
            "SELECT id, email, display_name, disabled, password_hash IS NOT NULL
             FROM users WHERE id = ?1",
            [user_id],
            row_to_summary,
        )
        .optional()?
        .ok_or(IdentityError::UserNotFound)
    }

    /// The display zone exactly the account behind `user_id` has chosen, or
    /// `None` if it never has.
    ///
    /// Deliberately **not** a [`senken_acl`]-guarded query, for the same
    /// reason [`get_own_profile`](Self::get_own_profile) is not one: reading
    /// your own display zone needs no role or grant, and this is safe only
    /// because every caller (`GET /api/me/zone`) supplies a `user_id` it
    /// already obtained from a resolved session
    /// ([`AuthenticatedUser::user_id`]) — never one taken from a request
    /// parameter naming someone else.
    ///
    /// # Errors
    /// [`IdentityError::UserNotFound`] if `user_id` no longer exists;
    /// [`IdentityError::CorruptZone`] if the stored value no longer parses
    /// as an [`IanaZone`] (this crate never writes one that would not —
    /// see [`set_zone`](Self::set_zone) — so this means the row was written
    /// by an incompatible version of this crate or edited by hand);
    /// otherwise as [`IdentityError::Database`].
    pub fn get_zone(&self, user_id: UserId) -> Result<Option<IanaZone>, IdentityError> {
        let conn = self.lock();
        let stored: Option<String> = conn
            .query_row(
                "SELECT display_zone FROM users WHERE id = ?1",
                [user_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(IdentityError::UserNotFound)?;
        stored
            .map(|id| {
                IanaZone::new(&id).map_err(|_| {
                    IdentityError::CorruptZone(format!(
                        "stored display zone `{id}` no longer parses"
                    ))
                })
            })
            .transpose()
    }

    /// Sets the display zone of the account behind `user_id`.
    ///
    /// Takes an already-validated [`IanaZone`] rather than a raw string —
    /// the caller (the `PUT /api/me/zone` request body's own `Deserialize`
    /// impl) has already checked it against the bundled time zone database,
    /// so this method has nothing left to reject. Not guarded by
    /// [`senken_acl`], for the same "own account, no grant needed, caller
    /// supplies a session-derived `user_id`" reasoning as
    /// [`get_zone`](Self::get_zone). Unlike a privilege change
    /// (`grant_direct`, `assign_role`), this does not invalidate the
    /// account's other sessions — a display preference is not a security
    /// boundary.
    ///
    /// # Errors
    /// [`IdentityError::UserNotFound`] if `user_id` no longer exists;
    /// otherwise as [`IdentityError::Database`].
    pub fn set_zone(&self, user_id: UserId, zone: &IanaZone) -> Result<(), IdentityError> {
        let conn = self.lock();
        let updated = conn.execute(
            "UPDATE users SET display_zone = ?1 WHERE id = ?2",
            params![zone.as_str(), user_id],
        )?;
        if updated == 0 {
            return Err(IdentityError::UserNotFound);
        }
        Ok(())
    }

    /// Loads every plugin permission previously recorded for `plugin_id`
    /// (the `plugin_permissions` table), whether currently
    /// registered or orphaned.
    ///
    /// This is the read half of the coordination gap Q2 and Q7 each left
    /// to the other: pass the result as `previous` to
    /// `senken_plugin::reconcile_plugin_permissions` together with what the
    /// plugin declares this activation, then persist the reconciled result
    /// with [`save_plugin_permissions`](Self::save_plugin_permissions).
    /// This crate does not depend on `senken-plugin` itself (that would
    /// pull the plugin contract, and everything a plugin can register
    /// against, into the storage layer backwards) — calling the pure
    /// reconciliation function is the caller's job, exactly like a
    /// `senken_acl::decide` call is the caller's job for core permissions.
    ///
    /// # Errors
    /// [`IdentityError::CorruptGrant`] if a stored `name` no longer parses
    /// as a [`PluginPermissionName`] (the database was written by an
    /// incompatible version of this crate, or edited by hand); otherwise as
    /// [`IdentityError::Database`].
    pub fn load_plugin_permissions(
        &self,
        plugin_id: &str,
    ) -> Result<Vec<PluginPermissionRecord>, IdentityError> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT name, orphaned FROM plugin_permissions WHERE plugin_id = ?1")?;
        let rows = stmt
            .query_map(params![plugin_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        rows.into_iter()
            .map(|(name, orphaned)| {
                let name = PluginPermissionName::parse(&name).map_err(|source| {
                    IdentityError::CorruptGrant(format!(
                        "stored plugin permission `{name}` no longer parses: {source}"
                    ))
                })?;
                let record = PluginPermissionRecord::registered(name);
                Ok(if orphaned { record.orphan() } else { record })
            })
            .collect()
    }

    /// Persists `records` — the output of
    /// `senken_plugin::reconcile_plugin_permissions` — as `plugin_id`'s
    /// current set of known permissions.
    ///
    /// Upserts by `name`, which is globally unique by construction (it
    /// embeds the owning plugin's namespace,), so calling this
    /// again with the same reconciled state is idempotent and a
    /// permission's `registered_at` is set once, on its first appearance,
    /// never bumped by a later re-registration.
    ///
    /// This never touches `role_plugin_grants`/`user_plugin_grants`: a
    /// plugin may register a permission but never grant one,
    /// so nothing on this path assigns the permission to anyone. Assigning
    /// it is an admin operation.
    ///
    /// # Errors
    /// As [`IdentityError::Database`].
    pub fn save_plugin_permissions(
        &self,
        plugin_id: &str,
        records: &[PluginPermissionRecord],
    ) -> Result<(), IdentityError> {
        let conn = self.lock();
        let now = now_unix();
        for record in records {
            conn.execute(
                "INSERT INTO plugin_permissions (id, plugin_id, name, registered_at, orphaned)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET orphaned = excluded.orphaned",
                params![
                    PluginPermissionId::new(),
                    plugin_id,
                    record.name().as_str(),
                    now,
                    record.is_orphaned(),
                ],
            )?;
        }
        Ok(())
    }

    /// Enables or disables the account with `email`, invalidating its
    /// existing sessions when disabling it — a disabled account must not
    /// keep working on a session it minted before it was disabled.
    ///
    /// # Errors
    /// [`IdentityError::UserNotFound`], or as [`IdentityError::Database`].
    pub fn set_disabled(&self, email: &str, disabled: bool) -> Result<(), IdentityError> {
        let conn = self.lock();
        let user_id: UserId = conn
            .query_row("SELECT id FROM users WHERE email = ?1", [email], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or(IdentityError::UserNotFound)?;
        conn.execute(
            "UPDATE users SET disabled = ?1 WHERE id = ?2",
            params![disabled, user_id],
        )?;
        if disabled {
            delete_all_sessions_for(&conn, user_id)?;
        }
        Ok(())
    }

    /// Lists users visible to `auth`, scoped: the
    /// `WHERE` clause — not a post-fetch filter — is chosen by
    /// `auth`'s `senken_acl::decide`d [`Scope`] for `(Action::View,
    /// Resource::User)`, and [`Page::total`] is counted under that same
    /// clause. There is no unscoped equivalent of this function to fall
    /// back to.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s account is behind
    /// the B4 fence; [`IdentityError::Forbidden`] if the actor may not
    /// view users at all, or if `decide` returns a [`Scope`] variant this
    /// function does not yet translate to SQL; otherwise as
    /// [`IdentityError::Database`].
    pub fn list_users(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<UserSummary>, IdentityError> {
        let scope = auth.authorize(Action::View, Resource::User)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let id = auth.user_id();
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM users WHERE id = ?1", [id], |row| {
                        row.get(0)
                    })?;
                let mut stmt = conn.prepare(
                    "SELECT id, email, display_name, disabled, password_hash IS NOT NULL
                     FROM users WHERE id = ?1 ORDER BY email LIMIT ?2 OFFSET ?3",
                )?;
                let rows = stmt
                    .query_map(params![id, limit, offset], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(
                    "SELECT id, email, display_name, disabled, password_hash IS NOT NULL
                     FROM users ORDER BY email LIMIT ?1 OFFSET ?2",
                )?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_summary)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — a future
            // variant this crate has not been taught to turn into a
            // `WHERE` clause must fail closed, never fall back to an
            // unfiltered query.
            _ => return Err(IdentityError::Forbidden),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Lists roles visible to `auth`, scoped.
    ///
    /// A role has no owning-user column the way `users` does, so
    /// `Scope::Own` cannot mean "roles this actor created" the way it does
    /// for `list_users`. This crate reads it instead as "the roles `auth`'s
    /// own account currently holds" (a join through `user_roles`) — the one
    /// reading of "own roles" that is meaningful for administrative data
    /// nobody owns outright, and a real self-service use case ("what roles
    /// do I have"). `Scope::All` is every role in the system, for admin
    /// management — the case the seeded `Superadmin` role holds.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s account is behind
    /// the B4 fence; [`IdentityError::Forbidden`] if the actor may not view
    /// roles at all, or if `decide` returns a [`Scope`] variant this
    /// function does not yet translate to SQL; otherwise as
    /// [`IdentityError::Database`] or [`IdentityError::CorruptGrant`] if a
    /// stored grant cannot be decoded.
    pub fn list_roles(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<RoleSummary>, IdentityError> {
        let scope = auth.authorize(Action::View, Resource::Role)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, ids): (i64, Vec<RoleId>) = match scope {
            Scope::Own => {
                let id = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM roles r
                     JOIN user_roles ur ON ur.role_id = r.id WHERE ur.user_id = ?1",
                    [id],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(
                    "SELECT r.id FROM roles r JOIN user_roles ur ON ur.role_id = r.id
                     WHERE ur.user_id = ?1 ORDER BY r.name LIMIT ?2 OFFSET ?3",
                )?;
                let ids = stmt
                    .query_map(params![id, limit, offset], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, ids)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM roles", [], |row| row.get(0))?;
                let mut stmt =
                    conn.prepare("SELECT id FROM roles ORDER BY name LIMIT ?1 OFFSET ?2")?;
                let ids = stmt
                    .query_map(params![limit, offset], |row| row.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, ids)
            }
            // Same fail-closed reasoning as `list_users`.
            _ => return Err(IdentityError::Forbidden),
        };

        let mut role_stmt =
            conn.prepare("SELECT name, description, builtin FROM roles WHERE id = ?1")?;
        let mut grant_stmt =
            conn.prepare("SELECT action, resource, scope FROM role_grants WHERE role_id = ?1")?;
        let mut rows = Vec::with_capacity(ids.len());
        for id in ids {
            let (name, description, builtin): (String, String, bool) =
                role_stmt.query_row([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            let grants = grant_stmt
                .query_map([id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|(a, r, s)| decode_grant(&a, &r, &s))
                .collect::<Result<Vec<_>, _>>()?;
            rows.push(RoleSummary {
                id,
                name,
                description,
                builtin,
                grants,
            });
        }

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Removes a direct grant from `user_id` — the inverse of
    /// [`grant_direct`](Self::grant_direct). Invalidates the
    /// account's other sessions for the same B15 reason `grant_direct`
    /// does: losing a grant is still a privilege change.
    ///
    /// Matching by `(user_id, action, resource)` only, not `scope`, mirrors
    /// `user_grants`' own primary key: at most one grant per
    /// `(user_id, action, resource)` can exist at all (`grant_direct` uses
    /// `INSERT OR REPLACE` for exactly this reason), so there is nothing a
    /// caller-supplied `scope` could disambiguate. Not an error if `grant`
    /// was never attached to `user_id` — revoking a grant that already
    /// is not there leaves the account in the state the caller wanted
    /// either way.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::User` — the same
    /// guarded shape as [`grant_direct`](Self::grant_direct), its inverse,
    /// and for the same reason: without it this method would take no
    /// [`AuthenticatedUser`] at all, leaving a headless caller no check to
    /// inherit.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit users, or otherwise as [`IdentityError::Database`].
    pub fn revoke_direct(
        &self,
        auth: &AuthenticatedUser,
        user_id: UserId,
        grant: Grant,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::User)?;
        let conn = self.lock();
        let (action, resource, _scope) = encode_grant(grant)?;
        conn.execute(
            "DELETE FROM user_grants WHERE user_id = ?1 AND action = ?2 AND resource = ?3",
            params![user_id, action, resource],
        )?;
        delete_all_sessions_for(&conn, user_id)?;
        Ok(())
    }

    /// Grants the plugin permission `name` to `user_id` directly (plugin permissions are opaque names, granted whole, never interpreted by this crate). Invalidates the account's other
    /// sessions, the same B15 privilege-change rule
    /// [`grant_direct`](Self::grant_direct) follows.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::User` (see [`revoke_direct`](Self::revoke_direct)'s doc for why this
    /// crate, not just `senken-api`'s router, must be the one to check it):
    /// granting a plugin permission to a user is a change to that user's
    /// own record, the same category [`grant_direct`](Self::grant_direct)
    /// is.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit users, [`IdentityError::PluginPermissionNotFound`] if `name` has
    /// never been registered by any plugin; [`IdentityError::PluginPermissionOrphaned`]
    /// if it was registered once but the owning plugin has since stopped
    /// declaring it (an orphan stays attached to whatever already holds it, but is not newly grantable); otherwise as
    /// [`IdentityError::Database`].
    pub fn grant_plugin_permission_to_user(
        &self,
        auth: &AuthenticatedUser,
        user_id: UserId,
        name: &PluginPermissionName,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::User)?;
        let conn = self.lock();
        let permission_id = plugin_permission_id_for_grant(&conn, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO user_plugin_grants (user_id, plugin_permission_id)
             VALUES (?1, ?2)",
            params![user_id, permission_id],
        )?;
        delete_all_sessions_for(&conn, user_id)?;
        Ok(())
    }

    /// Revokes the plugin permission `name` from `user_id` — the inverse of
    /// [`grant_plugin_permission_to_user`](Self::grant_plugin_permission_to_user).
    /// Unlike granting, this succeeds even for an orphaned or
    /// already-ungranted permission: revocation only ever narrows access,
    /// so it has nothing to refuse.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::User` (see [`revoke_direct`](Self::revoke_direct)'s doc).
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit users, or otherwise as [`IdentityError::Database`].
    pub fn revoke_plugin_permission_from_user(
        &self,
        auth: &AuthenticatedUser,
        user_id: UserId,
        name: &PluginPermissionName,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::User)?;
        let conn = self.lock();
        conn.execute(
            "DELETE FROM user_plugin_grants WHERE user_id = ?1 AND plugin_permission_id IN
             (SELECT id FROM plugin_permissions WHERE name = ?2)",
            params![user_id, name.as_str()],
        )?;
        delete_all_sessions_for(&conn, user_id)?;
        Ok(())
    }

    /// Grants the plugin permission `name` to every user holding
    /// `role_id`.
    ///
    /// Invalidates the sessions of **every** user who currently holds
    /// `role_id`, not just one account: a role's grants are shared by every
    /// member, so the B15 "sessions rotate on privilege change" rule
    /// reaches all of them when the role's own grants change.
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Role` (see [`revoke_direct`](Self::revoke_direct)'s doc for why this
    /// crate must check it itself): changing what a role grants is a
    /// change to that role's own record, not any one member's.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit roles, or otherwise as
    /// [`grant_plugin_permission_to_user`](Self::grant_plugin_permission_to_user).
    pub fn grant_plugin_permission_to_role(
        &self,
        auth: &AuthenticatedUser,
        role_id: RoleId,
        name: &PluginPermissionName,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::Role)?;
        let conn = self.lock();
        let permission_id = plugin_permission_id_for_grant(&conn, name)?;
        conn.execute(
            "INSERT OR IGNORE INTO role_plugin_grants (role_id, plugin_permission_id)
             VALUES (?1, ?2)",
            params![role_id, permission_id],
        )?;
        delete_all_sessions_for_role(&conn, role_id)?;
        Ok(())
    }

    /// Revokes the plugin permission `name` from `role_id` — the inverse of
    /// [`grant_plugin_permission_to_role`](Self::grant_plugin_permission_to_role).
    ///
    /// Requires `auth` to hold `Action::Edit` on `Resource::Role` (see [`grant_plugin_permission_to_role`](Self::grant_plugin_permission_to_role)'s
    /// doc).
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while `auth`'s own account is
    /// behind the B4 fence, [`IdentityError::Forbidden`] if `auth` may not
    /// edit roles, or otherwise as [`IdentityError::Database`].
    pub fn revoke_plugin_permission_from_role(
        &self,
        auth: &AuthenticatedUser,
        role_id: RoleId,
        name: &PluginPermissionName,
    ) -> Result<(), IdentityError> {
        auth.authorize(Action::Edit, Resource::Role)?;
        let conn = self.lock();
        conn.execute(
            "DELETE FROM role_plugin_grants WHERE role_id = ?1 AND plugin_permission_id IN
             (SELECT id FROM plugin_permissions WHERE name = ?2)",
            params![role_id, name.as_str()],
        )?;
        delete_all_sessions_for_role(&conn, role_id)?;
        Ok(())
    }
}

/// Looks up the id of a **registered, non-orphaned** plugin permission named
/// `name`, the precondition both `grant_plugin_permission_to_user` and
/// `grant_plugin_permission_to_role` share (a plugin may register a permission but never grant one, and an orphaned permission is not newly grantable even though it stays attached to whatever already holds it).
fn plugin_permission_id_for_grant(
    conn: &Connection,
    name: &PluginPermissionName,
) -> Result<PluginPermissionId, IdentityError> {
    let row: Option<(PluginPermissionId, bool)> = conn
        .query_row(
            "SELECT id, orphaned FROM plugin_permissions WHERE name = ?1",
            [name.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (id, orphaned) =
        row.ok_or_else(|| IdentityError::PluginPermissionNotFound(name.as_str().to_owned()))?;
    if orphaned {
        return Err(IdentityError::PluginPermissionOrphaned(
            name.as_str().to_owned(),
        ));
    }
    Ok(id)
}

/// Deletes every session belonging to `user_id`. Shared by role/grant
/// changes and the first-run branch of
/// [`IdentityStore::set_password`].
fn delete_all_sessions_for(conn: &Connection, user_id: UserId) -> Result<(), IdentityError> {
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

/// Deletes every session belonging to any user who currently holds
/// `role_id`: a role's grants are shared by every member,
/// so a change to what the role itself grants must rotate every member's
/// sessions, not just one account's.
fn delete_all_sessions_for_role(conn: &Connection, role_id: RoleId) -> Result<(), IdentityError> {
    conn.execute(
        "DELETE FROM sessions WHERE user_id IN
         (SELECT user_id FROM user_roles WHERE role_id = ?1)",
        params![role_id],
    )?;
    Ok(())
}

/// Writes `hash` as `user_id`'s password and rotates sessions shared by [`IdentityStore::set_password`] and
/// [`IdentityStore::set_password_for`] so the two entry points (by email,
/// by `user_id`) cannot drift on the rotation rule.
fn apply_password(
    conn: &Connection,
    user_id: UserId,
    hash: &str,
    current_token: Option<&str>,
) -> Result<(), IdentityError> {
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE id = ?2",
        params![hash, user_id],
    )?;

    match current_token {
        Some(token) => {
            let keep = TokenHash::of(token);
            conn.execute(
                "DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2",
                params![user_id, keep],
            )?;
        }
        None => delete_all_sessions_for(conn, user_id)?,
    }
    Ok(())
}

/// The current time as a Unix timestamp, for the INTEGER timestamp columns
/// `users.created_at`, `sessions.created_at/last_seen_at/expires_at` use.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::{
        ALL_ACTIONS, ALL_RESOURCES, DEFAULT_ADMIN_EMAIL, IdentityStore, SUPERADMIN_ROLE_NAME,
    };
    use senken_core::IanaZone;
    use tempfile::TempDir;

    /// Clears the seeded default admin's B4 fence and returns the
    /// [`crate::AuthenticatedUser`] needed to call `create_user` — this
    /// module's own minimal counterpart to `crate::tests::admin_auth`
    /// (private to that other test module, so not reusable from here).
    const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";
    fn admin_auth(store: &IdentityStore) -> crate::AuthenticatedUser {
        store
            .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD, None)
            .unwrap();
        let (_uid, token) = store
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        store.resolve_session(token.reveal()).unwrap().unwrap()
    }

    #[test]
    fn an_account_that_has_never_chosen_a_zone_reads_back_as_not_yet_chosen() {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let admin = admin_auth(&store);

        assert_eq!(
            store.get_zone(admin.user_id()).unwrap(),
            None,
            "a zone nobody has set must read back as `None`, never an error"
        );
    }

    #[test]
    fn setting_a_zone_and_reading_it_back_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let admin = admin_auth(&store);
        let tokyo = IanaZone::new("Asia/Tokyo").unwrap();

        store.set_zone(admin.user_id(), &tokyo).unwrap();

        assert_eq!(store.get_zone(admin.user_id()).unwrap(), Some(tokyo));
    }

    /// Two different accounts' zones must never bleed into each other —
    /// the property the API's `GET`/`PUT /api/me/zone` handlers rely on
    /// being true at this layer, since they derive `user_id` from the
    /// caller's own resolved session and nothing else.
    #[test]
    fn two_accounts_display_zones_are_independent() {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let admin = admin_auth(&store);
        let other = store
            .create_user(&admin, "other@example.com", "Other User", None)
            .unwrap();

        let new_york = IanaZone::new("America/New_York").unwrap();
        let jakarta = IanaZone::new("Asia/Jakarta").unwrap();
        store.set_zone(admin.user_id(), &new_york).unwrap();
        store.set_zone(other, &jakarta).unwrap();

        assert_eq!(store.get_zone(admin.user_id()).unwrap(), Some(new_york));
        assert_eq!(
            store.get_zone(other).unwrap(),
            Some(jakarta),
            "setting the admin's zone must not have overwritten the other account's"
        );
    }

    #[test]
    fn getting_or_setting_the_zone_of_an_unknown_user_id_is_user_not_found() {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let unknown = crate::UserId::new();

        assert!(matches!(
            store.get_zone(unknown),
            Err(crate::IdentityError::UserNotFound)
        ));
        assert!(matches!(
            store.set_zone(unknown, &IanaZone::utc()),
            Err(crate::IdentityError::UserNotFound)
        ));
    }

    /// `set_zone` only ever writes a string [`IanaZone::new`] already
    /// accepted, so the only way `display_zone` holds something that no
    /// longer parses is a row written by hand or by an incompatible
    /// version of this crate — exactly the scenario `CorruptZone` exists
    /// to report rather than silently guess at.
    #[test]
    fn a_display_zone_the_bundled_database_no_longer_recognises_is_reported_not_guessed_at() {
        let dir = TempDir::new().unwrap();
        let store = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let admin = admin_auth(&store);

        {
            let conn = store.lock();
            conn.execute(
                "UPDATE users SET display_zone = 'Not/AZone' WHERE id = ?1",
                [admin.user_id()],
            )
            .unwrap();
        }

        let err = store.get_zone(admin.user_id()).unwrap_err();
        assert!(matches!(err, crate::IdentityError::CorruptZone(_)));
    }

    fn grant_count(store: &IdentityStore) -> i64 {
        let conn = store.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM role_grants g
             JOIN roles r ON r.id = g.role_id
             WHERE r.builtin = 1 AND r.name = ?1",
            [SUPERADMIN_ROLE_NAME],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Adding a `Resource` variant used to be a silent, delayed failure: the
    /// closed enum forces its authorisation to be written and a *new*
    /// database grants it, but every database that already existed kept the
    /// grants it was seeded with — so a long-running install was simply told
    /// it had no permission, with nothing in the code looking wrong.
    #[test]
    fn a_grant_missing_from_the_builtin_role_is_restored_on_the_next_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");

        let expected = {
            let store = IdentityStore::open(&path).unwrap();
            let total = grant_count(&store);
            assert_eq!(
                total,
                i64::try_from(ALL_ACTIONS.len() * ALL_RESOURCES.len()).unwrap(),
                "the built-in role is seeded with every action on every resource"
            );
            // Exactly what an older database looks like after a resource is
            // added to the enum: the row for it was never written.
            let conn = store.lock();
            conn.execute(
                "DELETE FROM role_grants WHERE resource IN ('watchlist', 'note', 'indicator')",
                [],
            )
            .unwrap();
            total
        };

        let store = IdentityStore::open(&path).unwrap();
        assert_eq!(
            grant_count(&store),
            expected,
            "reopening must give the built-in role back every grant its own description promises"
        );
    }

    /// The reconciliation is insertions only, and only for the built-in role:
    /// a role an administrator authored is theirs, and a grant deliberately
    /// removed from it must stay removed.
    #[test]
    fn a_custom_roles_grants_are_never_rewritten() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("accounts.db");
        {
            let store = IdentityStore::open(&path).unwrap();
            let conn = store.lock();
            conn.execute(
                "INSERT INTO roles (id, name, description, builtin) VALUES ('role-x', 'Analyst', '', 0)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO role_grants (role_id, action, resource, scope)
                 VALUES ('role-x', 'view', 'chart_workspace', 'own')",
                [],
            )
            .unwrap();
        }

        let store = IdentityStore::open(&path).unwrap();
        let conn = store.lock();
        let custom: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM role_grants WHERE role_id = 'role-x'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            custom, 1,
            "a custom role keeps exactly the grants it was given"
        );
    }
}
