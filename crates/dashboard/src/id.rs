//! Opaque identifiers for rows this crate owns.
//!
//! Copied from `senken_identity`/`senken_chart`'s own macro rather than
//! sharing it (that macro is private to each crate) — the guarded-query
//! *pattern* is copied exactly while small mechanical pieces like this one
//! are necessarily re-declared per crate.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// A dashboard workspace's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DashboardWorkspaceId(Uuid);

/// A placed widget's primary key — one row of `dashboard_widgets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DashboardWidgetId(Uuid);

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

uuid_id!(DashboardWorkspaceId);
uuid_id!(DashboardWidgetId);

#[cfg(test)]
mod tests {
    use super::{DashboardWidgetId, DashboardWorkspaceId};

    #[test]
    fn two_freshly_generated_ids_of_each_kind_differ() {
        assert_ne!(DashboardWorkspaceId::new(), DashboardWorkspaceId::new());
        assert_ne!(DashboardWidgetId::new(), DashboardWidgetId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = DashboardWorkspaceId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<uuid::Uuid>().unwrap(), id.0);
    }
}
