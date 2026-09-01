use serde::Serialize;
use utoipa::ToSchema;

/// `POST /api/ws/ticket` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WsTicketResponse {
    /// A single-use, seconds-lived ticket. Presented once on the WebSocket
    /// handshake's query string (safe specifically because it is single-use
    /// and short-lived, unlike a session token) and discarded by the server
    /// on redemption. Matches `packages/web/src/lib/api/websocket.ts`'s
    /// `WsTicketResponse` field-for-field.
    pub ticket: String,
}
