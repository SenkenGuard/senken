//! CORS policy: deny by default, allow the server's own
//! origin, and treat any additional origin as an explicit setting — never a
//! wildcard.

use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Builds the CORS layer for `allowed_origins`
/// `ServeOptions::allowed_origins`).
///
/// The server's own origin needs no entry here at all: browsers only apply
/// CORS to *cross-origin* requests, and the desktop client's embedded
/// server, `senken serve`'s own page, and the Vite dev proxy all serve the
/// API from the same origin the page was loaded from. This layer exists
/// only for the case B1 introduces — a client pointed at a different,
/// remote server — and an empty `allowed_origins` denies every one of
/// those, matching "deny by default."
///
/// `allow_credentials` stays `false`: the session travels as
/// `Authorization: Bearer`, which a browser never attaches automatically,
/// so this server has no cookie-based credential for a wildcard-plus-
/// credentials mistake to even be possible.
pub(crate) fn build(allowed_origins: &[String]) -> CorsLayer {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|origin| match origin.parse::<HeaderValue>() {
            Ok(value) => Some(value),
            Err(source) => {
                tracing::warn!(origin, %source, "ignoring an invalid CORS origin");
                None
            }
        })
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_credentials(false)
}
