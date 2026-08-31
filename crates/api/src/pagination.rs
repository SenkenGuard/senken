//! `?limit=&offset=` query parameters shared by every listing endpoint in
//! this crate. `admin_handlers` declared this privately at first; two more
//! listings — workspaces and alerts — need the exact same clamping, so it
//! moved here rather than being copied a second and third time.

use serde::Deserialize;

/// Default page size for a listing endpoint that received no `limit` query
/// parameter.
pub(crate) const DEFAULT_LIMIT: u32 = 50;
/// The largest page a caller may request in one call, regardless of what
/// `limit` asks for — an unbounded `limit` would let a single request force
/// the server to materialise (and the client to receive) an arbitrarily
/// large response.
pub(crate) const MAX_LIMIT: u32 = 200;

/// `?limit=&offset=` query parameters shared by every listing endpoint.
#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct PaginationQuery {
    #[serde(default)]
    pub(crate) limit: Option<u32>,
    #[serde(default)]
    pub(crate) offset: Option<u32>,
}

/// Clamps a caller-supplied page request to `(1..=MAX_LIMIT, offset)`, with
/// [`DEFAULT_LIMIT`] when `limit` is omitted.
pub(crate) fn normalize_pagination(query: PaginationQuery) -> (u32, u32) {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    (limit, query.offset.unwrap_or(0))
}
