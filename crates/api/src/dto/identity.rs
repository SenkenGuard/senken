use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_acl::Grant;

/// `POST /api/login` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct LoginRequest {
    /// The account's email.
    pub email: String,
    /// The account's password, in plain text over an already-authenticated
    /// transport (`Authorization: Bearer`, not a cookie, so there is no separate CSRF surface to protect this call from).
    pub password: String,
}

/// `POST /api/login` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LoginResponse {
    /// The freshly minted session token. The client attaches this as
    /// `Authorization: Bearer <token>` on every later request — it is never placed in a cookie or a URL.
    pub token: String,
}

/// `POST /api/set-password` request body.
///
/// `email` is required only for the anonymous first-run path:
/// an authenticated caller changing their own password is identified by
/// their session instead, and any `email` they supply is ignored — there is
/// no self-service way to name a *different* account here.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetPasswordRequest {
    /// The account to set a password for. Required when the request carries
    /// no `Authorization` header; ignored when it does.
    #[serde(default)]
    pub email: Option<String>,
    /// The new password (length floor only, checked by `senken-identity`).
    pub new_password: String,
}

/// `GET /api/me` response body: the caller's own profile, plus the roles and effective grants that let the client cosmetically hide
/// admin sections. **UI convenience only**: every
/// endpoint re-checks a real grant on every request regardless of what this
/// response says.
///
/// This is also the endpoint a client should poll to know
/// whether it is really still authenticated, instead of `GET /api/health`:
/// `health` needs no credential at all, so a successful poll of it reads as
/// "authenticated" whether or not a session exists, which is exactly the
/// authentication bug this avoids. `me` requires
/// [`crate::auth::EndpointPermission::Authenticated`], so a `200` here
/// really does mean a live, unfenced session, and a `401` really does mean
/// the credential is gone.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MeResponse {
    /// The account's id.
    pub id: String,
    /// The account's email.
    pub email: String,
    /// The account's display name.
    pub display_name: String,
    /// `true` if the account is disabled.
    pub disabled: bool,
    /// `true` once the account has set a password. Always `true` here in
    /// practice — a session can only be resolved for an account after it
    /// exists, and the fence gate means `/api/me` itself already refused a
    /// fenced account's session — but reported anyway rather than assumed,
    /// so a client never has to special-case why the field is missing.
    pub password_set: bool,
    /// The names of every role this account holds.
    pub roles: Vec<String>,
    /// This account's effective grants: every `(Action, Resource, Scope)`
    /// it holds through any role or direct grant, one entry per
    /// `(Action, Resource)` pair at the widest scope granted for it (see
    /// `senken_identity::AuthenticatedUser::effective_grants`).
    pub grants: Vec<GrantDto>,
}

/// A `senken_acl::Grant` as this crate serialises it: three fields, each
/// carrying its `senken_acl` enum value directly (already `Serialize`,
/// since `senken-acl` derives it) but documented to `utoipa` as a plain
/// string via `#[schema(value_type = String)]`. That attribute is needed,
/// not cosmetic: `Action`/`Resource`/`Scope` cannot derive `utoipa::ToSchema`
/// themselves without editing `senken-acl` (out of this crate's owned
/// paths, consumed as-is), and Rust's orphan
/// rule forbids implementing that foreign trait for that foreign type from
/// here. The wire format is exactly each enum's variant name (`"View"`,
/// `"Workspace"`, `"Own"`, …) — the same spelling
/// `packages/web/src/lib/components/settings/sections/access-section.svelte`
/// already mirrors from `crates/acl/src/{action,resource,scope}.rs` for its
/// (currently disabled) grant matrix.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct GrantDto {
    /// What the grant permits doing.
    #[schema(value_type = String)]
    pub action: senken_acl::Action,
    /// What it permits doing it to.
    #[schema(value_type = String)]
    pub resource: senken_acl::Resource,
    /// How far that permission reaches.
    #[schema(value_type = String)]
    pub scope: senken_acl::Scope,
}

impl From<Grant> for GrantDto {
    fn from(grant: Grant) -> Self {
        Self {
            action: grant.action,
            resource: grant.resource,
            scope: grant.scope,
        }
    }
}

impl From<GrantDto> for Grant {
    fn from(dto: GrantDto) -> Self {
        Self::new(dto.action, dto.resource, dto.scope)
    }
}
