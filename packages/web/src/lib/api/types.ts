// Request/response types, generated rather than hand-written.
//
// `utoipa` 5.5.0 derives an OpenAPI document from the same `serde` structs
// the Rust server uses (`crates/api/src/dto.rs`, `crates/api/src/lib.rs`),
// served at `GET /api/openapi.json`; `openapi-typescript` 7.13.0 turns that
// document into `./generated.ts` (`bunx openapi-typescript <url> -o
// src/lib/api/generated.ts`, run against a live server). Regenerate that
// file whenever the server's route or schema set changes — do not hand-edit
// it, per its own "do not make direct changes" banner.
//
// Every other module in `src/lib/api/` imports its request/response shapes
// from here, not from `./generated.ts` directly, so a future re-shuffle of
// the generated file's structure (a schema renamed, a path regrouped) only
// touches this one file.
import type { components } from './generated';

type Schemas = components['schemas'];

export type HealthResponse = Schemas['Health'];
export type LoginRequest = Schemas['LoginRequest'];
export type LoginResponse = Schemas['LoginResponse'];
export type SetPasswordRequest = Schemas['SetPasswordRequest'];
export type MeResponse = Schemas['MeResponse'];
export type WsTicketResponse = Schemas['WsTicketResponse'];
export type ErrorBody = Schemas['ErrorBody'];

// User/role/grant management.
export type GrantDto = Schemas['GrantDto'];
export type IdResponse = Schemas['IdResponse'];
export type UserSummaryDto = Schemas['UserSummaryDto'];
export type UsersPage = Schemas['UsersPage'];
export type RoleSummaryDto = Schemas['RoleSummaryDto'];
export type RolesPage = Schemas['RolesPage'];
export type CreateUserRequest = Schemas['CreateUserRequest'];
export type CreateRoleRequest = Schemas['CreateRoleRequest'];
export type AssignRoleRequest = Schemas['AssignRoleRequest'];
export type PluginGrantRequest = Schemas['PluginGrantRequest'];

// Workspaces, layouts, panes and layers. `crates/api` mounts these now (its
// implementation notes at the end of),
// so — unlike the alerts block below — these were
// always generated, never hand-written.
export type WorkspaceDto = Schemas['WorkspaceDto'];
export type WorkspacesPage = Schemas['WorkspacesPage'];
export type CreateWorkspaceRequest = Schemas['CreateWorkspaceRequest'];
export type RenameWorkspaceRequest = Schemas['RenameWorkspaceRequest'];
export type DefaultWorkspaceResponse = Schemas['DefaultWorkspaceResponse'];
export type LayoutSummaryDto = Schemas['LayoutSummaryDto'];
export type LayoutDetailDto = Schemas['LayoutDetailDto'];
export type PaneDto = Schemas['PaneDto'];
export type PaneInputDto = Schemas['PaneInputDto'];
export type LayerDto = Schemas['LayerDto'];
export type LayerInputDto = Schemas['LayerInputDto'];
export type LayerKindDto = Schemas['LayerKindDto'];
export type DrawingDto = Schemas['DrawingDto'];
export type DrawingInputDto = Schemas['DrawingInputDto'];
export type DrawingKindDto = Schemas['DrawingKindDto'];
export type DrawingPointDto = Schemas['DrawingPointDto'];
export type DrawingLineStyleDto = Schemas['DrawingLineStyleDto'];
export type ReplaceLayoutRequest = Schemas['ReplaceLayoutRequest'];

// Bars.
export type TimeRangeDto = Schemas['TimeRangeDto'];
export type BarsRequirementDto = Schemas['BarsRequirementDto'];
export type BarDto = Schemas['BarDto'];
export type BarRangeResponse = Schemas['BarRangeResponse'];
export type EnsureBarsRequest = Schemas['EnsureBarsRequest'];
export type EnsureBarsResponse = Schemas['EnsureBarsResponse'];
export type BarJobDto = Schemas['BarJobDto'];

// Indicators.
export type IndicatorSpecDto = Schemas['IndicatorSpecDto'];
export type IndicatorCatalogEntry = Schemas['IndicatorCatalogEntry'];
export type ComputeIndicatorRequest = Schemas['ComputeIndicatorRequest'];
export type IndicatorDrawableDto = Schemas['IndicatorDrawableDto'];
export type IndicatorDrawablePointDto = Schemas['IndicatorDrawablePointDto'];
export type ComputeIndicatorResponse = Schemas['ComputeIndicatorResponse'];

// Alerts. `crates/api` mounts
// `senken_alerts::AlertStore` as of S1, so these are now generated from
// `GET /api/openapi.json` like everything else above, replacing the
// hand-written block this file carried before that mount landed (its own
// former comment said to do exactly this "at that point rather than
// keeping both"). `ConditionDto.field`/`.comparator` come back from
// `openapi-typescript` typed as plain `string` — `crates/api/src/dto.rs`
// documents them to `utoipa` via `#[schema(value_type = String)]` (the
// orphan rule blocks a real `ToSchema` impl for `senken_alerts`' foreign
// enums, the same reason `GrantDto`'s fields do the same) — so the two
// literal unions below stay hand-declared, mirroring
// `crates/alerts/src/condition.rs`'s `IndicatorField`/`Comparator`
// variants exactly; a value typed against either is still assignable
// wherever the generated `string` is expected.
export type IndicatorFieldDto =
	| 'Value'
	| 'MacdLine'
	| 'MacdSignal'
	| 'MacdHistogram'
	| 'StochasticK'
	| 'StochasticD'
	| 'BollingerUpper'
	| 'BollingerMiddle'
	| 'BollingerLower';

export type ComparatorDto = 'GreaterThan' | 'LessThan' | 'CrossesAbove' | 'CrossesBelow';

export type ConditionDto = Schemas['ConditionDto'];
export type AlertDto = Schemas['AlertDto'];
export type AlertsPage = Schemas['AlertsPage'];
export type CreateAlertRequest = Schemas['CreateAlertRequest'];

// Sources (`GET /api/sources`) — which registered sources can chart at all
// (`bars`) and which can also stream a live price (`live`, never true
// without `bars`). Drives the live-price indicator's `liveState`
// (`$lib/charts/live-state.ts`) and the alerts panel's "this venue cannot
// run" state.
export type SourceCapabilityDto = Schemas['SourceCapabilityDto'];
export type SourcesResponse = Schemas['SourcesResponse'];

// Instrument search (`GET /api/instruments`) — the catalog-search gap plan
// 006 B1 left open, closed in S5: `routes/charts/+page.svelte`'s symbol/
// overlay pickers and `workspace-store.svelte.ts`'s `nextInstrument` all
// search through `apiClient.searchInstruments` now — the former fixed
// `INSTRUMENT_CATALOG` in `terminal/chart-config.ts` is gone.
export type InstrumentSummaryDto = Schemas['InstrumentSummaryDto'];
export type InstrumentsPage = Schemas['InstrumentsPage'];
