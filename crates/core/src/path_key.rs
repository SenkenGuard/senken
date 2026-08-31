//! Percent-encoding an untrusted symbol into a safe filesystem path
//! component, and back.
//!
//! This is a pure function, not a stored field: plugins normalise
//! *symbols*, which is a market concern, but filesystem encoding is not —
//! it belongs in exactly one place, tested once. A stored field would also
//! be denormalised state that drifts the moment the encoder's rules change.

use std::borrow::Cow;

/// Windows reserved device names. Comparison against them is
/// case-insensitive, and unaffected by anything appended after them.
const DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Encodes `symbol` into a string safe to use as a single path component on
/// every platform this project targets (POSIX and Windows alike).
///
/// Rules, applied in order:
///
/// 1. Percent-encode every byte outside `[A-Za-z0-9_-]`, including `%`
///    itself, with uppercase hex. This encodes `.` unconditionally, which
///    removes the Windows trailing-dot trap without a separate positional
///    rule.
/// 2. If the result is a Windows device name (`CON`, `PRN`, `AUX`, `NUL`,
///    `COM1`–`COM9`, `LPT1`–`LPT9`, case-insensitively), percent-encode its
///    first character: `CON` becomes `%43ON`. No special case is needed
///    when decoding — [`symbol_from_path`] reverses both rules with the
///    same percent-decode pass.
///
/// Returns [`Cow::Borrowed`] when neither rule changed anything, which is
/// the common case: measured on the live catalog, roughly 98% of symbols
/// contain no character outside `[A-Za-z0-9_-]`.
///
/// # Examples
/// ```
/// use std::borrow::Cow;
/// use senken_core::path_key::path_key;
///
/// assert!(matches!(path_key("BTCUSDT"), Cow::Borrowed("BTCUSDT")));
/// assert_eq!(path_key("D.O.G.E."), "D%2EO%2EG%2EE%2E");
/// assert_eq!(path_key("CON"), "%43ON");
/// ```
#[must_use]
pub fn path_key(symbol: &str) -> Cow<'_, str> {
    let percent_encoded = if symbol.bytes().any(needs_escape) {
        let mut out = String::with_capacity(symbol.len());
        for byte in symbol.bytes() {
            if needs_escape(byte) {
                push_percent_encoded(&mut out, byte);
            } else {
                out.push(char::from(byte));
            }
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(symbol)
    };

    if is_device_name(&percent_encoded) {
        Cow::Owned(escape_first_byte(&percent_encoded))
    } else {
        percent_encoded
    }
}

/// Reverses [`path_key`]: plain percent-decoding undoes both the escaping
/// and the device-name rule, since the device-name rule only ever produces
/// a percent-escape of its own.
///
/// # Errors
///
/// Returns [`PathKeyError`] rather than panicking on a `%` not followed by
/// two hex digits, or on decoded bytes that are not valid UTF-8.
///
/// # Examples
/// ```
/// use senken_core::path_key::{path_key, symbol_from_path};
///
/// let key = path_key("D.O.G.E.");
/// assert_eq!(symbol_from_path(&key).unwrap(), "D.O.G.E.");
/// ```
pub fn symbol_from_path(key: &str) -> Result<String, PathKeyError> {
    let bytes = key.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let escape = bytes
                .get(i + 1..i + 3)
                .ok_or_else(|| PathKeyError::Truncated(key.to_owned()))?;
            let byte = decode_hex_pair(escape)
                .ok_or_else(|| PathKeyError::InvalidEscape(hex_text(escape), key.to_owned()))?;
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| PathKeyError::InvalidUtf8(key.to_owned()))
}

/// Why [`symbol_from_path`] could not decode a key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathKeyError {
    /// A `%` near the end of the string was not followed by two more bytes.
    #[error("truncated percent-escape in {0:?}")]
    Truncated(String),
    /// The two bytes after a `%` were not both hex digits.
    #[error("invalid percent-escape %{0} in {1:?}")]
    InvalidEscape(String, String),
    /// The decoded bytes were not valid UTF-8.
    #[error("percent-decoding {0:?} did not produce valid UTF-8")]
    InvalidUtf8(String),
}

/// `true` for a byte that must be percent-encoded: anything outside
/// `[A-Za-z0-9_-]`.
fn needs_escape(byte: u8) -> bool {
    !(byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Appends `%XX` for `byte`, uppercase hex.
fn push_percent_encoded(out: &mut String, byte: u8) {
    out.push('%');
    out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
    out.push(char::from(HEX_DIGITS[usize::from(byte & 0x0F)]));
}

/// Percent-encodes only the first byte of `s`.
///
/// Called only on strings that matched [`is_device_name`], which are all
/// pure ASCII, so indexing and slicing at byte offset 1 is always a valid
/// char boundary.
fn escape_first_byte(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + 2);
    push_percent_encoded(&mut out, bytes[0]);
    out.push_str(&s[1..]);
    out
}

/// `true` when `s` is one of [`DEVICE_NAMES`], compared case-insensitively.
fn is_device_name(s: &str) -> bool {
    DEVICE_NAMES.iter().any(|name| s.eq_ignore_ascii_case(name))
}

/// Decodes two ASCII hex digits into the byte they represent.
fn decode_hex_pair(pair: &[u8]) -> Option<u8> {
    let hi = hex_value(pair[0])?;
    let lo = hex_value(pair[1])?;
    Some((hi << 4) | lo)
}

/// The value of a single ASCII hex digit.
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

/// Renders raw bytes as a lossy string for an error message.
fn hex_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::{PathKeyError, path_key, symbol_from_path};
    use std::borrow::Cow;

    /// The non-alphanumeric characters actually present in the live
    /// catalog, one example symbol per character (measured 2026-08-30: `_`
    /// x659, `$` x10, `(` x9, `)` x9, `.` x4, `@` x1).
    const CATALOG_SYMBOLS: &[&str] = &[
        "PF_AGLDUSD",
        "$UUSDT",
        "ATOM(ARC20)USDT",
        "GOLD(PAXG)USDT",
        "D.O.G.E.USDT",
        "SHIBBTC@OEXHK",
    ];

    #[test]
    fn round_trips_every_character_class_seen_in_the_live_catalog() {
        for symbol in CATALOG_SYMBOLS {
            let key = path_key(symbol);
            let decoded = symbol_from_path(&key).unwrap_or_else(|e| panic!("{symbol}: {e}"));
            assert_eq!(&decoded, symbol, "round-trip failed for {symbol}");
        }
    }

    #[test]
    fn a_plain_symbol_borrows_rather_than_allocating() {
        assert!(matches!(path_key("BTCUSDT"), Cow::Borrowed("BTCUSDT")));
    }

    #[test]
    fn dotted_and_undotted_symbols_are_injective() {
        assert_ne!(path_key("D.O.G.E."), path_key("DOGE"));
    }

    #[test]
    fn every_windows_device_name_is_escaped_case_insensitively() {
        let names = [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ];
        for name in names {
            for variant in [name.to_string(), name.to_lowercase(), titlecase(name)] {
                let key = path_key(&variant);
                assert!(
                    key.starts_with('%'),
                    "{variant} (a device name) was not escaped, got {key}"
                );
                assert_eq!(symbol_from_path(&key).unwrap(), variant);
            }
        }
    }

    fn titlecase(s: &str) -> String {
        let mut chars = s.chars();
        match chars.next() {
            Some(first) => {
                first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
            }
            None => String::new(),
        }
    }

    #[test]
    fn a_non_device_name_containing_one_is_left_alone() {
        // Only an exact match is a device name; a longer symbol is not.
        assert!(matches!(path_key("CONTRACT"), Cow::Borrowed("CONTRACT")));
    }

    #[test]
    fn symbol_from_path_rejects_a_truncated_escape_instead_of_panicking() {
        assert_eq!(
            symbol_from_path("%4"),
            Err(PathKeyError::Truncated("%4".to_string()))
        );
        assert_eq!(
            symbol_from_path("%"),
            Err(PathKeyError::Truncated("%".to_string()))
        );
        assert_eq!(
            symbol_from_path("BTC%"),
            Err(PathKeyError::Truncated("BTC%".to_string()))
        );
    }

    #[test]
    fn symbol_from_path_rejects_non_hex_escapes_instead_of_panicking() {
        assert_eq!(
            symbol_from_path("%ZZ"),
            Err(PathKeyError::InvalidEscape(
                "ZZ".to_string(),
                "%ZZ".to_string()
            ))
        );
        assert_eq!(
            symbol_from_path("%2G"),
            Err(PathKeyError::InvalidEscape(
                "2G".to_string(),
                "%2G".to_string()
            ))
        );
    }

    #[test]
    fn symbol_from_path_rejects_decoded_bytes_that_are_not_utf8() {
        // 0xFF alone is never valid UTF-8, on its own or as a continuation.
        assert_eq!(
            symbol_from_path("%FF"),
            Err(PathKeyError::InvalidUtf8("%FF".to_string()))
        );
    }

    #[test]
    fn hex_digits_are_uppercase() {
        assert_eq!(path_key("."), "%2E");
        assert_ne!(path_key("."), "%2e");
    }
}
