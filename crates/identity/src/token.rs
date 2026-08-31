//! Opaque session tokens: generated with the OS RNG, never
//! stored raw, and never compared with a plain `==`.

use rand::RngExt;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Bytes of entropy in a freshly generated token. 256 bits, matching the
/// digest size of the hash it is stored as — enough that guessing one is
/// infeasible regardless of how many live sessions exist.
const TOKEN_BYTES: usize = 32;

/// A session token as handed to a caller of
/// [`IdentityStore::login`](crate::IdentityStore::login): the only time the
/// raw value exists outside this process's memory. The database never sees
/// it — only a hash of it (this crate's private `TokenHash`).
#[derive(Clone, PartialEq, Eq)]
pub struct RawSessionToken(String);

impl std::fmt::Debug for RawSessionToken {
    /// Redacted on purpose — the derived `Debug` would print a live
    /// credential into any panic message or log line that touches a
    /// `Result` carrying this type (e.g. `unwrap_err` on a `Result<(UserId,
    /// RawSessionToken), _>`, which needs the `Ok` side to be `Debug` even
    /// when only the `Err` side is printed).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RawSessionToken(REDACTED)")
    }
}

impl RawSessionToken {
    /// A fresh token from the OS RNG (never a seeded PRNG),
    /// encoded as lowercase hex so it is plain ASCII wherever it travels
    /// (an `Authorization` header, a URL-unsafe-free ticket, a log line
    /// that must never contain it).
    pub(crate) fn generate() -> Self {
        let mut bytes = [0_u8; TOKEN_BYTES];
        rand::rng().fill(&mut bytes);
        Self(to_hex(&bytes))
    }

    /// The token as given to a client. Not [`Display`](std::fmt::Display)
    /// or [`Debug`](std::fmt::Debug) on purpose — printing a session token
    /// anywhere (a log line, a panic message) hands out a live credential.
    #[must_use]
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

/// A SHA-256 digest of a [`RawSessionToken`] — what `sessions.token_hash`
/// actually stores ("a read-only leak of the database must not hand over live sessions"). A fast, unsalted hash is safe here
/// specifically *because* the input is already 256 bits of RNG output, not
/// a human-chosen password: there is nothing for an offline attacker to
/// dictionary-search, only a preimage to find, which a fast hash resists
/// exactly as well as a slow one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TokenHash([u8; 32]);

impl TokenHash {
    /// Hashes a raw token (either a freshly generated one, before its first
    /// save, or one presented back by a client to be looked up).
    pub(crate) fn of(raw: &str) -> Self {
        let digest = Sha256::digest(raw.as_bytes());
        Self(digest.into())
    }

    /// Constant-time equality ("a plain `==` on a session token leaks its prefix through timing"). Used to double-check a row
    /// fetched by its indexed hash before trusting it — the index lookup
    /// narrows candidates by a value that is already a one-way hash of the
    /// secret, but the final accept/reject decision never goes through
    /// `PartialEq` on secret-derived bytes.
    pub(crate) fn ct_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl ToSql for TokenHash {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.to_vec()))
    }
}

impl FromSql for TokenHash {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> FromSqlResult<Self> {
        let bytes = value.as_blob()?;
        <[u8; 32]>::try_from(bytes)
            .map(Self)
            .map_err(|_| FromSqlError::InvalidBlobSize {
                expected_size: 32,
                blob_size: bytes.len(),
            })
    }
}

/// Lowercase hex encoding. Small enough not to justify a dependency on the
/// `hex` crate for the one call site that needs it.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{RawSessionToken, TokenHash};

    #[test]
    fn two_freshly_generated_tokens_differ() {
        assert_ne!(
            RawSessionToken::generate().reveal(),
            RawSessionToken::generate().reveal()
        );
    }

    #[test]
    fn a_generated_token_is_lowercase_hex_of_the_expected_length() {
        let token = RawSessionToken::generate();
        let text = token.reveal();
        assert_eq!(text.len(), 64, "32 bytes as hex is 64 characters");
        assert!(
            text.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn hashing_the_same_token_twice_gives_the_same_hash() {
        let token = RawSessionToken::generate();
        assert!(TokenHash::of(token.reveal()).ct_eq(&TokenHash::of(token.reveal())));
    }

    #[test]
    fn hashing_different_tokens_gives_different_hashes() {
        let a = RawSessionToken::generate();
        let b = RawSessionToken::generate();
        assert!(!TokenHash::of(a.reveal()).ct_eq(&TokenHash::of(b.reveal())));
    }

    #[test]
    fn the_hash_is_not_the_raw_token() {
        let token = RawSessionToken::generate();
        // `TokenHash` has no accessor that would let this compare directly,
        // which is itself the point — assert the shape instead: the hash's
        // `ToSql` output is 32 raw bytes, the token's `reveal()` is a
        // 64-character hex string, so the two could not be equal even if
        // compared.
        assert_eq!(std::mem::size_of::<TokenHash>(), 32);
        assert_eq!(token.reveal().len(), 64);
    }
}
