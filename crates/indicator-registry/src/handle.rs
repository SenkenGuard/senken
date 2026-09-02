//! [`Handle`]: a validated, user-chosen name that resolves to a `UserId`.
//!
//! A registry namespace is always a `UserId` — see this crate's own module
//! docs for why that alone already closes impersonation. What a `UserId`
//! does not give anyone is something to *type*: asking a trader to install
//! `@550e8400-e29b-41d4-a716-446655440000/supertrend` instead of
//! `@alice/supertrend` makes the registry technically safe and practically
//! unusable. A [`Handle`] is the fix, and it is additive, never a
//! replacement: `owner_id` (the `UserId`) stays the one thing
//! `indicator_registry_entries.owner_id` and every authorisation check in
//! [`crate::store`] ever reasons about. A handle only ever *resolves to*
//! one, through [`crate::RegistryStore::resolve_handle`].
//!
//! Two properties make a handle safe to add without reopening the
//! impersonation hole a bare `UserId` namespace already closed:
//!
//! - **Validated shape.** [`Handle::new`] is the only way to build one, and
//!   it accepts nothing that is not lowercase ASCII letters, digits and
//!   hyphens, 3–32 characters, starting and ending with a letter or digit —
//!   never anything that could be confused with a `UserId`'s own textual
//!   form or used to smuggle formatting into a rendered address.
//! - **Globally unique, enforced at the database.** `registry_handles.handle`
//!   carries a `UNIQUE` constraint (see `senken-identity`'s schema v13) —
//!   the same reasoning `(owner_id, name)` already gives published
//!   indicators — so a handle is structurally a pointer at exactly one
//!   account, never two accounts racing to be "the real alice".

use std::fmt;

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};

use crate::error::RegistryError;

/// Shortest legal handle — long enough that it is a chosen name, not a
/// single throwaway character.
const MIN_LEN: usize = 3;
/// Longest legal handle — generous for a name someone actually wants to
/// read and type, without turning into a sentence.
const MAX_LEN: usize = 32;

/// A validated registry handle.
///
/// See this module's doc comment for exactly what "validated" means and
/// why both the shape check here and the database's own `UNIQUE`
/// constraint are needed — one closes malformed input, the other closes
/// the race a check-then-insert alone cannot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Handle(String);

impl Handle {
    /// Validates `raw` as a legal handle.
    ///
    /// # Errors
    /// [`RegistryError::InvalidHandle`] if `raw` is shorter than 3 or
    /// longer than 32 characters, contains anything other than lowercase
    /// ASCII letters, digits or hyphens, or starts or ends with a hyphen.
    pub fn new(raw: &str) -> Result<Self, RegistryError> {
        if is_valid(raw) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(RegistryError::InvalidHandle(raw.to_owned()))
        }
    }

    /// This handle's text, exactly as validated.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_valid(raw: &str) -> bool {
    if raw.len() < MIN_LEN || raw.len() > MAX_LEN {
        return false;
    }
    let is_edge_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    let Some(first) = raw.chars().next() else {
        return false;
    };
    let last = raw.chars().next_back().unwrap_or(first);
    is_edge_char(first) && is_edge_char(last) && raw.chars().all(|c| is_edge_char(c) || c == '-')
}

impl fmt::Display for Handle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ToSql for Handle {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.clone()))
    }
}

impl FromSql for Handle {
    /// Re-validates on the way back out, not just on the way in: this
    /// crate is the table's only writer and only ever writes a string
    /// [`Handle::new`] already accepted, so reaching an invalid value here
    /// means the row was written by an incompatible version of this crate
    /// or edited by hand — the same "do not guess" reasoning
    /// `senken_identity::IdentityError::CorruptZone` documents for itself.
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Handle::new(text).map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::Handle;
    use crate::error::RegistryError;

    #[test]
    fn a_well_formed_handle_is_accepted() {
        for raw in [
            "alice",
            "trader-42",
            "a1b",
            "x23456789012345678901234567890y",
        ] {
            assert_eq!(Handle::new(raw).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn display_round_trips_the_original_text() {
        let handle = Handle::new("alice").unwrap();
        assert_eq!(handle.to_string(), "alice");
    }

    #[test]
    fn too_short_or_too_long_is_rejected() {
        assert!(matches!(
            Handle::new("ab"),
            Err(RegistryError::InvalidHandle(_))
        ));
        let too_long = "a".repeat(33);
        assert!(matches!(
            Handle::new(&too_long),
            Err(RegistryError::InvalidHandle(_))
        ));
    }

    #[test]
    fn uppercase_is_rejected_not_silently_folded() {
        // A handle is validated, never normalised -- silently lowercasing
        // `Alice` would let two different-looking inputs collide on the
        // same stored value without the caller ever being told.
        assert!(matches!(
            Handle::new("Alice"),
            Err(RegistryError::InvalidHandle(_))
        ));
    }

    #[test]
    fn a_leading_or_trailing_hyphen_is_rejected() {
        assert!(matches!(
            Handle::new("-alice"),
            Err(RegistryError::InvalidHandle(_))
        ));
        assert!(matches!(
            Handle::new("alice-"),
            Err(RegistryError::InvalidHandle(_))
        ));
    }

    #[test]
    fn a_character_outside_the_allowed_set_is_rejected() {
        for raw in ["al ice", "alice_bob", "alice.bob", "alice/bob", "@alice"] {
            assert!(
                matches!(Handle::new(raw), Err(RegistryError::InvalidHandle(_))),
                "{raw:?} should have been rejected"
            );
        }
    }

    #[test]
    fn an_empty_string_is_rejected() {
        assert!(matches!(
            Handle::new(""),
            Err(RegistryError::InvalidHandle(_))
        ));
    }
}
