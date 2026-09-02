//! Shared plumbing for Senken venue plugins.
//!
//! Every venue adapter does the same three things — fetch a document over
//! HTTP, decode numbers that the venue encodes in whichever way it likes,
//! and expose the result as a [`MarketDataSource`]. This crate owns those
//! three things so a plugin is left with only what is genuinely specific to
//! its venue: the URL and the mapping from its JSON to [`Instrument`].
//!
//! A venue plugin typically registers several sources — one per market type
//!   — by calling [`HttpSource::new`] once per market and handing each to
//! `ActivationContext::register_marketdata_source`. Which markets a venue
//! exposes, and how they are split into sources, is entirely the plugin's
//! decision; `senken-marketdata` only ever sees sources that list
//! instruments.
//!
//! [`HttpSource`] fetches through a [`VenueClient`], not a bare
//! [`reqwest::Client`]: several sources of one venue — `binance-spot`,
//! `binance-usdm`, `binance-coinm` — share one IP-level quota, so the budget
//! is keyed by [`LimitGroup`] (one per venue) rather than by source
//! . A plugin builds one group and shares clones of the
//! `VenueClient` built from it across every source it registers.

use async_trait::async_trait;
use senken_marketdata::instrument::Instrument;
use senken_marketdata::source::{MarketDataSource, SourceError};

mod jitter;
mod limit_group;
mod num;
mod retry;
mod venue_client;

pub use crate::jitter::full_jitter;
pub use crate::limit_group::{ConnectPermit, LimitGroup};
pub use crate::num::Num;
pub use crate::retry::RetryPolicy;
pub use crate::venue_client::VenueClient;

/// Decodes one venue document into normalised instruments.
///
/// A plain function pointer, not a closure: it keeps [`HttpSource`] simple
/// to construct and lets the same parser be unit-tested against a saved
/// fixture without any HTTP at all.
pub type ParseInstruments = fn(&[u8]) -> Result<Vec<Instrument>, SourceError>;

/// Reads the cursor that asks a venue for the next page, or `None` when the
/// document just decoded was the last one.
pub type ReadCursor = fn(&[u8]) -> Option<String>;

/// How a venue hands over a catalog too large for one response.
///
/// This is the opposite direction from
/// [`InstrumentQuery::with_page`](senken_marketdata::InstrumentQuery::with_page):
/// that one slices results the caller already has, while this one is about
/// getting the rows out of the venue in the first place. A catalog that is
/// never fetched cannot be ranked, so the two never substitute for each
/// other.
#[derive(Debug, Clone, Copy)]
struct Pagination {
    read_cursor: ReadCursor,
    /// The query parameter the cursor is sent back in.
    parameter: &'static str,
    /// A backstop against a venue that keeps handing out cursors.
    max_pages: usize,
}

/// Fetches a document, turning any non-success status into
/// [`SourceError::Http`] with a truncated copy of the body.
///
/// A failure that [`SourceError::is_retryable`] accepts — a transport
/// error, a 429, a 5xx — is retried with a doubling, jittered backoff, per
/// [`RetryPolicy::INTERACTIVE`]. Everything else fails immediately, because
/// asking a venue twice to reject the same malformed request only wastes its
/// quota.
///
/// This performs no rate limiting of its own — it is the retry primitive
/// [`VenueClient`] is built on, for callers that only need retry. A venue
/// plugin should prefer `VenueClient::get` so its traffic shares that
/// venue's budget.
///
/// # Errors
/// [`SourceError::Transport`] when the request does not complete, or
/// [`SourceError::Http`] when the venue answers with a non-2xx status.
pub async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, SourceError> {
    fetch_bytes_with_policy(client, url, RetryPolicy::default()).await
}

/// As [`fetch_bytes`], but with the number of attempts and the initial
/// backoff as a caller-supplied [`RetryPolicy`] rather than a fixed constant
///   — a background backfill can afford to try more times than a chart waiting
/// on a response.
///
/// # Errors
/// See [`fetch_bytes`].
pub async fn fetch_bytes_with_policy(
    client: &reqwest::Client,
    url: &str,
    policy: RetryPolicy,
) -> Result<Vec<u8>, SourceError> {
    let mut backoff = policy.first_backoff;
    for attempt in 1..=policy.max_attempts {
        let error = match fetch_once(client, url).await {
            Ok(body) => return Ok(body),
            Err(error) => error,
        };
        if !error.is_retryable() || attempt == policy.max_attempts {
            return Err(error);
        }
        // Full jitter (see the `jitter` module): every backoff is sampled
        // from `[0, backoff]`, not slept as computed, so many callers whose
        // backoffs started from the same instant do not retry in lockstep.
        let wait = jitter::full_jitter(backoff);
        tracing::warn!(
            url,
            attempt,
            of = policy.max_attempts,
            ?backoff,
            ?wait,
            %error,
            "venue request failed; retrying"
        );
        tokio::time::sleep(wait).await;
        backoff *= 2;
    }
    // The loop either returns a body or the final error.
    unreachable!("the last attempt always returns")
}

async fn fetch_once(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, SourceError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(SourceError::transport)?;
    let status = response.status();
    let body = response.bytes().await.map_err(SourceError::transport)?;
    if !status.is_success() {
        return Err(SourceError::http(
            status.as_u16(),
            String::from_utf8_lossy(&body),
        ));
    }
    Ok(body.to_vec())
}

/// The weight charged against a source's [`LimitGroup`] for one instrument
/// document. Instrument catalogs are fetched at most once per TTL (today 24
/// hours), never per user action, so a nominal, uniform cost is enough —
/// unlike bar fetching, no venue's documented weight for this
/// endpoint has been fetched and verified, so none is invented here.
///
/// [`LimitGroup`]: crate::LimitGroup
const INSTRUMENT_FETCH_COST: u32 = 1;

/// One market of one venue, listed from a single HTTP endpoint.
///
/// The venue-specific part is the `parse` function; everything else is the
/// same for every exchange. Construct one per market type — spot, linear
/// perpetuals, inverse perpetuals, dated futures — each with its own source
/// id, so a user can search or refresh them independently.
#[derive(Debug, Clone)]
pub struct HttpSource {
    id: Box<str>,
    name: Box<str>,
    url: String,
    client: VenueClient,
    parse: ParseInstruments,
    pagination: Option<Pagination>,
}

impl HttpSource {
    /// A source with id `id`, display name `name`, listing instruments from
    /// `url` and decoding them with `parse`.
    ///
    /// `client` fetches through `client`'s [`LimitGroup`], so every source
    /// built from clones of the same client shares one venue budget — the
    /// property M3.1 exists for.
    ///
    /// The id is owned rather than `&'static str` because some venues split
    /// one market across several documents — OKX lists options per
    /// underlying family — and each of those becomes its own source.
    ///
    /// [`LimitGroup`]: crate::LimitGroup
    #[must_use]
    pub fn new(
        id: impl Into<Box<str>>,
        name: impl Into<Box<str>>,
        url: impl Into<String>,
        client: VenueClient,
        parse: ParseInstruments,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            url: url.into(),
            client,
            parse,
            pagination: None,
        }
    }

    /// Follows this venue's paging cursor until the catalog runs out.
    ///
    /// `read_cursor` pulls the cursor out of a decoded page and returns
    /// `None` on the last one; `parameter` is the query parameter it is
    /// sent back in. `max_pages` bounds the walk — reaching it is logged
    /// loudly, never silently truncated.
    #[must_use]
    pub fn paginated(
        mut self,
        read_cursor: ReadCursor,
        parameter: &'static str,
        max_pages: usize,
    ) -> Self {
        self.pagination = Some(Pagination {
            read_cursor,
            parameter,
            max_pages,
        });
        self
    }

    /// This source's URL with `parameter` set to `cursor`.
    fn page_url(&self, parameter: &str, cursor: &str) -> Result<String, SourceError> {
        let mut url = reqwest::Url::parse(&self.url).map_err(SourceError::transport)?;
        // Replace rather than append, so a walk never grows the query
        // string one cursor per page.
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| key != parameter)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(kept)
            .append_pair(parameter, cursor);
        Ok(url.into())
    }

    /// Points the source at a different URL — a regional host, a mirror, or
    /// a local stand-in in tests.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// The URL this source lists from.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait]
impl MarketDataSource for HttpSource {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
        let mut instruments = Vec::new();
        let mut url = self.url.clone();

        for page in 1.. {
            let body = self.client.get(&url, INSTRUMENT_FETCH_COST).await?;
            instruments.extend((self.parse)(&body)?);

            let Some(pagination) = &self.pagination else {
                break;
            };
            let Some(cursor) = (pagination.read_cursor)(&body) else {
                break;
            };
            if page >= pagination.max_pages {
                // Never truncate silently.
                tracing::warn!(
                    source = %self.id,
                    pages = page,
                    listed = instruments.len(),
                    "stopped at the page limit; the venue still had more instruments"
                );
                break;
            }
            url = self.page_url(pagination.parameter, &cursor)?;
        }

        if instruments.is_empty() {
            // Adapters declare every field with a serde default so an
            // unrelated venue-side change cannot break decoding. The cost
            // is that a *renamed* field decodes to nothing at all rather
            // than failing, so an empty catalog is worth saying out loud.
            tracing::warn!(
                source = %self.id,
                "venue returned no usable instruments; its document may have changed shape"
            );
        } else {
            tracing::debug!(
                source = %self.id,
                count = instruments.len(),
                "venue instruments normalised"
            );
        }
        Ok(instruments)
    }
}

/// Logs and skips a venue entry that cannot satisfy the [`Instrument`]
/// contract, so one malformed row never fails a whole catalog.
///
/// Returns `None` so it composes directly with `filter_map`.
pub fn skip<T>(source: &str, symbol: &str, reason: &str) -> Option<T> {
    tracing::warn!(source, symbol, reason, "skipping unusable venue instrument");
    None
}

/// Normalises a venue symbol into the cross-venue form: upper case, with
/// this venue's own separators removed. On a venue that writes `BTC-USDT`,
/// `normalise_symbol("BTC-USDT", &['-'])` is `BTCUSDT`.
///
/// **Only `separators` are removed — nothing else.** Punctuation is part of
/// a token's name far more often than it is a separator: venues really do
/// list `$U` beside `U`, `D.O.G.E.` beside `DOGE` and `H_OLD` beside
/// `HOLD`. Stripping punctuation wholesale collapses each of those pairs
/// onto one symbol, and a catalog can only keep one of them. Pass the
/// characters this venue actually uses between legs, and nothing more.
///
/// # Examples
/// ```
/// use senken_venue::normalise_symbol;
///
/// assert_eq!(normalise_symbol("BTC-USDT", &['-']), "BTCUSDT");
/// assert_eq!(normalise_symbol("BTCUSDT_260925", &['_']), "BTCUSDT260925");
/// // `$` names a token here; it is not a separator.
/// assert_eq!(normalise_symbol("$U-USDT", &['-']), "$UUSDT");
/// assert_ne!(
///     normalise_symbol("$U-USDT", &['-']),
///     normalise_symbol("U-USDT", &['-'])
/// );
/// ```
#[must_use]
pub fn normalise_symbol(source_symbol: &str, separators: &[char]) -> String {
    source_symbol
        .chars()
        .filter(|c| !c.is_whitespace() && !separators.contains(c))
        .flat_map(char::to_uppercase)
        .collect()
}

/// Splits a venue symbol such as `KRW-BTC` or `BTC_USDT` on its separator.
/// `None` when there is no separator to split on.
#[must_use]
pub fn split_pair(source_symbol: &str, separator: char) -> Option<(&str, &str)> {
    source_symbol.split_once(separator)
}

/// Reads an RFC 3339 / ISO 8601 timestamp — `2026-09-25T12:00:00.000Z` —
/// as Unix milliseconds, the form [`Contract::expiry`] holds.
///
/// Venues are split roughly evenly between epoch milliseconds and this,
/// so a plugin reading the latter converts once here.
///
/// [`Contract::expiry`]: senken_marketdata::Contract::expiry
#[must_use]
pub fn iso8601_ms(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// The smallest common scale that can represent every value in `values`
/// without losing precision — the maximum of each value's own
/// [`senken_marketdata::decimal_places`].
///
/// A page of bars arrives as several rows of decimal strings that
/// must share **one** fixed-point scale per column: parsing each row's
/// price independently could legitimately land on a different scale per
/// row (a venue's own price formatting varies by magnitude), which is
/// meaningless for values meant to sit in one `i64` column together. This
/// is the same "pick the scale the data actually needs" idea
/// [`parse_increment`](senken_marketdata::parse_increment) already applies
/// to a single tick or step size, generalised to a whole batch.
///
/// Returns `0` for an empty iterator, the scale of a plain integer.
///
/// # Examples
/// ```
/// use senken_venue::common_scale;
///
/// assert_eq!(common_scale(["78169.48000000", "78146.0", "1"]), 2);
/// assert_eq!(common_scale(std::iter::empty()), 0);
/// ```
#[must_use]
pub fn common_scale<'a>(values: impl IntoIterator<Item = &'a str>) -> u8 {
    values
        .into_iter()
        .map(senken_marketdata::decimal_places)
        .max()
        .unwrap_or(0)
}

/// [`common_scale`], but `None` when a value in the batch cannot actually
/// be represented as a scaled `i64` at it.
///
/// [`common_scale`] answers "how many fractional digits did the venue
/// write", which is the right question for every venue whose precision
/// fits — and most do. Some do not: KuCoin reports quantities with twenty
/// decimal places, which at that scale is 8.9e21 against an `i64` ceiling
/// of 9.2e18.
///
/// There is deliberately no smaller scale to fall back to.
/// [`senken_core::parse_scaled`] refuses a value with more decimals than
/// the scale it is given rather than dropping them, so this project never
/// silently rounds a price or a quantity; walking the scale down until it
/// fits would be circumventing that guard from the outside. The batch
/// either fits or it does not, and a caller that gets `None` reports an
/// honest absence — never a rounded number.
///
/// # Examples
/// ```
/// use senken_venue::exact_common_scale;
///
/// assert_eq!(exact_common_scale(["78169.48", "1"]), Some(2));
/// // Twenty decimal places overflow an `i64` at their own scale.
/// assert_eq!(exact_common_scale(["89.56968223943530450117"]), None);
/// ```
#[must_use]
pub fn exact_common_scale<'a>(values: impl IntoIterator<Item = &'a str> + Clone) -> Option<u8> {
    let scale = common_scale(values.clone());
    values
        .into_iter()
        .all(|value| senken_core::parse_scaled(value, scale).is_some())
        .then_some(scale)
}

#[cfg(test)]
mod tests {
    use super::{
        HttpSource, LimitGroup, ParseInstruments, VenueClient, common_scale, iso8601_ms,
        normalise_symbol, split_pair,
    };

    /// A client with no configured window and the default concurrency
    /// ceiling — everything these tests need, since none of them exercise
    /// rate limiting itself.
    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    #[test]
    fn iso_timestamps_become_epoch_milliseconds() {
        assert_eq!(iso8601_ms("1970-01-01T00:00:01Z"), Some(1_000));
        assert_eq!(
            iso8601_ms("2026-09-25T12:00:00.000Z"),
            Some(1_790_337_600_000)
        );
        assert_eq!(iso8601_ms("not a date"), None);
        assert_eq!(iso8601_ms(""), None);
    }
    use senken_marketdata::source::MarketDataSource;

    const PARSE: ParseInstruments = |_| Ok(Vec::new());

    #[test]
    fn separators_vanish_but_identity_survives() {
        assert_eq!(normalise_symbol("BTC-USDT", &['-']), "BTCUSDT");
        assert_eq!(normalise_symbol("BTC_USDT", &['_']), "BTCUSDT");
        assert_eq!(normalise_symbol("btc/usdt", &['/']), "BTCUSDT");
        assert_eq!(
            normalise_symbol("BTCUSDT_260925", &['_']),
            "BTCUSDT260925",
            "a dated future must not collapse onto its perpetual"
        );
    }

    #[test]
    fn non_ascii_token_names_are_not_stripped() {
        // Binance really does list these, and dropping the non-ASCII part
        // collapsed several distinct symbols onto a bare `USDT`.
        assert_eq!(normalise_symbol("币安人生USDT", &['_']), "币安人生USDT");
        assert_ne!(
            normalise_symbol("币安人生USDT", &['_']),
            normalise_symbol("龙虾USDT", &['_']),
            "two different tokens must not share one symbol"
        );
    }

    #[test]
    fn punctuation_inside_a_token_name_is_never_a_separator() {
        // Every pair below is two *different* tokens that BingX lists side
        // by side. Stripping punctuation wholesale collapsed each pair onto
        // one symbol, and the catalog then dropped whichever came second.
        let bingx = |symbol: &str| normalise_symbol(symbol, &['-']);
        for (left, right) in [
            ("$U-USDT", "U-USDT"),
            ("$RIF-USDT", "RIF-USDT"),
            ("D.O.G.E.-USDT", "DOGE-USDT"),
            ("$1-USDT", "1-USDT"),
            ("H_OLD-USDT", "HOLD-USDT"),
        ] {
            assert_ne!(
                bingx(left),
                bingx(right),
                "`{left}` and `{right}` are different tokens"
            );
        }
        assert_eq!(bingx("$U-USDT"), "$UUSDT");
        assert_eq!(bingx("D.O.G.E.-USDT"), "D.O.G.E.USDT");
        assert_eq!(bingx("H_OLD-USDT"), "H_OLDUSDT");
    }

    #[test]
    fn a_venue_with_no_separator_keeps_its_symbol_whole() {
        assert_eq!(normalise_symbol("BTCUSD", &[]), "BTCUSD");
        assert_eq!(normalise_symbol("sBTCUSDT", &[]), "SBTCUSDT");
    }

    #[test]
    fn pairs_split_on_their_separator() {
        assert_eq!(split_pair("KRW-BTC", '-'), Some(("KRW", "BTC")));
        assert_eq!(split_pair("BTC_USDT", '_'), Some(("BTC", "USDT")));
        assert_eq!(split_pair("BTCUSDT", '-'), None);
    }

    #[test]
    fn a_source_reports_its_identity_and_url() {
        let source = HttpSource::new(
            "demo-spot",
            "Demo Spot",
            "https://example.invalid/a",
            test_client(),
            PARSE,
        );
        assert_eq!(source.id(), "demo-spot");
        assert_eq!(source.name(), "Demo Spot");
        assert_eq!(source.url(), "https://example.invalid/a");
        assert_eq!(
            source.with_url("https://example.invalid/b").url(),
            "https://example.invalid/b"
        );
    }

    #[test]
    fn common_scale_is_the_widest_precision_present() {
        assert_eq!(common_scale(["78169.48000000", "78146.0", "1"]), 2);
        assert_eq!(common_scale(["1", "2", "3"]), 0);
        assert_eq!(common_scale(std::iter::empty()), 0);
    }
}
