//! [`TradeAccountStore`]: the guarded query API for the broker and exchange
//! accounts a user has attached.
//!
//! Follows `senken-watchlist`'s and `senken-chart`'s pattern exactly —
//! every read and write takes a [`AuthenticatedUser`], calls
//! [`AuthenticatedUser::authorize`] before touching a row, and turns the
//! [`Scope`] that comes back into a `WHERE` clause, including in every
//! listing's total. It shares `senken-identity`'s own connection rather
//! than opening a second database, for the reasons that crate's own module
//! docs give.
//!
//! # Two rules this store adds that the others do not have
//!
//! Both exist because this is the store that holds credentials and decides
//! who may spend money.
//!
//! **A credential is only ever loaded for the account's own owner.**
//! `Scope::All` widens what an operator can *see* — that an account exists,
//! whose it is, what it is called — and does not widen what they can read
//! inside it. [`settings_for`](TradeAccountStore::settings_for) is the only
//! method that returns settings at all, and it checks ownership directly
//! rather than consulting a scope.
//!
//! **Trading is owner-only, whatever the role says.**
//! [`account_for_trading`](TradeAccountStore::account_for_trading) is the
//! one door an order goes through, and it refuses an account the caller
//! does not own even for an actor holding `Order`/`All`. An administrator
//! can manage this platform; that is a different thing from being able to
//! spend other people's money with it, and no role should be able to
//! confuse the two.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};

use crate::error::TradeError;
use crate::id::TradeAccountId;
use crate::settings::{SettingsInput, SettingsSchema, SettingsValues};

/// An attached account, as a guarded listing reports it.
///
/// Carries no settings: see this module's docs — a credential is loaded
/// through one method, for one caller, and never as part of a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeAccountSummary {
    /// The account's id.
    pub id: TradeAccountId,
    /// Who attached it.
    pub owner_id: UserId,
    /// The adapter it trades through.
    pub adapter_id: String,
    /// The label its owner gave it.
    pub label: String,
    /// Whether it may be used. A disabled account still lists, so a user
    /// can see it and turn it back on.
    pub enabled: bool,
    /// Unix timestamp of attachment.
    pub created_at: i64,
    /// Unix timestamp of the last change.
    pub updated_at: i64,
}

/// Guarded queries over attached trade accounts.
#[derive(Debug)]
pub struct TradeAccountStore {
    conn: Arc<Mutex<Connection>>,
}

impl TradeAccountStore {
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

    /// Lists the accounts `auth` may view, scoped: the `WHERE` clause and
    /// the total both come from `auth`'s decided [`Scope`] for
    /// `(Action::View, Resource::Account)`.
    ///
    /// # Errors
    /// [`TradeError::Identity`] if `auth` may not view accounts, or the
    /// scope is one this crate cannot express as SQL; otherwise
    /// [`TradeError::Database`].
    pub fn list_accounts(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<TradeAccountSummary>, TradeError> {
        const COLUMNS: &str = "id, owner_id, adapter_id, label, enabled, created_at, updated_at";

        let scope = auth.authorize(Action::View, Resource::Account)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM trade_accounts WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM trade_accounts WHERE owner_id = ?1
                     ORDER BY created_at ASC, label ASC LIMIT ?2 OFFSET ?3"
                ))?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_account)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM trade_accounts", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(&format!(
                    "SELECT {COLUMNS} FROM trade_accounts
                     ORDER BY created_at ASC, label ASC LIMIT ?1 OFFSET ?2"
                ))?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_account)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]`: a variant this crate has not
            // been taught to express must fail closed, never widen to an
            // unfiltered query.
            _ => return Err(TradeError::Identity(IdentityError::Forbidden)),
        };

        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Attaches a new account for `auth`, validating `settings` against
    /// `schema` first.
    ///
    /// Takes the raw submission rather than typed values: validation is
    /// this store's job and happens on every write, so there is no shape a
    /// caller could hand over that skipped it.
    ///
    /// Requires `Action::Create` on `Resource::Account`. The account is
    /// always owned by the caller: there is no parameter to attach one on
    /// someone else's behalf, because the settings it would carry are that
    /// person's credentials.
    ///
    /// # Errors
    /// [`TradeError::Settings`] when the settings do not fit the schema,
    /// [`TradeError::DuplicateLabel`] when the caller already has an
    /// account by this label on this adapter, [`TradeError::Identity`] when
    /// the caller may not create accounts.
    pub fn create_account(
        &self,
        auth: &AuthenticatedUser,
        adapter_id: &str,
        label: &str,
        schema: &SettingsSchema,
        settings: &SettingsInput,
    ) -> Result<TradeAccountId, TradeError> {
        auth.authorize(Action::Create, Resource::Account)?;
        let label = label.trim();
        if label.is_empty() {
            return Err(TradeError::invalid("an account needs a name"));
        }
        let settings = schema.validate(settings)?;
        let encoded = settings.to_storage_json()?;

        let conn = self.lock();
        let id = TradeAccountId::new();
        let now = now_unix();
        conn.execute(
            "INSERT INTO trade_accounts
                 (id, owner_id, adapter_id, label, settings, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
            params![id, auth.user_id(), adapter_id, label, encoded, now],
        )
        .map_err(duplicate_label_or)?;
        Ok(id)
    }

    /// One account's summary, if `auth` may view it.
    ///
    /// # Errors
    /// [`TradeError::UnknownAccount`] when it does not exist or the
    /// caller's scope does not reach it.
    pub fn account(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
    ) -> Result<TradeAccountSummary, TradeError> {
        let scope = auth.authorize(Action::View, Resource::Account)?;
        let conn = self.lock();
        let account = load_account(&conn, id)?;
        if scope != Scope::All && account.owner_id != auth.user_id() {
            // Reported as "not found" rather than "forbidden": whether an
            // account someone else owns exists is itself not this caller's
            // to learn.
            return Err(TradeError::UnknownAccount);
        }
        Ok(account)
    }

    /// The account plus its settings, **for its own owner only**.
    ///
    /// This is the one method that returns credentials, and it does not
    /// consult a scope: an operator holding `Account`/`All` can see that
    /// this account exists and manage it, and still cannot read the API key
    /// inside it. Broker credentials are per user and are not shared by any
    /// role.
    ///
    /// # Errors
    /// [`TradeError::UnknownAccount`] when it does not exist or the caller
    /// does not own it.
    pub fn settings_for(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
    ) -> Result<(TradeAccountSummary, SettingsValues), TradeError> {
        auth.authorize(Action::View, Resource::Account)?;
        let conn = self.lock();
        let account = load_account(&conn, id)?;
        if account.owner_id != auth.user_id() {
            return Err(TradeError::UnknownAccount);
        }
        let raw: String = conn.query_row(
            "SELECT settings FROM trade_accounts WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok((account, SettingsValues::from_storage_json(&raw)?))
    }

    /// The account an order is about to be sent through.
    ///
    /// **Owner-only by design, whatever grants the caller holds.** An
    /// account that is disabled is refused here too — that is what the
    /// switch is for.
    ///
    /// # Errors
    /// [`TradeError::UnknownAccount`] when it does not exist or is not the
    /// caller's, [`TradeError::AccountDisabled`] when its owner has turned
    /// it off, [`TradeError::Identity`] when the caller may not trade at
    /// all.
    pub fn account_for_trading(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
    ) -> Result<(TradeAccountSummary, SettingsValues), TradeError> {
        auth.authorize(Action::Create, Resource::Order)?;
        let (account, settings) = self.settings_for(auth, id)?;
        if !account.enabled {
            return Err(TradeError::AccountDisabled);
        }
        Ok((account, settings))
    }

    /// Renames an account, or turns it on and off.
    ///
    /// Requires `Action::Edit` on `Resource::Account`, with the returned
    /// scope applied against this account's owner.
    ///
    /// # Errors
    /// As [`account`](Self::account), plus
    /// [`TradeError::DuplicateLabel`].
    pub fn update_account(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
        label: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<(), TradeError> {
        let scope = auth.authorize(Action::Edit, Resource::Account)?;
        let conn = self.lock();
        let account = load_account(&conn, id)?;
        ensure_scope_allows(scope, account.owner_id, auth.user_id())?;

        let label = match label {
            Some(label) => {
                let label = label.trim();
                if label.is_empty() {
                    return Err(TradeError::invalid("an account needs a name"));
                }
                label.to_owned()
            }
            None => account.label,
        };
        let enabled = enabled.unwrap_or(account.enabled);
        conn.execute(
            "UPDATE trade_accounts SET label = ?1, enabled = ?2, updated_at = ?3 WHERE id = ?4",
            params![label, enabled, now_unix(), id],
        )
        .map_err(duplicate_label_or)?;
        Ok(())
    }

    /// Replaces an account's settings, **for its own owner only** and after
    /// validating them against `schema`.
    ///
    /// Owner-only for the same reason [`settings_for`](Self::settings_for)
    /// is: this writes credentials.
    ///
    /// # Errors
    /// [`TradeError::UnknownAccount`] when the caller does not own it,
    /// [`TradeError::Settings`] when the values do not fit the schema.
    pub fn replace_settings(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
        schema: &SettingsSchema,
        settings: SettingsInput,
    ) -> Result<SettingsValues, TradeError> {
        auth.authorize(Action::Edit, Resource::Account)?;
        let (_, previous) = self.settings_for(auth, id)?;
        // A form the client rendered never contained the stored
        // credentials, so a submission that leaves them blank means "keep
        // them", not "erase them" — and a required credential already on
        // file must satisfy the schema without being re-typed, which is why
        // this happens before validation rather than after it.
        let validated = schema.validate(&settings.carry_secrets_from(&previous, schema))?;
        let encoded = validated.to_storage_json()?;

        let conn = self.lock();
        conn.execute(
            "UPDATE trade_accounts SET settings = ?1, updated_at = ?2 WHERE id = ?3",
            params![encoded, now_unix(), id],
        )?;
        Ok(validated)
    }

    /// Detaches an account.
    ///
    /// # Errors
    /// As [`account`](Self::account), with `Action::Delete`.
    pub fn delete_account(
        &self,
        auth: &AuthenticatedUser,
        id: TradeAccountId,
    ) -> Result<(), TradeError> {
        let scope = auth.authorize(Action::Delete, Resource::Account)?;
        let conn = self.lock();
        let account = load_account(&conn, id)?;
        ensure_scope_allows(scope, account.owner_id, auth.user_id())?;
        conn.execute("DELETE FROM trade_accounts WHERE id = ?1", params![id])?;
        Ok(())
    }
}

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<TradeAccountSummary> {
    Ok(TradeAccountSummary {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        adapter_id: row.get(2)?,
        label: row.get(3)?,
        enabled: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn load_account(conn: &Connection, id: TradeAccountId) -> Result<TradeAccountSummary, TradeError> {
    conn.query_row(
        "SELECT id, owner_id, adapter_id, label, enabled, created_at, updated_at
         FROM trade_accounts WHERE id = ?1",
        params![id],
        row_to_account,
    )
    .optional()?
    .ok_or(TradeError::UnknownAccount)
}

/// Applies a decided scope against one row's owner.
fn ensure_scope_allows(scope: Scope, owner: UserId, caller: UserId) -> Result<(), TradeError> {
    match scope {
        Scope::All => Ok(()),
        Scope::Own if owner == caller => Ok(()),
        // Not "forbidden": an account someone else owns is not this
        // caller's to learn the existence of.
        Scope::Own => Err(TradeError::UnknownAccount),
        _ => Err(TradeError::Identity(IdentityError::Forbidden)),
    }
}

/// Turns the one constraint this table has into a named error, leaving
/// every other SQLite failure alone.
fn duplicate_label_or(error: rusqlite::Error) -> TradeError {
    if let rusqlite::Error::SqliteFailure(inner, _) = &error
        && inner.code == rusqlite::ErrorCode::ConstraintViolation
    {
        return TradeError::DuplicateLabel;
    }
    TradeError::Database(error)
}

/// Seconds since the Unix epoch.
///
/// The `created_at`/`updated_at` columns of every other table in this
/// database are seconds, so these are too — a mixed-unit column set is the
/// exact confusion `UnixNanos` exists to prevent elsewhere, and the fix
/// here is consistency with the neighbours rather than a different unit in
/// one table.
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityStore};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{TradeAccountStore, TradeError};
    use crate::settings::{FieldKind, SecretString, SettingField, SettingsInput, SettingsSchema};

    const TEST_PASSWORD: &str = "correct horse battery staple";

    fn temp_stores() -> (TempDir, IdentityStore, TradeAccountStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let accounts = TradeAccountStore::new(&identity);
        (dir, identity, accounts)
    }

    /// The seeded `Superadmin`, who holds every `(Action, Resource)` pair
    /// at `Scope::All` — the actor the owner-only rules below have to hold
    /// against.
    fn admin_auth(identity: &IdentityStore) -> AuthenticatedUser {
        identity
            .set_password(senken_identity::DEFAULT_ADMIN_EMAIL, TEST_PASSWORD, None)
            .unwrap();
        let (_uid, token) = identity
            .login(senken_identity::DEFAULT_ADMIN_EMAIL, TEST_PASSWORD)
            .unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    /// An ordinary trader: `Account` and `Order` at `Scope::Own`, which is
    /// what a real "Trader" role would carry.
    fn trader(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Trader", Some(TEST_PASSWORD))
            .unwrap();
        for resource in [Resource::Account, Resource::Order] {
            for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
                identity
                    .grant_direct(admin, user_id, Grant::new(action, resource, Scope::Own))
                    .unwrap();
            }
        }
        let (_uid, token) = identity.login(email, TEST_PASSWORD).unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    fn schema() -> SettingsSchema {
        SettingsSchema::new(vec![
            SettingField::new(
                "api_key",
                "API key",
                FieldKind::Secret {
                    placeholder: String::new(),
                },
            ),
            SettingField::new(
                "leverage",
                "Leverage",
                FieldKind::Number {
                    default: Some(1),
                    min: 1,
                    max: 100,
                    unit: "x".to_owned(),
                },
            ),
        ])
    }

    fn settings_with_key(key: &str) -> SettingsInput {
        SettingsInput::new().with("api_key", json!(key))
    }

    #[test]
    fn an_attached_account_is_listed_to_its_owner() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");

        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Growth",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        let page = accounts.list_accounts(&alice, 20, 0).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows[0].id, id);
        assert_eq!(page.rows[0].label, "Growth");
        assert!(page.rows[0].enabled);
    }

    #[test]
    fn an_owner_scoped_listing_counts_only_its_own_rows_in_the_total_too() {
        // Scope has to reach the `WHERE` clause and the count together:
        // hiding the rows while still reporting "1-1 of 2" leaks that
        // someone else's account exists.
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let bob = trader(&identity, &admin, "bob@example.com");

        accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("a"),
            )
            .unwrap();
        accounts
            .create_account(&bob, "simulator", "Bob", &schema(), &settings_with_key("b"))
            .unwrap();

        let page = accounts.list_accounts(&alice, 20, 0).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(
            page.total, 1,
            "the total must be counted under the same scope"
        );
    }

    #[test]
    fn an_admin_at_scope_all_sees_that_every_account_exists() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("a"),
            )
            .unwrap();

        let page = accounts.list_accounts(&admin, 20, 0).unwrap();
        assert_eq!(page.total, 1);
    }

    #[test]
    fn an_admin_at_scope_all_still_cannot_read_someone_elses_credentials() {
        // The rule this store exists to hold: `Scope::All` widens what an
        // operator can see about an account, never what they can read
        // inside it. Broker credentials are per user and no role shares
        // them.
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("sk-live-alice"),
            )
            .unwrap();

        assert!(
            accounts.account(&admin, id).is_ok(),
            "an operator may still see that the account exists"
        );
        let error = accounts.settings_for(&admin, id).unwrap_err();
        assert!(matches!(error, TradeError::UnknownAccount), "got {error:?}");
    }

    #[test]
    fn an_admin_at_scope_all_cannot_trade_on_someone_elses_account() {
        // Managing the platform and spending other people's money are
        // different authorities. Nothing a role can grant merges them.
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        let error = accounts.account_for_trading(&admin, id).unwrap_err();

        assert!(matches!(error, TradeError::UnknownAccount), "got {error:?}");
    }

    #[test]
    fn an_owner_can_trade_on_their_own_account() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        let (account, settings) = accounts.account_for_trading(&alice, id).unwrap();

        assert_eq!(account.id, id);
        assert_eq!(
            settings.secret("api_key").map(SecretString::expose),
            Some("k")
        );
    }

    #[test]
    fn a_disabled_account_refuses_to_trade_while_still_listing() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        accounts
            .update_account(&alice, id, None, Some(false))
            .unwrap();

        assert!(
            matches!(
                accounts.account_for_trading(&alice, id).unwrap_err(),
                TradeError::AccountDisabled
            ),
            "a switched-off account must refuse orders"
        );
        assert_eq!(
            accounts.list_accounts(&alice, 20, 0).unwrap().total,
            1,
            "and must still be visible, so its owner can switch it back on"
        );
    }

    #[test]
    fn a_user_without_the_order_grant_cannot_trade_their_own_account_either() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        // Only `Account`, deliberately: the account exists and is theirs,
        // but placing orders is a grant of its own.
        let user_id = identity
            .create_user(&admin, "viewer@example.com", "Viewer", Some(TEST_PASSWORD))
            .unwrap();
        for action in [Action::View, Action::Create] {
            identity
                .grant_direct(
                    &admin,
                    user_id,
                    Grant::new(action, Resource::Account, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login("viewer@example.com", TEST_PASSWORD).unwrap();
        let viewer = identity.resolve_session(token.reveal()).unwrap().unwrap();

        let id = accounts
            .create_account(
                &viewer,
                "simulator",
                "Mine",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        let error = accounts.account_for_trading(&viewer, id).unwrap_err();
        assert!(
            matches!(error, TradeError::Identity(_)),
            "viewing a portfolio and trading it are separate grants; got {error:?}"
        );
    }

    #[test]
    fn a_second_account_with_the_same_name_on_one_adapter_is_refused() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        let error = accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap_err();

        assert!(matches!(error, TradeError::DuplicateLabel), "got {error:?}");
    }

    #[test]
    fn two_users_may_each_call_their_account_the_same_thing() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let bob = trader(&identity, &admin, "bob@example.com");

        accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema(),
                &settings_with_key("a"),
            )
            .unwrap();
        accounts
            .create_account(
                &bob,
                "simulator",
                "Main",
                &schema(),
                &settings_with_key("b"),
            )
            .unwrap();
    }

    #[test]
    fn settings_that_do_not_fit_the_schema_never_reach_the_database() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");

        let error = accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema(),
                &SettingsInput::new(),
            )
            .unwrap_err();

        assert!(matches!(error, TradeError::Settings(_)), "got {error:?}");
        assert_eq!(accounts.list_accounts(&alice, 20, 0).unwrap().total, 0);
    }

    #[test]
    fn editing_settings_without_retyping_the_credential_keeps_it() {
        // The exact round trip a settings dialog performs: it renders from
        // a document the credential was never in, and posts it back.
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let schema = schema();
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema,
                &settings_with_key("sk-original"),
            )
            .unwrap();

        accounts
            .replace_settings(
                &alice,
                id,
                &schema,
                SettingsInput::new().with("leverage", json!(20)),
            )
            .unwrap();

        let (_, stored) = accounts.settings_for(&alice, id).unwrap();
        assert_eq!(
            stored.secret("api_key").map(SecretString::expose),
            Some("sk-original")
        );
        assert_eq!(stored.number("leverage"), Some(20));
    }

    #[test]
    fn an_owner_scoped_actor_cannot_rename_or_delete_someone_elses_account() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let bob = trader(&identity, &admin, "bob@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Alice",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        assert!(matches!(
            accounts
                .update_account(&bob, id, Some("Stolen"), None)
                .unwrap_err(),
            TradeError::UnknownAccount
        ));
        assert!(matches!(
            accounts.delete_account(&bob, id).unwrap_err(),
            TradeError::UnknownAccount
        ));
        assert_eq!(accounts.account(&alice, id).unwrap().label, "Alice");
    }

    #[test]
    fn deleting_an_account_removes_it() {
        let (_dir, identity, accounts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = trader(&identity, &admin, "alice@example.com");
        let id = accounts
            .create_account(
                &alice,
                "simulator",
                "Main",
                &schema(),
                &settings_with_key("k"),
            )
            .unwrap();

        accounts.delete_account(&alice, id).unwrap();

        assert_eq!(accounts.list_accounts(&alice, 20, 0).unwrap().total, 0);
        assert!(matches!(
            accounts.account(&alice, id).unwrap_err(),
            TradeError::UnknownAccount
        ));
    }
}
