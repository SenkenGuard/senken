//! How many times, and how long, a failed request is retried.
//!
//! Kept as a value the caller supplies rather than a crate constant, because
//! the right answer depends on who is waiting: a chart open on an interactive
//! fetch should give up quickly, while a background backfill can afford to
//! keep trying.

use std::time::Duration;

/// How many attempts a request gets, and how the wait between them grows.
///
/// The wait between attempt `n` and `n + 1` is `first_backoff * 2^(n - 1)`,
/// then run through full jitter (sampled uniformly from `[0, wait]`) so
/// concurrent callers do not retry in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts, including the first. `1` means "no retry".
    pub max_attempts: u32,
    /// The backoff before the second attempt; it doubles after that.
    pub first_backoff: Duration,
}

impl RetryPolicy {
    /// A user is waiting: fail fast rather than make a chart hang.
    ///
    /// Matches the retry behaviour `senken-venue` already had before M3
    /// (`MAX_ATTEMPTS = 3`, `FIRST_BACKOFF = 250ms`) — M3 adds jitter and
    /// makes the numbers a parameter, it does not change the interactive
    /// default.
    pub const INTERACTIVE: Self = Self {
        max_attempts: 3,
        first_backoff: Duration::from_millis(250),
    };

    /// Nobody is watching a spinner: a background backfill can spend more
    /// attempts riding out transient trouble before giving up a chunk.
    ///
    /// `8` is a deliberately conservative default, not a venue-derived
    /// figure — no venue documents how many retries a backfill "should" get,
    /// because that is a policy choice on our side, not theirs.
    pub const BACKFILL: Self = Self {
        max_attempts: 8,
        first_backoff: Duration::from_millis(250),
    };
}

impl Default for RetryPolicy {
    /// Callers that do not say otherwise get the interactive policy: it is
    /// the behaviour every existing caller of [`fetch_bytes`](crate::fetch_bytes)
    /// already saw before M3.
    fn default() -> Self {
        Self::INTERACTIVE
    }
}
