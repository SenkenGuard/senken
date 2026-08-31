//! Opaque identifiers for rows in the identity store.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use uuid::Uuid;

/// A user's primary key.
///
/// Wrapping [`Uuid`] rather than passing it around bare keeps a user id and
/// a role id from being swapped by accident at a call site — the type
/// checker catches what a raw `Uuid` parameter cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(Uuid);

/// A role's primary key. See [`UserId`] for why this is not a bare [`Uuid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoleId(Uuid);

/// A `plugin_permissions` row's primary key. Distinct from
/// the permission's `name` (a
/// [`senken_acl::PluginPermissionName`](../../senken_acl/struct.PluginPermissionName.html)
/// like `mychart.dashboard:view`, already unique by construction) so that
/// `role_plugin_grants`/`user_plugin_grants` reference a stable id rather
/// than a string a future rename of the *type* (not the value) would have
/// to migrate — matching how `UserId`/`RoleId` are never the email or role
/// name either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PluginPermissionId(Uuid);

macro_rules! uuid_id {
    ($vis:vis $name:ident) => {
        impl $name {
            /// A fresh, randomly generated id.
            #[must_use]
            $vis fn new() -> Self {
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

            /// Parses the same textual form [`Display`](fmt::Display) produces
            /// (an HTTP path segment like `/api/users/{id}` is this id's `Display` output, and a handler needs a way back from that string to the typed id without going through SQL).
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

uuid_id!(pub UserId);
uuid_id!(pub RoleId);
uuid_id!(pub(crate) PluginPermissionId);

#[cfg(test)]
mod tests {
    use super::{PluginPermissionId, RoleId, UserId};

    #[test]
    fn two_freshly_generated_ids_differ() {
        assert_ne!(UserId::new(), UserId::new());
        assert_ne!(RoleId::new(), RoleId::new());
        assert_ne!(PluginPermissionId::new(), PluginPermissionId::new());
    }

    #[test]
    fn display_round_trips_through_uuid_parsing() {
        let id = UserId::new();
        let text = id.to_string();
        assert_eq!(text.parse::<uuid::Uuid>().unwrap(), id.0);
    }

    #[test]
    fn from_str_round_trips_through_display() {
        let id = UserId::new();
        let parsed: UserId = id.to_string().parse().unwrap();
        assert_eq!(parsed, id);
        let role_id = RoleId::new();
        let parsed_role: RoleId = role_id.to_string().parse().unwrap();
        assert_eq!(parsed_role, role_id);
    }

    #[test]
    fn from_str_rejects_a_non_uuid_string() {
        assert!("not-a-uuid".parse::<UserId>().is_err());
    }
}
