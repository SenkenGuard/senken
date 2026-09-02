//! The decision function: the one place an authorisation question is
//! answered, and the only way to obtain a [`Decision`].

use crate::action::Action;
use crate::actor::Actor;
use crate::resource::Resource;
use crate::scope::Scope;

/// The outcome of an authorisation check.
///
/// A `Decision` cannot be constructed outside this module — its inner state
/// is private and there is no public constructor — so the only way to have
/// one is to call [`decide`]. This is what makes an unguarded query
/// unrepresentable: there is no way to write code that
/// produces a [`Scope`] to filter a query by without also producing the
/// `Decision` it came from, and no way to produce that `Decision` without
/// actually checking an [`Actor`]'s grants. A denied decision carries no
/// scope at all — [`scope`](Self::scope) returns `None` — so a caller
/// cannot accidentally read a stale or default scope out of a denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision(DecisionState);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionState {
    Allowed(Scope),
    Denied,
}

impl Decision {
    fn allowed(scope: Scope) -> Self {
        Self(DecisionState::Allowed(scope))
    }

    fn denied() -> Self {
        Self(DecisionState::Denied)
    }

    /// `true` when the actor may perform the action that was checked.
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        matches!(self.0, DecisionState::Allowed(_))
    }

    /// The scope a storage layer must apply as a `WHERE` clause, or `None` when the action is denied.
    ///
    /// There is deliberately no separate "was this allowed" check a caller
    /// could pass without also handling scope: reading `scope()` is the
    /// only way to learn whether — and how far — access reaches, so a
    /// storage layer cannot run a query while forgetting to apply what it
    /// returns.
    #[must_use]
    pub fn scope(&self) -> Option<Scope> {
        match self.0 {
            DecisionState::Allowed(scope) => Some(scope),
            DecisionState::Denied => None,
        }
    }
}

/// Decides whether `actor` may perform `action` on `resource`.
///
/// This is the single place a resource's authorisation is answered, and the
/// only function in this crate that can produce a [`Decision`]. The `match`
/// below names every [`Resource`] variant explicitly in its pattern, with
/// **no wildcard arm** — adding a variant to
/// `Resource` fails to compile here until this function says how the new
/// resource is authorised, rather than silently falling through to a
/// default (or, worse, being satisfied by an existing wildcard and never
/// reaching this function at all). See the crate-level docs for the
/// compiler-error experiment that demonstrates this.
///
/// Every variant currently defers to the same grant lookup, because
/// scope-limited grants are, so far, the only authorisation rule this
/// system has — they are combined into one arm (rather than eight
/// identical ones) because repeating the same body per variant is exactly
/// the "identical match arms" smell `clippy::match_same_arms` exists to
/// catch, and combining them changes nothing about the enforcement: the
/// pattern still names every variant, so a new one is still unhandled
/// until it is added here. A resource that needs a *different* rule earns
/// its own arm instead of joining this one — `Adapter` is a candidate,
/// since adapter credentials must never cross users
/// regardless of what a role grants — and splitting it out of the list
/// below is what makes that decision visible at this one call site rather
/// than buried in a condition somewhere else.
#[must_use]
pub fn decide(actor: &Actor, action: Action, resource: Resource) -> Decision {
    match resource {
        Resource::ChartWorkspace
        | Resource::ChartLayout
        | Resource::Alert
        | Resource::Strategy
        | Resource::Account
        | Resource::Order
        | Resource::Adapter
        | Resource::User
        | Resource::Role
        | Resource::Indicator
        | Resource::Watchlist
        | Resource::Note
        | Resource::Storage => decide_by_grant(actor, action, resource),
    }
}

/// Looks up every scope `actor` has been granted for `(action, resource)`
/// and, when at least one exists, widens them into the single most
/// permissive [`Decision`].
fn decide_by_grant(actor: &Actor, action: Action, resource: Resource) -> Decision {
    actor
        .scopes_for(action, resource)
        .reduce(Scope::widen)
        .map_or_else(Decision::denied, Decision::allowed)
}

#[cfg(test)]
mod tests {
    use super::decide;
    use crate::{Action, Actor, Grant, Resource, Role, Scope};

    #[test]
    fn an_actor_with_no_grants_is_denied() {
        let decision = decide(&Actor::new(), Action::View, Resource::ChartLayout);
        assert!(!decision.is_allowed());
        assert_eq!(decision.scope(), None);
    }

    #[test]
    fn a_matching_direct_grant_is_allowed_with_its_scope() {
        let actor = Actor::new().with_direct_grant(Grant::new(
            Action::View,
            Resource::ChartLayout,
            Scope::Own,
        ));

        let decision = decide(&actor, Action::View, Resource::ChartLayout);
        assert!(decision.is_allowed());
        assert_eq!(decision.scope(), Some(Scope::Own));
    }

    #[test]
    fn a_matching_role_grant_is_allowed_with_its_scope() {
        let actor = Actor::new().with_role(Role::new("viewer").with_grant(Grant::new(
            Action::View,
            Resource::ChartLayout,
            Scope::Own,
        )));

        let decision = decide(&actor, Action::View, Resource::ChartLayout);
        assert!(decision.is_allowed());
        assert_eq!(decision.scope(), Some(Scope::Own));
    }

    #[test]
    fn a_grant_for_a_different_action_does_not_allow_this_one() {
        let actor = Actor::new().with_direct_grant(Grant::new(
            Action::View,
            Resource::ChartLayout,
            Scope::All,
        ));

        let decision = decide(&actor, Action::Edit, Resource::ChartLayout);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn a_grant_for_a_different_resource_does_not_allow_this_one() {
        let actor = Actor::new().with_direct_grant(Grant::new(
            Action::View,
            Resource::ChartLayout,
            Scope::All,
        ));

        let decision = decide(&actor, Action::View, Resource::ChartWorkspace);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn the_most_permissive_of_several_matching_grants_wins() {
        // One role scopes the actor to their own layouts; a direct grant —
        // perhaps assigned individually by an admin — extends that to
        // every layout. The actor is not held back to `Own` just because
        // one of the two grants was narrower.
        let actor = Actor::new()
            .with_role(Role::new("viewer").with_grant(Grant::new(
                Action::View,
                Resource::ChartLayout,
                Scope::Own,
            )))
            .with_direct_grant(Grant::new(Action::View, Resource::ChartLayout, Scope::All));

        let decision = decide(&actor, Action::View, Resource::ChartLayout);
        assert_eq!(decision.scope(), Some(Scope::All));
    }

    #[test]
    fn decide_is_exhaustive_over_every_resource_variant() {
        // Not a proof by itself (see the crate-level docs for the
        // compiler-error experiment) — this is a regression guard: every
        // `Resource` variant that exists today must produce *some*
        // decision without panicking, so a future refactor that
        // accidentally drops an arm and replaces it with a `_ => todo!()`
        // is caught here too.
        let resources = [
            Resource::ChartWorkspace,
            Resource::ChartLayout,
            Resource::Alert,
            Resource::Strategy,
            Resource::Account,
            Resource::Order,
            Resource::Adapter,
            Resource::User,
            Resource::Role,
            Resource::Indicator,
            Resource::Watchlist,
            Resource::Note,
            Resource::Storage,
        ];
        for resource in resources {
            let _ = decide(&Actor::new(), Action::View, resource);
        }
    }
}
