//! Why a trade-engine operation failed.

use std::error::Error;

use senken_marketdata::InstrumentId;

use crate::id::PositionId;

/// A type-erased error an adapter can wrap.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Everything an adapter call, or the engine in front of it, can fail with.
///
/// Split finely on purpose. "The venue said no" and "we never reached the
/// venue" are the same outcome for the caller's order but not for what
/// happens next: a rejection is final and must be shown, a transport
/// failure leaves the order's fate genuinely unknown and must never be
/// reported as "not placed".
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TradeError {
    /// No adapter is registered under this id.
    #[error("unknown trade adapter `{0}`")]
    UnknownAdapter(String),

    /// An adapter with this id is already registered.
    #[error("trade adapter `{0}` is already registered")]
    DuplicateAdapter(String),

    /// The adapter id is not a valid slug.
    #[error("trade adapter id `{0}` must be lowercase [a-z0-9-]")]
    InvalidAdapterId(String),

    /// The account is not attached, or not visible to the caller.
    ///
    /// Deliberately the same answer for "does not exist" and "is not
    /// yours": whether someone else's account exists is not a fact a
    /// caller who cannot reach it should be able to establish.
    #[error("no trade account found for that id")]
    UnknownAccount,

    /// The account exists and is the caller's, but its owner has turned it
    /// off.
    #[error("this account is disabled")]
    AccountDisabled,

    /// The account may be read but not traded — a read-only login (an
    /// investor password, an API key minted without trade scope), reported
    /// by the adapter itself rather than discovered from a rejection at the
    /// venue.
    #[error("this account is read-only{}", note.as_ref().map(|n| format!(": {n}")).unwrap_or_default())]
    ReadOnly {
        /// The adapter that reported the restriction.
        adapter: String,
        /// One line of product copy explaining why, when the adapter gave
        /// one.
        note: Option<String>,
    },

    /// The caller already has an account by this name on this adapter.
    #[error("you already have an account with that name on this adapter")]
    DuplicateLabel,

    /// The underlying SQLite call failed.
    #[cfg(feature = "accounts")]
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The permission check itself — `senken_identity`'s own error, reused
    /// rather than re-declared so a caller sees the same
    /// `PasswordNotSet`/`Forbidden` every other guarded store produces.
    #[cfg(feature = "accounts")]
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),

    /// The adapter does not trade this instrument.
    #[error("adapter `{adapter}` does not trade {instrument}")]
    InstrumentNotTradable {
        /// The adapter that was asked.
        adapter: String,
        /// The instrument it does not cover.
        instrument: InstrumentId,
    },

    /// The instrument is not in any catalog this installation knows about,
    /// so its tick size and step size — which every price and quantity must
    /// be rounded to — cannot be read.
    #[error("instrument {0} is not in any loaded catalog")]
    UnknownInstrument(InstrumentId),

    /// No price is available to mark this instrument at.
    ///
    /// A separate variant rather than a generic rejection because it is the
    /// one failure a user can actually fix themselves, by loading history
    /// for the instrument.
    #[error("no price is available for {0} yet")]
    NoMarkPrice(InstrumentId),

    /// The request is malformed or violates the instrument's own rules —
    /// a price off the tick, a quantity under the minimum, a limit order to
    /// an adapter that has no limit orders.
    #[error("{0}")]
    InvalidRequest(String),

    /// The account does not hold enough to cover this order.
    #[error("insufficient balance: {0}")]
    InsufficientBalance(String),

    /// No order with this id exists on this account.
    #[error("no order found for that id")]
    UnknownOrder,

    /// No open position exists for this instrument.
    ///
    /// A close is built from the position the adapter reports, so this is
    /// what a "close" on an instrument nobody holds refuses with — never a
    /// generic `InvalidRequest`, since a client needs to tell "there is
    /// nothing to close" apart from "the request itself was malformed".
    #[error("no open position for {0}")]
    UnknownPosition(InstrumentId),

    /// No position with that id is open on the account.
    ///
    /// Distinct from [`UnknownPosition`](Self::UnknownPosition), which
    /// names an instrument: a hedging account can hold several positions on
    /// one instrument, so "nothing is open on BTCUSDT" and "the position
    /// you asked to close is already gone" are different answers, and a
    /// client that closed a stale row needs to tell them apart.
    #[error("no open position with that id")]
    UnknownPositionId(PositionId),

    /// The order exists but cannot be changed any more — already filled,
    /// already cancelled, already expired.
    #[error("order is no longer open")]
    OrderNotOpen,

    /// The adapter refused: bad credentials, a venue-side error code, an
    /// account restriction. Final; retrying unchanged will fail the same
    /// way.
    #[error("adapter rejected the request: {0}")]
    Rejected(String),

    /// The adapter could not be reached at all.
    ///
    /// **Distinct from [`Rejected`](Self::Rejected) because the order's fate
    /// is unknown**: a request that timed out may still have been accepted
    /// by the venue, so a caller must reconcile rather than assume nothing
    /// happened.
    #[error("could not reach the adapter")]
    Transport {
        /// The underlying failure.
        #[source]
        source: BoxError,
    },

    /// The adapter does not implement this capability at all.
    #[error("adapter `{adapter}` does not support {capability}")]
    Unsupported {
        /// The adapter that was asked.
        adapter: String,
        /// What it cannot do, in words a user can read.
        capability: String,
    },

    /// The settings supplied for an account did not satisfy the adapter's
    /// own schema.
    #[error(transparent)]
    Settings(#[from] crate::settings::SettingsError),

    /// Anything else an adapter needs to report.
    #[error("trade adapter failed")]
    Adapter {
        /// The underlying failure.
        #[source]
        source: BoxError,
    },
}

impl TradeError {
    /// Wraps a transport-level failure — one that leaves the request's fate
    /// unknown.
    pub fn transport(source: impl Into<BoxError>) -> Self {
        Self::Transport {
            source: source.into(),
        }
    }

    /// Wraps an adapter-internal failure.
    pub fn adapter(source: impl Into<BoxError>) -> Self {
        Self::Adapter {
            source: source.into(),
        }
    }

    /// Builds an [`InvalidRequest`](Self::InvalidRequest).
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidRequest(reason.into())
    }

    /// Builds an [`Unsupported`](Self::Unsupported).
    pub fn unsupported(adapter: impl Into<String>, capability: impl Into<String>) -> Self {
        Self::Unsupported {
            adapter: adapter.into(),
            capability: capability.into(),
        }
    }

    /// `true` when the request definitely did **not** reach the venue, so a
    /// caller may safely retry it unchanged.
    ///
    /// [`Transport`](Self::Transport) is deliberately **not** in this set:
    /// a timed-out order may well have been accepted, and blindly retrying
    /// one is how a position ends up doubled.
    #[must_use]
    pub fn is_safe_to_retry(&self) -> bool {
        matches!(
            self,
            Self::NoMarkPrice(_) | Self::UnknownInstrument(_) | Self::UnknownAccount
        )
    }
}

#[cfg(test)]
mod tests {
    use super::TradeError;

    #[test]
    fn a_transport_failure_is_never_reported_as_safe_to_retry() {
        // An order that timed out may have reached the venue; retrying it
        // is how a position silently doubles.
        assert!(!TradeError::transport("connection reset").is_safe_to_retry());
    }

    #[test]
    fn a_rejection_is_final_and_not_retryable() {
        assert!(!TradeError::Rejected("code 51008".to_owned()).is_safe_to_retry());
    }
}
