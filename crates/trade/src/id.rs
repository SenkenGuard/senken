//! Identifiers: the one this crate mints, and the two a venue does.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The primary key of an account a user has attached to the trade engine.
///
/// This one is minted here because the attachment is Senken's own record —
/// unlike [`OrderId`], which belongs to whatever system actually accepted
/// the order.
///
/// Serialises as its hyphenated text form rather than as a `uuid` crate
/// value: the wire shape is what a browser and an `OpenAPI` document see,
/// and a UUID's byte-array serialisation is neither readable nor stable
/// across that crate's own feature flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TradeAccountId(Uuid);

impl Serialize for TradeAccountId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for TradeAccountId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

impl TradeAccountId {
    /// A fresh, randomly generated id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TradeAccountId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TradeAccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for TradeAccountId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(feature = "accounts")]
impl rusqlite::types::ToSql for TradeAccountId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::from(self.0.to_string()))
    }
}

#[cfg(feature = "accounts")]
impl rusqlite::types::FromSql for TradeAccountId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let text = value.as_str()?;
        Uuid::parse_str(text)
            .map(Self)
            .map_err(|error| rusqlite::types::FromSqlError::Other(Box::new(error)))
    }
}

/// An order's identifier **as the executing venue reports it**.
///
/// Opaque text on purpose. Binance answers with a 64-bit integer, MetaTrader
/// with a ticket number, an OAuth broker with a UUID, this project's own
/// simulator with a UUID of its own — a numeric type here would force every
/// adapter whose venue does not use one to invent a mapping, and a mapping
/// that has to be inverted is a place ids get confused between venues. The
/// engine only ever echoes this value back to the adapter that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderId(Box<str>);

impl OrderId {
    /// Wraps whatever the venue called this order.
    pub fn new(raw: impl Into<Box<str>>) -> Self {
        Self(raw.into())
    }

    /// The venue's own text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A position's identifier **as the holding venue reports it**.
///
/// Opaque text for the same reason [`OrderId`] is, and needed for a reason
/// an instrument alone cannot cover: two of the systems Senken simulates
/// hold more than one position on the same instrument at once. A
/// MetaTrader 5 hedging account does it by design — every deal opens its
/// own ticket, and holding a long and a short at the same time is the
/// point. A crypto futures account in hedge mode does it by
/// configuration.
///
/// Without this, "the position on BTCUSDT" is a question with no answer on
/// those accounts, and closing one is not expressible. A netting or spot
/// account still holds at most one per instrument and simply mints an id
/// that is stable for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PositionId(Box<str>);

impl PositionId {
    /// Wraps whatever the venue called this position.
    pub fn new(raw: impl Into<Box<str>>) -> Self {
        Self(raw.into())
    }

    /// The venue's own text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PositionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The longest client order id every venue this project has looked at
/// accepts. Deliberately conservative: it is far easier to widen this later
/// than to discover that one venue in ten silently truncates, which would
/// make two distinct orders collide on lookup.
pub const MAX_CLIENT_ORDER_ID_LEN: usize = 32;

/// Why a [`ClientOrderId`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ClientOrderIdError {
    /// The id was empty.
    #[error("a client order id must not be empty")]
    Empty,
    /// The id was longer than [`MAX_CLIENT_ORDER_ID_LEN`].
    #[error("a client order id must be at most {MAX_CLIENT_ORDER_ID_LEN} characters")]
    TooLong,
    /// The id used a character outside `[A-Za-z0-9-_]`.
    #[error("a client order id may only contain ASCII letters, digits, `-` and `_`")]
    InvalidCharacter,
}

/// A caller-supplied idempotency key for an order.
///
/// Constrained to `[A-Za-z0-9-_]` and [`MAX_CLIENT_ORDER_ID_LEN`] because
/// that is the intersection of what venues accept, and a value a venue
/// rejects (or worse, truncates) is one the caller cannot use to find out
/// whether their order arrived — which is the entire point of sending one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ClientOrderId(Box<str>);

impl ClientOrderId {
    /// Validates and wraps a caller-supplied id.
    ///
    /// # Errors
    /// [`ClientOrderIdError`] when the id is empty, too long, or contains a
    /// character outside `[A-Za-z0-9-_]`.
    pub fn new(raw: &str) -> Result<Self, ClientOrderIdError> {
        if raw.is_empty() {
            return Err(ClientOrderIdError::Empty);
        }
        if raw.len() > MAX_CLIENT_ORDER_ID_LEN {
            return Err(ClientOrderIdError::TooLong);
        }
        if !raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(ClientOrderIdError::InvalidCharacter);
        }
        Ok(Self(raw.into()))
    }

    /// The validated text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClientOrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ClientOrderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::new(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{ClientOrderId, ClientOrderIdError, OrderId, TradeAccountId};

    #[test]
    fn two_freshly_generated_account_ids_differ() {
        assert_ne!(TradeAccountId::new(), TradeAccountId::new());
    }

    #[test]
    fn an_account_id_round_trips_through_its_display_form() {
        let id = TradeAccountId::new();
        assert_eq!(id.to_string().parse::<TradeAccountId>().unwrap(), id);
    }

    #[test]
    fn a_venue_order_id_keeps_whatever_text_the_venue_used() {
        assert_eq!(OrderId::new("28457471").as_str(), "28457471");
        assert_eq!(
            OrderId::new("x-1a2b-c3d4").as_str(),
            "x-1a2b-c3d4",
            "an id that is not a number must survive unchanged"
        );
    }

    #[test]
    fn a_client_order_id_accepts_the_characters_every_venue_does() {
        assert_eq!(
            ClientOrderId::new("senken-01_A").unwrap().as_str(),
            "senken-01_A"
        );
    }

    #[test]
    fn a_client_order_id_rejects_what_a_venue_would_truncate_or_refuse() {
        assert_eq!(ClientOrderId::new(""), Err(ClientOrderIdError::Empty));
        assert_eq!(
            ClientOrderId::new(&"a".repeat(33)),
            Err(ClientOrderIdError::TooLong)
        );
        assert_eq!(
            ClientOrderId::new("has space"),
            Err(ClientOrderIdError::InvalidCharacter)
        );
        assert_eq!(
            ClientOrderId::new("emoji-🙂"),
            Err(ClientOrderIdError::InvalidCharacter)
        );
    }

    #[test]
    fn a_client_order_id_deserialized_from_json_is_validated_not_trusted() {
        assert!(serde_json::from_str::<ClientOrderId>("\"ok-1\"").is_ok());
        assert!(
            serde_json::from_str::<ClientOrderId>("\"not ok\"").is_err(),
            "an id arriving over HTTP must go through the same check as one built in Rust"
        );
    }
}
