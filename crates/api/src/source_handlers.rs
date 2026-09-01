//! What each registered market-data source can actually do
//! (`GET /api/sources`).
//!
//! A catalog entry existing does not mean the source can draw a chart or
//! stream a price: a source needs a registered `BarSource` for the first and
//! a live subscription pool for the second, and most have neither. A client
//! that cannot tell the difference either offers instruments it can do
//! nothing with, or shows a live-price state for a feed that is not running.
//! Both facts are settled at startup; this reports them rather than making
//! the browser guess.
//!
//! Market data is global and never tenanted, so this needs a valid session
//! and nothing more — the same reasoning `instrument_handlers` applies.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::{Extension, Json};

use senken_feed::okx_book;
use senken_marketdata::SourceSummary;
use senken_subscription::BookSource;
use senken_venue::{LimitGroup, VenueClient};

use crate::AppState;
use crate::auth::Authed;
use crate::dto::{BookCapabilityDto, SourceCapabilityDto, SourcesResponse};

/// The one source id this build can serve a book-depth snapshot for —
/// matching `feed::OKX_SPOT_SOURCE`, the same id `feed::build_feed_pools`
/// wires a live quote/price pool under.
const OKX_BOOK_SOURCE: &str = "okx-spot";

/// Builds the book-depth sources this build actually has, keyed by source
/// id — registration is the capability declaration, the same contract every
/// other market-data type in this project uses (see
/// `senken_subscription::BookSource`'s own docs).
///
/// Called once per server, in [`crate::serve_with_feed_pools`], and wired
/// directly against `senken-feed`'s OKX adapter rather than through the
/// plugin/runtime registry — mirroring `crate::feed::build_feed_pools`'s own
/// shape and, for the same reason that function documents, drawing its own
/// independent `LimitGroup` rather than the OKX plugin's REST budget (which
/// lives inside plugin activation and is not exposed to this crate).
pub(crate) fn build_book_sources() -> HashMap<String, Arc<dyn BookSource>> {
    let mut sources: HashMap<String, Arc<dyn BookSource>> = HashMap::new();
    let client = VenueClient::new(reqwest::Client::new(), LimitGroup::new("okx"));
    sources.insert(
        OKX_BOOK_SOURCE.to_owned(),
        Arc::new(okx_book::book_source(client)) as Arc<dyn BookSource>,
    );
    sources
}

/// Joins the registered sources against the capability sets.
///
/// Iteration is over `sources`, never over the capability sets: a loader or
/// a pool keyed by something the catalog does not know is not a venue any
/// client could use, and inventing a row for it would offer an empty
/// instrument list under a real-looking name.
fn capabilities(
    sources: Vec<SourceSummary>,
    chartable: &[&str],
    streamable: impl Fn(&str) -> bool,
    has_book: impl Fn(&str) -> bool,
) -> Vec<SourceCapabilityDto> {
    let mut rows: Vec<SourceCapabilityDto> = sources
        .into_iter()
        .map(|summary| SourceCapabilityDto {
            bars: chartable.contains(&summary.id.as_str()),
            live: streamable(&summary.id),
            // The only registered live pool is OKX's combined trades/tickers
            // protocol, so a pool is also the quote capability declaration.
            quotes: streamable(&summary.id),
            book: BookCapabilityDto {
                supported: has_book(&summary.id),
            },
            id: summary.id,
            name: summary.name,
        })
        .collect();
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows
}

/// `GET /api/sources`: every registered source with what it can do.
#[utoipa::path(
    get,
    path = "/api/sources",
    responses(
        (status = 200, body = SourcesResponse),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_sources(
    State(state): State<AppState>,
    Extension(_auth): Authed,
) -> Json<SourcesResponse> {
    let sources = capabilities(
        state.runtime.marketdata().sources(),
        &state.runtime.series().source_ids(),
        |id| state.feed_pools.contains_key(id),
        |id| state.book_sources.contains_key(id),
    );
    Json(SourcesResponse { sources })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::serve_unfenced_test_server;

    fn summary(id: &str) -> SourceSummary {
        SourceSummary {
            id: id.to_owned(),
            name: id.to_uppercase(),
        }
    }

    #[test]
    fn each_capability_is_reported_independently() {
        let rows = capabilities(
            vec![
                summary("okx-spot"),
                summary("binance-spot"),
                summary("gate"),
            ],
            &["okx-spot", "binance-spot"],
            |id| id == "okx-spot",
            |id| id == "okx-spot",
        );

        let find = |id: &str| rows.iter().find(|row| row.id == id).unwrap();
        assert!(find("okx-spot").bars && find("okx-spot").live);
        assert!(find("okx-spot").quotes);
        assert!(find("okx-spot").book.supported);
        // Chartable but not streamable is the common case, and the one a
        // client must be able to distinguish: history renders, no live price.
        assert!(find("binance-spot").bars && !find("binance-spot").live);
        assert!(!find("binance-spot").quotes);
        assert!(!find("binance-spot").book.supported);
        assert!(!find("gate").bars && !find("gate").live);
    }

    /// A source can stream a live price/quote through a pool without ever
    /// carrying book depth — the two capabilities come from different
    /// registries and must not be conflated the way `live`/`quotes` are.
    #[test]
    fn book_is_independent_of_live_and_quotes() {
        let rows = capabilities(
            vec![summary("okx-spot")],
            &["okx-spot"],
            |id| id == "okx-spot",
            |_| false,
        );

        let row = &rows[0];
        assert!(row.live && row.quotes, "streamable is still reported");
        assert!(
            !row.book.supported,
            "book must not be inferred from live/quotes capability"
        );
    }

    #[test]
    fn a_capability_for_an_unregistered_source_invents_no_row() {
        let rows = capabilities(
            vec![summary("okx-spot")],
            &["ghost"],
            |id| id == "ghost",
            |_| false,
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "okx-spot");
        assert!(!rows[0].bars && !rows[0].live);
    }

    #[tokio::test]
    async fn listing_sources_requires_a_session() {
        let (handle, _identity, _dir, _runtime_dir) = serve_unfenced_test_server().await;
        let addr = handle.local_addr();

        let status = reqwest::get(format!("http://{addr}/api/sources"))
            .await
            .unwrap()
            .status();

        assert_eq!(status, reqwest::StatusCode::UNAUTHORIZED);
    }
}
