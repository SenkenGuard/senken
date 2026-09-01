use serde::Serialize;
use utoipa::ToSchema;

/// The body of every non-2xx JSON error response this crate returns.
///
/// One shape for every failure (the login requirement —
/// "identical response... for unknown-user and wrong-password" —
/// generalised to every endpoint): a caller branches on the HTTP status,
/// never on the body's contents.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    /// A human-readable explanation. Never account-specific detail that
    /// would help enumerate accounts or diagnose *why* a request was
    /// rejected beyond what the status code already says.
    pub error: String,
}

impl ErrorBody {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}
