//! Argon2id password hashing, with parameters fixed here rather
//! than left to this crate's defaults — the OWASP Password Storage Cheat
//! Sheet's second recommended configuration (19 MiB, 2 iterations), chosen
//! over the alternative (46 MiB, 1 iteration) because Senken shares a
//! laptop with a chart engine and a Parquet cache, and memory is the
//! scarcer resource here.

use std::sync::OnceLock;

use argon2::{Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version};

use crate::error::IdentityError;

/// Minimum password length: a length floor and nothing else
///   — no composition rules, no forced rotation. Composition rules produce
/// `Password1!`; rotation produces `Password2!`.
///
/// Eight is the ordinary floor people expect. The plan first said twelve,
/// which was a preference rather than a requirement, and a longer minimum
/// than users meet elsewhere mostly teaches them to reuse a password they
/// already have.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Memory cost in KiB (19 MiB).
const M_COST: u32 = 19_456;
/// Iteration count.
const T_COST: u32 = 2;
/// Degree of parallelism.
const P_COST: u32 = 1;

/// A password used to compute a hash nobody ever logs in with, so that a
/// login attempt for an unknown email still pays the full Argon2 cost
/// instead of returning early and leaking, through timing,
/// that the account does not exist.
const DUMMY_PASSWORD: &str = "senken-dummy-password-for-timing-only";

/// Builds the `Argon2` instance this crate always uses. A `Params::new`
/// failure here would mean the constants above are wrong, which is a bug
/// in this file, not a runtime condition — so this panics rather than
/// threading an error through every caller for something that can only
/// fail at compile-time-fixed values.
fn argon2() -> Argon2<'static> {
    let params =
        Params::new(M_COST, T_COST, P_COST, None).expect("hard-coded Argon2 parameters are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Rejects a password shorter than [`MIN_PASSWORD_LEN`]. Length is counted
/// in `char`s, not bytes, so a password using multi-byte characters is not
/// penalised for its encoding.
///
/// # Errors
/// [`IdentityError::PasswordTooShort`] if `password` is too short.
pub(crate) fn check_password_len(password: &str) -> Result<(), IdentityError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(IdentityError::PasswordTooShort {
            minimum: MIN_PASSWORD_LEN,
        });
    }
    Ok(())
}

/// Hashes `password`, returning a PHC string safe to store in
/// `users.password_hash`.
///
/// # Errors
/// [`IdentityError::Hashing`] if Argon2 itself fails (e.g. `password`
/// longer than Argon2's internal limit).
pub(crate) fn hash_password(password: &str) -> Result<String, IdentityError> {
    let hash = argon2().hash_password(password.as_bytes())?;
    Ok(hash.to_string())
}

/// `true` if `password` matches the PHC-encoded hash previously produced by
/// [`hash_password`].
///
/// A malformed `phc` (a hand-edited row, or one written by an incompatible
/// version of this crate) is treated as "does not match" rather than
/// propagated as an error: either way the caller's answer is "this
/// credential does not authenticate", and surfacing a different error type
/// for corrupt-hash-vs-wrong-password would reopen the account-enumeration
/// hole [`verify_dummy`] exists to close.
#[must_use]
pub(crate) fn verify_password(password: &str, phc: &str) -> bool {
    argon2().verify_password(password.as_bytes(), phc).is_ok()
}

/// How many times [`verify_dummy`] has actually run the Argon2 verify,
/// counted only in test builds. Production has no use for this — it exists
/// so a test can *assert* the dummy-hash path executed for an unknown-email
/// login instead of merely assuming it from the returned error looking
/// right ("the dummy-hash verify actually runs —
/// assert it, do not assume").
#[cfg(test)]
pub(crate) static DUMMY_VERIFY_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Runs a full Argon2 verify against a fixed dummy hash, ignoring the
/// result. Call this exactly where a real [`verify_password`] call would
/// otherwise have gone — when the account looked up by
/// [`IdentityStore::login`](crate::IdentityStore::login) does not exist, or
/// has no password set — so that "no such account" costs the same wall
/// clock time as "wrong password".
pub(crate) fn verify_dummy(password: &str) {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(|| {
        hash_password(DUMMY_PASSWORD).expect("hashing the fixed dummy password cannot fail")
    });
    let _ = verify_password(password, dummy);
    #[cfg(test)]
    DUMMY_VERIFY_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::{
        DUMMY_PASSWORD, DUMMY_VERIFY_CALLS, IdentityError, MIN_PASSWORD_LEN, check_password_len,
        hash_password, verify_dummy, verify_password,
    };

    #[test]
    fn a_password_at_the_minimum_length_is_accepted() {
        assert!(check_password_len(&"a".repeat(MIN_PASSWORD_LEN)).is_ok());
    }

    #[test]
    fn a_password_one_short_of_the_minimum_is_rejected() {
        let err = check_password_len(&"a".repeat(MIN_PASSWORD_LEN - 1)).unwrap_err();
        assert!(matches!(
            err,
            IdentityError::PasswordTooShort {
                minimum: MIN_PASSWORD_LEN
            }
        ));
    }

    #[test]
    fn length_is_counted_in_chars_not_bytes() {
        // Each of these is a two-byte UTF-8 character; 12 of them is a
        // valid password by char count despite being 24 bytes.
        let password: String = "é".repeat(MIN_PASSWORD_LEN);
        assert!(check_password_len(&password).is_ok());
    }

    #[test]
    fn a_hashed_password_verifies_against_itself() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
    }

    #[test]
    fn a_hashed_password_does_not_verify_against_a_different_password() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(!verify_password("wrong password entirely", &hash));
    }

    #[test]
    fn a_malformed_hash_fails_verification_instead_of_panicking_or_erroring() {
        assert!(!verify_password("anything", "not a phc string"));
    }

    #[test]
    fn the_stored_hash_is_never_the_plaintext_password() {
        let hash = hash_password(DUMMY_PASSWORD).unwrap();
        assert_ne!(hash, DUMMY_PASSWORD);
        assert!(!hash.contains(DUMMY_PASSWORD));
    }

    #[test]
    fn verify_dummy_runs_without_panicking_regardless_of_the_supplied_password() {
        // There is nothing to assert about the *result* — the point of
        // `verify_dummy` is the side effect of spending the same wall-clock
        // time a real verification would, not any particular answer.
        verify_dummy("whatever a caller happened to type");
    }

    #[test]
    fn verify_dummy_actually_runs_the_argon2_verify_rather_than_being_a_no_op() {
        // `DUMMY_VERIFY_CALLS` is one process-wide static and `cargo test`
        // runs tests concurrently, so `>` (not `== before + 1`) is the
        // assertion that is actually safe under parallel execution — other
        // tests may add calls between the two reads, but never remove the
        // one this test makes.
        let before = DUMMY_VERIFY_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        verify_dummy("anything");
        let after = DUMMY_VERIFY_CALLS.load(std::sync::atomic::Ordering::SeqCst);
        assert!(after > before);
    }
}
