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
export type UserZoneResponse = Schemas['UserZoneResponse'];
export type SetZoneRequest = Schemas['SetZoneRequest'];
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
export type UpdateWorkspaceSettingsRequest = Schemas['UpdateWorkspaceSettingsRequest'];

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
export type IndicatorPointDto = Schemas['IndicatorPointDto'];
export type IndicatorPriceCoordDto = Schemas['IndicatorPriceCoordDto'];
export type IndicatorScaledPriceDto = Schemas['IndicatorScaledPriceDto'];
export type IndicatorExtendDto = Schemas['IndicatorExtendDto'];
export type IndicatorLabelAnchorDto = Schemas['IndicatorLabelAnchorDto'];
export type ComputeIndicatorResponse = Schemas['ComputeIndicatorResponse'];
export type IndicatorPluginDto = Schemas['IndicatorPluginDto'];
export type IndicatorPluginOriginDto = Schemas['IndicatorPluginOriginDto'];
export type IndicatorPluginStateDto = Schemas['IndicatorPluginStateDto'];
export type PluginHealthDto = Schemas['PluginHealthDto'];
export type PluginCircuitStateDto = Schemas['PluginCircuitStateDto'];
export type PluginLogLineDto = Schemas['PluginLogLineDto'];
export type PluginLogSeverityDto = Schemas['PluginLogSeverityDto'];
export type SetIndicatorPluginEnabledRequest = Schemas['SetIndicatorPluginEnabledRequest'];
export type CompileIndicatorRequest = Schemas['CompileIndicatorRequest'];
export type CompileIndicatorErrorDto = Schemas['CompileIndicatorErrorDto'];

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

// Instrument search (`GET /api/instruments`): `routes/charts/+page.svelte`'s
// symbol/overlay pickers search through `apiClient.searchInstruments` — the
// former fixed `INSTRUMENT_CATALOG` in `terminal/chart-config.ts` is gone.
export type InstrumentSummaryDto = Schemas['InstrumentSummaryDto'];
export type InstrumentsPage = Schemas['InstrumentsPage'];

// Watchlists (`/api/watchlists`, `/api/watchlist-members`) — a user-owned
// group of watched instruments and its membership.
export type WatchlistGroupDto = Schemas['WatchlistGroupDto'];
export type WatchlistGroupsPage = Schemas['WatchlistGroupsPage'];
export type CreateWatchlistGroupRequest = Schemas['CreateWatchlistGroupRequest'];
export type RenameWatchlistGroupRequest = Schemas['RenameWatchlistGroupRequest'];
export type ReorderWatchlistGroupsRequest = Schemas['ReorderWatchlistGroupsRequest'];
export type WatchlistMemberDto = Schemas['WatchlistMemberDto'];
export type AddWatchlistMemberRequest = Schemas['AddWatchlistMemberRequest'];
export type ReorderWatchlistMembersRequest = Schemas['ReorderWatchlistMembersRequest'];

// Notes (`/api/notes`) — a user-owned freeform note. `NoteSummaryDto` (the
// listing row) never carries a note's body; only `NoteDto` (`GET
// /api/notes/{id}`) does.
export type NoteSummaryDto = Schemas['NoteSummaryDto'];
export type NotesPage = Schemas['NotesPage'];
export type NoteDto = Schemas['NoteDto'];
export type CreateNoteRequest = Schemas['CreateNoteRequest'];
export type UpdateNoteRequest = Schemas['UpdateNoteRequest'];

// The indicator registry (`/api/registry/indicators`) — publish, search,
// install indicator-lang source. `IndicatorSummaryDto` (a listing row)
// never carries an entry's source; only `IndicatorEntryDto` (`GET
// /api/registry/indicators/{namespace}/{name}`) does.
export type IndicatorSummaryDto = Schemas['IndicatorSummaryDto'];
export type RegistryPage = Schemas['RegistryPage'];
export type IndicatorEntryDto = Schemas['IndicatorEntryDto'];
export type PublishIndicatorRequest = Schemas['PublishIndicatorRequest'];
export type SetHandleRequest = Schemas['SetHandleRequest'];
export type HandleResponse = Schemas['HandleResponse'];

// Storage (`GET /api/storage`, `POST /api/storage/delete`) — what this
// server is holding on disk, and reclaiming it. Only market data gets a
// tree (`senken-store`'s Parquet layout); everything else lives in the
// accounts database and is reported as a single figure per `databases` row.
export type StorageReportDto = Schemas['StorageReportDto'];
export type MarketDataUsageDto = Schemas['MarketDataUsageDto'];
export type StorageSourceDto = Schemas['StorageSourceDto'];
export type StorageInstrumentDto = Schemas['StorageInstrumentDto'];
export type StorageSeriesDto = Schemas['StorageSeriesDto'];
export type StorageSeriesKindDto = Schemas['StorageSeriesKindDto'];
export type StorageDatabaseDto = Schemas['StorageDatabaseDto'];
export type DeleteStorageRequest = Schemas['DeleteStorageRequest'];
export type DeleteStorageResponse = Schemas['DeleteStorageResponse'];

// Trade engine (`/api/trade/*`) — the adapters a plugin registered, the
// accounts a user attached to them, and the orders, positions and balances
// read back through those adapters.
//
// `ScaledDto` is the one shape worth knowing: a price, a quantity, a
// balance and a fee all arrive as `{ scale, value }` where `value` is a
// **decimal string**. A JSON number would put every one of them through a
// double, and a size that changes in its last digit on the way to a venue
// is exactly what the scaled-integer contract exists to prevent. Format
// them with `formatScaled` (`$lib/trade/scaled.ts`); never add two of them
// as numbers.
export type ScaledDto = Schemas['ScaledDto'];

// Four documents an adapter declares as data — its settings form, its
// custom actions, what it can do, and which instruments it trades.
//
// Hand-declared for the same reason `IndicatorFieldDto`/`ComparatorDto`
// above are: they are `senken-trade`'s own foreign types, so the orphan
// rule blocks a real `utoipa::ToSchema` impl for them and
// `#[schema(value_type = Object)]` is what the server can honestly
// promise. The shapes below mirror `crates/trade/src/settings.rs`,
// `capability.rs` and `adapter.rs` exactly; a value typed against either
// is still assignable wherever the generated `Record<string, never>` is
// expected.
export type FieldKindDto =
	| { type: 'text'; default?: string; max_len: number; placeholder?: string }
	| { type: 'secret'; placeholder?: string }
	| { type: 'number'; default?: number; min: number; max: number; unit?: string }
	| { type: 'decimal'; scale: number; default?: number; min: number; max: number; unit?: string }
	| { type: 'toggle'; default: boolean }
	| { type: 'choice'; default?: string; options: { value: string; label: string }[] };

export type SettingFieldDto = {
	key: string;
	label: string;
	help?: string;
	required: boolean;
} & FieldKindDto;

export interface SettingsSchemaDto {
	fields: SettingFieldDto[];
}

export interface AdapterActionDto {
	id: string;
	label: string;
	description?: string;
	confirm: boolean;
	destructive: boolean;
	form: SettingsSchemaDto;
}

export type OrderKindTagDto = 'market' | 'limit' | 'stop' | 'stop_limit';
export type TimeInForceDto = 'gtc' | 'ioc' | 'fok' | 'day';
export type OrderSideDto = 'buy' | 'sell';
export type PositionSideDto = 'long' | 'short';
export type OrderStatusDto =
	| 'pending'
	| 'open'
	| 'partially_filled'
	| 'filled'
	| 'cancelled'
	| 'rejected'
	| 'expired';

export type AdapterFeatureDto =
	| 'reduce_only'
	| 'post_only'
	| 'cancel_orders'
	| 'modify_orders'
	| 'leverage'
	| 'fills';

export interface AdapterCapabilitiesDto {
	order_kinds: OrderKindTagDto[];
	time_in_force: TimeInForceDto[];
	quantity_unit: 'base' | 'contracts' | 'lots' | 'quote_notional';
	position_mode: 'netting' | 'hedging' | 'spot_holdings';
	features: AdapterFeatureDto[];
}

export type InstrumentCoverageDto =
	| { coverage: 'universal' }
	| { coverage: 'sources'; source_ids: string[] }
	| { coverage: 'instruments'; instruments: string[] };

/** A settings value, as stored. A secret always arrives as `null` — that is
 * `SecretString`'s own serialisation on the server, not something the API
 * layer strips, so there is no shape in which one could arrive populated. */
export type SettingValueDto = string | number | boolean | null;

export type SettingsValuesDto = Record<string, SettingValueDto>;

/** What a settings or action form submits: raw values per key, typed and
 * bounds-checked server-side against the schema whatever the client did. */
export type SettingsInputDto = Record<string, string | number | boolean | null>;

/** `AdapterDto` with its four `Object`-typed documents given their real
 * shapes. */
export type AdapterDto = Omit<
	Schemas['AdapterDto'],
	'capabilities' | 'coverage' | 'settings_schema' | 'actions'
> & {
	capabilities: AdapterCapabilitiesDto;
	coverage: InstrumentCoverageDto;
	settings_schema: SettingsSchemaDto;
	actions: AdapterActionDto[];
};
export interface AdaptersResponse {
	adapters: AdapterDto[];
}
export type TradeAccountDto = Schemas['TradeAccountDto'];
export type TradeAccountsPage = Schemas['TradeAccountsPage'];

/** `trade` (orders may be placed, amended and cancelled) or `read_only`
 * (balances, positions and orders may be read; nothing may be sent) — a
 * MetaTrader 5 investor login and an exchange key minted without trade
 * scope both resolve to the latter. */
export type AccessLevelDto = 'trade' | 'read_only';

/** `AccountAccessDto` with its `Object`-typed `capabilities` given its real
 * shape, for the same orphan-rule reason `AdapterDto` above does. Narrower
 * than the adapter's own `AdapterCapabilitiesDto` when this particular
 * account is restricted. */
export type AccountAccessDto = Omit<Schemas['AccountAccessDto'], 'capabilities' | 'level'> & {
	level: AccessLevelDto;
	capabilities: AdapterCapabilitiesDto;
};
export type CreateTradeAccountRequest = Omit<Schemas['CreateTradeAccountRequest'], 'settings'> & {
	settings: SettingsInputDto;
};
export type UpdateTradeAccountRequest = Schemas['UpdateTradeAccountRequest'];
export type TradeAccountSettingsDto = Omit<Schemas['TradeAccountSettingsDto'], 'settings' | 'secrets_set'> & {
	settings: SettingsValuesDto;
	/** Which secret fields hold a credential — the only thing ever reported
	 * about one. */
	secrets_set: Record<string, boolean>;
};
export interface ReplaceSettingsRequest {
	settings: SettingsInputDto;
}
export type BalancesDto = Schemas['BalancesDto'];
export type AssetBalanceDto = Schemas['AssetBalanceDto'];
export type PositionDto = Schemas['PositionDto'];
export type OrderDto = Schemas['OrderDto'];
export type FillDto = Schemas['FillDto'];
export type PlaceOrderRequest = Schemas['PlaceOrderRequest'];
/** `POST /api/trade/accounts/{id}/close` request body — the instrument to
 * close, as `source:symbol`. The size sent is never the caller's to choose:
 * the server closes exactly what the adapter currently reports held. */
export type CloseRequest = Schemas['CloseRequest'];
/** `PATCH /api/trade/accounts/{id}/orders/{orderId}` request body. Every
 * field is optional; a field left out leaves that part of the order alone. */
export type AmendOrderRequest = Schemas['AmendOrderRequest'];
export interface AdapterHealthDto {
	state: 'connected' | 'degraded' | 'disconnected';
	reason?: string;
}

export interface HealthDto {
	health: AdapterHealthDto;
}

/** `GET /api/trade/accounts/{id}` response body: the account, its resolved
 * access and its health in one round trip, replacing three requests a
 * screen previously needed. `Object`-typed `health` given its real shape
 * for the same orphan-rule reason as `AccountAccessDto`'s `capabilities`. */
export type TradeAccountStateDto = Omit<Schemas['TradeAccountStateDto'], 'access' | 'health'> & {
	access: AccountAccessDto;
	health: AdapterHealthDto;
};
export interface RunActionRequest {
	params: SettingsInputDto;
}
export type ActionOutcomeDto = Schemas['ActionOutcomeDto'];
