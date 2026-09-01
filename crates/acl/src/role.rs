//! A named, reusable set of grants.

use serde::{Deserialize, Serialize};

use crate::grant::Grant;

/// A named set of [`Grant`]s.
///
/// Permissions are code — the `Action`/`Resource`/`Scope` enums and
/// [`crate::decide`] are fixed at compile time. Roles are data: a
/// superadmin builds a "Charts Only" role at runtime out of existing
/// grants, without a deploy. This type is the shape of that data; *storing*
/// it in `SQLite`, editing it at runtime, and assigning it to users is a
/// storage-layer concern, out of scope here.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Role {
    name: String,
    grants: Vec<Grant>,
}

impl Role {
    /// An empty role named `name`. Add grants with [`with_grant`](Self::with_grant).
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            grants: Vec::new(),
        }
    }

    /// Adds one grant to the role.
    #[must_use]
    pub fn with_grant(mut self, grant: Grant) -> Self {
        self.grants.push(grant);
        self
    }

    /// The role's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The grants this role carries.
    #[must_use]
    pub fn grants(&self) -> &[Grant] {
        &self.grants
    }
}

#[cfg(test)]
mod tests {
    use super::Role;
    use crate::{Action, Grant, Resource, Scope};

    #[test]
    fn a_new_role_carries_no_grants() {
        assert!(Role::new("Charts Only").grants().is_empty());
    }

    #[test]
    fn with_grant_appends_in_order() {
        let role = Role::new("Charts Only")
            .with_grant(Grant::new(Action::View, Resource::ChartLayout, Scope::Own))
            .with_grant(Grant::new(
                Action::View,
                Resource::ChartWorkspace,
                Scope::Own,
            ));

        assert_eq!(role.grants().len(), 2);
        assert_eq!(role.grants()[0].resource, Resource::ChartLayout);
        assert_eq!(role.grants()[1].resource, Resource::ChartWorkspace);
    }

    #[test]
    fn name_returns_what_it_was_constructed_with() {
        assert_eq!(Role::new("Charts Only").name(), "Charts Only");
    }
}
