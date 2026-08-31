//! Fully-qualified instrument identifiers.

use std::convert::Infallible;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The character separating source from symbol in an [`InstrumentId`].
pub const ID_SEPARATOR: char = ':';

const MAX_ID_LEN: usize = 128;

/// Why a string is not a valid [`InstrumentId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InstrumentIdError {
    /// Nothing but whitespace.
    #[error("instrument id is empty")]
    Empty,

    /// No `:` anywhere in the input.
    #[error("instrument id `{0}` is missing the `:` separator")]
    MissingSeparator(String),

    /// Nothing before the `:`.
    #[error("instrument id `{0}` has an empty source")]
    EmptySource(String),

    /// Nothing after the `:`.
    #[error("instrument id `{0}` has an empty symbol")]
    EmptySymbol(String),

    /// The source part is not lowercase `[a-z0-9-]`.
    #[error("source `{0}` must be lowercase [a-z0-9-]")]
    InvalidSource(String),

    /// Longer than the fixed maximum.
    #[error("instrument id is longer than {MAX_ID_LEN} bytes")]
    TooLong,
}

impl From<Infallible> for InstrumentIdError {
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

/// A fully-qualified instrument identifier: `source:symbol`.
///
/// The source part is a lowercase `[a-z0-9-]` id such as `binance-spot`; the
/// symbol part is the venue's normalised symbol with its casing preserved.
/// Only the first `:` splits, so symbols may themselves contain colons.
///
/// Stored as a single allocation plus the separator offset, so
/// [`source`](Self::source) and [`symbol`](Self::symbol) are slices.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstrumentId {
    raw: Box<str>,
    separator: u16,
}

impl InstrumentId {
    /// Joins `source` and `symbol`, validating the result. One allocation:
    /// the joined string becomes the id's own buffer.
    ///
    /// # Errors
    /// As [`parse`](Self::parse).
    pub fn new(source: &str, symbol: &str) -> Result<Self, InstrumentIdError> {
        let mut raw = String::with_capacity(source.len() + 1 + symbol.len());
        raw.push_str(source);
        raw.push(ID_SEPARATOR);
        raw.push_str(symbol);
        Self::from_string(raw)
    }

    /// Parses `source:symbol`, trimming surrounding whitespace.
    ///
    /// # Errors
    /// See [`InstrumentIdError`].
    ///
    /// # Examples
    /// ```
    /// use senken_marketdata::InstrumentId;
    ///
    /// let id = InstrumentId::parse("binance-spot:BTCUSDT")?;
    /// assert_eq!(id.source(), "binance-spot");
    /// assert_eq!(id.symbol(), "BTCUSDT");
    /// # Ok::<(), senken_marketdata::InstrumentIdError>(())
    /// ```
    pub fn parse(raw: &str) -> Result<Self, InstrumentIdError> {
        let raw = raw.trim();
        let separator = Self::validate(raw)?;
        Ok(Self {
            raw: raw.into(),
            separator,
        })
    }

    /// Parses an owned string, reusing its buffer unless trimming shrinks it.
    fn from_string(raw: String) -> Result<Self, InstrumentIdError> {
        if raw.trim().len() != raw.len() {
            return Self::parse(&raw);
        }
        let separator = Self::validate(&raw)?;
        Ok(Self {
            raw: raw.into_boxed_str(),
            separator,
        })
    }

    /// Checks a trimmed `source:symbol` and returns the separator offset.
    fn validate(raw: &str) -> Result<u16, InstrumentIdError> {
        if raw.is_empty() {
            return Err(InstrumentIdError::Empty);
        }
        if raw.len() > MAX_ID_LEN {
            return Err(InstrumentIdError::TooLong);
        }

        let Some(separator) = raw.find(ID_SEPARATOR) else {
            return Err(InstrumentIdError::MissingSeparator(raw.to_owned()));
        };

        let (source, symbol) = (&raw[..separator], &raw[separator + 1..]);
        if source.is_empty() {
            return Err(InstrumentIdError::EmptySource(raw.to_owned()));
        }
        if symbol.is_empty() {
            return Err(InstrumentIdError::EmptySymbol(raw.to_owned()));
        }
        if !Self::is_valid_source(source) {
            return Err(InstrumentIdError::InvalidSource(source.to_owned()));
        }

        // MAX_ID_LEN < u16::MAX, so this cannot fail; keep the check honest anyway.
        u16::try_from(separator).map_err(|_| InstrumentIdError::TooLong)
    }

    /// `true` if `source` is acceptable as the source half of an id:
    /// non-empty, lowercase ASCII letters, digits and hyphens only.
    #[must_use]
    pub fn is_valid_source(source: &str) -> bool {
        !source.is_empty()
            && source
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    /// `true` when [`new`](Self::new) would succeed for an already-valid
    /// `source`: the trimmed symbol is non-empty and the joined id fits.
    /// The check search runs per candidate, so it must not allocate.
    #[cfg(feature = "registry")]
    #[must_use]
    pub(crate) fn can_join(source: &str, symbol: &str) -> bool {
        let symbol = symbol.trim();
        !symbol.is_empty() && source.len() + 1 + symbol.len() <= MAX_ID_LEN
    }

    /// The source id, e.g. `binance-spot`.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.raw[..usize::from(self.separator)]
    }

    /// The symbol, e.g. `BTCUSDT`.
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.raw[usize::from(self.separator) + 1..]
    }

    /// The whole id, `source:symbol`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `pad`, not `write_str`, so `{:<28}` and friends work.
        f.pad(&self.raw)
    }
}

impl fmt::Debug for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstrumentId({})", self.raw)
    }
}

impl FromStr for InstrumentId {
    type Err = InstrumentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl TryFrom<&str> for InstrumentId {
    type Error = InstrumentIdError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for InstrumentId {
    type Error = InstrumentIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_string(value)
    }
}

impl From<&InstrumentId> for InstrumentId {
    fn from(value: &InstrumentId) -> Self {
        value.clone()
    }
}

impl From<InstrumentId> for String {
    fn from(id: InstrumentId) -> Self {
        id.raw.into_string()
    }
}

impl AsRef<str> for InstrumentId {
    fn as_ref(&self) -> &str {
        &self.raw
    }
}

impl Serialize for InstrumentId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.raw)
    }
}

impl<'de> Deserialize<'de> for InstrumentId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct IdVisitor;

        impl Visitor<'_> for IdVisitor {
            type Value = InstrumentId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an instrument id of the form `source:symbol`")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                InstrumentId::parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(IdVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{InstrumentId, InstrumentIdError};

    #[test]
    fn splits_source_from_symbol() {
        let id = InstrumentId::parse("binance-spot:XAUTUSDT").unwrap();
        assert_eq!(id.source(), "binance-spot");
        assert_eq!(id.symbol(), "XAUTUSDT");
        assert_eq!(id.as_str(), "binance-spot:XAUTUSDT");
    }

    #[test]
    fn only_the_first_separator_splits() {
        let id = InstrumentId::parse("deribit:BTC-27JUN25-60000-C").unwrap();
        assert_eq!(id.source(), "deribit");
        assert_eq!(id.symbol(), "BTC-27JUN25-60000-C");
    }

    #[test]
    fn symbols_keep_their_exact_venue_casing() {
        let id = InstrumentId::new("okx", "BTC-USDT").unwrap();
        assert_eq!(id.symbol(), "BTC-USDT");
        assert_eq!(id.to_string(), "okx:BTC-USDT");
    }

    #[test]
    fn rejects_malformed_ids() {
        assert_eq!(InstrumentId::parse(""), Err(InstrumentIdError::Empty));
        assert!(matches!(
            InstrumentId::parse("BTCUSDT"),
            Err(InstrumentIdError::MissingSeparator(_))
        ));
        assert!(matches!(
            InstrumentId::parse(":BTCUSDT"),
            Err(InstrumentIdError::EmptySource(_))
        ));
        assert!(matches!(
            InstrumentId::parse("okx:"),
            Err(InstrumentIdError::EmptySymbol(_))
        ));
        assert!(matches!(
            InstrumentId::parse("OKX:BTCUSDT"),
            Err(InstrumentIdError::InvalidSource(_))
        ));
        assert!(matches!(
            InstrumentId::parse("bin ance:BTC"),
            Err(InstrumentIdError::InvalidSource(_))
        ));
        assert_eq!(
            InstrumentId::parse(&format!("okx:{}", "X".repeat(200))),
            Err(InstrumentIdError::TooLong)
        );
    }

    #[test]
    fn display_honours_width_and_alignment() {
        let id = InstrumentId::parse("okx:BTC").unwrap();
        assert_eq!(format!("[{id:<10}]"), "[okx:BTC   ]");
        assert_eq!(format!("[{id:>10}]"), "[   okx:BTC]");
    }

    #[test]
    fn converts_from_borrowed_and_owned_forms() {
        let id = InstrumentId::parse("okx:BTC-USDT").unwrap();
        let from_ref: InstrumentId = (&id).into();
        let from_string: InstrumentId = String::from("okx:BTC-USDT").try_into().unwrap();
        let back: String = id.clone().into();
        assert_eq!(from_ref, id);
        assert_eq!(from_string, id);
        assert_eq!(back, "okx:BTC-USDT");
    }

    #[test]
    fn round_trips_through_json_as_a_plain_string() {
        let id = InstrumentId::parse("okx:BTC-USDT").unwrap();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"okx:BTC-USDT\"");
        assert_eq!(serde_json::from_str::<InstrumentId>(&json).unwrap(), id);
    }

    #[test]
    fn rejects_bad_ids_at_deserialise_time() {
        assert!(serde_json::from_str::<InstrumentId>("\"nope\"").is_err());
    }

    #[cfg(feature = "registry")]
    #[test]
    fn can_join_agrees_with_new() {
        assert!(InstrumentId::can_join("okx", "BTC-USDT"));
        assert!(!InstrumentId::can_join("okx", "   "));
        assert!(!InstrumentId::can_join("okx", &"X".repeat(200)));
        assert!(InstrumentId::new("okx", "BTC-USDT").is_ok());
        assert!(InstrumentId::new("okx", "   ").is_err());
        assert!(InstrumentId::new("okx", &"X".repeat(200)).is_err());
    }

    #[test]
    fn holds_one_allocation_not_two() {
        let id = InstrumentId::parse("binance-spot:XAUTUSDT").unwrap();
        assert_eq!(
            id.source().as_ptr() as usize + "binance-spot:".len(),
            id.symbol().as_ptr() as usize
        );
    }
}
