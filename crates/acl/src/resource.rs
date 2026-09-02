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
    /// A saved arrangement of chart workspace UI. Named `ChartWorkspace`
    /// (not `Workspace`) because a dashboard is its own, separate
    /// aggregate with its own workspace concept — "workspace" alone
    /// stopped naming one aggregate the day a second one existed.
    ChartWorkspace,
    /// A chart layout within a chart workspace.
    ChartLayout,
    /// A price or condition alert.
    Alert,
    /// A trading strategy definition.
    Strategy,
    /// A broker or exchange account attached by a user.
    Account,
    /// An order, and the fills it produced, on one of those accounts.
    ///
    /// Separate from [`Account`](Self::Account) because configuring an
    /// account and sending money-moving instructions with it are different
    /// authorities: a role may reasonably read a portfolio without being
    /// allowed to trade it, and an operator may attach an account for
    /// someone else to trade. `Create` places an order, `Delete` cancels
    /// one, `Edit` amends one, `View` reads the order book and the fill
    /// history.
    Order,
    /// A connected adapter (broker/exchange integration instance).
    Adapter,
    /// A user record — creating, editing or removing accounts.
    User,
    /// A role definition — creating, editing or assigning roles.
    Role,
    /// An indicator definition or instance.
    Indicator,
    /// A user-authored group of watched instruments, and its membership —
    /// a saved artifact like a chart workspace, so it takes the same
    /// ordinary scope-limited grants rather than a special rule of its
    /// own.
    Watchlist,
    /// A user-authored freeform note.
    Note,
    /// Administering what the server keeps on disk — usage reporting and
    /// reclamation — distinct from the market data itself, which has no
    /// owner to check a grant against.
    Storage,
}

#[cfg(test)]
mod tests {
    use super::Resource;

    #[test]
    fn resources_with_the_same_variant_are_equal() {
        assert_eq!(Resource::ChartWorkspace, Resource::ChartWorkspace);
        assert_ne!(Resource::ChartWorkspace, Resource::ChartLayout);
    }
}
