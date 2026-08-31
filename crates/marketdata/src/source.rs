//! The contract a venue adapter implements, and the errors it may return.

use std::error::Error;
use std::fmt;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::instrument::Instrument;

/// A type-erased error a source can wrap.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Why a source could not deliver its instruments.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// The request never completed: DNS, connect, timeout, TLS.
    #[error("transport to source failed")]
    Transport {
        /// The underlying failure.
        #[source]
        source: BoxError,
    },

    /// The venue answered with a non-success HTTP status.
    #[error("source returned HTTP {status}: {body}")]
    Http {
        /// The HTTP status code.
        status: u16,
        /// A (possibly truncated) copy of the response body.
        body: String,
    },

    /// The venue rejected the request at the application level, e.g. an
    /// error code inside an HTTP 200 body.
    #[error("source rejected the request: {reason}")]
    Rejected {
        /// The venue's own explanation.
        reason: String,
    },

    /// The body did not have the shape the adapter expects.
    #[error("source response did not match the expected shape")]
    Decode {
        /// The parse failure.
        #[source]
        source: BoxError,
    },
}

impl SourceError {
    /// Wraps a transport-level failure.
    pub fn transport(source: impl Into<BoxError>) -> Self {
        Self::Transport {
            source: source.into(),
        }
    }

    /// Wraps a non-success HTTP response. The body is truncated to a sane
    /// length so an HTML error page does not end up in a log line.
    #[must_use]
    pub fn http(status: u16, body: impl Into<String>) -> Self {
        const MAX_BODY: usize = 512;
        let mut body = body.into();
        if body.len() > MAX_BODY {
            let cut = (0..=MAX_BODY)
                .rev()
                .find(|&i| body.is_char_boundary(i))
                .unwrap_or(0);
            body.truncate(cut);
            body.push('…');
        }
        Self::Http { status, body }
    }

    /// Wraps an application-level rejection.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::Rejected {
            reason: reason.into(),
        }
    }

    /// Wraps a decode failure.
    pub fn decode(source: impl Into<BoxError>) -> Self {
        Self::Decode {
            source: source.into(),
        }
    }

    /// `true` when trying again later is reasonable: transport failures,
    /// rate limiting (429), request timeouts (408) and server errors (5xx).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport { .. } => true,
            Self::Http { status, .. } => {
                matches!(*status, 408 | 429) || (500..=599).contains(status)
            }
            Self::Rejected { .. } | Self::Decode { .. } => false,
        }
    }
}

/// A venue that can list its instruments.
///
/// Implementors must be cheap to construct and hold no exclusive resources:
/// [`MarketData`](crate::MarketData) calls [`instruments`](Self::instruments)
/// at most once per cache lifetime, never concurrently for the same source.
#[async_trait]
pub trait MarketDataSource: Send + Sync {
    /// Stable, unique, lowercase `[a-z0-9-]` identifier such as `binance-spot`.
    /// It becomes the source half of every [`InstrumentId`](crate::InstrumentId).
    fn id(&self) -> &str;

    /// Human-readable name such as `Binance Spot`.
    fn name(&self) -> &str;

    /// Fetches the full instrument list from the venue.
    ///
    /// # Errors
    /// See [`SourceError`].
    async fn instruments(&self) -> Result<Vec<Instrument>, SourceError>;
}

impl fmt::Debug for dyn MarketDataSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarketDataSource")
            .field("id", &self.id())
            .field("name", &self.name())
            .finish()
    }
}

/// A registered source, as reported without loading its catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSummary {
    /// The source id.
    pub id: String,
    /// The source's display name.
    pub name: String,
}

/// A source together with statistics about its loaded catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceDetail {
    /// The source id.
    pub id: String,
    /// The source's display name.
    pub name: String,
    /// Instruments in the catalog, whatever their status.
    pub instrument_count: usize,
    /// Instruments whose status is [`Trading`](crate::InstrumentStatus::Trading).
    pub tradable_count: usize,
    /// When the catalog was fetched from the venue.
    pub synced_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::SourceError;

    #[test]
    fn rate_limits_and_server_errors_are_retryable() {
        assert!(SourceError::http(429, "slow down").is_retryable());
        assert!(SourceError::http(503, "").is_retryable());
        assert!(SourceError::transport("boom").is_retryable());
        assert!(!SourceError::http(400, "bad").is_retryable());
        assert!(!SourceError::http(418, "banned").is_retryable());
        assert!(!SourceError::rejected("code 50011").is_retryable());
    }

    #[test]
    fn long_bodies_are_truncated_on_a_char_boundary() {
        let body = "é".repeat(1000);
        let SourceError::Http { body, .. } = SourceError::http(500, body) else {
            panic!("wrong variant");
        };
        assert!(body.len() <= 512 + '…'.len_utf8());
        assert!(body.ends_with('…'));
    }
}
