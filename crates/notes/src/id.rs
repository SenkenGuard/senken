//! Opaque identifier for rows this crate owns.
//!
//! Copied from `senken_chart::id`'s own macro rather than sharing it (that
//! macro is private to that crate, which itself copied it from
//! `senken_identity`) — see `senken_chart`'s module docs for why the
//! guarded-query *pattern* is copied exactly while small mechanical pieces
//! like this one are necessarily re-declared per crate.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// A note's primary key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteId(Uuid);

impl NoteId {
    /// A fresh, randomly generated id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for NoteId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl ToSql for NoteId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_string()))
    }
}

impl FromSql for NoteId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Uuid::parse_str(text)
            .map(NoteId)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl std::str::FromStr for NoteId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::NoteId;

    #[test]
    fn two_freshly_generated_ids_differ() {
        assert_ne!(NoteId::new(), NoteId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = NoteId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<uuid::Uuid>().unwrap(), id.0);
    }
}
