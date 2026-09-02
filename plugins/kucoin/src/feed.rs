//! KuCoin's public `/market/match` channel.
//!
//! # KuCoin's endpoint is not a constant
//!
//! Every other venue in this project is dialled at a fixed URL. KuCoin is
//! not: the WebSocket host and a short-lived token are issued by an HTTP
//! call, and a `GET` on it is refused —
//!
//! ```text
//! GET  https://api.kucoin.com/api/v1/bullet-public → {"msg":"Method Not Allowed","code":"405000"}
//! POST https://api.kucoin.com/api/v1/bullet-public → {"code":"200000","data":{"token":"2neAiuYvAU61…","instanceServers":[{"endpoint":"wss://ws-api-spot.kucoin.com/","encrypt":true,"protocol":"websocket","pingInterval":18000,"pingTimeout":10000}]}}
//! ```
//!
//! Both confirmed live, 2026-09-02. That is why
//! [`VenueProtocol::endpoint`] exists: the token has to be fetched again
//! for **every** dial, including every reconnect, and a URL captured once
//! at startup goes stale.
//!
//! `pingInterval` in that response is the venue's own number — 18 seconds
//! — so this is one keep-alive interval in the project that is not a
//! conservative guess. [`KEEPALIVE`] sits under it.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to the issued endpoint with the token, sent
//! `{"id":"1","type":"subscribe","topic":"/market/match:BTC-USDT","privateChannel":false,"response":true}`,
//! received `{"id":"senken1","type":"welcome"}` and `{"id":"1","type":"ack"}`,
//! then:
//!
//! ```json
//! {"topic":"/market/match:BTC-USDT","type":"message","subject":"trade.l3match","data":{"makerOrderId":"6a97d481ab7e1c0007dc369b","price":"77550.9","sequence":"24193081206521856","side":"buy","size":"0.01061604","symbol":"BTC-USDT","takerOrderId":"484534653953355776","time":"1788335265722000000","tradeId":"…"}}
//! ```
//!
//! Read from that capture:
//! - **`time` is epoch *nanoseconds*, as a string** — nineteen digits,
//!   where every other venue here sends milliseconds. Reading it as
//!   milliseconds puts the trade some 56 million years in the future.
//! - `price` and `size` are strings, `size` is the base-asset quantity,
//!   and the symbol is on each entry as `BTC-USDT` — the catalog's own
//!   `source_symbol`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::{VenueClient, normalise_symbol};
use serde::Deserialize;

/// `POST` here for the endpoint and token to dial with — confirmed live
/// 2026-09-02.
pub(crate) const KUCOIN_BULLET_URL: &str = "https://api.kucoin.com/api/v1/bullet-public";

/// KuCoin joins base and quote with `-`.
const SEPARATOR: char = '-';

/// Under the venue's own `pingInterval` of 18 seconds, read from the
/// `bullet-public` response quoted in this module's docs — not a guess.
const KEEPALIVE: Duration = Duration::from_secs(15);

/// The weight one token request costs against KuCoin's shared budget.
const BULLET_COST: u32 = 1;

/// KuCoin's public trade-match channel.
pub(crate) struct KucoinTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
    client: VenueClient,
    /// Where to ask for a token. Overridable for the same reason this
    /// plugin's other sources take a URL — so a test can point at a local
    /// server instead of the venue.
    bullet_url: String,
}

impl KucoinTradesProtocol {
    pub(crate) fn new(
        source_id: impl Into<Box<str>>,
        symbols: Arc<dyn SymbolMap>,
        client: VenueClient,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
            client,
            bullet_url: KUCOIN_BULLET_URL.to_owned(),
        }
    }

    /// Points the token request at `url` instead of KuCoin's own.
    #[cfg(test)]
    pub(crate) fn with_bullet_url(mut self, url: impl Into<String>) -> Self {
        self.bullet_url = url.into();
        self
    }

    fn topic(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no KuCoin native symbol known for {instrument}"))
        })?;
        Ok(format!("/market/match:{symbol}"))
    }
}

#[async_trait]
impl VenueProtocol for KucoinTradesProtocol {
    /// Only ever a fallback for a caller that does not await
    /// [`endpoint`](VenueProtocol::endpoint): dialling this without a
    /// token is refused by the venue. The dial path uses `endpoint`.
    fn url(&self) -> &str {
        KUCOIN_BULLET_URL
    }

    async fn endpoint(&self) -> Result<String, ConnectionError> {
        let body = self
            .client
            .post(&self.bullet_url, BULLET_COST)
            .await
            .map_err(|source| {
                ConnectionError::new(format!("KuCoin refused a WebSocket token: {source}"))
            })?;
        let bullet: Bullet = serde_json::from_slice(&body).map_err(|source| {
            ConnectionError::new(format!("KuCoin's token response did not decode: {source}"))
        })?;
        let server = bullet.data.instance_servers.first().ok_or_else(|| {
            ConnectionError::new("KuCoin's token response named no WebSocket endpoint")
        })?;
        // The endpoint arrives with its trailing slash (`wss://…kucoin.com/`)
        // and needs to keep it: KuCoin answers a handshake whose path is
        // empty with `400 Bad Request`, which looks exactly like a
        // transport fault in the logs.
        Ok(format!(
            "{}?token={}&connectId=senken",
            server.endpoint, bullet.data.token
        ))
    }

    fn venue(&self) -> &'static str {
        "kucoin"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let topic = self.topic(instrument)?;
        Ok(format!(
            r#"{{"id":"{topic}","type":"subscribe","topic":"{topic}","privateChannel":false,"response":true}}"#
        ))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let topic = self.topic(instrument)?;
        Ok(format!(
            r#"{{"id":"{topic}","type":"unsubscribe","topic":"{topic}","privateChannel":false,"response":true}}"#
        ))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<Frame>(text) else {
            return Vec::new();
        };
        if frame.kind != "message" {
            return Vec::new();
        }
        let Some(data) = frame.data else {
            return Vec::new();
        };
        let Ok(instrument) = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(&data.symbol, &[SEPARATOR]),
        ) else {
            return Vec::new();
        };
        // Nanoseconds, not milliseconds — see this module's docs.
        let Ok(nanos) = data.time.trim().parse::<i64>() else {
            return Vec::new();
        };
        trade(
            senken_core::UnixNanos::from_nanos(nanos),
            &data.price,
            &data.size,
        )
        .map(|update| vec![(instrument, LiveUpdate::Price(update))])
        .unwrap_or_default()
    }

    fn keepalive(&self) -> Option<(Duration, String)> {
        Some((KEEPALIVE, r#"{"id":"senken","type":"ping"}"#.to_owned()))
    }
}

/// The `bullet-public` response.
#[derive(Debug, Deserialize)]
struct Bullet {
    data: BulletData,
}

#[derive(Debug, Deserialize)]
struct BulletData {
    token: String,
    #[serde(rename = "instanceServers")]
    instance_servers: Vec<InstanceServer>,
}

#[derive(Debug, Deserialize)]
struct InstanceServer {
    endpoint: String,
}

/// One inbound frame. `welcome`, `ack` and `pong` carry no `data`.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    data: Option<Match>,
}

#[derive(Debug, Deserialize)]
struct Match {
    symbol: String,
    price: String,
    /// Base-asset quantity.
    size: String,
    /// Epoch **nanoseconds**, as a string.
    time: String,
}

/// KuCoin's live-feed registration — spot only. KuCoin Futures issues its
/// token from a different host that no capture here has reached.
pub(crate) struct KucoinFeedSource {
    source_ids: Vec<String>,
    client: VenueClient,
}

impl KucoinFeedSource {
    pub(crate) fn new(client: VenueClient) -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
            client,
        }
    }
}

impl FeedSource for KucoinFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(KucoinTradesProtocol::new(
            crate::SPOT_ID,
            symbols,
            self.client.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::KucoinTradesProtocol;
    use senken_marketdata::InstrumentId;
    use senken_subscription::{LiveUpdate, SymbolMap, VenueProtocol};
    use senken_venue::{LimitGroup, VenueClient};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct DashedMap;
    impl SymbolMap for DashedMap {
        fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
            instrument
                .symbol()
                .strip_suffix("USDT")
                .map(|base| format!("{base}-USDT"))
        }
    }

    fn client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("kucoin-test"))
    }

    fn protocol() -> KucoinTradesProtocol {
        KucoinTradesProtocol::new(crate::SPOT_ID, Arc::new(DashedMap), client())
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSDT").unwrap()
    }

    #[test]
    fn the_subscribe_frame_matches_the_confirmed_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["type"], "subscribe");
        assert_eq!(frame["topic"], "/market/match:BTC-USDT");
        assert_eq!(frame["privateChannel"], false);
    }

    #[test]
    fn an_unsubscribe_frame_names_the_same_topic() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["type"], "unsubscribe");
        assert_eq!(frame["topic"], "/market/match:BTC-USDT");
    }

    /// Byte-for-byte a frame from this module's live capture. `time` is
    /// nanoseconds; reading it as milliseconds would date the trade some
    /// 56 million years from now.
    #[test]
    fn the_captured_match_frame_reads_its_nanosecond_timestamp_as_nanoseconds() {
        let frame = r#"{"topic":"/market/match:BTC-USDT","type":"message","subject":"trade.l3match","data":{"makerOrderId":"6a97d481ab7e1c0007dc369b","price":"77550.9","sequence":"24193081206521856","side":"buy","size":"0.01061604","symbol":"BTC-USDT","takerOrderId":"484534653953355776","time":"1788335265722000000","tradeId":"1"}}"#;

        let updates = protocol().parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trade.l3match frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 775_509);
        assert_eq!(update.price_scale, 1);
        assert_eq!(update.qty, senken_series::Volume::Real(1_061_604));
        assert_eq!(update.qty_scale, 8);
        assert_eq!(update.ts.as_nanos(), 1_788_335_265_722_000_000);
        assert_eq!(update.ts.as_millis(), 1_788_335_265_722);
    }

    #[test]
    fn the_captured_welcome_and_ack_yield_nothing() {
        assert!(
            protocol()
                .parse_message(r#"{"id":"senken1","type":"welcome"}"#)
                .is_empty()
        );
        assert!(
            protocol()
                .parse_message(r#"{"id":"1","type":"ack"}"#)
                .is_empty()
        );
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    /// The endpoint is built from the venue's own answer, not assembled
    /// from a guessed host: a token issued for one instance server does
    /// not authorise another, and a token captured once goes stale.
    #[tokio::test]
    async fn the_endpoint_carries_the_token_the_venue_just_issued() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bullet-public"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"code":"200000","data":{"token":"a-token","instanceServers":[{"endpoint":"wss://ws-api-spot.kucoin.com/","encrypt":true,"protocol":"websocket","pingInterval":18000,"pingTimeout":10000}]}}"#,
            ))
            .mount(&server)
            .await;

        let protocol = protocol().with_bullet_url(format!("{}/api/v1/bullet-public", server.uri()));

        assert_eq!(
            protocol.endpoint().await.unwrap(),
            "wss://ws-api-spot.kucoin.com/?token=a-token&connectId=senken",
            "the venue's own trailing slash is kept: a path-less handshake is refused with 400"
        );
    }

    /// A `GET` on this endpoint is refused by the venue with 405, so a
    /// dial that used one would fail on every attempt. The mock accepts
    /// only `POST`, and an unmatched request is a 404 the decode rejects.
    #[tokio::test]
    async fn a_token_request_that_is_refused_fails_the_dial_rather_than_dialling_untokened() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bullet-public"))
            .respond_with(
                ResponseTemplate::new(405)
                    .set_body_string(r#"{"msg":"Method Not Allowed","code":"405000"}"#),
            )
            .mount(&server)
            .await;

        let protocol = protocol().with_bullet_url(format!("{}/api/v1/bullet-public", server.uri()));

        assert!(protocol.endpoint().await.is_err());
    }
}
