//! Notes over HTTP.
//!
//! Mirrors `alert_handlers`/`watchlist_handlers` exactly: every handler
//! extracts `Extension(ctx): Authed` and passes `&ctx.user` straight through
//! to `senken_notes::NoteStore`, which performs its own guarded check on
//! every read and write.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_notes::NoteId;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    CreateNoteRequest, IdResponse, NoteDto, NoteSummaryDto, NotesPage, UpdateNoteRequest,
};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as a [`NoteId`], failing with `400` (not
/// `500`) for a malformed one.
fn parse_note_id(raw: &str) -> Result<NoteId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid note id".to_owned()))
}

/// `GET /api/notes`. Scoped by `NoteStore::list_notes` itself — a
/// superadmin sees every note, an ordinary user sees only their own, and
/// the reported `total` already respects that scope too. Never carries a
/// note's body — see [`crate::dto::NoteSummaryDto`]'s own docs.
#[utoipa::path(
    get,
    path = "/api/notes",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = NotesPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_notes(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<NotesPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.notes.list_notes(&ctx.user, limit, offset)?;
    Ok(Json(NotesPage {
        rows: page.rows.into_iter().map(NoteSummaryDto::from).collect(),
        total: page.total,
    }))
}

/// `POST /api/notes`.
#[utoipa::path(
    post,
    path = "/api/notes",
    request_body = CreateNoteRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_note(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateNoteRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let id = state
        .notes
        .create_note(&ctx.user, &body.title, &body.body)?;
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `GET /api/notes/{note_id}`: the full note, body included.
#[utoipa::path(
    get,
    path = "/api/notes/{note_id}",
    params(("note_id" = String, Path)),
    responses(
        (status = 200, body = NoteDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_note(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(note_id): Path<String>,
) -> Result<Json<NoteDto>, HandlerError> {
    let note_id = parse_note_id(&note_id)?;
    let note = state.notes.get_note(&ctx.user, note_id)?;
    Ok(Json(note.into()))
}

/// `PUT /api/notes/{note_id}`: replaces both title and body.
#[utoipa::path(
    put,
    path = "/api/notes/{note_id}",
    request_body = UpdateNoteRequest,
    params(("note_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn update_note(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(note_id): Path<String>,
    Json(body): Json<UpdateNoteRequest>,
) -> Result<StatusCode, HandlerError> {
    let note_id = parse_note_id(&note_id)?;
    state
        .notes
        .update_note(&ctx.user, note_id, &body.title, &body.body)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/notes/{note_id}`.
#[utoipa::path(
    delete,
    path = "/api/notes/{note_id}",
    params(("note_id" = String, Path)),
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_note(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(note_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let note_id = parse_note_id(&note_id)?;
    state.notes.delete_note(&ctx.user, note_id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use senken_acl::{Action, Grant, Resource, Scope};
    use senken_identity::DEFAULT_ADMIN_EMAIL;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
        serve_unfenced_test_server,
    };

    async fn login_token(addr: std::net::SocketAddr, email: &str, password: &str) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn notes_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Notes User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Edit, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Note, Scope::Own),
                )
                .unwrap();
        }
        login_token(addr, email, "a very long password").await
    }

    #[tokio::test]
    async fn a_note_round_trips_and_the_listing_carries_no_body() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = notes_user(addr, &identity, &admin, "alice@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/notes"),
            &alice_token,
            serde_json::json!({ "title": "Trade journal", "body": "Bought the dip." }),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::CREATED);
        let note_id = body_json(create).await["id"].as_str().unwrap().to_owned();

        let page =
            body_json(get_auth(format!("http://{addr}/api/notes"), &alice_token).await).await;
        assert_eq!(page["total"], 1);
        assert_eq!(page["rows"][0]["title"], "Trade journal");
        assert!(
            page["rows"][0].get("body").is_none(),
            "GET /api/notes must not carry a note's body: {:?}",
            page["rows"][0]
        );

        let full =
            body_json(get_auth(format!("http://{addr}/api/notes/{note_id}"), &alice_token).await)
                .await;
        assert_eq!(full["title"], "Trade journal");
        assert_eq!(
            full["body"], "Bought the dip.",
            "GET /api/notes/{{id}} must carry the body"
        );

        let update = reqwest::Client::new()
            .put(format!("http://{addr}/api/notes/{note_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&serde_json::json!({ "title": "Final", "body": "v2" })).unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(update.status(), reqwest::StatusCode::NO_CONTENT);

        let updated =
            body_json(get_auth(format!("http://{addr}/api/notes/{note_id}"), &alice_token).await)
                .await;
        assert_eq!(updated["title"], "Final");
        assert_eq!(updated["body"], "v2");

        let delete = reqwest::Client::new()
            .delete(format!("http://{addr}/api/notes/{note_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/notes"), &alice_token).await).await;
        assert_eq!(page["total"], 0);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn two_users_cannot_read_each_others_notes_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = notes_user(addr, &identity, &admin, "alice2@example.com").await;
        let bob_token = notes_user(addr, &identity, &admin, "bob2@example.com").await;

        let bobs_note = body_json(
            post_json_auth(
                format!("http://{addr}/api/notes"),
                &bob_token,
                serde_json::json!({ "title": "Bob's private note", "body": "shh" }),
            )
            .await,
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = get_auth(format!("http://{addr}/api/notes/{bobs_note}"), &alice_token).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "403, not 401 -- alice has a valid session, she just may not see bob's note"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_request_with_no_session_is_401() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/notes"))
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_user_with_no_grant_is_403_not_401() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        identity
            .create_user(
                &admin,
                "ungranted@example.com",
                "No Grants",
                Some("a very long password"),
            )
            .unwrap();
        let ungranted_token =
            login_token(addr, "ungranted@example.com", "a very long password").await;

        let response = get_auth(format!("http://{addr}/api/notes"), &ungranted_token).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FORBIDDEN,
            "403, not 401 -- ungranted has a valid session, they just hold no grant"
        );

        handle.shutdown().await.unwrap();
    }
}
