//! Alerts over HTTP.
//!
//! Mirrors `workspace_handlers` exactly: every handler extracts
//! `Extension(ctx): Authed` and passes `&ctx.user` straight through to
//! `senken_alerts::AlertStore`, which performs its own guarded check on
//! every read and write. R6 flagged that `AlertStore`'s
//! `all_enabled_for_engine`/`record_fire` deliberately take no
//! `AuthenticatedUser` at all — this module does not mount either of them,
//! since both answer "what does the server need to keep running", never a
//! caller's own request.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use senken_alerts::AlertId;
use senken_marketdata::InstrumentId;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{AlertDto, AlertsPage, CreateAlertRequest, IdResponse};
use crate::pagination::{PaginationQuery, normalize_pagination};

/// Parses an HTTP path segment as an [`AlertId`], failing with `400` (not
/// `500`) for a malformed one.
fn parse_alert_id(raw: &str) -> Result<AlertId, HandlerError> {
    raw.parse()
        .map_err(|_| HandlerError::BadRequest("not a valid alert id".to_owned()))
}

/// `GET /api/alerts`. Scoped by `AlertStore::list_alerts`
/// itself — a superadmin sees every alert, an ordinary
/// user sees only their own, and the reported `total` already respects
/// that scope too.
#[utoipa::path(
    get,
    path = "/api/alerts",
    params(
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = AlertsPage),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_alerts(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<AlertsPage>, HandlerError> {
    let (limit, offset) = normalize_pagination(query);
    let page = state.alerts.list_alerts(&ctx.user, limit, offset)?;
    Ok(Json(AlertsPage {
        rows: page.rows.into_iter().map(AlertDto::from).collect(),
        total: page.total,
    }))
}

/// `GET /api/alerts/{alert_id}` ("fired-state" — the row itself already carries `last_fired_at`/`last_fired_value`/`fire_count`).
#[utoipa::path(
    get,
    path = "/api/alerts/{alert_id}",
    responses(
        (status = 200, body = AlertDto),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn get_alert(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(alert_id): Path<String>,
) -> Result<Json<AlertDto>, HandlerError> {
    let alert_id = parse_alert_id(&alert_id)?;
    let record = state.alerts.get_alert(&ctx.user, alert_id)?;
    Ok(Json(record.into()))
}

/// `POST /api/alerts`. Refuses an indicator that cannot even
/// be built before it is ever persisted, the same "refuse at the door"
/// discipline `senken_alerts::AlertStore::create_alert` itself already
/// applies.
#[utoipa::path(
    post,
    path = "/api/alerts",
    request_body = CreateAlertRequest,
    responses(
        (status = 201, body = IdResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn create_alert(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CreateAlertRequest>,
) -> Result<(StatusCode, Json<IdResponse>), HandlerError> {
    let instrument = InstrumentId::parse(&body.instrument)
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let timeframe =
        body.timeframe
            .parse()
            .map_err(|source: senken_series::ParseBarSpecError| {
                HandlerError::BadRequest(source.to_string())
            })?;
    let indicator = body.indicator.into();
    let id = state.alerts.create_alert(
        &ctx.user,
        &instrument,
        timeframe,
        &indicator,
        body.condition.into(),
    )?;
    // an alert must start running the moment it is created,
    // not on the next server restart. `get_alert` re-reads it back rather
    // than reassembling a record by hand, so the engine sees exactly the
    // row `all_enabled_for_engine` would.
    if let Ok(record) = state.alerts.get_alert(&ctx.user, id) {
        state.alert_engine.register(record);
    }
    Ok((StatusCode::CREATED, Json(IdResponse { id: id.to_string() })))
}

/// `DELETE /api/alerts/{alert_id}`.
#[utoipa::path(
    delete,
    path = "/api/alerts/{alert_id}",
    responses(
        (status = 204),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn delete_alert(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Path(alert_id): Path<String>,
) -> Result<StatusCode, HandlerError> {
    let alert_id = parse_alert_id(&alert_id)?;
    state.alerts.delete_alert(&ctx.user, alert_id)?;
    // a deleted alert must stop being evaluated immediately —
    // this drops its lease the same way closing a chart pane would.
    state.alert_engine.unregister(alert_id);
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

    async fn alerts_user(
        addr: std::net::SocketAddr,
        identity: &senken_identity::IdentityStore,
        admin: &senken_identity::AuthenticatedUser,
        email: &str,
    ) -> String {
        let user_id = identity
            .create_user(admin, email, "Alerts User", Some("a very long password"))
            .unwrap();
        for action in [Action::View, Action::Create, Action::Delete] {
            identity
                .grant_direct(
                    admin,
                    user_id,
                    Grant::new(action, Resource::Alert, Scope::Own),
                )
                .unwrap();
        }
        login_token(addr, email, "a very long password").await
    }

    fn rsi_alert_body() -> serde_json::Value {
        serde_json::json!({
            "instrument": "binance-spot:BTCUSDT",
            "timeframe": "1h",
            "indicator": { "name": "Rsi", "params": r#"{"period":14}"# },
            "condition": { "field": "Value", "comparator": "GreaterThan", "threshold": 70.0 },
        })
    }

    #[tokio::test]
    async fn two_users_cannot_see_each_others_alerts_over_http() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = alerts_user(addr, &identity, &admin, "alice@example.com").await;
        let bob_token = alerts_user(addr, &identity, &admin, "bob@example.com").await;

        post_json_auth(
            format!("http://{addr}/api/alerts"),
            &alice_token,
            rsi_alert_body(),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/alerts"),
            &bob_token,
            rsi_alert_body(),
        )
        .await;
        post_json_auth(
            format!("http://{addr}/api/alerts"),
            &bob_token,
            rsi_alert_body(),
        )
        .await;

        let alice_page =
            body_json(get_auth(format!("http://{addr}/api/alerts"), &alice_token).await).await;
        assert_eq!(alice_page["total"], 1);
        assert_eq!(alice_page["rows"].as_array().unwrap().len(), 1);

        let bob_page =
            body_json(get_auth(format!("http://{addr}/api/alerts"), &bob_token).await).await;
        assert_eq!(
            bob_page["total"], 2,
            "the total must respect scope too, or pagination leaks how many alerts exist"
        );

        // The wrong user cannot fetch or delete alice's alert — 403, not 401.
        let alice_alert_id = alice_page["rows"][0]["id"].as_str().unwrap();
        let get_forbidden = get_auth(
            format!("http://{addr}/api/alerts/{alice_alert_id}"),
            &bob_token,
        )
        .await;
        assert_eq!(
            get_forbidden.status(),
            reqwest::StatusCode::FORBIDDEN,
            "403, not 401 -- bob has a valid session, he just may not see alice's alert"
        );

        let delete_forbidden = reqwest::Client::new()
            .delete(format!("http://{addr}/api/alerts/{alice_alert_id}"))
            .header("authorization", format!("Bearer {bob_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete_forbidden.status(), reqwest::StatusCode::FORBIDDEN);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn creating_then_deleting_an_alert_over_http_reflects_in_the_list() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = alerts_user(addr, &identity, &admin, "alice2@example.com").await;

        let create = post_json_auth(
            format!("http://{addr}/api/alerts"),
            &alice_token,
            rsi_alert_body(),
        )
        .await;
        assert_eq!(create.status(), reqwest::StatusCode::CREATED);
        let alert_id = body_json(create).await["id"].as_str().unwrap().to_owned();

        let get =
            body_json(get_auth(format!("http://{addr}/api/alerts/{alert_id}"), &alice_token).await)
                .await;
        assert_eq!(get["enabled"], true);
        assert_eq!(get["fire_count"], 0);
        assert_eq!(get["last_fired_at"], serde_json::Value::Null);

        let delete = reqwest::Client::new()
            .delete(format!("http://{addr}/api/alerts/{alert_id}"))
            .header("authorization", format!("Bearer {alice_token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

        let page =
            body_json(get_auth(format!("http://{addr}/api/alerts"), &alice_token).await).await;
        assert_eq!(page["total"], 0);

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn creating_an_alert_with_an_unknown_indicator_over_http_is_400() {
        let (handle, identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        let alice_token = alerts_user(addr, &identity, &admin, "alice3@example.com").await;

        let response = post_json_auth(
            format!("http://{addr}/api/alerts"),
            &alice_token,
            serde_json::json!({
                "instrument": "binance-spot:BTCUSDT",
                "timeframe": "1h",
                "indicator": { "name": "NotReal", "params": "{}" },
                "condition": { "field": "Value", "comparator": "GreaterThan", "threshold": 1.0 },
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }
}
