//! How far a permission reaches.

use serde::{Deserialize, Serialize};

/// How far an allowed [`Action`](crate::Action) reaches: only the actor's
/// own rows, or every row of that [`Resource`](crate::Resource).
///
/// A storage layer must translate `Scope` into a `WHERE` clause, never into
/// a post-fetch filter — filtering after the query still
/// leaks existence through totals, pagination and timing.
///
/// `#[non_exhaustive]`, and **`Team` is deliberately absent**.
/// Shared workspaces would change ownership from a single owner to
/// membership, which is a real feature this plan does not need yet — the
/// enum is left open so that variant can arrive later without breaking
/// every existing match on `Scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[non_exhaustive]
pub enum Scope {
    /// Only rows the actor owns.
    Own,
    /// Every row of the resource, regardless of owner.
    All,
}

impl Scope {
    /// Combines two scopes granted for the same `(Action, Resource)` pair,
    /// keeping whichever is more permissive.
    ///
    /// `All` removes the ownership restriction `Own` imposes, so it always
    /// wins: an actor who holds an `Own` grant from one role and an `All`
    /// grant from another (or a direct grant) is not limited to their own
    /// rows just because one of the two grants was narrower.
    #[must_use]
    pub fn widen(self, other: Self) -> Self {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => Self::All,
            (Self::Own, Self::Own) => Self::Own,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;

    #[test]
    fn widen_of_own_and_own_is_own() {
        assert_eq!(Scope::Own.widen(Scope::Own), Scope::Own);
    }

    #[test]
    fn widen_prefers_all_regardless_of_argument_order() {
        assert_eq!(Scope::Own.widen(Scope::All), Scope::All);
        assert_eq!(Scope::All.widen(Scope::Own), Scope::All);
    }

    #[test]
    fn widen_of_all_and_all_is_all() {
        assert_eq!(Scope::All.widen(Scope::All), Scope::All);
    }
}
