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
}

/// `GET /api/sources` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourcesResponse {
    /// Every registered source, ordered by id.
    pub sources: Vec<SourceCapabilityDto>,
}
