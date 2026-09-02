//! Opaque identifier for rows this crate owns.
//!
//! Copied from `senken_notes::id`'s own macro-free shape rather than shared
//! with it (that type is private to that crate) — see `senken_chart`'s
//! module docs for why the guarded-query *pattern* is copied exactly while
//! small mechanical pieces like this one are necessarily re-declared per
//! crate.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// A published indicator's primary key — identifies one `(namespace, name)`
/// entry, not any particular version of its source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IndicatorEntryId(Uuid);

impl IndicatorEntryId {
    /// A fresh, randomly generated id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for IndicatorEntryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for IndicatorEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl ToSql for IndicatorEntryId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_string()))
    }
}

impl FromSql for IndicatorEntryId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Uuid::parse_str(text)
            .map(IndicatorEntryId)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

impl std::str::FromStr for IndicatorEntryId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::IndicatorEntryId;

    #[test]
    fn two_freshly_generated_ids_differ() {
        assert_ne!(IndicatorEntryId::new(), IndicatorEntryId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = IndicatorEntryId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<uuid::Uuid>().unwrap(), id.0);
    }
}
