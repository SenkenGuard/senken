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

use senken_indicators::ConcreteIndicator;
use senken_marketdata::InstrumentId;
use senken_subscription::{BookState, IndicatorSessionKey, QuoteSource, SubscriptionPool};

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
    upgrade.on_upgrade(move |socket| handle_socket(socket, auth.user_id(), feed_pools, state))
}

/// How many levels a book-depth snapshot carries per side. A fixed panel
/// choice, not a venue-documented ceiling — see `okx_book`'s own module
/// docs in `senken-feed` for OKX's own (unconfirmed) maximum.
///
/// Read by [`crate::serve_with_feed_pools`] when it builds the one
/// [`BookSessionRegistry`] for the server: depth belongs to the registry
/// rather than to each subscriber, so two connections sharing a session
/// cannot disagree about it (see that type's own docs).
pub(crate) const PANEL_BOOK_DEPTH: usize = 20;

/// One order-book level on the wire: `[price, size]` at
/// [`ServerFrame::Book`]'s shared `price_scale`/`qty_scale`.
#[derive(Serialize)]
struct BookLevelWire {
    price: i64,
    size: i64,
}

impl From<senken_subscription::BookLevel> for BookLevelWire {
    fn from(level: senken_subscription::BookLevel) -> Self {
        Self {
            price: level.price,
            size: level.size,
        }
    }
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
    /// The request was one this build supports, and it failed anyway.
    ///
    /// Distinct from `Unsupported` on purpose: "this venue does not serve
    /// depth" and "we asked and could not get it" are different facts, and a
    /// client that cannot tell them apart shows the wrong thing for one of
    /// them. Sending nothing — which is what a failed snapshot used to do —
    /// is worse than either: an absence is indistinguishable from a request
    /// still in flight, so the panel waits for a frame that will never come.
    ///
    /// Carries no message. The reason is a transport string (a URL, a query
    /// string, a status code) and belongs in the log next to it, not on a
    /// screen; the client already has product copy for "could not load".
    Failed {
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
        qty: crate::dto::VolumeDto,
        qty_scale: u8,
        ts: i64,
    },
    /// A best bid and offer from a source that explicitly reports quotes.
    /// `topic` is namespaced as `quote:<source>:<symbol>` so it cannot be
    /// confused with the last-trade stream for the same instrument.
    Quote {
        topic: &'a str,
        bid: i64,
        ask: i64,
        price_scale: u8,
        bid_size: i64,
        ask_size: i64,
        qty_scale: u8,
        ts: i64,
    },
    /// A fixed-depth order-book snapshot from a source that reports one.
    ///
    /// Each frame is one whole venue-reported instant, never a delta to be
    /// applied to the last one: this build maintains no local book
    /// (`senken_subscription::BookSource`'s own docs say why). Frames keep
    /// arriving for as long as the topic is subscribed —
    /// `senken_subscription::BookSessionRegistry` refreshes the snapshot on
    /// a cadence and republishes it — so a client renders the newest frame
    /// and never has to ask for the next one.
    ///
    /// `topic` is namespaced as `book:<source>:<symbol>`, the same
    /// convention [`Quote`](Self::Quote) uses.
    Book {
        topic: &'a str,
        /// Resting bids, best price first, as the venue reported them.
        bids: Vec<BookLevelWire>,
        /// Resting asks, best price first, as the venue reported them.
        asks: Vec<BookLevelWire>,
        price_scale: u8,
        qty_scale: u8,
        ts: i64,
    },
    /// One field of a live indicator's latest reading — a closed bar's own
    /// value once `provisional` is `false`, or a snapshot-computed read of
    /// the still-forming bar while it is `true`. `topic` is namespaced as
    /// `indicator:<source>:<symbol>:<spec>:<name>:<params>` so it cannot be
    /// confused with the price/quote streams for the same instrument. A
    /// compound indicator (MACD, Stochastic, Bollinger Bands) sends one
    /// frame per field it reports rather than nesting them, matching how
    /// [`Price`](Self::Price) is already one frame per tick rather than a
    /// batch.
    Indicator {
        topic: &'a str,
        /// The wire name of the value this frame carries, e.g. `"value"` or
        /// `"macd_line"` — [`senken_indicators::IndicatorField::wire_name`].
        field: &'a str,
        value: f64,
        /// The bar this reading was computed from, Unix nanoseconds —
        /// matching `BarDto`'s own convention rather than `Price`'s
        /// milliseconds, since a client already renders indicator points
        /// against bars in nanoseconds (`POST /api/indicators/compute`'s
        /// response uses the same unit).
        ts_open: i64,
        /// `false` once the bar this reading covers has actually closed.
        /// A chart draws both; nothing that acts on a value (an alert, a
        /// backtest) may ever treat `true` as settled.
        provisional: bool,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Subscribe {
        topic: String,
    },
    Unsubscribe {
        topic: String,
    },
    /// Opens a live indicator session and starts streaming its readings as
    /// [`ServerFrame::Indicator`] frames. Structured rather than a bare
    /// `topic` string like [`Subscribe`](Self::Subscribe): `params` is
    /// itself JSON, which cannot be embedded unambiguously in a
    /// colon-delimited topic once an instrument's own symbol may contain a
    /// colon. The server computes and echoes back the canonical topic
    /// string in its `Subscribed` reply; unsubscribing uses that string
    /// with the existing [`Unsubscribe`](Self::Unsubscribe) frame.
    SubscribeIndicator {
        /// `source:symbol`, matching [`Subscribe`](Self::Subscribe)'s own
        /// `topic` convention.
        instrument: String,
        /// The bar timeframe, e.g. `"1h"`.
        spec: String,
        /// The indicator's name — see `GET /api/indicators`'s catalogue.
        indicator: String,
        /// The indicator's parameters, as JSON-object text.
        params: String,
        /// Inclusive start of the range this session warms up over, Unix
        /// nanoseconds — the same `from`/`to` a client already sends to
        /// `POST /api/indicators/compute`.
        from: i64,
        /// Exclusive end of the warm-up range, Unix nanoseconds.
        to: i64,
    },
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
    state: AppState,
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
                                subscribe(&feed_pools, &state, &topic, &out_tx, &mut subscriptions);
                            }
                            ClientFrame::Unsubscribe { topic } => {
                                if let Some(handle) = subscriptions.remove(&topic) {
                                    handle.abort();
                                }
                                let _ = out_tx.send(send_frame(&ServerFrame::Unsubscribed { topic: &topic }));
                            }
                            ClientFrame::SubscribeIndicator {
                                instrument,
                                spec,
                                indicator,
                                params,
                                from,
                                to,
                            } => {
                                let request = IndicatorSubscribeRequest {
                                    instrument,
                                    spec,
                                    indicator,
                                    params,
                                    from,
                                    to,
                                };
                                subscribe_indicator(&state, request, &out_tx, &mut subscriptions);
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

/// A valid market-data topic. Last-trade topics predate namespacing and keep
/// their instrument wire form for compatibility; quote and book topics are
/// explicitly namespaced so a client can lease more than one stream for one
/// instrument without them colliding.
enum StreamTopic {
    Price(InstrumentId),
    Quote(InstrumentId),
    /// A fixed-depth order-book snapshot, refreshed on a cadence for as
    /// long as it is subscribed — see [`ServerFrame::Book`]'s own docs.
    Book(InstrumentId),
}

fn parse_topic(topic: &str) -> Result<StreamTopic, senken_marketdata::InstrumentIdError> {
    if let Some(instrument) = topic.strip_prefix("quote:") {
        return InstrumentId::parse(instrument).map(StreamTopic::Quote);
    }
    if let Some(instrument) = topic.strip_prefix("book:") {
        return InstrumentId::parse(instrument).map(StreamTopic::Book);
    }
    InstrumentId::parse(topic).map(StreamTopic::Price)
}

/// Leases `topic` (idempotently — a repeated `Subscribe` for a topic already
/// held just re-acks) and spawns a forwarder for its stream until aborted.
///
/// A topic that is not a parseable [`InstrumentId`] is silently not
/// subscribed — the "acknowledge, do not error" shape this endpoint has
/// always had for a malformed frame.
///
/// A price/quote topic whose source has no entry in `feed_pools`, or a book
/// topic whose source has no registered
/// [`senken_subscription::BookSource`], is answered with
/// [`ServerFrame::Unsupported`] instead: that is not a malformed request but
/// a real answer the client needs, and silence cannot carry it.
fn subscribe(
    feed_pools: &Arc<HashMap<String, SubscriptionPool>>,
    state: &AppState,
    topic: &str,
    out_tx: &mpsc::UnboundedSender<Message>,
    subscriptions: &mut HashMap<String, AbortHandle>,
) {
    if subscriptions.contains_key(topic) {
        let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic }));
        return;
    }
    let Ok(stream) = parse_topic(topic) else {
        return;
    };

    if let StreamTopic::Book(instrument) = stream {
        subscribe_book(state, instrument, topic, out_tx, subscriptions);
        return;
    }

    let instrument = match &stream {
        StreamTopic::Price(instrument) | StreamTopic::Quote(instrument) => instrument,
        StreamTopic::Book(_) => unreachable!("handled and returned above"),
    };
    let Some(pool) = feed_pools.get(instrument.source()).cloned() else {
        let _ = out_tx.send(send_frame(&ServerFrame::Unsupported { topic }));
        return;
    };

    let forward_tx = out_tx.clone();
    let forward_topic = topic.to_owned();
    let task = tokio::spawn(async move {
        match stream {
            StreamTopic::Price(instrument) => {
                let lease = match pool.lease(instrument).await {
                    Ok(lease) => lease,
                    Err(error) => {
                        tracing::warn!(%error, topic = %forward_topic, "could not lease a live price subscription");
                        return;
                    }
                };
                let mut updates = lease.updates();
                loop {
                    // Read what is already there *before* waiting for a change. A
                    // `watch::Receiver` marks the value present at its creation as
                    // seen, so a tick published between `lease()` returning above
                    // and this receiver existing would never be reported by
                    // `changed()` — on a quiet instrument that first tick could be
                    // the only one for a long time, and waiting for a second one
                    // means showing nothing at all meanwhile. The borrow is copied
                    // out and dropped before the send, so no guard is held across an
                    // await point.
                    let current = *updates.borrow_and_update();
                    if let Some(update) = current {
                        let frame = ServerFrame::Price {
                            topic: &forward_topic,
                            price: update.price,
                            price_scale: update.price_scale,
                            qty: update.qty.into(),
                            qty_scale: update.qty_scale,
                            ts: update.ts.as_millis(),
                        };
                        if forward_tx.send(send_frame(&frame)).is_err() {
                            return; // the connection loop has already ended
                        }
                    }
                    if updates.changed().await.is_err() {
                        return; // the pool's actor task is gone; nothing left to forward
                    }
                }
            }
            StreamTopic::Quote(instrument) => {
                let lease = match pool.lease_quote(instrument).await {
                    Ok(lease) => lease,
                    Err(error) => {
                        tracing::warn!(%error, topic = %forward_topic, "could not lease a live quote subscription");
                        return;
                    }
                };
                let mut updates = lease.updates();
                loop {
                    // A quote published between the lease above and this receiver
                    // existing is invisible to `changed()` for the same reason the
                    // price stream reads first: a receiver treats the value already
                    // present as seen.
                    let current = *updates.borrow_and_update();
                    if let Some(update) = current {
                        let frame = ServerFrame::Quote {
                            topic: &forward_topic,
                            bid: update.bid,
                            ask: update.ask,
                            price_scale: update.price_scale,
                            bid_size: update.bid_size,
                            ask_size: update.ask_size,
                            qty_scale: update.qty_scale,
                            ts: update.ts.as_millis(),
                        };
                        if forward_tx.send(send_frame(&frame)).is_err() {
                            return; // the connection loop has already ended
                        }
                    }
                    if updates.changed().await.is_err() {
                        return; // the pool's actor task is gone; nothing left to forward
                    }
                }
            }
            StreamTopic::Book(_) => unreachable!(
                "StreamTopic::Book returns earlier in `subscribe`, before this task is ever spawned"
            ),
        }
    });

    subscriptions.insert(topic.to_owned(), task.abort_handle());
    let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic }));
}

/// Joins (or starts) the shared live book session for `instrument` and
/// forwards every snapshot it publishes as a [`ServerFrame::Book`] — the
/// book counterpart of [`subscribe`]'s price/quote forwarders, and the same
/// shape: a `watch::Receiver` read now and then awaited for changes.
///
/// A book source is a request/response port with nothing to await, so the
/// cadence comes from `senken_subscription::BookSessionRegistry`'s own poll
/// loop rather than from a venue stream. That loop is shared: every
/// connection watching one instrument costs the venue one request per
/// interval between them, not one each.
///
/// The session handle lives in the spawned task, so an `Unsubscribe` — which
/// aborts that task — releases this connection's share of it, and the poll
/// loop stops as soon as the last share is gone.
fn subscribe_book(
    state: &AppState,
    instrument: InstrumentId,
    topic: &str,
    out_tx: &mpsc::UnboundedSender<Message>,
    subscriptions: &mut HashMap<String, AbortHandle>,
) {
    let Some(source) = state.runtime.book_source(instrument.source()).cloned() else {
        let _ = out_tx.send(send_frame(&ServerFrame::Unsupported { topic }));
        return;
    };

    let state = state.clone();
    let forward_tx = out_tx.clone();
    let forward_topic = topic.to_owned();
    let task = tokio::spawn(async move {
        let hit = match state.runtime.marketdata().instrument(&instrument).await {
            Ok(Some(hit)) => hit,
            Ok(None) => {
                tracing::warn!(topic = %forward_topic, "book snapshot requested for an unknown instrument");
                let _ = forward_tx.send(send_frame(&ServerFrame::Failed {
                    topic: &forward_topic,
                }));
                return;
            }
            Err(error) => {
                tracing::warn!(%error, topic = %forward_topic, "could not resolve the instrument behind a book snapshot request");
                let _ = forward_tx.send(send_frame(&ServerFrame::Failed {
                    topic: &forward_topic,
                }));
                return;
            }
        };
        let symbol = hit.instrument.source_symbol();
        let session = state
            .book_sessions
            .get_or_create(instrument, source, symbol)
            .await;
        let mut updates = session.updates();
        loop {
            // Read what is already there before waiting for a change, for
            // the same reason the price forwarder does: joining a session
            // that already holds a snapshot must show it now, not one
            // refresh interval from now.
            let current = updates.borrow_and_update().clone();
            let frame = match &current {
                BookState::Pending => None,
                BookState::Live(snapshot) => Some(ServerFrame::Book {
                    topic: &forward_topic,
                    bids: snapshot
                        .bids
                        .iter()
                        .copied()
                        .map(BookLevelWire::from)
                        .collect(),
                    asks: snapshot
                        .asks
                        .iter()
                        .copied()
                        .map(BookLevelWire::from)
                        .collect(),
                    price_scale: snapshot.price_scale,
                    qty_scale: snapshot.qty_scale,
                    ts: snapshot.ts.as_millis(),
                }),
                BookState::Failed => Some(ServerFrame::Failed {
                    topic: &forward_topic,
                }),
            };
            if let Some(frame) = frame
                && forward_tx.send(send_frame(&frame)).is_err()
            {
                return; // the connection loop has already ended
            }
            if updates.changed().await.is_err() {
                return; // the session's poll task is gone; nothing left to forward
            }
        }
    });

    subscriptions.insert(topic.to_owned(), task.abort_handle());
    let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic }));
}

/// The client-supplied half of a [`ClientFrame::SubscribeIndicator`]
/// request, bundled so [`subscribe_indicator`] and
/// [`drive_indicator_subscription`] each take one argument for it instead
/// of six.
struct IndicatorSubscribeRequest {
    instrument: String,
    spec: String,
    indicator: String,
    params: String,
    from: i64,
    to: i64,
}

impl IndicatorSubscribeRequest {
    /// The canonical topic string for this subscription — built from the
    /// client's own raw fields (not the parsed/validated forms), so it can
    /// be computed before any of the async validation in
    /// [`drive_indicator_subscription`] has run, matching [`subscribe`]'s
    /// "acknowledge, then validate in the background" shape.
    fn topic(&self) -> String {
        format!(
            "indicator:{}:{}:{}:{}",
            self.instrument, self.spec, self.indicator, self.params
        )
    }
}

/// Opens (or reuses, via [`indicator_sessions`]) a live indicator session
/// and spawns a forwarder streaming its readings as
/// [`ServerFrame::Indicator`] frames — the live counterpart to `POST
/// /api/indicators/compute`, replacing the once-a-second poll that endpoint
/// was never meant to serve.
///
/// Idempotent like [`subscribe`]: a repeated request for the same topic
/// just re-acks. Every fallible step (unknown instrument, unknown
/// indicator, an unresolvable range) happens inside the spawned task and is
/// only logged, never reported back to the client — the same shape
/// [`subscribe`]'s own lease failure already has, since a malformed or
/// momentarily-unresolvable request is not distinguishable here from a
/// venue outage.
fn subscribe_indicator(
    state: &AppState,
    request: IndicatorSubscribeRequest,
    out_tx: &mpsc::UnboundedSender<Message>,
    subscriptions: &mut HashMap<String, AbortHandle>,
) {
    let topic = request.topic();
    if subscriptions.contains_key(&topic) {
        let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic: &topic }));
        return;
    }

    let state = state.clone();
    let forward_tx = out_tx.clone();
    let forward_topic = topic.clone();
    let task = tokio::spawn(async move {
        drive_indicator_subscription(&state, &request, &forward_topic, &forward_tx).await;
    });

    subscriptions.insert(topic.clone(), task.abort_handle());
    let _ = out_tx.send(send_frame(&ServerFrame::Subscribed { topic: &topic }));
}

/// Everything [`drive_indicator_subscription`] needs to lease (or join) a
/// live indicator session: the pool to lease from, the dedup key, the
/// already-constructed indicator, and the warm-up history to replay into
/// it.
struct ResolvedIndicatorSubscription {
    pool: SubscriptionPool,
    session_key: IndicatorSessionKey,
    concrete: ConcreteIndicator,
    warm_up: Vec<senken_series::Bar>,
}

/// Resolves everything a live indicator subscription needs, exactly the
/// way `POST /api/indicators/compute` does (reusing
/// [`crate::bars_handlers::resolve_bar_target`],
/// [`crate::bars_handlers::loader_for`] and
/// [`crate::indicator_handlers::warmup_extended_range`] rather than a
/// second copy of that logic).
///
/// `Ok(None)` means the instrument's source has no live feed in this
/// build — not malformed, a real answer
/// [`drive_indicator_subscription`] reports as
/// [`ServerFrame::Unsupported`]. `Err` carries a one-line reason for
/// every other failure, logged by the caller rather than returned to the
/// client (see that function's own docs for why).
async fn resolve_indicator_subscription(
    state: &AppState,
    request: &IndicatorSubscribeRequest,
) -> Result<Option<ResolvedIndicatorSubscription>, String> {
    let (id, spec, _hit) =
        crate::bars_handlers::resolve_bar_target(state, &request.instrument, &request.spec)
            .await
            .map_err(|error| format!("could not resolve instrument/spec: {error:?}"))?;
    let Some(pool) = state.feed_pools.get(id.source()).cloned() else {
        return Ok(None);
    };
    let loader = crate::bars_handlers::loader_for(state, &id)
        .map_err(|error| format!("no loader registered for this source: {error:?}"))?;
    let range = crate::bars_handlers::parse_range(request.from, request.to)
        .map_err(|error| format!("invalid range: {error:?}"))?;
    let descriptor = senken_indicators::descriptor(&request.indicator)
        .ok_or_else(|| format!("unknown indicator {:?}", request.indicator))?;
    let resolve_range =
        crate::indicator_handlers::warmup_extended_range(descriptor, spec, &request.params, range)
            .map_err(|error| format!("could not compute the warm-up range: {error:?}"))?;
    let key = senken_series::SeriesKey::new(
        id.source(),
        id.symbol(),
        senken_series::Origin::Derived,
        spec,
    );
    let resolved = loader
        .resolve(&key, resolve_range, senken_series::Anchor::UTC)
        .await
        .map_err(|error| format!("bars resolve failed while warming up: {error}"))?;
    let concrete = ConcreteIndicator::build(&request.indicator, &request.params)
        .map_err(|error| format!("could not build the named indicator: {error}"))?;
    let session_key =
        IndicatorSessionKey::new(id, spec, request.indicator.clone(), request.params.clone());
    Ok(Some(ResolvedIndicatorSubscription {
        pool,
        session_key,
        concrete,
        warm_up: resolved.bars,
    }))
}

/// The actual work behind [`subscribe_indicator`]: resolves the
/// subscription via [`resolve_indicator_subscription`], then leases (or
/// joins) a live indicator session from [`indicator_sessions`] and
/// forwards every reading it produces until the session ends or this
/// connection does.
///
/// Every fallible step is only logged, never reported back to the client —
/// the same shape [`subscribe`]'s own lease failure already has, since a
/// malformed or momentarily-unresolvable request is not distinguishable
/// here from a venue outage.
async fn drive_indicator_subscription(
    state: &AppState,
    request: &IndicatorSubscribeRequest,
    topic: &str,
    out_tx: &mpsc::UnboundedSender<Message>,
) {
    let resolved = match resolve_indicator_subscription(state, request).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            let _ = out_tx.send(send_frame(&ServerFrame::Unsupported { topic }));
            return;
        }
        Err(reason) => {
            tracing::warn!(topic, reason, "could not open a live indicator session");
            return;
        }
    };
    let handle = match state
        .indicator_sessions
        .get_or_create(
            &resolved.pool,
            resolved.session_key,
            resolved.concrete,
            &resolved.warm_up,
        )
        .await
    {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!(%error, topic, "could not lease a live indicator session");
            return;
        }
    };

    let mut updates = handle.updates();
    loop {
        // Same "read what is already there before waiting" discipline
        // `subscribe`'s own forwarders use, and for the same reason: a
        // reading published between the session being obtained above and
        // this receiver existing would otherwise never be seen.
        let current = updates.borrow_and_update().clone();
        if let Some(reading) = current {
            for &(field, value) in &reading.values {
                let frame = ServerFrame::Indicator {
                    topic,
                    field: field.wire_name(),
                    value,
                    ts_open: reading.ts_open.as_nanos(),
                    provisional: reading.provisional,
                };
                if out_tx.send(send_frame(&frame)).is_err() {
                    return; // the connection loop has already ended
                }
            }
        }
        if updates.changed().await.is_err() {
            return; // the session has ended; nothing left to forward
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;
    use std::time::Duration;

    use futures::{SinkExt, StreamExt};
    use senken_core::UnixNanos;
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_marketdata::InstrumentId;
    use senken_subscription::SubscriptionPool;
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

    /// A trivial [`VenueConnection`]/[`VenueConnector`] pair standing in for
    /// a real venue — reproduced here (not imported: private to each
    /// crate's own test module) the same way `senken_alerts`' and
    /// `senken_subscription`'s own tests each keep their own copy.
    /// `pool.publish` below drives ticks directly, so this fake never needs
    /// to do anything beyond acknowledging (un)subscribe.
    struct FakeConnection;

    #[async_trait::async_trait]
    impl senken_subscription::VenueConnection for FakeConnection {
        async fn shutdown(&self) {}

        async fn subscribe(
            &self,
            _instrument: &InstrumentId,
        ) -> Result<(), senken_subscription::ConnectionError> {
            Ok(())
        }

        async fn unsubscribe(
            &self,
            _instrument: &InstrumentId,
        ) -> Result<(), senken_subscription::ConnectionError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeConnector;

    #[async_trait::async_trait]
    impl senken_subscription::VenueConnector for FakeConnector {
        async fn connect(
            &self,
            _venue: &str,
        ) -> Result<
            Arc<dyn senken_subscription::VenueConnection>,
            senken_subscription::ConnectionError,
        > {
            Ok(Arc::new(FakeConnection))
        }
    }

    /// End-to-end proof of this jalur's actual deliverable: a client can
    /// subscribe to a live indicator over this same WS endpoint and receive
    /// [`ServerFrame::Indicator`] frames as ticks arrive, instead of
    /// polling `POST /api/indicators/compute` once a second.
    #[tokio::test]
    async fn subscribing_to_a_live_indicator_streams_readings_as_ticks_arrive() {
        use crate::bars_handlers::test_support::{
            TEST_SOURCE, TEST_SYMBOL, runtime_with_fake_venue,
        };

        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());

        let pool = SubscriptionPool::new(TEST_SOURCE, FakeConnector);
        let feed_pools = HashMap::from([(TEST_SOURCE.to_owned(), pool.clone())]);

        let (handle, _store, _dir) =
            crate::test_support::serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
        let addr = handle.local_addr();
        let token = login_token(addr).await;
        let ticket = ticket_for(addr, &token).await;

        let (mut stream, _response) = connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
        // `connected`
        let _ = stream.next().await.unwrap().unwrap();

        let subscribe = serde_json::json!({
            "type": "subscribe_indicator",
            "instrument": format!("{TEST_SOURCE}:{TEST_SYMBOL}"),
            "spec": "1m",
            "indicator": "Sma",
            "params": r#"{"period":1}"#,
            "from": 0,
            "to": 1,
        });
        stream
            .send(WsMessage::text(subscribe.to_string()))
            .await
            .unwrap();

        let expected_topic = format!(
            "indicator:{TEST_SOURCE}:{TEST_SYMBOL}:1m:Sma:{}",
            r#"{"period":1}"#
        );

        // `subscribed`, acknowledged synchronously before the session's own
        // async warm-up has necessarily finished.
        let subscribed: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let WsMessage::Text(text) = stream.next().await.unwrap().unwrap() else {
                    continue;
                };
                let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                if parsed["type"] == "subscribed" {
                    return parsed;
                }
            }
        })
        .await
        .expect("a `subscribed` frame must arrive well within this timeout");
        assert_eq!(subscribed["topic"], expected_topic);

        // A live tick, published directly on the same pool the session
        // leases from — `Sma(1)` initializes on its very first bar, so this
        // one tick is already enough to produce a reading once the tick
        // opens (and, being alone, only ever provisionally forms) a bucket.
        //
        // `subscribed` above only proves the *frame* was acknowledged —
        // the session's own async warm-up and lease still happen in a
        // background task, so this tick can arrive before that task has
        // actually leased the instrument and would otherwise vanish
        // (nothing subscribed to the pool for it yet). Republishing it
        // alongside the wait, rather than once beforehand, closes that
        // race without a fixed sleep.
        let instrument = InstrumentId::new(TEST_SOURCE, TEST_SYMBOL).unwrap();
        let publish_tick = || {
            pool.publish(
                instrument.clone(),
                senken_subscription::PriceUpdate {
                    ts: UnixNanos::from_secs(0).unwrap(),
                    price: 12345,
                    price_scale: 2,
                    qty: senken_series::Volume::Real(1),
                    qty_scale: 0,
                },
            );
        };
        publish_tick();

        let reading: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let incoming =
                    tokio::time::timeout(Duration::from_millis(100), stream.next()).await;
                let Ok(Some(Ok(WsMessage::Text(text)))) = incoming else {
                    // No frame within this short window — the session may
                    // not have leased yet; publish again and keep waiting.
                    publish_tick();
                    continue;
                };
                let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                if parsed["type"] == "indicator" {
                    return parsed;
                }
            }
        })
        .await
        .expect("an `indicator` frame must arrive well within this timeout");

        assert_eq!(reading["topic"], expected_topic);
        assert_eq!(reading["field"], "value");
        // `senken-indicators` reads a bar's own scaled-integer `close`
        // as-is (the scale lives on the series, not the bar), so the raw
        // tick price above is exactly the reported value.
        assert!((reading["value"].as_f64().unwrap() - 12345.0).abs() < 1e-9);
        assert_eq!(
            reading["provisional"], true,
            "a lone tick only ever opens a still-forming bucket"
        );

        let _ = stream.close(None).await;
        handle.shutdown().await.unwrap();
    }

    /// The live indicator session registry is an [`AppState`] field, not a
    /// process-wide static: two servers running in the same process (as
    /// this test itself does) must not share sessions. If they did, the
    /// second server's `SubscribeIndicator` for a topic already open on the
    /// first would *join* that existing session instead of leasing its own
    /// pool — and a tick published only on the second server's own pool
    /// would then never reach it, because the shared session's lease would
    /// still be against the first server's pool.
    #[tokio::test]
    async fn two_servers_in_one_process_do_not_share_indicator_sessions() {
        use crate::bars_handlers::test_support::{
            TEST_SOURCE, TEST_SYMBOL, runtime_with_fake_venue,
        };

        async fn stand_up_server() -> (ServerHandle, tempfile::TempDir, SubscriptionPool) {
            let runtime_dir = tempfile::TempDir::new().unwrap();
            let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
            let pool = SubscriptionPool::new(TEST_SOURCE, FakeConnector);
            let feed_pools = HashMap::from([(TEST_SOURCE.to_owned(), pool.clone())]);
            let (handle, _store, _dir) =
                crate::test_support::serve_unfenced_test_server_with_feed(runtime, feed_pools)
                    .await;
            (handle, runtime_dir, pool)
        }

        async fn open_subscription(
            addr: std::net::SocketAddr,
        ) -> tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        > {
            let token = login_token(addr).await;
            let ticket = ticket_for(addr, &token).await;
            let (mut stream, _response) =
                connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
                    .await
                    .unwrap();
            // `connected`
            let _ = stream.next().await.unwrap().unwrap();

            let subscribe = serde_json::json!({
                "type": "subscribe_indicator",
                "instrument": format!("{TEST_SOURCE}:{TEST_SYMBOL}"),
                "spec": "1m",
                "indicator": "Sma",
                "params": r#"{"period":1}"#,
                "from": 0,
                "to": 1,
            });
            stream
                .send(WsMessage::text(subscribe.to_string()))
                .await
                .unwrap();

            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let WsMessage::Text(text) = stream.next().await.unwrap().unwrap() else {
                        continue;
                    };
                    let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if parsed["type"] == "subscribed" {
                        return;
                    }
                }
            })
            .await
            .expect("a `subscribed` frame must arrive well within this timeout");

            stream
        }

        let (handle_a, _dir_a, pool_a) = stand_up_server().await;
        let (handle_b, _dir_b, pool_b) = stand_up_server().await;

        // Byte-for-byte the same topic on both servers — a shared registry
        // would resolve both to the same `IndicatorSessionKey`.
        let mut stream_a = open_subscription(handle_a.local_addr()).await;
        let mut stream_b = open_subscription(handle_b.local_addr()).await;

        let instrument = InstrumentId::new(TEST_SOURCE, TEST_SYMBOL).unwrap();
        let tick = || senken_subscription::PriceUpdate {
            ts: UnixNanos::from_secs(0).unwrap(),
            price: 12345,
            price_scale: 2,
            qty: senken_series::Volume::Real(1),
            qty_scale: 0,
        };

        // Only server B's own pool ever publishes. A session wrongly joined
        // from a shared registry would be leased against server A's pool
        // and would never see this tick.
        let reading_b: serde_json::Value = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                pool_b.publish(instrument.clone(), tick());
                let incoming =
                    tokio::time::timeout(Duration::from_millis(100), stream_b.next()).await;
                let Ok(Some(Ok(WsMessage::Text(text)))) = incoming else {
                    continue;
                };
                let parsed: serde_json::Value = serde_json::from_str(&text).unwrap();
                if parsed["type"] == "indicator" {
                    return parsed;
                }
            }
        })
        .await
        .expect(
            "server B must lease its own pool for this topic, not join a session \
             leased against server A's pool",
        );
        assert_eq!(reading_b["field"], "value");

        let _ = stream_a.close(None).await;
        let _ = stream_b.close(None).await;
        handle_a.shutdown().await.unwrap();
        handle_b.shutdown().await.unwrap();
        let _ = pool_a;
    }
}
