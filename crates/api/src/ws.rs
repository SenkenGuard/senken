//! The WebSocket endpoint and its short-lived ticket exchange.
//!
//! A browser cannot set an `Authorization` header on a WebSocket handshake,
//! so the session token never goes near a WS connection at all. Instead:
//! an already-authenticated client asks [`issue_ticket`] for a single-use,
//! seconds-lived ticket over ordinary REST (where `Authorization: Bearer`
//! works fine), then presents *that* ticket — not the session token — in
//! the WS handshake's query string. [`ws_handler`] redeems it exactly once
//! against [`TicketStore`] and resolves the session it was minted from,
//! using the same guarded [`senken_identity::IdentityStore::resolve_session`]
//! path every other endpoint uses. A leaked ticket is worthless by the time
//! it could be replayed: it is deleted on first use and expires in seconds
//! regardless.
//!
//! Matches `packages/web/src/lib/api/websocket.ts`'s client half exactly:
//! `POST /api/ws/ticket` returns `{ ticket }`, and the client connects to
//! `GET /api/ws?ticket=<ticket>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, extract::rejection::QueryRejection};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use senken_marketdata::InstrumentId;
use senken_subscription::SubscriptionPool;

use crate::AppState;
use crate::auth::Authed;
use crate::dto::{ErrorBody, WsTicketResponse};

/// How long a ticket is valid for ("valid for seconds"). Long
/// enough to cover the round trip of requesting a ticket and immediately
/// opening a WebSocket to the same server, short enough that a ticket
/// sitting in an access log or a proxy log is useless by the time anyone
/// could read it there.
const TICKET_TTL: Duration = Duration::from_secs(30);

struct TicketEntry {
    /// The raw session token this ticket was minted from — never the
    /// ticket's own identity, so redeeming it re-runs the exact same
    /// [`senken_identity::IdentityStore::resolve_session`] check (idle-timer
    /// refresh, disabled-account check, expiry) a normal request would.
    session_token: String,
    expires_at: Instant,
}

/// In-memory, single-process store for outstanding WS tickets.
///
/// Deliberately not persisted anywhere (not SQLite, not `senken-identity`'s
/// schema): a ticket's entire reason to exist is to be redeemed within
/// seconds of being minted, on the same server process, so surviving a
/// restart has no value and a database round trip would only add latency
/// to the one part of this exchange that must be fast. Layer ownership
/// places this squarely in the API/transport layer, not the identity
/// domain.
#[derive(Default)]
pub(crate) struct TicketStore {
    tickets: Mutex<HashMap<String, TicketEntry>>,
}

impl TicketStore {
    /// Mints a fresh, single-use ticket for `session_token`.
    pub(crate) fn issue(&self, session_token: String) -> String {
        let ticket = uuid::Uuid::new_v4().to_string();
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Opportunistic cleanup: bounds this map's size without a separate
        // background sweeper, which nothing at this scale (one ticket per
        // WS (re)connect attempt) needs.
        let now = Instant::now();
        tickets.retain(|_, entry| entry.expires_at > now);
        tickets.insert(
            ticket.clone(),
            TicketEntry {
                session_token,
                expires_at: now + TICKET_TTL,
            },
        );
        ticket
    }

    /// Redeems `ticket`, returning the session token it was minted from —
    /// exactly once. The entry is removed whether or not it had already
    /// expired, so a leaked, expired ticket cannot be probed repeatedly:
    /// the first presentation of any given ticket string is also its last.
    pub(crate) fn redeem(&self, ticket: &str) -> Option<String> {
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = tickets.remove(ticket)?;
        (entry.expires_at > Instant::now()).then_some(entry.session_token)
    }

    /// Test-only: mints a ticket that is already expired, so
    /// [`TICKET_TTL`]'s effect can be tested deterministically rather than
    /// by actually sleeping 30 seconds.
    #[cfg(test)]
    pub(crate) fn issue_already_expired(&self, session_token: String) -> String {
        let ticket = uuid::Uuid::new_v4().to_string();
        let mut tickets = self
            .tickets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        tickets.insert(
            ticket.clone(),
            TicketEntry {
                session_token,
                expires_at: Instant::now().checked_sub(Duration::from_secs(1)).expect(
                    "the process has been up for at least a second by the time a test runs",
                ),
            },
        );
        ticket
    }
}

/// `POST /api/ws/ticket`: mints a ticket for the caller's own,
/// already-authenticated session. Requires
/// [`crate::auth::EndpointPermission::Authenticated`] — the same
/// requirement as any other endpoint that needs a live, unfenced session.
#[utoipa::path(
    post,
    path = "/api/ws/ticket",
    responses((status = 200, body = WsTicketResponse), (status = 401, body = ErrorBody))
)]
pub(crate) async fn issue_ticket(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Json<WsTicketResponse> {
    let ticket = state.ws_tickets.issue(ctx.token.clone());
    Json(WsTicketResponse { ticket })
}

#[derive(Deserialize)]
pub(crate) struct WsQuery {
    /// The single-use ticket from [`issue_ticket`]. **Never** a session
    /// token: `ws_handler` only ever exchanges this value through
    /// [`TicketStore::redeem`], which recognises only strings this
    /// process itself minted — a raw session token, no matter how valid,
    /// was never inserted into that map, so presenting one here always
    /// fails to redeem rather than accidentally authenticating (the /// required test: "a session token in a query string is rejected").
    ticket: String,
}

/// `GET /api/ws`: the WebSocket endpoint itself.
///
/// Mounted at [`crate::auth::EndpointPermission::Public`] — not because it
/// is unauthenticated, but because a WebSocket handshake cannot carry the
/// `Authorization` header the shared middleware looks for. This handler
/// performs its own authentication by redeeming the `ticket` query
/// parameter before ever upgrading the connection.
pub(crate) async fn ws_handler(
    State(state): State<AppState>,
    query: Result<Query<WsQuery>, QueryRejection>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let Ok(Query(query)) = query else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorBody::new(
                "missing or malformed `ticket` query parameter".to_owned(),
            )),
        )
            .into_response();
    };

    let Some(session_token) = state.ws_tickets.redeem(&query.ticket) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody::new(
                "invalid, expired, or already-used ticket".to_owned(),
            )),
        )
            .into_response();
    };

    let resolved = state.identity.resolve_session(&session_token);
    let auth = match resolved {
        Ok(Some(auth)) => auth,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(ErrorBody::new("session is no longer valid".to_owned())),
            )
                .into_response();
        }
        Err(source) => {
            tracing::error!(%source, "resolving the session behind a WS ticket failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if !auth.password_set() {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorBody::new(
                "this account has not set a password yet".to_owned(),
            )),
        )
            .into_response();
    }

    let feed_pools = Arc::clone(&state.feed_pools);
    upgrade.on_upgrade(move |socket| handle_socket(socket, auth.user_id(), feed_pools))
}

/// Wire envelope for every message this endpoint sends, matching
/// `packages/web/src/lib/api/websocket.ts`'s `parseWsMessage`: any object
/// with a `type` field. `Price` is the addition: one message
/// per tick, for whichever topic (an `InstrumentId`'s `source:symbol` wire
/// form) the connection currently has a live lease on.
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerFrame<'a> {
    Connected,
    Subscribed {
        topic: &'a str,
    },
    Unsubscribed {
        topic: &'a str,
    },
    /// This build has no live feed for the topic's source, so no price will
    /// ever arrive for it. Sent instead of `Subscribed` so a client can tell
    /// "this venue does not stream" apart from "the feed is fine, no trade
    /// has happened yet" — an absence cannot distinguish the two.
    Unsupported {
        topic: &'a str,
    },
    /// `price`/`price_scale` are exactly `PriceUpdate`'s own fields — a
    /// scaled integer, never `f64` (`AGENTS.md`: no float for a price on the
    /// market-data path). `ts` is the venue's own tick timestamp, in
    /// milliseconds, matching `BarDto`'s convention.
    Price {
        topic: &'a str,
        price: i64,
        price_scale: u8,
        /// Base-asset quantity traded at `price`, at `qty_scale` digits — the
        /// V of OHLCV. A client building the forming bar from these ticks
        /// needs it, or every volume-reading indicator stops at the last
        /// stored bar.
        qty: i64,
        qty_scale: u8,
        ts: i64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Subscribe { topic: String },
    Unsubscribe { topic: String },
}

fn send_frame(frame: &ServerFrame<'_>) -> Message {
    // `ServerFrame` is this module's own type, serialised by this module's
    // own call — a failure here would mean the enum above cannot round
    // through `serde_json`, which is a bug in this file, not a runtime
    // condition to thread an error through every caller for.
    Message::text(serde_json::to_string(frame).expect("ServerFrame always serialises"))
}

/// Drives one upgraded WebSocket connection: a chart pane,
/// a watchlist row, an alert or a position all take a lease the same way —
/// here, that means a `Subscribe { topic }` frame leases `topic` (parsed as
/// an `InstrumentId`) from whichever pool in `feed_pools` serves its
/// source. `_user_id` is accepted (not `_`) so the signature stays honest
/// about what per-connection scoping this stage still does not do —
/// every topic this connection can name reaches the same, unscoped pools;
/// market data is global (`AGENTS.md`), so that is the whole story here.
///
/// **The lease stays a `Drop` guard all the way up.** Each subscribed
/// topic's [`senken_subscription::Lease`] lives inside its own forwarder
/// task, moved there and never touched again from here; this function only
/// ever holds that task's [`AbortHandle`]. Aborting the task — on an
/// explicit `Unsubscribe`, or implicitly when this whole function returns
/// (the loop below breaks, or the socket errors, or the browser/laptop just
/// vanishes) — drops the task's stack, which drops its `Lease`, which
/// releases the subscription. Nobody has to remember to call anything.
async fn handle_socket(
    socket: WebSocket,
    _user_id: senken_identity::UserId,
    feed_pools: Arc<HashMap<String, SubscriptionPool>>,
) {
    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();

    if out_tx.send(send_frame(&ServerFrame::Connected)).is_err() {
        return;
    }

    // One forwarder task per currently-subscribed topic, keyed by the exact
    // topic string the client used — see this function's own doc for why
    // aborting the entry here is the entire release mechanism.
    let mut subscriptions: HashMap<String, AbortHandle> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            outgoing = out_rx.recv() => {
                let Some(message) = outgoing else { break };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(frame) = serde_json::from_str::<ClientFrame>(&text) else {
                            continue;
                        };
                        match frame {
                            ClientFrame::Subscribe { topic } => {
                                subscribe(&feed_pools, &topic, &out_tx, &mut subscriptions);
                            }
                            ClientFrame::Unsubscribe { topic } => {
                                if let Some(handle) = subscriptions.remove(&topic) {
                                    handle.abort();
                                }
                                let _ = out_tx.send(send_frame(&ServerFrame::Unsubscribed { topic: &topic }));
                            }
                        }
                    }
                    Some(Ok(_non_text)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }
    }

    for (_, handle) in subscriptions {
        handle.abort();
    }
}

/// Leases `topic` (idempotently — a repeated `Subscribe` for a topic already
/// held just re-acks) and spawns the task that forwards its ticks as
/// [`ServerFrame::Price`] until aborted.
///
/// A topic that is not a parseable [`InstrumentId`] is silently not
/// subscribed — the "acknowledge, do not error" shape this endpoint has
/// always had for a malformed frame.
///
/// A topic whose source has no entry in `feed_pools` is answered with
/// [`ServerFrame::Unsupported`] instead: that is not a malformed request but
/// a real answer the client needs, and silence cannot carry it.
fn subscribe(
    feed_pools: &Arc<HashMap<String, SubscriptionPool>>,
    topic: &str,
    out_tx: &mpsc::UnboundedSender<Message>,
    subscriptions: &mut HashMap<String, AbortHandle>,
) {
    if subscriptions.contains_key(topic) {
        let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic }));
        return;
    }
    let Ok(instrument) = InstrumentId::parse(topic) else {
        return;
    };
    let Some(pool) = feed_pools.get(instrument.source()).cloned() else {
        let _ = out_tx.send(send_frame(&ServerFrame::Unsupported { topic }));
        return;
    };

    let forward_tx = out_tx.clone();
    let forward_topic = topic.to_owned();
    let task = tokio::spawn(async move {
        let lease = match pool.lease(instrument).await {
            Ok(lease) => lease,
            Err(error) => {
                tracing::warn!(%error, topic = %forward_topic, "could not lease a live price subscription");
                return;
            }
        };
        let mut updates = lease.updates();
        loop {
            if updates.changed().await.is_err() {
                return; // the pool's actor task is gone; nothing left to forward
            }
            let Some(update) = *updates.borrow() else {
                continue; // the channel's pre-first-tick initial value
            };
            let frame = ServerFrame::Price {
                topic: &forward_topic,
                price: update.price,
                price_scale: update.price_scale,
                qty: update.qty,
                qty_scale: update.qty_scale,
                ts: update.ts.as_millis(),
            };
            if forward_tx.send(send_frame(&frame)).is_err() {
                return; // the connection loop has already ended
            }
        }
    });

    subscriptions.insert(topic.to_owned(), task.abort_handle());
    let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic }));
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use futures::StreamExt;
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    use super::TicketStore;
    use crate::test_support::{body_json, post_json, post_json_auth, temp_identity_store};
    use crate::{ServeOptions, ServerHandle, serve};

    const ADMIN_PASSWORD: &str = "correct horse battery staple";

    async fn serve_unfenced() -> (ServerHandle, tempfile::TempDir) {
        let (dir, store) = temp_identity_store();
        store
            .set_password(DEFAULT_ADMIN_EMAIL, ADMIN_PASSWORD, None)
            .unwrap();
        let (_runtime_dir, runtime) = crate::test_support::temp_empty_runtime();
        let handle = serve(
            ServeOptions {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
                allowed_origins: Vec::new(),
            },
            Arc::new(store),
            Arc::new(runtime),
        )
        .await
        .unwrap();
        (handle, dir)
    }

    async fn login_token(addr: std::net::SocketAddr) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_PASSWORD }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    async fn ticket_for(addr: std::net::SocketAddr, token: &str) -> String {
        let response = post_json_auth(
            format!("http://{addr}/api/ws/ticket"),
            token,
            serde_json::json!({}),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        body_json(response).await["ticket"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn a_ws_connection_authenticates_with_a_ticket_and_gets_a_connected_frame() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        let token = login_token(addr).await;
        let ticket = ticket_for(addr, &token).await;

        let (mut stream, _response) = connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .expect("a valid ticket must be accepted");

        let first = stream
            .next()
            .await
            .expect("a message")
            .expect("not a transport error");
        let WsMessage::Text(text) = first else {
            panic!("expected a text frame, got {first:?}");
        };
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed["type"], "connected");

        let _ = stream.close(None).await;
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_raw_session_token_presented_as_a_ws_ticket_is_rejected() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();
        // A real, currently-valid session token — never registered as a
        // ticket in `TicketStore`, so it must not work as one either (plan
        // 004's required test: "a session token in a query string is
        // rejected").
        let token = login_token(addr).await;

        let result = connect_async(format!("ws://{addr}/api/ws?ticket={token}")).await;
        assert!(
            result.is_err(),
            "a raw session token must never be accepted as a WS ticket"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_ticket_query_parameter_is_rejected() {
        let (handle, _dir) = serve_unfenced().await;
        let addr = handle.local_addr();

        let result = connect_async(format!("ws://{addr}/api/ws")).await;
        assert!(result.is_err());

        handle.shutdown().await.unwrap();
    }

    #[test]
    fn a_ticket_can_only_be_redeemed_once() {
        let store = TicketStore::default();
        let ticket = store.issue("some-session-token".to_owned());

        assert_eq!(store.redeem(&ticket).as_deref(), Some("some-session-token"));
        assert_eq!(
            store.redeem(&ticket),
            None,
            "a second redemption of the same ticket must fail"
        );
    }

    #[test]
    fn an_expired_ticket_cannot_be_redeemed() {
        let store = TicketStore::default();
        let ticket = store.issue_already_expired("some-session-token".to_owned());

        assert_eq!(store.redeem(&ticket), None);
    }

    #[test]
    fn an_unknown_ticket_cannot_be_redeemed() {
        let store = TicketStore::default();
        assert_eq!(store.redeem("never-issued"), None);
    }
}
