//! Turning a resolved session into the `senken_acl::Actor` a permission
//! check needs, and enforcing the B4 password fence in front of it.
//!
//! `senken_acl::Actor`'s own docs describe this crate's job precisely:
//! combining a `Scope` with *who* the actor is, to build a concrete
//! `WHERE owner_id = ?`, is the storage layer's job, "since it is the thing
//! that resolved a session into this `Actor` in the first place." This
//! module is exactly that resolution step.

use std::collections::HashMap;

use rusqlite::Connection;
use senken_acl::{Action, Actor, Grant, Resource, Role, Scope};

use crate::error::IdentityError;
use crate::id::UserId;

/// A session, resolved all the way to the actor behind it (the /// "combining scope with identity is the storage layer's job") plus the
/// one fact that gates *everything* before permissions even enter the
/// picture: whether the account's password is set yet.
///
/// There is no public constructor — the only way to obtain one is
/// [`IdentityStore::resolve_session`](crate::IdentityStore::resolve_session),
/// which is the only code path that has actually checked a session token
/// against the database. This mirrors `senken_acl::Decision`: a value that
/// can only exist because a real check already happened.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    user_id: UserId,
    actor: Actor,
    password_set: bool,
    role_names: Vec<String>,
    effective_grants: Vec<Grant>,
}

impl AuthenticatedUser {
    pub(crate) fn new(
        user_id: UserId,
        actor: Actor,
        password_set: bool,
        role_names: Vec<String>,
        effective_grants: Vec<Grant>,
    ) -> Self {
        Self {
            user_id,
            actor,
            password_set,
            role_names,
            effective_grants,
        }
    }

    /// The id of the account behind this session.
    #[must_use]
    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    /// `true` once this account has set a password — `false` for an
    /// account still behind the B4 first-run fence.
    #[must_use]
    pub fn password_set(&self) -> bool {
        self.password_set
    }

    /// The names of every role this account holds (`GET /api/me` reports these for **cosmetic** use only — B8 still holds, and every endpoint re-checks a real grant regardless of what a client was told here).
    #[must_use]
    pub fn role_names(&self) -> &[String] {
        &self.role_names
    }

    /// This account's effective grants: every `(Action, Resource, Scope)`
    /// this account holds through any role plus any direct grant, collapsed
    /// to one entry per `(Action, Resource)` pair at the most permissive
    /// [`Scope`] among the grants that named it (the same widening
    /// [`senken_acl::decide`] itself performs, replayed here across every
    /// pair rather than one at a time) — the "`MeResponse` must
    /// carry the caller's effective permissions." Like [`role_names`](Self::role_names),
    /// this is a UI convenience, never a substitute for the real check a
    /// guarded query or [`authorize`](Self::authorize) call performs.
    #[must_use]
    pub fn effective_grants(&self) -> &[Grant] {
        &self.effective_grants
    }

    /// Checks `action` on `resource`, enforcing the B4 fence first.
    ///
    /// Every guarded query in [`IdentityStore`](crate::IdentityStore) goes
    /// through this one function to obtain a [`Scope`] to filter its query
    /// by — the same shape as `senken_acl::decide` itself: no other
    /// function in this crate can produce a `Scope`, so a query that
    /// forgets to call this has no scope to filter with at all.
    ///
    /// While the password is unset, every check fails with
    /// [`IdentityError::PasswordNotSet`] regardless of what the actor's
    /// roles or grants say — setting the password is the one operation
    /// exempt from this, and it does not go through `AuthenticatedUser` at
    /// all (there is nothing to scope: a user always sets their own
    /// password).
    ///
    /// Public so `senken-api`'s router-level ACL guard
    /// (`crate::auth::EndpointPermission::Acl` there) can perform the exact
    /// same check `senken-identity`'s own guarded queries use internally,
    /// rather than reimplementing it against `senken_acl::decide` directly
    ///   — which it could not do anyway, since `Actor` is private to this
    /// type.
    ///
    /// # Errors
    /// [`IdentityError::PasswordNotSet`] while the B4 fence is up;
    /// [`IdentityError::Forbidden`] if `senken_acl::decide` denies the
    /// check or returns a [`Scope`] variant this crate does not translate
    /// into SQL (scope must reach the query, which is
    /// impossible for a scope this crate cannot interpret).
    pub fn authorize(&self, action: Action, resource: Resource) -> Result<Scope, IdentityError> {
        if !self.password_set {
            return Err(IdentityError::PasswordNotSet);
        }
        senken_acl::decide(&self.actor, action, resource)
            .scope()
            .ok_or(IdentityError::Forbidden)
    }
}

/// The result of resolving a session all the way down to the actor behind
/// it (this reaches beyond the `Actor` `senken_acl::decide`
/// needs, to also carry what `GET /api/me` reports for cosmetic use: the
/// role names and the effective, widened grants).
pub(crate) struct LoadedActor {
    pub(crate) actor: Actor,
    pub(crate) role_names: Vec<String>,
    pub(crate) effective_grants: Vec<Grant>,
}

/// Loads every role and direct grant `user_id` holds, assembling both the
/// `senken_acl::Actor` a permission check needs and the plain-data summary
/// (role names, widened effective grants) the `MeResponse` needs.
///
/// Widening happens here, not in `senken_acl` (which is not in this
/// milestone's owned paths): grants for the same `(Action, Resource)` pair,
/// whether from two roles or a role and a direct grant, are collapsed to
/// the single most permissive [`Scope`] via [`Scope::widen`] — the same
/// combination `senken_acl::decide` performs per pair, replayed here across
/// every pair the actor holds any grant for, so `GET /api/me` reports one
/// row per `(Action, Resource)` rather than every contributing grant
/// separately.
pub(crate) fn load_actor(conn: &Connection, user_id: UserId) -> Result<LoadedActor, IdentityError> {
    let mut actor = Actor::new();
    let mut role_names = Vec::new();
    let mut widened: HashMap<(Action, Resource), Scope> = HashMap::new();

    let mut role_stmt = conn.prepare(
        "SELECT r.id, r.name FROM roles r
         JOIN user_roles ur ON ur.role_id = r.id
         WHERE ur.user_id = ?1",
    )?;
    let role_rows = role_stmt.query_map([user_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut grant_stmt =
        conn.prepare("SELECT action, resource, scope FROM role_grants WHERE role_id = ?1")?;

    for row in role_rows {
        let (role_id, role_name) = row?;
        role_names.push(role_name.clone());
        let mut role = Role::new(role_name);
        let grants = grant_stmt.query_map([&role_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for grant in grants {
            let (action, resource, scope) = grant?;
            let grant = decode_grant(&action, &resource, &scope)?;
            widen_into(&mut widened, grant);
            role = role.with_grant(grant);
        }
        actor = actor.with_role(role);
    }

    let mut direct_stmt =
        conn.prepare("SELECT action, resource, scope FROM user_grants WHERE user_id = ?1")?;
    let direct_rows = direct_stmt.query_map([user_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for grant in direct_rows {
        let (action, resource, scope) = grant?;
        let grant = decode_grant(&action, &resource, &scope)?;
        widen_into(&mut widened, grant);
        actor = actor.with_direct_grant(grant);
    }

    let effective_grants = widened
        .into_iter()
        .map(|((action, resource), scope)| Grant::new(action, resource, scope))
        .collect();

    Ok(LoadedActor {
        actor,
        role_names,
        effective_grants,
    })
}

/// Folds `grant` into `widened`, keeping the more permissive [`Scope`] when
/// `(action, resource)` already has an entry.
fn widen_into(widened: &mut HashMap<(Action, Resource), Scope>, grant: Grant) {
    widened
        .entry((grant.action, grant.resource))
        .and_modify(|scope| *scope = scope.widen(grant.scope))
        .or_insert(grant.scope);
}

/// Encodes an `(Action, Resource, Scope)` triple as the three TEXT columns
/// `role_grants`/`user_grants` store it in.
///
/// `resource_to_sql` matches every `Resource` variant with **no wildcard
/// arm**, the same discipline `senken_acl::decide` uses:
/// `Resource` is a closed enum, so adding a variant there fails this match
/// too, until this crate is taught the new resource's on-disk spelling.
pub(crate) fn encode_grant(
    grant: Grant,
) -> Result<(&'static str, &'static str, &'static str), IdentityError> {
    Ok((
        action_to_sql(grant.action)?,
        resource_to_sql(grant.resource),
        scope_to_sql(grant.scope)?,
    ))
}

/// Decodes the three TEXT columns `role_grants`/`user_grants` store a grant
/// as, back into a [`Grant`]. `pub(crate)` so
/// [`crate::store::IdentityStore::list_roles`] can decode a role's grants
/// for its response the same way this module already does when assembling
/// an `Actor`.
pub(crate) fn decode_grant(
    action: &str,
    resource: &str,
    scope: &str,
) -> Result<Grant, IdentityError> {
    Ok(Grant::new(
        sql_to_action(action)?,
        sql_to_resource(resource)?,
        sql_to_scope(scope)?,
    ))
}

fn action_to_sql(action: Action) -> Result<&'static str, IdentityError> {
    Ok(match action {
        Action::View => "view",
        Action::Create => "create",
        Action::Edit => "edit",
        Action::Delete => "delete",
        Action::Share => "share",
        // `Action` is `#[non_exhaustive]`: a future variant
        // must be added here deliberately rather than falling through to a
        // guessed spelling.
        other => {
            return Err(IdentityError::CorruptGrant(format!(
                "unmapped action {other:?}"
            )));
        }
    })
}

fn sql_to_action(text: &str) -> Result<Action, IdentityError> {
    Ok(match text {
        "view" => Action::View,
        "create" => Action::Create,
        "edit" => Action::Edit,
        "delete" => Action::Delete,
        "share" => Action::Share,
        other => {
            return Err(IdentityError::CorruptGrant(format!(
                "unknown action `{other}`"
            )));
        }
    })
}

/// `pub(crate)` (not private) so [`crate::store::IdentityStore`]'s
/// resource-backfill can name a resource's on-disk token without a
/// throwaway [`Grant`] to extract it from.
pub(crate) fn resource_to_sql(resource: Resource) -> &'static str {
    match resource {
        // Schema v8 renames these two tokens from `workspace`/`layout` to
        // match `Resource::ChartWorkspace`/`Resource::ChartLayout` — see
        // `schema::migrate_workspace_to_chart_grants`, which rewrites every
        // existing `role_grants`/`user_grants` row still holding the old
        // token so upgrading never silently drops a user's chart
        // permissions.
        Resource::ChartWorkspace => "chart_workspace",
        Resource::ChartLayout => "chart_layout",
        Resource::Alert => "alert",
        Resource::Strategy => "strategy",
        Resource::Account => "account",
        Resource::Adapter => "adapter",
        Resource::User => "user",
        Resource::Role => "role",
        Resource::Indicator => "indicator",
        Resource::Watchlist => "watchlist",
        Resource::Note => "note",
        Resource::Storage => "storage",
    }
}

fn sql_to_resource(text: &str) -> Result<Resource, IdentityError> {
    Ok(match text {
        "chart_workspace" => Resource::ChartWorkspace,
        "chart_layout" => Resource::ChartLayout,
        "alert" => Resource::Alert,
        "strategy" => Resource::Strategy,
        "account" => Resource::Account,
        "adapter" => Resource::Adapter,
        "user" => Resource::User,
        "role" => Resource::Role,
        "indicator" => Resource::Indicator,
        "watchlist" => Resource::Watchlist,
        "note" => Resource::Note,
        "storage" => Resource::Storage,
        other => {
            return Err(IdentityError::CorruptGrant(format!(
                "unknown resource `{other}`"
            )));
        }
    })
}

fn scope_to_sql(scope: Scope) -> Result<&'static str, IdentityError> {
    Ok(match scope {
        Scope::Own => "own",
        Scope::All => "all",
        // `Scope` is `#[non_exhaustive]` (`Team` deliberately absent) — same reasoning as `action_to_sql` above.
        other => {
            return Err(IdentityError::CorruptGrant(format!(
                "unmapped scope {other:?}"
            )));
        }
    })
}

fn sql_to_scope(text: &str) -> Result<Scope, IdentityError> {
    Ok(match text {
        "own" => Scope::Own,
        "all" => Scope::All,
        other => {
            return Err(IdentityError::CorruptGrant(format!(
                "unknown scope `{other}`"
            )));
        }
    })
}
