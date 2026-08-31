//! Live price feed wiring: one [`SubscriptionPool`] per source
//! this build actually has a live [`senken_feed::VenueProtocol`] for.
//!
//! Only OKX's public trades channel is implemented and verified live
//! (`senken_feed::okx`'s own module docs) — every other registered
//! marketdata source has no live-price protocol in this build at all, so an
//! open pane or an alert on one of them simply has no pool to lease from
//! (see `ws::subscribe`/`senken_alerts::AlertEngine`, which both skip a
//! topic/alert with no matching pool rather than failing loudly for a gap
//! this crate cannot close). Extending live coverage to another venue is a
//! matter of implementing its own `VenueProtocol` in `senken-feed` and
//! adding one more branch below — the dial/reconnect engine underneath is
//! already generic across venues.

use std::collections::HashMap;
use std::sync::Arc;

use senken_feed::okx::OkxTradesProtocol;
use senken_feed::{SymbolMap, VenueProtocol, WsVenueConnector};
use senken_marketdata::{InstrumentId, InstrumentQuery, MarketData};
use senken_subscription::SubscriptionPool;
use senken_venue::LimitGroup;

/// The one source id this build streams live prices for.
const OKX_SPOT_SOURCE: &str = "okx-spot";

/// Resolves an instrument's OKX-native symbol from a snapshot of that
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

async fn warm_symbol_map(marketdata: &MarketData, source_id: &str) -> CatalogSymbolMap {
    let page = marketdata
        .instruments(InstrumentQuery::all().with_source(source_id))
        .await;
    if !page.is_complete() {
        tracing::warn!(
            source = source_id,
            "some sources failed while warming the live-feed symbol catalog; \
             affected instruments will not be leaseable this run"
        );
    }
    let symbols = page
        .matches
        .into_iter()
        .map(|hit| (hit.id, hit.instrument.source_symbol))
        .collect();
    CatalogSymbolMap { symbols }
}

/// Builds one [`SubscriptionPool`] per source this build can stream live
/// prices for, keyed by source id. Called once, at server startup —
/// [`SubscriptionPool::new`] spawns a background actor task per pool
/// , not something to call per request.
pub(crate) async fn build_feed_pools(marketdata: &MarketData) -> HashMap<String, SubscriptionPool> {
    let mut pools = HashMap::new();

    let has_okx_spot = marketdata
        .sources()
        .iter()
        .any(|source| source.id == OKX_SPOT_SOURCE);
    if has_okx_spot {
        let symbols = warm_symbol_map(marketdata, OKX_SPOT_SOURCE).await;
        let protocol = OkxTradesProtocol::new(OKX_SPOT_SOURCE, Arc::new(symbols));
        // `SubscriptionPool` must be built with the *protocol's own* venue
        // name ("okx"), not the marketdata source id ("okx-spot") this pool
        // is keyed by below: `WsVenueConnector::connect` refuses to dial
        // when the pool's venue and `VenueProtocol::venue()` disagree (a
        // caller-bug guard against exactly this kind of drift), and OKX's
        // one physical venue serves several marketdata sources (spot, swap,
        // futures) under distinct source ids for exactly one instrument
        // stream today.
        let venue_name = protocol.venue().to_owned();
        // A dedicated dial/reconnect budget for this one WS connection.
        // `senken_runtime::Runtime` does not expose the `LimitGroup` the OKX
        // plugin's own REST client dials through — it lives inside
        // `senken_plugin::ActivationContext`, local to plugin activation and
        // dropped once `RuntimeBuilder::build` returns — so this is a
        // second, independent budget for the same venue rather than the
        // literally shared one `senken-feed`'s own module docs describe as
        // the ideal. Flagged here rather than silently claimed as shared.
        let group = LimitGroup::new(&venue_name);
        let connector = WsVenueConnector::new(protocol, group);
        let pool = SubscriptionPool::new(venue_name, connector.clone());
        connector.bind_pool(pool.clone());
        pools.insert(OKX_SPOT_SOURCE.to_owned(), pool);
    }

    pools
}
