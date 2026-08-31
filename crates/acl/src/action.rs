//! What an actor is trying to do.

use serde::{Deserialize, Serialize};

/// The verb half of a permission check: what an actor is trying to do to a
/// [`Resource`](crate::Resource).
///
/// `#[non_exhaustive]`: the set of verbs this system can check
/// is expected to grow (an audit-log "view history" action, say), and a
/// downstream crate that matches on `Action` must already carry a wildcard
/// arm rather than being silently broken by an addition here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Action {
    /// Read the resource, or list resources of this kind.
    View,
    /// Create a new instance of the resource.
    Create,
    /// Modify an existing instance.
    Edit,
    /// Remove an existing instance.
    Delete,
    /// Grant another actor access to the resource.
    Share,
}

#[cfg(test)]
mod tests {
    use super::Action;

    #[test]
    fn actions_with_the_same_variant_are_equal() {
        assert_eq!(Action::View, Action::View);
        assert_ne!(Action::View, Action::Edit);
    }
}
