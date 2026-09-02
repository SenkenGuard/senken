//! Dialling a venue WebSocket through an HTTP proxy, when one is configured.
//!
//! # Why this exists
//!
//! `reqwest` honours `HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY` by itself, so every
//! REST call this project makes already goes through a configured proxy with
//! no code. `tokio_tungstenite::connect_async` does not: it opens a plain
//! `TcpStream` to the venue and has no proxy support at all.
//!
//! That asymmetry is worse than it sounds, because of *where* it bites. A
//! proxy is generally configured precisely because the venue is unreachable
//! directly — Binance answers HTTP 451 and Bybit's CDN answers 403 from a
//! restricted jurisdiction. On such a network the REST half of a venue
//! (instruments, klines) would work while its live feed failed, and the
//! failure would be quiet: a pool that cannot dial logs a warning and retries,
//! and a client sees a subscribed topic that never ticks. Half a venue
//! working is the most confusing possible outcome.
//!
//! # What is supported, and what is not
//!
//! An HTTP `CONNECT` tunnel, the mechanism a proxy uses for any TLS
//! connection — the same one `HTTPS_PROXY` already gives the REST client.
//! `ws://` and `wss://` both tunnel the same way; TLS, when the URL asks for
//! it, is negotiated end-to-end *inside* the tunnel, so the proxy sees only
//! an opaque byte stream and never the venue traffic.
//!
//! Deliberately not supported: SOCKS proxies, and proxy auth schemes beyond
//! Basic. Both would be guesses — nothing in this project has exercised
//! either, and a half-implemented auth scheme fails in ways that look like a
//! venue problem.

use std::env;

use base64::Engine as _;
use senken_subscription::ConnectionError;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A proxy read from the environment, in the order a caller's shell is
/// expected to set them.
///
/// `ALL_PROXY` is consulted last rather than first: a shell that sets both
/// usually means the more specific one. `NO_PROXY` is honoured so a local
/// or intranet venue (a test's own fake server) is never tunnelled.
pub(crate) struct HttpProxy {
    host: String,
    port: u16,
    /// `Proxy-Authorization` header value, already encoded.
    auth: Option<String>,
}

impl HttpProxy {
    /// The proxy configured for `target_host`, or `None` when there is none
    /// or `NO_PROXY` exempts the target.
    pub(crate) fn for_host(target_host: &str) -> Option<Self> {
        if no_proxy_matches(target_host) {
            return None;
        }
        let raw = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
            .iter()
            .find_map(|name| env::var(name).ok())
            .filter(|value| !value.trim().is_empty())?;
        Self::parse(&raw)
    }

    /// Parses `scheme://[user:pass@]host[:port]`.
    ///
    /// A SOCKS URL returns `None` rather than being tunnelled as HTTP: a
    /// SOCKS proxy would reject a `CONNECT` line, and failing at the dial
    /// with a clear absence beats failing mid-handshake with a parse error.
    fn parse(raw: &str) -> Option<Self> {
        let rest = raw.split_once("://").map_or(raw, |(scheme, rest)| {
            if scheme.starts_with("socks") {
                ""
            } else {
                rest
            }
        });
        if rest.is_empty() {
            return None;
        }
        let (credentials, authority) = match rest.rsplit_once('@') {
            Some((credentials, authority)) => (Some(credentials), authority),
            None => (None, rest),
        };
        let authority = authority.trim_end_matches('/');
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), port.parse().ok()?),
            None => (authority.to_owned(), 80),
        };
        if host.is_empty() {
            return None;
        }
        Some(Self {
            host,
            port,
            auth: credentials.map(|credentials| {
                format!(
                    "Basic {}",
                    base64::engine::general_purpose::STANDARD.encode(credentials)
                )
            }),
        })
    }

    /// Opens a tunnel to `host:port` and returns the stream on the far side.
    ///
    /// # Errors
    /// [`ConnectionError`] if the proxy cannot be reached, refuses the
    /// tunnel, or answers anything but a 2xx status.
    pub(crate) async fn tunnel(&self, host: &str, port: u16) -> Result<TcpStream, ConnectionError> {
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|source| {
                ConnectionError::new(format!("could not reach the configured proxy: {source}"))
            })?;

        let mut request = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
        if let Some(auth) = &self.auth {
            use std::fmt::Write as _;
            let _ = write!(request, "Proxy-Authorization: {auth}\r\n");
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|source| ConnectionError::new(format!("proxy CONNECT failed: {source}")))?;

        // Read exactly the status line and headers, and no further: whatever
        // follows the blank line is already the venue's own bytes, and
        // consuming any of it here would corrupt the TLS handshake that runs
        // next.
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).await.map_err(|source| {
            ConnectionError::new(format!("proxy closed before answering CONNECT: {source}"))
        })?;
        if !proxy_accepted(&status) {
            return Err(ConnectionError::new(format!(
                "proxy refused the tunnel: {}",
                status.trim()
            )));
        }
        loop {
            let mut header = String::new();
            let read = reader.read_line(&mut header).await.map_err(|source| {
                ConnectionError::new(format!("proxy closed mid-response: {source}"))
            })?;
            if read == 0 || header == "\r\n" || header == "\n" {
                break;
            }
        }
        Ok(reader.into_inner())
    }
}

/// Whether a `CONNECT` status line reports success.
fn proxy_accepted(status: &str) -> bool {
    status
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

/// Whether the `NO_PROXY` in this process's environment exempts `host`.
fn no_proxy_matches(host: &str) -> bool {
    ["NO_PROXY", "no_proxy"]
        .iter()
        .find_map(|name| env::var(name).ok())
        .is_some_and(|list| no_proxy_list_matches(&list, host))
}

/// Whether `list` — a `NO_PROXY` value — exempts `host`.
///
/// Matches a bare host, a leading-dot suffix (`.example.com`), and the
/// wildcard `*`: the forms every client this project talks to understands.
///
/// Takes the list rather than reading the environment so it can be tested
/// against every form without mutating process-wide state — which this
/// workspace could not do anyway, since `set_var` is `unsafe` and
/// `unsafe_code` is forbidden here.
fn no_proxy_list_matches(list: &str, host: &str) -> bool {
    list.split(',').map(str::trim).any(|entry| {
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }
        let entry = entry.trim_start_matches('.');
        host == entry || host.ends_with(&format!(".{entry}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{HttpProxy, no_proxy_list_matches, proxy_accepted};

    #[test]
    fn a_proxy_url_with_credentials_is_split_into_authority_and_basic_auth() {
        let proxy = HttpProxy::parse("http://alice:s3cret@proxy.internal:3128").unwrap();
        assert_eq!(proxy.host, "proxy.internal");
        assert_eq!(proxy.port, 3128);
        // "alice:s3cret" base64-encoded.
        assert_eq!(proxy.auth.as_deref(), Some("Basic YWxpY2U6czNjcmV0"));
    }

    #[test]
    fn a_proxy_url_without_a_port_defaults_to_eighty() {
        let proxy = HttpProxy::parse("http://proxy.internal").unwrap();
        assert_eq!(proxy.port, 80);
        assert!(proxy.auth.is_none());
    }

    #[test]
    fn a_socks_proxy_is_declined_rather_than_tunnelled_as_http() {
        // A SOCKS proxy would reject a `CONNECT` line. Declining here fails
        // at the dial with a clear absence instead of mid-handshake with a
        // parse error that reads like a venue problem.
        assert!(HttpProxy::parse("socks5://proxy.internal:1080").is_none());
        assert!(HttpProxy::parse("socks5h://proxy.internal:1080").is_none());
    }

    #[test]
    fn nonsense_is_declined_rather_than_dialled() {
        assert!(HttpProxy::parse("").is_none());
        assert!(HttpProxy::parse("http://").is_none());
        assert!(HttpProxy::parse("http://proxy.internal:not-a-port").is_none());
    }

    #[test]
    fn only_a_two_hundred_range_status_opens_the_tunnel() {
        assert!(proxy_accepted("HTTP/1.1 200 Connection established\r\n"));
        assert!(!proxy_accepted(
            "HTTP/1.1 407 Proxy Authentication Required\r\n"
        ));
        assert!(!proxy_accepted("HTTP/1.1 502 Bad Gateway\r\n"));
        assert!(!proxy_accepted("garbage\r\n"));
    }

    #[test]
    fn no_proxy_exempts_a_bare_host_and_a_dotted_suffix() {
        let list = "localhost,.internal.test";
        assert!(no_proxy_list_matches(list, "localhost"));
        assert!(no_proxy_list_matches(list, "venue.internal.test"));
        assert!(!no_proxy_list_matches(list, "ws.okx.com"));
    }

    #[test]
    fn a_suffix_entry_does_not_match_a_host_that_merely_ends_in_the_same_letters() {
        // `.example.com` must not exempt `notexample.com`. Matching on a
        // bare `ends_with` would, and would quietly send a venue's traffic
        // direct on a network where only the proxy can reach it.
        assert!(!no_proxy_list_matches(".example.com", "notexample.com"));
        assert!(no_proxy_list_matches(".example.com", "api.example.com"));
    }

    #[test]
    fn the_wildcard_exempts_everything() {
        assert!(no_proxy_list_matches("*", "ws.okx.com"));
    }

    #[test]
    fn an_empty_or_blank_list_exempts_nothing() {
        // A trailing comma is ordinary in a shell-set list; it must not
        // become a wildcard.
        assert!(!no_proxy_list_matches("", "ws.okx.com"));
        assert!(!no_proxy_list_matches("localhost,", "ws.okx.com"));
    }
}
