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
    /// A saved dashboard workspace and its widget grid. Its own variant
    /// (not folded into `ChartWorkspace`) for the same reason that one is
    /// not named plain `Workspace`: a dashboard and a chart are two
    /// separate aggregates, each with grants a role or a direct grant may
    /// need to scope independently.
    DashboardWorkspace,
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
    /// A published indicator registry entry — source an account has
    /// published for other accounts to search and install. Its own
    /// variant, distinct from `Indicator` (an indicator definition or
    /// instance in use on a chart), because publishing to the public
    /// registry is a separate, author-scoped action from configuring an
    /// indicator for oneself.
    IndicatorRegistry,
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
    /// A dynamic widget UI package: installing, enabling/disabling and
    /// removing one from the dashboard's widget catalog. Its own variant,
    /// distinct from `Storage`, because a widget package is third-party
    /// code the server will run in a sandboxed iframe for every user of
    /// this server — a different, narrower administrative concern from
    /// "how much disk this install is using", even though both are
    /// properties of the whole server rather than any one account.
    WidgetPlugin,
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
