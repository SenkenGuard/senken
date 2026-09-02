//! [`IanaZone`] — a time zone identifier from the IANA Time Zone Database,
//! validated at construction against the copy this project bundles.

use std::fmt;

use serde::{Deserialize, Serialize};

/// An IANA time zone identifier, e.g. `"America/New_York"` or `"UTC"`.
///
/// Construction resolves the id against the bundled time zone database (see
/// the [`crate::time`] module docs for why it is bundled rather than read
/// from the host machine), so an `IanaZone` that exists names a zone this
/// project can actually compute with. There is no constructor that skips
/// that check — the same reasoning that gives [`crate::UnixNanos`] no
/// `From<i64>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct IanaZone(String);

impl IanaZone {
    /// Coordinated Universal Time — the zero-offset zone, always present in
    /// any copy of the database.
    #[must_use]
    pub fn utc() -> Self {
        Self("UTC".to_string())
    }

    /// Validates `id` against the bundled time zone database.
    ///
    /// # Errors
    ///
    /// Returns [`UnknownZone`] when `id` is not a zone the bundled database
    /// recognises.
    pub fn new(id: impl Into<String>) -> Result<Self, UnknownZone> {
        let id = id.into();
        if jiff::tz::TimeZone::get(&id).is_err() {
            return Err(UnknownZone(id));
        }
        Ok(Self(id))
    }

    /// The zone id as written, e.g. `"Europe/London"`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Resolves this id against the bundled database.
    ///
    /// Infallible because construction already proved the id resolves — see
    /// [`IanaZone::new`] — so a lookup failure here would mean the bundled
    /// database itself changed underneath a live value, which cannot happen
    /// within one process.
    pub(crate) fn to_jiff(&self) -> jiff::tz::TimeZone {
        jiff::tz::TimeZone::get(&self.0).expect("IanaZone was validated at construction")
    }
}

impl fmt::Display for IanaZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IanaZone {
    /// Validates against the bundled database on the way in, so a request
    /// carrying an unknown or misspelled zone id fails to deserialize with a
    /// named error rather than silently storing a string nothing can compute
    /// with.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::new(id).map_err(serde::de::Error::custom)
    }
}

/// A time zone id that the bundled database does not recognise.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown IANA time zone id: {0:?}")]
pub struct UnknownZone(String);

#[cfg(test)]
mod tests {
    use super::IanaZone;

    #[test]
    fn known_zone_id_round_trips_through_json() {
        let zone = IanaZone::new("Asia/Jakarta").unwrap();
        let json = serde_json::to_string(&zone).unwrap();
        assert_eq!(json, "\"Asia/Jakarta\"");
        let back: IanaZone = serde_json::from_str(&json).unwrap();
        assert_eq!(back, zone);
    }

    #[test]
    fn utc_is_always_a_known_zone() {
        assert!(IanaZone::new("UTC").is_ok());
        assert_eq!(IanaZone::utc().as_str(), "UTC");
    }

    #[test]
    fn unknown_zone_id_is_rejected_at_construction() {
        assert!(IanaZone::new("Not/AZone").is_err());
    }

    #[test]
    fn unknown_zone_id_fails_to_deserialize_with_a_named_error() {
        let err = serde_json::from_str::<IanaZone>("\"Not/AZone\"").unwrap_err();
        assert!(err.to_string().contains("unknown IANA time zone id"));
    }
}
