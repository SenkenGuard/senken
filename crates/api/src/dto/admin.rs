use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_identity::{RoleSummary, UserSummary};

use super::GrantDto;

/// A response body carrying only a freshly created row's id (`POST /api/users`, `POST /api/roles`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IdResponse {
    /// The id of the row just created.
    pub id: String,
}

/// A user row as the user/role management endpoints report it — the same fields as [`MeResponse`]'s profile half, without the
/// roles/grants that are specific to *the caller's own* identity.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserSummaryDto {
    /// The user's id.
    pub id: String,
    /// The user's email.
    pub email: String,
    /// The user's display name.
    pub display_name: String,
    /// `true` if the account is disabled.
    pub disabled: bool,
    /// `true` once the account has set a password.
    pub password_set: bool,
}

impl From<UserSummary> for UserSummaryDto {
    fn from(summary: UserSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            email: summary.email,
            display_name: summary.display_name,
            disabled: summary.disabled,
            password_set: summary.password_set,
        }
    }
}

/// `GET /api/users` response body (scope reaches the
/// query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsersPage {
    /// The rows for this page.
    pub rows: Vec<UserSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// A role row as the user/role management endpoints report it, including the grants it carries so the client can render the grant
/// matrix `access-section.svelte` already built, disabled, against this
/// exact shape.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RoleSummaryDto {
    /// The role's id.
    pub id: String,
    /// The role's name.
    pub name: String,
    /// A human-readable description.
    pub description: String,
    /// `true` for a role seeded by `senken-identity` (e.g. `Superadmin`)
    /// rather than created by an admin.
    pub builtin: bool,
    /// The grants this role carries.
    pub grants: Vec<GrantDto>,
}

impl From<RoleSummary> for RoleSummaryDto {
    fn from(summary: RoleSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            name: summary.name,
            description: summary.description,
            builtin: summary.builtin,
            grants: summary.grants.into_iter().map(GrantDto::from).collect(),
        }
    }
}

/// `GET /api/roles` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RolesPage {
    /// The rows for this page.
    pub rows: Vec<RoleSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/users` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateUserRequest {
    /// The new account's email.
    pub email: String,
    /// The new account's display name.
    pub display_name: String,
    /// An initial password. Omitted (or `null`) leaves the account behind
    /// the same password fence the default admin is seeded with, so the new user
    /// sets their own password on first use.
    #[serde(default)]
    pub initial_password: Option<String>,
}

/// `POST /api/roles` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateRoleRequest {
    /// The role's name.
    pub name: String,
    /// A human-readable description.
    #[serde(default)]
    pub description: String,
    /// The grants this role carries (a role is a named set of
    /// `(Action, Resource, Scope)` triples, never free text).
    #[serde(default)]
    pub grants: Vec<GrantDto>,
}

/// `POST /api/users/{user_id}/roles` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AssignRoleRequest {
    /// The role to assign, as its id's `Display` form.
    pub role_id: String,
}

/// `POST /api/users/{user_id}/plugin-grants` (and its `role_id`/`.../revoke`
/// siblings) request body: a plugin permission is granted
/// or revoked whole, by name — never interpreted, unlike a core [`GrantDto`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PluginGrantRequest {
    /// The permission's full name, `<plugin-id>.<resource>:<operation>`.
    pub name: String,
}
