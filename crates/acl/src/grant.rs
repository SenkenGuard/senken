//! A single permission: an action, a resource, and how far it reaches.

use serde::{Deserialize, Serialize};

use crate::action::Action;
use crate::resource::Resource;
use crate::scope::Scope;

/// The right to perform `action` on `resource`, limited to `scope`.
///
/// A grant carries no owner and no actor — it is a plain fact ("View on
/// Layout, scoped to Own"), reusable across every [`Role`](crate::Role) or
/// [`Actor`](crate::Actor) that holds it. Every field is public because a
/// grant has no invariant beyond the three enums already enforcing their
/// own: any `(Action, Resource, Scope)` triple is a valid grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub struct Grant {
    /// What the grant permits doing.
    pub action: Action,
    /// What it permits doing it to.
    pub resource: Resource,
    /// How far that permission reaches.
    pub scope: Scope,
}

impl Grant {
    /// A grant permitting `action` on `resource`, limited to `scope`.
    #[must_use]
    pub fn new(action: Action, resource: Resource, scope: Scope) -> Self {
        Self {
            action,
            resource,
            scope,
        }
    }

    /// `true` when this grant covers `action` on `resource` — irrespective
    /// of scope, which the caller reads separately once it knows a grant
    /// matches.
    #[must_use]
    pub fn matches(&self, action: Action, resource: Resource) -> bool {
        self.action == action && self.resource == resource
    }
}

#[cfg(test)]
mod tests {
    use super::Grant;
    use crate::{Action, Resource, Scope};

    #[test]
    fn matches_is_true_only_for_the_same_action_and_resource() {
        let grant = Grant::new(Action::View, Resource::ChartLayout, Scope::Own);
        assert!(grant.matches(Action::View, Resource::ChartLayout));
        assert!(!grant.matches(Action::Edit, Resource::ChartLayout));
        assert!(!grant.matches(Action::View, Resource::ChartWorkspace));
    }

    #[test]
    fn matches_ignores_scope() {
        // A grant matches by (action, resource) alone; scope is read
        // separately by the caller once a match is found.
        let grant = Grant::new(Action::View, Resource::ChartLayout, Scope::All);
        assert!(grant.matches(Action::View, Resource::ChartLayout));
    }
}
