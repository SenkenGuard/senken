//! [`AlertId`] — the opaque primary key for a row this crate owns.
//!
//! Copied from `senken_identity`/`senken_workspace`'s own identical macro
//! rather than shared (it is private to each of those crates) — see
//! `senken-workspace`'s module docs for why the guarded-query *pattern* is
//! copied exactly while small mechanical pieces like this one are
//! necessarily re-declared per crate.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// An alert's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AlertId(Uuid);

impl AlertId {
    /// A fresh, randomly generated id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AlertId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AlertId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl ToSql for AlertId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_string()))
    }
}

impl FromSql for AlertId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Uuid::parse_str(text)
            .map(AlertId)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl std::str::FromStr for AlertId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::AlertId;

    #[test]
    fn two_freshly_generated_ids_differ() {
        assert_ne!(AlertId::new(), AlertId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = AlertId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<AlertId>().unwrap(), id);
    }
}
