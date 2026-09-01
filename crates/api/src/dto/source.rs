use serde::Serialize;
use utoipa::ToSchema;

/// What one registered market-data source can do.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourceCapabilityDto {
    /// The source id, e.g. `okx-spot` — the left half of an `InstrumentId`.
    pub id: String,
    /// The venue's own display name.
    pub name: String,
    /// A `BarSource` is registered for it, so its instruments can be charted.
    pub bars: bool,
    /// It has a live subscription pool, so its instruments can stream a
    /// price. Never true without `bars`.
    pub live: bool,
    /// It reports best bid and offer updates.
    pub quotes: bool,
    /// The order-book panel's own capability, nested rather than a fourth
    /// top-level flag: `bars`/`live`/`quotes` are already an established
    /// part of this response's wire shape (`source-capability.ts` reads
    /// each directly), so a newly added capability is grouped here instead
    /// of widening that flat set indefinitely.
    pub book: BookCapabilityDto,
}

/// Whether a source can serve the book panel a fixed-depth snapshot.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BookCapabilityDto {
    /// A `senken_subscription::BookSource` is registered for it. Never a
    /// locally-maintained book updated from venue deltas — see that
    /// trait's own docs.
    pub supported: bool,
}

/// `GET /api/sources` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourcesResponse {
    /// Every registered source, ordered by id.
    pub sources: Vec<SourceCapabilityDto>,
}
