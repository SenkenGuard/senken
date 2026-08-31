//! Instrument search over HTTP (`GET /api/instruments`).
//!
//! `senken-marketdata` already holds every registered venue's catalog,
//! cached on disk and ranked (`MarketData::instruments`) — `senken-cli`'s
//! own `search` subcommand is the existing caller. This is the same call,
//! reached over HTTP so the browser's symbol picker can search the real
//! catalog instead of a fixed list. Market data is global and never
//! tenanted (a locked decision — see `AGENTS.md`), so this needs a valid
//! session and nothing more: no `senken_acl::Resource` to scope against,
//! the same reasoning `bars_handlers`/`indicator_handlers` already apply.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::Deserialize;

use senken_marketdata::InstrumentQuery;

use crate::AppState;
use crate::auth::Authed;
use crate::dto::InstrumentsPage;
use crate::pagination::{PaginationQuery, normalize_pagination};

#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentSearchQuery {
    /// Free text, optionally `source:term` — the exact grammar
    /// `InstrumentQuery::new` already parses for `senken-cli`'s `search`
    /// subcommand.
    #[serde(default)]
    q: String,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    offset: Option<u32>,
}

/// `GET /api/instruments`: ranked, multi-source instrument search
/// (`InstrumentQuery`/`MarketData::instruments`, reused rather than
/// reimplemented).
#[utoipa::path(
    get,
    path = "/api/instruments",
    params(
        ("q" = Option<String>, Query, description = "free text, or `source:term` to narrow to one venue"),
        ("limit" = Option<u32>, Query, description = "page size, default 50, max 200"),
        ("offset" = Option<u32>, Query, description = "rows to skip, default 0"),
    ),
    responses(
        (status = 200, body = InstrumentsPage),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn search_instruments(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Query(query): Query<InstrumentSearchQuery>,
) -> Json<InstrumentsPage> {
    let (limit, offset) = normalize_pagination(PaginationQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let search = InstrumentQuery::new(&query.q)
        .with_offset(offset as usize)
        .with_limit(limit as usize);
    let page = state.runtime.marketdata().instruments(search).await;
    let total = u64::try_from(page.total_matched).unwrap_or(u64::MAX);
    let complete = page.is_complete();
    Json(InstrumentsPage {
        rows: page.matches.into_iter().map(Into::into).collect(),
        total,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_marketdata::{Instrument, InstrumentStatus, MarketDataSource, SourceError};
    use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
    use senken_runtime::Runtime;

    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, serve_unfenced_test_server,
        serve_unfenced_test_server_with,
    };

    /// A searchable fake venue — unlike `bars_handlers::test_support`'s own
    /// fixture, whose instrument is deliberately built with the default
    /// (non-searchable) `InstrumentStatus::Unknown` since its own tests
    /// never search. `MarketData::instruments`' ranking treats
    /// searchability as a real filter, so a search test needs a `Trading`
    /// row to find anything at all.
    struct SearchableFakeSource;

    #[async_trait]
    impl MarketDataSource for SearchableFakeSource {
        fn id(&self) -> &'static str {
            "test-venue"
        }

        fn name(&self) -> &'static str {
            "Test Venue"
        }

        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(vec![
                Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT")
                    .with_status(InstrumentStatus::Trading),
            ])
        }
    }

    struct SearchableFakePlugin;

    impl Plugin for SearchableFakePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: "test-venue".to_owned(),
                name: "Test Venue".to_owned(),
                version: "0".to_owned(),
                description: String::new(),
                permissions: Vec::new(),
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(SearchableFakeSource));
            Ok(())
        }
    }

    async fn admin_token(addr: std::net::SocketAddr) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_TEST_PASSWORD }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn an_empty_query_lists_something_and_a_narrow_one_returns_it_too() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(runtime_dir.path())
            .plugin(SearchableFakePlugin)
            .build()
            .unwrap();
        let (handle, _identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let all = body_json(get_auth(format!("http://{addr}/api/instruments"), &token).await).await;
        assert!(
            all["total"].as_u64().unwrap() > 0,
            "the test runtime's fake venue must be searchable with no query at all"
        );

        let narrowed = body_json(
            get_auth(
                format!("http://{addr}/api/instruments?q=test-venue:BTC"),
                &token,
            )
            .await,
        )
        .await;
        let rows = narrowed["rows"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert_eq!(rows[0]["id"], "test-venue:BTCUSDT");

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_request_with_no_credentials_is_401() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let response = reqwest::get(format!("http://{addr}/api/instruments"))
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }
}
