//! What an action is performed against.

use serde::{Deserialize, Serialize};

/// The noun half of a permission check: what an [`Action`](crate::Action) is
/// performed against.
///
/// Deliberately **not** `#[non_exhaustive]`: this is a closed
/// set on purpose. [`crate::decide`] matches every variant with no wildcard
/// arm, so adding one here fails that match — and therefore fails to
/// compile the whole crate — until the new variant's authorisation is
/// written into [`crate::decide`]. A resource that plugins can name
/// dynamically is a different, open-ended concept and is
/// modelled separately as [`crate::PluginPermissionName`]; it is not a
/// `Resource` variant and never will be, because an open enum here would
/// undo the whole point of closing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Resource {
    /// A saved arrangement of the workspace UI.
    Workspace,
    /// A chart layout within a workspace.
    Layout,
    /// A price or condition alert.
    Alert,
    /// A trading strategy definition.
    Strategy,
    /// A broker or exchange account attached by a user.
    Account,
    /// A connected adapter (broker/exchange integration instance).
    Adapter,
    /// A user record — creating, editing or removing accounts.
    User,
    /// A role definition — creating, editing or assigning roles.
    Role,
    /// An indicator definition or instance.
    Indicator,
}

#[cfg(test)]
mod tests {
    use super::Resource;

    #[test]
    fn resources_with_the_same_variant_are_equal() {
        assert_eq!(Resource::Workspace, Resource::Workspace);
        assert_ne!(Resource::Workspace, Resource::Layout);
    }
}
