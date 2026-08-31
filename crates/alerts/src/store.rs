//! [`AlertStore`]: the guarded query API for alerts, plus the small,
//! deliberately *un*-guarded surface the evaluation engine itself needs
//! (see [`AlertStore::all_enabled_for_engine`]/[`AlertStore::record_fire`]'s
//! own docs for why).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};
use senken_acl::{Action, Resource, Scope};
use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore, Page, UserId};
use senken_marketdata::InstrumentId;
use senken_series::BarSpec;

use crate::condition::{Comparator, Condition, IndicatorField};
use crate::error::AlertError;
use crate::id::AlertId;
use crate::indicator_spec::IndicatorSpec;

/// One alert row, as read back from a guarded query.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertRecord {
    /// The alert's id.
    pub id: AlertId,
    /// The account that owns this alert.
    pub owner_id: UserId,
    /// The instrument this alert leases from the subscription pool.
    pub instrument: InstrumentId,
    /// The bar timeframe this alert's indicator evaluates over.
    pub timeframe: BarSpec,
    /// The indicator this alert evaluates.
    pub indicator: IndicatorSpec,
    /// The condition checked against the indicator's value each time a bar
    /// closes.
    pub condition: Condition,
    /// Whether this alert is currently being evaluated. A disabled alert is
    /// still a row a user can see and re-enable; it holds no lease and is
    /// skipped by [`AlertStore::all_enabled_for_engine`].
    pub enabled: bool,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to this alert's own fields.
    pub updated_at: i64,
    /// When this alert last fired, if ever — "firing is recording that it
    /// fired" (the scope note; notification delivery is out
    /// of scope).
    pub last_fired_at: Option<i64>,
    /// The indicator value that triggered the most recent fire, if any.
    pub last_fired_value: Option<f64>,
    /// How many times this alert has ever fired.
    pub fire_count: u32,
}

/// Guarded queries over alerts, following exactly the pattern
/// `senken-workspace` established for every read and write
/// takes an [`AuthenticatedUser`], calls
/// [`AuthenticatedUser::authorize`] before touching a row, and turns the
/// returned [`Scope`] into a `WHERE` clause (or, for a single-row
/// operation, a check against that row's owner) — including in every
/// listing's total.
///
/// Shares `senken-identity`'s own SQLite connection
/// ([`IdentityStore::shared_connection`]) rather than opening a second one,
/// for the exact reason `senken-workspace`'s module docs give: alerts
/// reference `users(id)`, so their table lives in the same file
/// `senken-identity` alone owns the `user_version` sequence for.
#[derive(Debug)]
pub struct AlertStore {
    conn: Arc<Mutex<Connection>>,
}

impl AlertStore {
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

    /// Creates a new alert owned by `auth`.
    ///
    /// Refuses to persist an alert whose indicator cannot even be built
    /// (an unknown name, or parameters missing a required field) — the same
    /// "refuse a value that could never be read back" discipline
    /// `senken-workspace` applies to an indicator layer's JSON, taken one
    /// step further here since this crate actually knows how to build the
    /// ten indicators.
    ///
    /// Requires `auth` to hold `Action::Create` on `Resource::Alert`.
    ///
    /// # Errors
    /// [`AlertError::IndicatorSpec`] if `indicator` does not name a known
    /// indicator or its `params` are invalid; [`AlertError::Identity`] if
    /// `auth` may not create an alert; otherwise as [`AlertError::Database`].
    pub fn create_alert(
        &self,
        auth: &AuthenticatedUser,
        instrument: &InstrumentId,
        timeframe: BarSpec,
        indicator: &IndicatorSpec,
        condition: Condition,
    ) -> Result<AlertId, AlertError> {
        auth.authorize(Action::Create, Resource::Alert)?;
        indicator.build()?; // refuse at the door rather than persist junk

        let id = AlertId::new();
        let now = now_unix();
        let (field, comparator, threshold) = encode_condition(condition);
        self.lock().execute(
            "INSERT INTO alerts (
                id, owner_id, instrument, timeframe, indicator_name, indicator_params,
                condition_field, condition_comparator, condition_threshold,
                enabled, created_at, updated_at, last_fired_at, last_fired_value, fire_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?10, NULL, NULL, 0)",
            params![
                id,
                auth.user_id(),
                instrument.as_str(),
                timeframe.to_string(),
                indicator.name,
                indicator.params,
                field,
                comparator,
                threshold,
                now,
            ],
        )?;
        Ok(id)
    }

    /// Lists alerts visible to `auth`, scoped: the
    /// `WHERE` clause is chosen by `auth`'s decided [`Scope`], and
    /// [`Page::total`] is counted under that same clause.
    ///
    /// # Errors
    /// [`AlertError::Identity`] if `auth` may not view alerts at all, or if
    /// `decide` returns a [`Scope`] this crate does not translate to SQL;
    /// otherwise as [`AlertError::Database`].
    pub fn list_alerts(
        &self,
        auth: &AuthenticatedUser,
        limit: u32,
        offset: u32,
    ) -> Result<Page<AlertRecord>, AlertError> {
        let scope = auth.authorize(Action::View, Resource::Alert)?;
        let conn = self.lock();
        let limit = i64::from(limit);
        let offset = i64::from(offset);

        let (total, rows) = match scope {
            Scope::Own => {
                let owner = auth.user_id();
                let total: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM alerts WHERE owner_id = ?1",
                    [owner],
                    |row| row.get(0),
                )?;
                let mut stmt = conn.prepare(&format!(
                    "{SELECT_ALERT} WHERE owner_id = ?1 ORDER BY created_at ASC LIMIT ?2 OFFSET ?3"
                ))?;
                let rows = stmt
                    .query_map(params![owner, limit, offset], row_to_alert)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            Scope::All => {
                let total: i64 =
                    conn.query_row("SELECT COUNT(*) FROM alerts", [], |row| row.get(0))?;
                let mut stmt = conn.prepare(&format!(
                    "{SELECT_ALERT} ORDER BY created_at ASC LIMIT ?1 OFFSET ?2"
                ))?;
                let rows = stmt
                    .query_map(params![limit, offset], row_to_alert)?
                    .collect::<Result<Vec<_>, _>>()?;
                (total, rows)
            }
            // `Scope` is `#[non_exhaustive]` — fail closed for
            // a future variant this crate has not been taught to interpret,
            // the same discipline `senken-workspace` applies.
            _ => return Err(AlertError::Identity(IdentityError::Forbidden)),
        };

        let rows = rows
            .into_iter()
            .map(decode_alert)
            .collect::<Result<_, _>>()?;
        Ok(Page {
            rows,
            total: u64::try_from(total).unwrap_or(0),
        })
    }

    /// Fetches one alert.
    ///
    /// Requires `auth` to hold `Action::View` on `Resource::Alert`, scoped
    /// against this row's owner.
    ///
    /// # Errors
    /// [`AlertError::AlertNotFound`] if `id` does not exist;
    /// [`AlertError::Identity`] if `auth` may not view this alert; otherwise
    /// as [`AlertError::Database`].
    pub fn get_alert(
        &self,
        auth: &AuthenticatedUser,
        id: AlertId,
    ) -> Result<AlertRecord, AlertError> {
        let scope = auth.authorize(Action::View, Resource::Alert)?;
        let conn = self.lock();
        let raw = load_raw(&conn, id)?;
        ensure_scope_allows(scope, raw.owner_id, auth.user_id())?;
        decode_alert(raw)
    }

    /// Deletes an alert.
    ///
    /// Requires `auth` to hold `Action::Delete` on `Resource::Alert`, scoped
    /// against this row's owner.
    ///
    /// # Errors
    /// [`AlertError::AlertNotFound`] if `id` does not exist;
    /// [`AlertError::Identity`] if `auth` may not delete this alert;
    /// otherwise as [`AlertError::Database`].
    pub fn delete_alert(&self, auth: &AuthenticatedUser, id: AlertId) -> Result<(), AlertError> {
        let scope = auth.authorize(Action::Delete, Resource::Alert)?;
        let conn = self.lock();
        let raw = load_raw(&conn, id)?;
        ensure_scope_allows(scope, raw.owner_id, auth.user_id())?;
        conn.execute("DELETE FROM alerts WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Every currently-enabled alert, for the evaluation engine's own
    /// startup/reconciliation loop to lease and run.
    ///
    /// **Deliberately not behind [`AuthenticatedUser`].** Every other method
    /// on this store answers "what can *this caller* see", which is exactly
    /// the question B6/its guarded-query discipline exists to police. This
    /// method answers a different question — "what does the server itself
    /// need to keep running" — asked by the trusted evaluation engine
    /// running inside the server process, never by a caller impersonating a
    /// user. It changes no row's ownership and exposes no alert's contents
    /// to any user who could not already see it through
    /// [`list_alerts`](Self::list_alerts); it only lets the engine find the
    /// rows it must lease. The same reasoning already applies, unremarked,
    /// to `senken-identity`'s own `resolve_session` updating
    /// `sessions.last_seen_at` with no `AuthenticatedUser` in sight — a
    /// system bookkeeping operation, not a data-access decision.
    ///
    /// # Errors
    /// [`AlertError::Database`] on a SQLite failure; a corrupt stored row
    /// (an instrument, timeframe or condition this build no longer parses)
    /// is skipped with a `tracing::warn!` rather than failing the whole
    /// call, since a startup reconciliation loop should still lease every
    /// *other* account's valid alerts.
    pub fn all_enabled_for_engine(&self) -> Result<Vec<AlertRecord>, AlertError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{SELECT_ALERT} WHERE enabled = 1"))?;
        let raw_rows = stmt
            .query_map([], row_to_alert)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(raw_rows
            .into_iter()
            .filter_map(|raw| match decode_alert(raw) {
                Ok(record) => Some(record),
                Err(error) => {
                    tracing::warn!(%error, "skipping a corrupt alert row during engine reconciliation");
                    None
                }
            })
            .collect())
    }

    /// Records that alert `id` fired with `value` at `fired_at` (a Unix
    /// timestamp) — "firing is recording that it fired" (the /// scope note; this crate never sends a notification of any kind).
    ///
    /// **Deliberately not behind [`AuthenticatedUser`]**, for the same
    /// reason [`all_enabled_for_engine`](Self::all_enabled_for_engine) is
    /// not: this is the evaluation engine's own bookkeeping write against a
    /// row it already holds by id (found via that same method), not a
    /// caller-driven mutation. It changes no ownership and grants no one
    /// access to anything.
    ///
    /// # Errors
    /// [`AlertError::AlertNotFound`] if `id` no longer exists (the alert was
    /// deleted between the engine loading it and this bar closing);
    /// otherwise as [`AlertError::Database`].
    pub fn record_fire(&self, id: AlertId, value: f64, fired_at: i64) -> Result<(), AlertError> {
        let conn = self.lock();
        let changed = conn.execute(
            "UPDATE alerts SET last_fired_at = ?1, last_fired_value = ?2, fire_count = fire_count + 1
             WHERE id = ?3",
            params![fired_at, value, id],
        )?;
        if changed == 0 {
            return Err(AlertError::AlertNotFound);
        }
        Ok(())
    }
}

/// The columns every read of an `alerts` row needs, shared by every query
/// above so the column list and `row_to_alert`'s indices can never drift
/// apart from each other.
const SELECT_ALERT: &str = "SELECT id, owner_id, instrument, timeframe, indicator_name, indicator_params, \
     condition_field, condition_comparator, condition_threshold, enabled, created_at, updated_at, \
     last_fired_at, last_fired_value, fire_count FROM alerts";

/// The as-stored shape of one row, before its text/JSON columns are parsed
/// back into [`InstrumentId`]/[`BarSpec`]/[`Condition`] — kept separate from
/// [`AlertRecord`] so a corrupt row can be reported with
/// [`AlertError::CorruptInstrumentId`]/[`AlertError::CorruptTimeframe`]/
/// [`AlertError::CorruptCondition`] rather than a generic SQLite type error.
struct RawAlertRow {
    id: AlertId,
    owner_id: UserId,
    instrument: String,
    timeframe: String,
    indicator_name: String,
    indicator_params: String,
    condition_field: String,
    condition_comparator: String,
    condition_threshold: f64,
    enabled: bool,
    created_at: i64,
    updated_at: i64,
    last_fired_at: Option<i64>,
    last_fired_value: Option<f64>,
    fire_count: u32,
}

fn row_to_alert(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawAlertRow> {
    Ok(RawAlertRow {
        id: row.get(0)?,
        owner_id: row.get(1)?,
        instrument: row.get(2)?,
        timeframe: row.get(3)?,
        indicator_name: row.get(4)?,
        indicator_params: row.get(5)?,
        condition_field: row.get(6)?,
        condition_comparator: row.get(7)?,
        condition_threshold: row.get(8)?,
        enabled: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        last_fired_at: row.get(12)?,
        last_fired_value: row.get(13)?,
        fire_count: row.get(14)?,
    })
}

fn load_raw(conn: &Connection, id: AlertId) -> Result<RawAlertRow, AlertError> {
    conn.query_row(
        &format!("{SELECT_ALERT} WHERE id = ?1"),
        params![id],
        row_to_alert,
    )
    .optional()?
    .ok_or(AlertError::AlertNotFound)
}

fn decode_alert(raw: RawAlertRow) -> Result<AlertRecord, AlertError> {
    let instrument = InstrumentId::parse(&raw.instrument)
        .map_err(|e| AlertError::CorruptInstrumentId(e.to_string()))?;
    let timeframe: BarSpec =
        raw.timeframe
            .parse()
            .map_err(|e: senken_series::ParseBarSpecError| {
                AlertError::CorruptTimeframe(e.to_string())
            })?;
    let condition = decode_condition(
        &raw.condition_field,
        &raw.condition_comparator,
        raw.condition_threshold,
    )?;
    Ok(AlertRecord {
        id: raw.id,
        owner_id: raw.owner_id,
        instrument,
        timeframe,
        indicator: IndicatorSpec {
            name: raw.indicator_name,
            params: raw.indicator_params,
        },
        condition,
        enabled: raw.enabled,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
        last_fired_at: raw.last_fired_at,
        last_fired_value: raw.last_fired_value,
        fire_count: raw.fire_count,
    })
}

/// Resolves `Scope` against a row's owner — the single-row counterpart to
/// `list_alerts`' `WHERE` clause, identical to `senken-workspace`'s own
/// `ensure_scope_allows`.
fn ensure_scope_allows(scope: Scope, owner: UserId, actor: UserId) -> Result<(), AlertError> {
    match scope {
        Scope::Own if owner == actor => Ok(()),
        Scope::All => Ok(()),
        _ => Err(AlertError::Identity(IdentityError::Forbidden)),
    }
}

fn encode_condition(condition: Condition) -> (&'static str, &'static str, f64) {
    let field = match condition.field {
        IndicatorField::Value => "value",
        IndicatorField::MacdLine => "macd_line",
        IndicatorField::MacdSignal => "macd_signal",
        IndicatorField::MacdHistogram => "macd_histogram",
        IndicatorField::StochasticK => "stochastic_k",
        IndicatorField::StochasticD => "stochastic_d",
        IndicatorField::BollingerUpper => "bollinger_upper",
        IndicatorField::BollingerMiddle => "bollinger_middle",
        IndicatorField::BollingerLower => "bollinger_lower",
    };
    let comparator = match condition.comparator {
        Comparator::GreaterThan => "greater_than",
        Comparator::LessThan => "less_than",
        Comparator::CrossesAbove => "crosses_above",
        Comparator::CrossesBelow => "crosses_below",
    };
    (field, comparator, condition.threshold)
}

fn decode_condition(
    field: &str,
    comparator: &str,
    threshold: f64,
) -> Result<Condition, AlertError> {
    let field = match field {
        "value" => IndicatorField::Value,
        "macd_line" => IndicatorField::MacdLine,
        "macd_signal" => IndicatorField::MacdSignal,
        "macd_histogram" => IndicatorField::MacdHistogram,
        "stochastic_k" => IndicatorField::StochasticK,
        "stochastic_d" => IndicatorField::StochasticD,
        "bollinger_upper" => IndicatorField::BollingerUpper,
        "bollinger_middle" => IndicatorField::BollingerMiddle,
        "bollinger_lower" => IndicatorField::BollingerLower,
        other => {
            return Err(AlertError::CorruptCondition(format!(
                "unknown condition_field `{other}`"
            )));
        }
    };
    let comparator = match comparator {
        "greater_than" => Comparator::GreaterThan,
        "less_than" => Comparator::LessThan,
        "crosses_above" => Comparator::CrossesAbove,
        "crosses_below" => Comparator::CrossesBelow,
        other => {
            return Err(AlertError::CorruptCondition(format!(
                "unknown condition_comparator `{other}`"
            )));
        }
    };
    Ok(Condition {
        field,
        comparator,
        threshold,
    })
}

/// The current time as a Unix timestamp, for `created_at`/`updated_at` — a
/// direct wall-clock read, exactly like `senken-workspace`'s own
/// `now_unix`. This is account/administrative bookkeeping, not the
/// market-data or replay path reserved for `senken_series::Clock`
/// (see this crate's evaluation path — `TickBarBuilder`/`AlertEvaluator` —
/// which reads no clock of its own at all, only timestamps already carried
/// on a `PriceUpdate`/`Bar`).
fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
mod tests {
    use super::{AlertError, AlertStore};
    use crate::condition::{Comparator, Condition, IndicatorField};
    use crate::indicator_spec::IndicatorSpec;
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::{AuthenticatedUser, IdentityError, IdentityStore};
    use senken_marketdata::InstrumentId;
    use senken_series::{BarSpec, BarUnit};
    use tempfile::TempDir;

    fn temp_stores() -> (TempDir, IdentityStore, AlertStore) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let alerts = AlertStore::new(&identity);
        (dir, identity, alerts)
    }

    const ADMIN_TEST_PASSWORD: &str = "correct horse battery staple";

    /// Sets the seeded default admin's password, logs in, and resolves the
    /// session — identical to `senken-workspace`'s own `admin_auth`.
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

    /// Creates an ordinary account with exactly the grants a real "Alerts
    /// User" role would carry — View/Create/Delete on Alert at
    /// `Scope::Own` — since a freshly created account otherwise holds no
    /// grants at all.
    fn alerts_user(
        identity: &IdentityStore,
        admin: &AuthenticatedUser,
        email: &str,
    ) -> AuthenticatedUser {
        let user_id = identity
            .create_user(admin, email, "Alerts User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Alert, Scope::Own),
                )
                .unwrap();
        }
        let (_uid, token) = identity.login(email, "a very long password").unwrap();
        identity.resolve_session(token.reveal()).unwrap().unwrap()
    }

    fn btcusdt() -> InstrumentId {
        InstrumentId::parse("binance-spot:BTCUSDT").unwrap()
    }

    fn rsi_above_70() -> (IndicatorSpec, Condition) {
        (
            IndicatorSpec {
                name: "Rsi".to_owned(),
                params: r#"{"period":14}"#.to_owned(),
            },
            Condition {
                field: IndicatorField::Value,
                comparator: Comparator::GreaterThan,
                threshold: 70.0,
            },
        )
    }

    // --- B6/B7: scope reaches the query, including the total -----------

    #[test]
    fn two_users_cannot_see_each_others_alerts_and_the_total_respects_scope_too() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice@example.com");
        let bob = alerts_user(&identity, &admin, "bob@example.com");
        let (indicator, condition) = rsi_above_70();

        alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();
        alerts
            .create_alert(
                &bob,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();
        alerts
            .create_alert(
                &bob,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();

        let alice_page = alerts.list_alerts(&alice, 50, 0).unwrap();
        assert_eq!(alice_page.rows.len(), 1);
        assert_eq!(
            alice_page.total, 1,
            "the total must respect scope too — otherwise pagination leaks how many alerts exist"
        );

        let bob_page = alerts.list_alerts(&bob, 50, 0).unwrap();
        assert_eq!(bob_page.rows.len(), 2);
        assert_eq!(bob_page.total, 2);
        assert!(
            bob_page.rows.iter().all(|a| a.owner_id == bob.user_id()),
            "bob must never see alice's alert"
        );
    }

    #[test]
    fn a_superadmin_sees_every_users_alerts() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice2@example.com");
        let bob = alerts_user(&identity, &admin, "bob2@example.com");
        let (indicator, condition) = rsi_above_70();
        alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();
        alerts
            .create_alert(
                &bob,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();

        let page = alerts.list_alerts(&admin, 50, 0).unwrap();
        assert_eq!(page.total, 2, "the superadmin must see both users' alerts");
        assert_eq!(page.rows.len(), 2);
    }

    // --- headless caller refused by the store itself --------------------

    #[test]
    fn a_headless_caller_without_the_create_alert_grant_is_refused_by_the_store_itself() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        identity
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
        let (indicator, condition) = rsi_above_70();

        let err = alerts
            .create_alert(
                &powerless,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap_err();
        assert!(
            matches!(err, AlertError::Identity(IdentityError::Forbidden)),
            "no HTTP layer is involved in this test at all — the store must refuse this itself"
        );
    }

    #[test]
    fn a_headless_caller_without_the_view_alert_grant_cannot_list_via_the_store() {
        let (_dir, identity, alerts) = temp_stores();
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

        let err = alerts.list_alerts(&powerless, 10, 0).unwrap_err();
        assert!(matches!(
            err,
            AlertError::Identity(IdentityError::Forbidden)
        ));
    }

    // --- creation refuses a junk indicator spec -------------------------

    #[test]
    fn creating_an_alert_with_an_unknown_indicator_is_refused_before_it_is_ever_persisted() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice3@example.com");

        let err = alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &IndicatorSpec {
                    name: "NotReal".to_owned(),
                    params: "{}".to_owned(),
                },
                Condition {
                    field: IndicatorField::Value,
                    comparator: Comparator::GreaterThan,
                    threshold: 1.0,
                },
            )
            .unwrap_err();
        assert!(matches!(err, AlertError::IndicatorSpec(_)));
        assert_eq!(alerts.list_alerts(&alice, 10, 0).unwrap().total, 0);
    }

    // --- delete ----------------------------------------------------------

    #[test]
    fn deleting_an_alert_removes_it_and_another_user_cannot_delete_it() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice4@example.com");
        let bob = alerts_user(&identity, &admin, "bob4@example.com");
        let (indicator, condition) = rsi_above_70();
        let id = alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();

        let err = alerts.delete_alert(&bob, id).unwrap_err();
        assert!(matches!(
            err,
            AlertError::Identity(IdentityError::Forbidden)
        ));

        alerts.delete_alert(&alice, id).unwrap();
        assert_eq!(alerts.list_alerts(&alice, 10, 0).unwrap().total, 0);
    }

    // --- the engine's own surface ----------------------------------------

    #[test]
    fn all_enabled_for_engine_sees_every_users_alerts_with_no_authenticated_user_at_all() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice5@example.com");
        let bob = alerts_user(&identity, &admin, "bob5@example.com");
        let (indicator, condition) = rsi_above_70();
        alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();
        alerts
            .create_alert(
                &bob,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();

        let engine_view = alerts.all_enabled_for_engine().unwrap();
        assert_eq!(
            engine_view.len(),
            2,
            "the engine must see every account's enabled alerts"
        );
    }

    #[test]
    fn record_fire_updates_the_row_and_shows_up_through_the_normal_guarded_read() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice6@example.com");
        let (indicator, condition) = rsi_above_70();
        let id = alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();

        let before = alerts.get_alert(&alice, id).unwrap();
        assert_eq!(before.last_fired_at, None);
        assert_eq!(before.fire_count, 0);

        alerts.record_fire(id, 74.5, 1_700_000_000).unwrap();

        let after = alerts.get_alert(&alice, id).unwrap();
        assert_eq!(after.last_fired_at, Some(1_700_000_000));
        assert_eq!(after.last_fired_value, Some(74.5));
        assert_eq!(after.fire_count, 1);
    }

    #[test]
    fn record_fire_on_a_deleted_alert_is_reported_not_silently_ignored() {
        let (_dir, identity, alerts) = temp_stores();
        let admin = admin_auth(&identity);
        let alice = alerts_user(&identity, &admin, "alice7@example.com");
        let (indicator, condition) = rsi_above_70();
        let id = alerts
            .create_alert(
                &alice,
                &btcusdt(),
                BarSpec::new(1, BarUnit::Hour),
                &indicator,
                condition,
            )
            .unwrap();
        alerts.delete_alert(&alice, id).unwrap();

        let err = alerts.record_fire(id, 1.0, 1).unwrap_err();
        assert!(matches!(err, AlertError::AlertNotFound));
    }
}
