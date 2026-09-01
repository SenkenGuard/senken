//! Opaque identifiers for rows this crate owns.
//!
//! Copied from `senken_identity::{UserId, RoleId}`'s own macro rather than
//! sharing it (that macro is private to that crate) — see this crate's
//! module docs for why the guarded-query *pattern* is copied exactly while
//! small mechanical pieces like this one are necessarily re-declared per
//! crate.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// A chart workspace's primary key. Carries the `Chart` prefix because a
/// dashboard is its own, separate aggregate with its own workspace
/// concept — see this crate's module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChartWorkspaceId(Uuid);

/// A chart layout's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChartLayoutId(Uuid);

/// A pane's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChartPaneId(Uuid);

/// A pane item's primary key — one row of `chart_pane_items`, whether it
/// is a computed indicator, a referenced overlay instrument, or an
/// anchored drawing. Replaces the separate `LayerId`/`DrawingId` this crate
/// used to have, since both named the exact same kind of row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneItemId(Uuid);

macro_rules! uuid_id {
    ($name:ident) => {
        impl $name {
            /// A fresh, randomly generated id.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0.to_string()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: rusqlite::types::ValueRef<'_>) -> FromSqlResult<Self> {
                let text = value.as_str()?;
                Uuid::parse_str(text)
                    .map($name)
                    .map_err(|e| FromSqlError::Other(Box::new(e)))
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

uuid_id!(ChartWorkspaceId);
uuid_id!(ChartLayoutId);
uuid_id!(ChartPaneId);
uuid_id!(PaneItemId);

#[cfg(test)]
mod tests {
    use super::{ChartLayoutId, ChartPaneId, ChartWorkspaceId, PaneItemId};

    #[test]
    fn two_freshly_generated_ids_of_each_kind_differ() {
        assert_ne!(ChartWorkspaceId::new(), ChartWorkspaceId::new());
        assert_ne!(ChartLayoutId::new(), ChartLayoutId::new());
        assert_ne!(ChartPaneId::new(), ChartPaneId::new());
        assert_ne!(PaneItemId::new(), PaneItemId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = ChartWorkspaceId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<uuid::Uuid>().unwrap(), id.0);
    }
}
