//! The indicator language's version, as this host currently compiles it.
//!
//! `senken-indicator-lang` does not expose a version of its own — it is an
//! internal compiler, versioned from `workspace.package.version` like every
//! other crate in this application rather than decoupled from the
//! application's own release cadence the way `senken-plugin-api` (a crate
//! published externally) deliberately is. This crate is versioned the same
//! way, from the same `workspace.package.version`, so its own
//! `CARGO_PKG_VERSION` and the compiler's evolve in lockstep by
//! construction: whenever a change to the language would break an older
//! published indicator, that change ships in the same release that bumps
//! this number.
//!
//! A published indicator records the *publishing* host's [`HOST_LANGUAGE_VERSION`]
//! at the moment it is compiled for validation (see
//! [`crate::RegistryStore::publish`]) — never a value the publisher's HTTP
//! request supplies, which would let anyone claim any version. An install
//! then compares that recorded version against the *installing* host's own
//! [`HOST_LANGUAGE_VERSION`]: two different Senken builds, potentially
//! months apart, are exactly the case this check exists for.

use crate::error::RegistryError;

/// The version of the indicator language this build compiles — see this
/// module's docs for why it is this crate's own package version rather
/// than `senken-indicator-lang`'s.
pub const HOST_LANGUAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parses a `major.minor.patch` version string into a tuple that orders the
/// same way semver precedence does for this narrow purpose (no pre-release
/// or build-metadata component — every version in play here comes from
/// `CARGO_PKG_VERSION`, which is always a plain three-part number).
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Rejects `required` when it names a language version newer than this
/// host currently compiles, naming both versions in the error — never a
/// bare failure to load.
///
/// # Errors
/// [`RegistryError::LanguageVersionTooNew`] if `required` parses and is
/// greater than [`HOST_LANGUAGE_VERSION`]. An unparseable `required` (which
/// should never happen — this crate is the only writer of the column it
/// comes from) is treated the same as "too new" rather than silently
/// accepted, since there is no version it could be that this host can
/// vouch for.
pub(crate) fn ensure_host_supports(required: &str) -> Result<(), RegistryError> {
    let host = HOST_LANGUAGE_VERSION;
    let host_parsed = parse(host);
    let required_parsed = parse(required);
    let supported = matches!((required_parsed, host_parsed), (Some(r), Some(h)) if r <= h);
    if supported {
        Ok(())
    } else {
        Err(RegistryError::LanguageVersionTooNew {
            required: required.to_owned(),
            host: host.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HOST_LANGUAGE_VERSION, ensure_host_supports};

    #[test]
    fn the_hosts_own_version_is_always_supported() {
        ensure_host_supports(HOST_LANGUAGE_VERSION).unwrap();
    }

    #[test]
    fn an_older_version_is_supported() {
        ensure_host_supports("0.0.0").unwrap();
    }

    #[test]
    fn a_newer_version_is_rejected_naming_both_versions() {
        let error = ensure_host_supports("999.0.0").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("999.0.0"), "{message}");
        assert!(message.contains(HOST_LANGUAGE_VERSION), "{message}");
    }

    #[test]
    fn an_unparseable_version_is_rejected_rather_than_silently_accepted() {
        ensure_host_supports("not-a-version").unwrap_err();
    }
}
