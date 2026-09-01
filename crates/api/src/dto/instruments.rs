use serde::Serialize;
use utoipa::ToSchema;

use senken_marketdata::{InstrumentKind, InstrumentMatch, InstrumentStatus};

/// One instrument-search hit (`GET /api/instruments`). Market data is global
/// and never tenanted, so this carries no owner — every field is drawn from
/// the same cached, ranked catalog `senken-cli`'s own `search`/`instrument`
/// subcommands already read.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstrumentSummaryDto {
    /// Fully-qualified id, `source:symbol` — what a caller passes back to
    /// the bars/indicators/alerts endpoints.
    pub id: String,
    /// The source's own id (`id`'s prefix, named separately since a client
    /// group-by-venue without re-parsing `id`).
    pub source_id: String,
    /// The source's display name.
    pub source_name: String,
    /// Normalised symbol (`id`'s suffix).
    pub symbol: String,
    /// Human-readable name (`BTC / USDT`).
    pub name: String,
    /// Base asset code.
    pub base: String,
    /// Quote asset code.
    pub quote: String,
    /// Contract type: `"spot"`, `"perpetual"`, `"future"` or `"option"`.
    pub kind: String,
    /// The venue's most recently catalogued state for this instrument.
    ///
    /// This is deliberately distinct from a live-feed capability. A venue
    /// can have no websocket in this build and still report that an
    /// instrument is closed; clients use that fact to avoid describing a
    /// closed market as a broken feed.
    pub status: String,
}

impl From<InstrumentMatch> for InstrumentSummaryDto {
    fn from(hit: InstrumentMatch) -> Self {
        Self {
            id: hit.id.as_str().to_owned(),
            source_id: hit.source_id().to_owned(),
            source_name: hit.source_name.to_string(),
            symbol: hit.instrument.symbol.clone(),
            name: hit.instrument.name.clone(),
            base: hit.instrument.base.clone(),
            quote: hit.instrument.quote.clone(),
            kind: kind_str(hit.instrument.kind).to_owned(),
            status: status_str(hit.instrument.status).to_owned(),
        }
    }
}

fn kind_str(kind: InstrumentKind) -> &'static str {
    match kind {
        InstrumentKind::Spot => "spot",
        InstrumentKind::Perpetual => "perpetual",
        InstrumentKind::Future => "future",
        InstrumentKind::Option => "option",
        // `InstrumentKind` is `#[non_exhaustive]` — fail closed for a future
        // variant this build does not recognise rather than refuse to
        // compile until every caller everywhere is updated in lockstep.
        _ => "unknown",
    }
}

fn status_str(status: InstrumentStatus) -> &'static str {
    match status {
        InstrumentStatus::Trading => "trading",
        InstrumentStatus::Halted => "halted",
        InstrumentStatus::PreOpen => "pre_open",
        InstrumentStatus::Closed => "closed",
        InstrumentStatus::Test => "test",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::status_str;
    use senken_marketdata::InstrumentStatus;

    #[test]
    fn a_closed_catalogue_status_keeps_its_meaning_at_the_http_boundary() {
        assert_eq!(status_str(InstrumentStatus::Closed), "closed");
    }
}

/// `GET /api/instruments` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstrumentsPage {
    /// The hits on this page, best match first.
    pub rows: Vec<InstrumentSummaryDto>,
    /// Hits across all pages, under the same query.
    pub total: u64,
    /// `false` if a source failed to load and is therefore absent from
    /// `rows`/`total` (`senken_marketdata::InstrumentPage::is_complete`) —
    /// surfaced rather than silently dropped, since it changes what
    /// "no match" means.
    pub complete: bool,
}
