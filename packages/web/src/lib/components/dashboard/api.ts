// A typed client for the dashboard endpoints, calling through the shared
// `apiClient.request` — the same credential/base-URL/401/403 handling
// every other feature's calls already go through, without adding a
// second `fetch` call site.
//
// These routes are not registered on the server yet and are not part of
// `$lib/api/generated.ts` (hand-editing that file is never allowed — it is
// produced from the server's own `serde` structs). The types below are
// written to match `crates/api/src/dto/dashboard.rs` field-for-field, so
// swapping to the generated client once the route is registered and
// regenerated is a type-import change, not a logic change.

import { apiClient } from '$lib/api/client';

/** Mirrors `senken_dashboard::DataSource` on the wire
 * (`crate::dto::dashboard::DashboardDataSourceDto`). */
export type DashboardDataSource = 'live' | 'mock';

export interface DashboardGridSize {
	width: number;
	height: number;
}

export interface DashboardWidgetDefinition {
	widget_type_id: string;
	provider_id: string;
	title: string;
	description: string;
	default_size: DashboardGridSize;
	min_size: DashboardGridSize;
	data_source: DashboardDataSource;
}

export interface DashboardWidgetCatalogResponse {
	widgets: DashboardWidgetDefinition[];
}

export interface DashboardWorkspaceDto {
	id: string;
	owner_id: string;
	name: string;
	columns: number;
	revision: number;
	created_at: number;
	updated_at: number;
}

export interface DashboardWorkspacesPage {
	rows: DashboardWorkspaceDto[];
	total: number;
}

export interface DefaultDashboardWorkspaceResponse {
	workspace_id: string;
}

export interface DashboardWidgetDto {
	id: string;
	provider_id: string;
	widget_type_id: string;
	position_x: number;
	position_y: number;
	width: number;
	height: number;
	visible: boolean;
	config: string;
	config_schema_version: number;
	created_at: number;
	updated_at: number;
}

export interface DashboardLayoutDto {
	workspace: DashboardWorkspaceDto;
	widgets: DashboardWidgetDto[];
}

export interface DashboardWidgetPlacementRequest {
	id?: string;
	provider_id: string;
	widget_type_id: string;
	position_x: number;
	position_y: number;
	width: number;
	height: number;
	visible: boolean;
	config: string;
	config_schema_version: number;
}

export interface ReplaceDashboardLayoutRequest {
	expected_revision: number;
	columns: number;
	widgets: DashboardWidgetPlacementRequest[];
}

/** `GET /api/dashboard/workspaces?limit=&offset=`. */
export async function listDashboardWorkspaces(
	limit = 50,
	offset = 0
): Promise<DashboardWorkspacesPage> {
	return apiClient.request<DashboardWorkspacesPage>(
		`/api/dashboard/workspaces?limit=${limit}&offset=${offset}`
	);
}

/** `POST /api/dashboard/workspaces`. */
export async function createDashboardWorkspace(name: string): Promise<{ id: string }> {
	return apiClient.request<{ id: string }>('/api/dashboard/workspaces', {
		method: 'POST',
		body: JSON.stringify({ name })
	});
}

/** `GET /api/dashboard/workspaces/default` — creates one on first call,
 * and a second call from anywhere never creates a second one. */
export async function defaultDashboardWorkspace(): Promise<DefaultDashboardWorkspaceResponse> {
	return apiClient.request<DefaultDashboardWorkspaceResponse>('/api/dashboard/workspaces/default');
}

/** `PATCH /api/dashboard/workspaces/{id}`. */
export async function renameDashboardWorkspace(workspaceId: string, name: string): Promise<void> {
	await apiClient.request<void>(`/api/dashboard/workspaces/${workspaceId}`, {
		method: 'PATCH',
		body: JSON.stringify({ name })
	});
}

/** `DELETE /api/dashboard/workspaces/{id}`. Deleting a user's last
 * workspace is not specially refused — the next call to
 * `defaultDashboardWorkspace` heals it. */
export async function deleteDashboardWorkspace(workspaceId: string): Promise<void> {
	await apiClient.request<void>(`/api/dashboard/workspaces/${workspaceId}`, {
		method: 'DELETE'
	});
}

/** `GET /api/dashboard/workspaces/{id}/layout`. */
export async function getDashboardLayout(workspaceId: string): Promise<DashboardLayoutDto> {
	return apiClient.request<DashboardLayoutDto>(`/api/dashboard/workspaces/${workspaceId}/layout`);
}

/** `PUT /api/dashboard/workspaces/{id}/layout`. Add, move, resize and
 * delete are all just different snapshots through this one call — see
 * `senken_dashboard::DashboardWorkspaceStore::replace_layout`'s own docs.
 * Throws `HttpError` with `status === 409` on a stale `expected_revision`:
 * another write (most likely a second open tab) landed first, and the
 * caller's own snapshot was discarded rather than silently overwriting it.
 *
 * Resolves with the full layout, not just the new revision — a newly
 * added widget's id is assigned by the server and never appears in
 * `body`, so this response is the only place a caller learns it in time
 * to name that widget by id on its very next move or resize. */
export async function replaceDashboardLayout(
	workspaceId: string,
	body: ReplaceDashboardLayoutRequest
): Promise<DashboardLayoutDto> {
	return apiClient.request<DashboardLayoutDto>(`/api/dashboard/workspaces/${workspaceId}/layout`, {
		method: 'PUT',
		body: JSON.stringify(body)
	});
}

/** `GET /api/dashboard/widgets/catalog` — every widget type this build's
 * server currently knows how to serve. */
export async function dashboardWidgetCatalog(): Promise<DashboardWidgetCatalogResponse> {
	return apiClient.request<DashboardWidgetCatalogResponse>('/api/dashboard/widgets/catalog');
}

// ---------------------------------------------------------------------
// Widget UI plugin packages — a manifest plus a static bundle (`index.html`
// + assets), built entirely outside this repository and served into a
// sandboxed iframe. Mirrors `crates/api/src/widget_plugin_handlers.rs`
// field-for-field; that module is not registered on the server's router
// yet either (see its own doc comment on the wiring it still needs), so
// these calls will 404 until an integrator mounts it — same situation the
// comment at the top of this file already notes for the dashboard routes
// themselves.
// ---------------------------------------------------------------------

/** One widget a plugin package contributes — a structural superset of
 * `DashboardWidgetDefinition` (same `widget_type_id`/`title`/`description`/
 * `default_size`/`min_size`/`data_source` fields, plus what only a plugin
 * widget needs), so a plugin definition can sit in the exact same
 * `DashboardWidgetDefinition[]` array `add-widget-picker.svelte` and
 * `dashboard-grid.svelte` already read from — no separate "plugin catalog"
 * type threaded through either of them. */
export interface WidgetPluginDefinition {
	widget_type_id: string;
	provider_id: string;
	title: string;
	description: string;
	category: string;
	default_size: DashboardGridSize;
	min_size: DashboardGridSize;
	max_size?: DashboardGridSize;
	config_schema_version: number;
	config_schema: unknown;
	required_permissions: string[];
	required_capabilities: string[];
	data_source: DashboardDataSource;
	/** The URL, on this server's own origin, serving this widget's entry
	 * document — point a sandboxed `<iframe src>` at exactly this. */
	entry_url: string;
}

export interface WidgetPluginCatalogResponse {
	widgets: WidgetPluginDefinition[];
}

export type WidgetPluginStatus =
	| { state: 'active' }
	| { state: 'disabled' }
	| { state: 'failed'; reason: string };

export interface WidgetPluginPackage {
	id: string;
	name: string;
	version: string;
	description: string;
	enabled: boolean;
	status: WidgetPluginStatus;
	digest: string;
	widget_count: number;
	/** `true` for the package this server installs on every fresh start.
	 * It can be disabled like any other package; `uninstallWidgetPlugin`
	 * refuses it (the server returns 400) rather than removing its files. */
	is_builtin: boolean;
}

export interface WidgetPluginListResponse {
	packages: WidgetPluginPackage[];
}

/** `GET /api/widget-plugins/catalog` — every widget every currently active
 * widget plugin package contributes. Every signed-in caller sees the same
 * catalog, the same choice `dashboardWidgetCatalog` already makes. */
export async function widgetPluginCatalog(): Promise<WidgetPluginCatalogResponse> {
	return apiClient.request<WidgetPluginCatalogResponse>('/api/widget-plugins/catalog');
}

/** `GET /api/widget-plugins` — every installed package, enabled or not.
 * Requires the caller to hold `Action::View` on `Resource::Storage` at
 * `Scope::All` (an ordinary account gets `ForbiddenError`). */
export async function listWidgetPlugins(): Promise<WidgetPluginListResponse> {
	return apiClient.request<WidgetPluginListResponse>('/api/widget-plugins');
}

/** `POST /api/widget-plugins`: installs a package from the raw bytes of a
 * zip archive (`manifest.json` at its root, assets under `web/`). Requires
 * `Action::Create` on `Resource::Storage` at `Scope::All`. */
export async function installWidgetPlugin(zipBytes: Uint8Array | ArrayBuffer): Promise<{ id: string }> {
	return apiClient.request<{ id: string }>('/api/widget-plugins', {
		method: 'POST',
		headers: { 'Content-Type': 'application/zip' },
		body: zipBytes instanceof ArrayBuffer ? zipBytes : new Uint8Array(zipBytes)
	});
}

/** `POST /api/widget-plugins/{id}/enabled`: flips whether this package's
 * widgets are in the effective catalog, without touching its files or any
 * placed instance's stored config — enabling restores it exactly. Requires
 * `Action::Edit` on `Resource::Storage` at `Scope::All`. */
export async function setWidgetPluginEnabled(id: string, enabled: boolean): Promise<void> {
	await apiClient.request<void>(`/api/widget-plugins/${encodeURIComponent(id)}/enabled`, {
		method: 'POST',
		body: JSON.stringify({ enabled })
	});
}

/** `DELETE /api/widget-plugins/{id}`: removes a package's files entirely.
 * Requires `Action::Delete` on `Resource::Storage` at `Scope::All`. */
export async function uninstallWidgetPlugin(id: string): Promise<void> {
	await apiClient.request<void>(`/api/widget-plugins/${encodeURIComponent(id)}`, {
		method: 'DELETE'
	});
}

/** `POST /api/widget-plugins/refresh`: an explicit rescan of the data
 * directory, for a package dropped directly on disk rather than uploaded —
 * never a filesystem watcher (a watcher can fire mid-copy and read a
 * half-written file). Requires `Action::View` on `Resource::Storage` at
 * `Scope::All`. */
export async function refreshWidgetPlugins(): Promise<WidgetPluginListResponse> {
	return apiClient.request<WidgetPluginListResponse>('/api/widget-plugins/refresh', {
		method: 'POST'
	});
}
