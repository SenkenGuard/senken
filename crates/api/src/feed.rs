//! Live price feed wiring: one [`SubscriptionPool`] per live feed the
//! active plugins actually registered.
//!
//! Nothing here names a venue. A plugin declares that it can stream by
//! registering a [`FeedSource`] (`senken_plugin::ActivationContext::register_feed_source`),
//! and this builds a pool for each one. A source with no registered feed
//! has no pool, and `ws::subscribe`/`AlertEngine` both treat that as
//! "nothing to lease" rather than an error — an absence a client is told
//! about, not one it has to infer.
//!
//! This replaced a hardcoded `if has_okx_spot { … }`: for as long as live
//! streaming was wired here rather than declared by a plugin, exactly one
//! venue could ever have it, however many plugins the build contained.

use std::collections::HashMap;
use std::sync::Arc;

use senken_feed::WsVenueConnector;
use senken_marketdata::{InstrumentId, InstrumentQuery, MarketData};
use senken_subscription::{FeedSource, SubscriptionPool, SymbolMap};
use senken_venue::LimitGroup;

/// Resolves an instrument's venue-native symbol from a snapshot of that
/// source's catalog taken when the pool was built.
///
/// [`senken_feed::SymbolMap::source_symbol`] is synchronous (a subscribe
/// frame is built inside a `VenueConnection`'s otherwise-synchronous framing
/// step) while [`MarketData::instrument`]/`instruments` are async — that
/// crate's own docs name this exact seam ("a real deployment resolves one
/// through whatever catalog already tracks it"). A process-lifetime
/// snapshot, refreshed only when this pool is (re)built at server startup,
/// is the trade-off made here: a symbol added to the venue after startup is
/// not leaseable until the next restart, which is an acceptable gap for a
/// catalog that changes in listings, not in the symbols of instruments
/// already trading.
struct CatalogSymbolMap {
    symbols: HashMap<InstrumentId, String>,
}

impl SymbolMap for CatalogSymbolMap {
    fn source_symbol(&self, instrument: &InstrumentId) -> Option<String> {
        self.symbols.get(instrument).cloned()
    }
}

async fn warm_symbol_map(marketdata: &MarketData, source_ids: &[String]) -> CatalogSymbolMap {
    let mut symbols = HashMap::new();
    for source_id in source_ids {
        let page = marketdata
            .instruments(InstrumentQuery::all().with_source(source_id.as_str()))
            .await;
        if !page.is_complete() {
            tracing::warn!(
                source = source_id.as_str(),
                "some sources failed while warming the live-feed symbol catalog; \
                 affected instruments will not be leaseable this run"
            );
        }
        symbols.extend(
            page.matches
                .into_iter()
                .map(|hit| (hit.id, hit.instrument.source_symbol)),
        );
    }
    CatalogSymbolMap { symbols }
}

/// Builds one [`SubscriptionPool`] per registered live feed, keyed by every
/// source id that feed serves.
///
/// Called once, at server startup — [`SubscriptionPool::new`] spawns a
/// background actor task per pool, not something to call per request.
///
/// A feed serving several source ids gets one pool shared by all of them:
/// a venue's physical socket is rarely split the way its markets are, and
/// opening one connection per market for the same wire would multiply the
/// venue's connection count for nothing.
pub(crate) async fn build_feed_pools(
    marketdata: &MarketData,
    feeds: &[Arc<dyn FeedSource>],
) -> HashMap<String, SubscriptionPool> {
    let mut pools = HashMap::new();

    // Every feed's catalog is warmed concurrently, and that matters more
    // the more plugins a build carries: warming them one after another
    // makes startup the *sum* of every venue's catalog fetch, and a server
    // does not accept connections until this returns. One venue hides
    // that; twenty would not.
    let warmed = futures::future::join_all(feeds.iter().map(|feed| async move {
        let known: Vec<String> = feed
            .source_ids()
            .iter()
            .filter(|id| marketdata.sources().iter().any(|source| &source.id == *id))
            .cloned()
            .collect();
        let symbols = if known.is_empty() {
            None
        } else {
            Some(warm_symbol_map(marketdata, &known).await)
        };
        (feed, known, symbols)
    }))
    .await;

    for (feed, known, symbols) in warmed {
        let Some(symbols) = symbols else {
            // A feed for sources this build's catalog does not carry is not
            // an error — a plugin may register markets whose catalog fetch
            // failed — but it has nothing to lease, so it gets no pool.
            continue;
        };
        let protocol = feed.protocol(Arc::new(symbols));
        // The pool must be built with the *protocol's own* venue name, not
        // a source id: `WsVenueConnector::connect` refuses to dial when the
        // pool's venue and `VenueProtocol::venue()` disagree (a caller-bug
        // guard against exactly this kind of drift), and one physical venue
        // commonly serves several marketdata sources under distinct ids.
        let venue_name = protocol.venue().to_owned();
        // A dedicated dial/reconnect budget for this venue's WS
        // connections. `senken_runtime::Runtime` does not expose the
        // `LimitGroup` a plugin's own REST client dials through — it lives
        // inside `senken_plugin::ActivationContext`, local to activation and
        // dropped once `RuntimeBuilder::build` returns — so this is a
        // second, independent budget for the same venue rather than a
        // literally shared one. Flagged rather than silently claimed.
        let group = LimitGroup::new(&venue_name);
        let connector = WsVenueConnector::from_arc(protocol, group);
        let pool = SubscriptionPool::new(venue_name, connector.clone());
        connector.bind_pool(pool.clone());
        for id in known {
            pools.insert(id, pool.clone());
        }
    }

    pools
}
