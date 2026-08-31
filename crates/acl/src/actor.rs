//! Who is asking: a bundle of roles and direct grants, resolved and ready to
//! check.

use crate::action::Action;
use crate::grant::Grant;
use crate::resource::Resource;
use crate::role::Role;
use crate::scope::Scope;

/// Everything [`crate::decide`] needs to know about the party requesting
/// access: the roles it holds, and any grants attached to it directly (plan
/// 004 B5 — "a user has roles, and may also hold direct grants").
///
/// `Actor` carries no identifier (no user id, no email). The scope
/// [`crate::decide`] returns tells a storage layer *how far* an allowed
/// action reaches; combining that with *who* the actor is, to build the
/// concrete `WHERE owner_id = ?` for `Scope::Own`, is the storage layer's
/// job — it already knows the actor's identity, since it is
/// the thing that resolved a session into this `Actor` in the first place.
/// Keeping identity out of this crate is also what keeps a decision
/// reproducible in a test with no notion of "the current user" at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Actor {
    roles: Vec<Role>,
    direct_grants: Vec<Grant>,
}

impl Actor {
    /// An actor with no roles and no direct grants — permitted to do
    /// nothing until roles or grants are added.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a role the actor holds.
    #[must_use]
    pub fn with_role(mut self, role: Role) -> Self {
        self.roles.push(role);
        self
    }

    /// Adds a grant attached to the actor directly, independent of any role.
    #[must_use]
    pub fn with_direct_grant(mut self, grant: Grant) -> Self {
        self.direct_grants.push(grant);
        self
    }

    /// Every scope granted to this actor, from any role or direct grant,
    /// for `action` on `resource`.
    ///
    /// More than one grant can match (two roles, or a role and a direct
    /// grant); the caller combines them with [`Scope::widen`].
    pub(crate) fn scopes_for(
        &self,
        action: Action,
        resource: Resource,
    ) -> impl Iterator<Item = Scope> + '_ {
        self.roles
            .iter()
            .flat_map(Role::grants)
            .chain(self.direct_grants.iter())
            .filter(move |grant| grant.matches(action, resource))
            .map(|grant| grant.scope)
    }
}

#[cfg(test)]
mod tests {
    use super::Actor;
    use crate::{Action, Grant, Resource, Role, Scope};

    #[test]
    fn a_new_actor_has_no_matching_scopes_for_anything() {
        let actor = Actor::new();
        assert_eq!(actor.scopes_for(Action::View, Resource::Layout).count(), 0);
    }

    #[test]
    fn scopes_for_collects_from_every_role_and_direct_grant() {
        let actor = Actor::new()
            .with_role(Role::new("viewer").with_grant(Grant::new(
                Action::View,
                Resource::Layout,
                Scope::Own,
            )))
            .with_role(Role::new("editor").with_grant(Grant::new(
                Action::Edit,
                Resource::Layout,
                Scope::Own,
            )))
            .with_direct_grant(Grant::new(Action::View, Resource::Layout, Scope::All));

        let scopes: Vec<Scope> = actor.scopes_for(Action::View, Resource::Layout).collect();
        assert_eq!(scopes, vec![Scope::Own, Scope::All]);
    }

    #[test]
    fn scopes_for_ignores_grants_for_a_different_action_or_resource() {
        let actor = Actor::new().with_direct_grant(Grant::new(
            Action::View,
            Resource::Workspace,
            Scope::All,
        ));

        assert_eq!(actor.scopes_for(Action::View, Resource::Layout).count(), 0);
        assert_eq!(
            actor.scopes_for(Action::Edit, Resource::Workspace).count(),
            0
        );
    }
}
