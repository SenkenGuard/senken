//! An HTTP client that shares one venue's rate/concurrency/failure budget.
//!
//! [`VenueClient`] is what [`HttpSource`](crate::HttpSource) fetches through,
//! and what bar fetching will share it with — the whole point of
//! keying the budget by [`LimitGroup`] rather than by source is that both
//! traffic kinds draw from the same venue quota.

use std::time::Duration;

use reqwest::header::HeaderMap;
use senken_marketdata::source::SourceError;

use crate::jitter::full_jitter;
use crate::limit_group::LimitGroup;
use crate::retry::RetryPolicy;

/// Response headers verified to carry a venue's own count for one
/// of our [`LimitGroup::per_window`] buckets. The venue's accounting is
/// authoritative; ours is a guess reconciled to it.
///
/// Only Binance is verified to send anything at all — OKX's public endpoints
/// send no rate-limit headers, confirmed the same day. Extend this list only
/// once a header is fetched and observed, never from documentation.
const KNOWN_WEIGHT_HEADERS: &[(&str, Duration)] = &[
    // `api.binance.com/api/v3/klines`, captured live 2026-08-30:
    // observed value `750` during capture.
    ("x-mbx-used-weight-1m", Duration::from_mins(1)),
];

/// An HTTP client bound to one venue's [`LimitGroup`].
///
/// Cheap to clone: cloning shares the underlying [`reqwest::Client`]
/// connection pool and the same group, exactly like [`reqwest::Client`]
/// itself.
#[derive(Debug, Clone)]
pub struct VenueClient {
    http: reqwest::Client,
    group: LimitGroup,
    retry_policy: RetryPolicy,
}

impl VenueClient {
    /// A client that fetches through `http`, gated by `group`.
    #[must_use]
    pub fn new(http: reqwest::Client, group: LimitGroup) -> Self {
        Self {
            http,
            group,
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Overrides how many attempts a request gets and how it backs off.
    /// Defaults to [`RetryPolicy::INTERACTIVE`] — a background job
    ///  should pass [`RetryPolicy::BACKFILL`] instead.
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    /// The group this client draws its budget from.
    #[must_use]
    pub fn group(&self) -> &LimitGroup {
        &self.group
    }

    /// Fetches `url`, weighted at `cost` against the group's proactive
    /// windows (a plain ping and a heavy kline page do not cost the same).
    ///
    /// Waits for the group's rate and concurrency budget, retries retryable
    /// failures with jittered backoff, reconciles the group's accounting from
    /// any recognised rate-limit headers on success, and fails fast without
    /// retrying at all if the group's circuit is open.
    ///
    /// # Errors
    /// [`SourceError::Transport`] or [`SourceError::Http`] when every attempt
    /// is exhausted; a [`SourceError::Rejected`] immediately if the group's
    /// circuit breaker is open.
    pub async fn get(&self, url: &str, cost: u32) -> Result<Vec<u8>, SourceError> {
        let _permit = self.group.acquire(cost).await?;

        let mut backoff = self.retry_policy.first_backoff;
        for attempt in 1..=self.retry_policy.max_attempts {
            let response = send_once(&self.http, url).await?;

            if response.status.is_success() {
                self.group.reconcile_from_headers(&response.headers);
                self.group.record_success();
                return Ok(response.body);
            }

            if response.status.as_u16() == 418 {
                // Binance's ban status. Queueing behind it only turns one
                // error into a stall, so trip the breaker and stop now
                // rather than spending the rest of this call's attempts.
                self.group.trip_circuit();
                return Err(http_error(&response));
            }

            if response.status.as_u16() == 429 {
                let tripped = self.group.record_429();
                if tripped {
                    return Err(http_error(&response));
                }
                if attempt == self.retry_policy.max_attempts {
                    return Err(http_error(&response));
                }
                let wait = retry_after(&response.headers).unwrap_or_else(|| {
                    let jittered = full_jitter(backoff);
                    backoff *= 2;
                    jittered
                });
                tokio::time::sleep(wait).await;
                continue;
            }

            let error = http_error(&response);
            if !error.is_retryable() || attempt == self.retry_policy.max_attempts {
                return Err(error);
            }
            tokio::time::sleep(full_jitter(backoff)).await;
            backoff *= 2;
        }
        unreachable!("the last attempt in the loop above always returns")
    }
}

struct RawResponse {
    status: reqwest::StatusCode,
    body: Vec<u8>,
    headers: HeaderMap,
}

fn http_error(response: &RawResponse) -> SourceError {
    SourceError::http(
        response.status.as_u16(),
        String::from_utf8_lossy(&response.body),
    )
}

/// One HTTP round trip, with no retry and no interpretation of the status —
/// callers decide what a given status means for their own budget.
async fn send_once(client: &reqwest::Client, url: &str) -> Result<RawResponse, SourceError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(SourceError::transport)?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.bytes().await.map_err(SourceError::transport)?;
    Ok(RawResponse {
        status,
        body: body.to_vec(),
        headers,
    })
}

/// Reads `Retry-After`, which [RFC 9110 §10.2.3] allows as either a delay in
/// seconds or an HTTP date. Both forms are handled; neither venue
/// is verified to send this header at all, so this is generic HTTP handling,
/// not a venue-specific fact.
///
/// [RFC 9110 §10.2.3]: https://www.rfc-editor.org/rfc/rfc9110#section-10.2.3
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = chrono::DateTime::parse_from_rfc2822(value.trim()).ok()?;
    let delta = when.timestamp() - chrono::Utc::now().timestamp();
    (delta > 0).then(|| Duration::from_secs(delta.unsigned_abs()))
}

impl LimitGroup {
    /// Parses every header in [`KNOWN_WEIGHT_HEADERS`] present on `headers`
    /// and reconciles the matching window. A group with a window of that
    /// duration but a venue that never sends the header (OKX)
    /// simply never reconciles — the proactive bucket alone still applies.
    fn reconcile_from_headers(&self, headers: &HeaderMap) {
        for (name, window) in KNOWN_WEIGHT_HEADERS {
            if let Some(used) = headers
                .get(*name)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                self.reconcile_window(*window, used);
            }
        }
    }
}
