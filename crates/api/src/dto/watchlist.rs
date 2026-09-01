use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_watchlist::{WatchlistGroupSummary, WatchlistMember};

/// A watchlist group row (list/create/rename/delete).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WatchlistGroupDto {
    /// The group's id.
    pub id: String,
    /// The group's display name.
    pub name: String,
    /// This group's display order among its owner's other groups.
    pub position: u32,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the group's own fields.
    pub updated_at: i64,
}

impl From<WatchlistGroupSummary> for WatchlistGroupDto {
    fn from(summary: WatchlistGroupSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            name: summary.name,
            position: summary.position,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/watchlists` response body (scope reaches the query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WatchlistGroupsPage {
    /// The rows for this page.
    pub rows: Vec<WatchlistGroupDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/watchlists` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateWatchlistGroupRequest {
    /// The new group's display name.
    pub name: String,
}

/// `PATCH /api/watchlists/{group_id}` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RenameWatchlistGroupRequest {
    /// The group's new display name.
    pub name: String,
}

/// `POST /api/watchlists/reorder` request body: `ids[0]` becomes the first
/// group, `ids[1]` the second, and so on.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReorderWatchlistGroupsRequest {
    /// Every group id the caller owns, in the new display order.
    pub ids: Vec<String>,
}

/// One instrument's membership in a watchlist group, as read back from a
/// listing.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WatchlistMemberDto {
    /// The member's id.
    pub id: String,
    /// The group this membership belongs to.
    pub group_id: String,
    /// The watched instrument, `source:symbol`.
    pub instrument: String,
    /// This member's display order within its group.
    pub position: u32,
}

impl From<WatchlistMember> for WatchlistMemberDto {
    fn from(member: WatchlistMember) -> Self {
        Self {
            id: member.id.to_string(),
            group_id: member.group_id.to_string(),
            instrument: member.instrument.as_str().to_owned(),
            position: member.position,
        }
    }
}

/// `POST /api/watchlists/{group_id}/members` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AddWatchlistMemberRequest {
    /// The instrument to add, `source:symbol`.
    pub instrument: String,
}

/// `POST /api/watchlists/{group_id}/members/reorder` request body, the
/// member counterpart of [`ReorderWatchlistGroupsRequest`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReorderWatchlistMembersRequest {
    /// Every member id in this group, in the new display order.
    pub ids: Vec<String>,
}
