// The one connection layer. No component may call
// `fetch` directly — everything that talks to a Senken server, REST or the
// WebSocket ticket exchange, goes through `apiClient` below. Same
// principle as `ActivationContext` for plugins: one funnel,
// so auth, error translation and retry policy have exactly one home.
import { activeServer, resolveBaseUrl, selectServer } from './servers.svelte';
import { connectionStore, setConnectionState } from './connection.svelte';
import { credentialReader, establishSession, endSession, refreshSessionPresence } from './session.svelte';
import { NetworkError, UnauthorizedError, ForbiddenError, HttpError, describeError, classifyResponse, errorBodyMessage } from './errors';
import { backoffDelay } from './backoff';
import { OnceGuard } from './once-guard';
import type {
	HealthResponse,
	LoginResponse,
	MeResponse,
	UserZoneResponse,
	SetZoneRequest,
	UsersPage,
	RolesPage,
	CreateUserRequest,
	CreateRoleRequest,
	GrantDto,
	IdResponse,
	AlertDto,
	AlertsPage,
	CreateAlertRequest,
	WorkspacesPage,
	CreateWorkspaceRequest,
	DefaultWorkspaceResponse,
	LayoutSummaryDto,
	LayoutDetailDto,
	LayerInputDto,
	DrawingInputDto,
	ReplaceLayoutRequest,
	BarsRequirementDto,
	BarRangeResponse,
	EnsureBarsRequest,
	EnsureBarsResponse,
	BarJobDto,
	IndicatorCatalogEntry,
	ComputeIndicatorRequest,
	ComputeIndicatorResponse,
	IndicatorPluginDto,
	PluginDto,
	PluginListResponse,
	SetPluginEnabledRequest,
	InstallPluginResponse,
	UserIndicatorSummaryDto,
	UserIndicatorDto,
	CreateUserIndicatorRequest,
	UpdateUserIndicatorRequest,
	SaveUserIndicatorResponse,
	IndicatorToolchainStatusResponse,
	InstrumentsPage,
	SourcesResponse,
	WatchlistGroupsPage,
	CreateWatchlistGroupRequest,
	WatchlistMemberDto,
	AddWatchlistMemberRequest,
	NotesPage,
	NoteDto,
	CreateNoteRequest,
	UpdateNoteRequest,
	StorageReportDto,
	DeleteStorageRequest,
	DeleteStorageResponse,
	AdaptersResponse,
	TradeAccountsPage,
	TradeAccountStateDto,
	CreateTradeAccountRequest,
	UpdateTradeAccountRequest,
	TradeAccountSettingsDto,
	ReplaceSettingsRequest,
	BalancesDto,
	PositionDto,
	OrderDto,
	FillDto,
	PlaceOrderRequest,
	CloseRequest,
	AmendOrderRequest,
	HealthDto,
	RunActionRequest,
	ActionOutcomeDto
} from './types';

export type SessionExpiredHandler = () => void;

// Routing to login must happen exactly once. `app-shell.svelte`'s auth gate
// plugs in the real navigation with one call to `setSessionExpiredHandler`;
// until that call happens the default no-op is fine because
// `connectionStore.state` already goes to `'disconnected'` (see
// `handleSessionExpired` below), which is the observable signal any UI
// needs to react to a dead session.
let sessionExpiredHandler: SessionExpiredHandler = () => {};

export function setSessionExpiredHandler(handler: SessionExpiredHandler): void {
	sessionExpiredHandler = handler;
}

/** How often `startHeartbeat` polls (`/api/health` or `/api/me`, see its
 * doc) while the connection looks healthy. Could become "poll whatever the
 * open panels are already requesting" instead of a dedicated timer once
 * more real endpoints exist, but there is nothing else to piggyback on
 * today. */
const HEARTBEAT_INTERVAL_MS = 5_000;

class ApiClient {
	// Guards the exactly-once session-expiry handling — see `once-guard.ts`
	// for why this is a separate, unit-tested primitive rather than an
	// inline flag.
	private readonly sessionExpiryGuard = new OnceGuard();
	private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
	private heartbeatAttempt = 0;
	private heartbeatGeneration = 0;

	/**
	 * Attaches the credential and base URL, and translates transport
	 * failures into typed errors. This is the only function in the app that
	 * calls `fetch`.
	 *
	 * `options.anonymous` skips attaching a stored credential even if one
	 * exists for the active server — needed for exactly one call
	 * (`setPasswordAnonymous`, the first-run bootstrap path): that
	 * request's whole meaning on the server depends on arriving with *no*
	 * `Authorization` header (`crates/api/src/identity_handlers.rs`'s
	 * `set_password` branches on the header's presence, not its validity),
	 * so it must never accidentally ride along on a session token some
	 * *other* login on this browser happens to have stored.
	 */
	async request<T>(path: string, init: RequestInit = {}, options: { anonymous?: boolean } = {}): Promise<T> {
		const server = activeServer();
		const base = resolveBaseUrl(server);
		const token = options.anonymous ? null : credentialReader.get(server.id);

		const headers = new Headers(init.headers);
		if (token) headers.set('Authorization', `Bearer ${token}`);
		if (init.body !== undefined && !headers.has('Content-Type')) {
			headers.set('Content-Type', 'application/json');
		}

		let response: Response;
		try {
			response = await fetch(`${base}${path}`, { ...init, headers });
		} catch (cause) {
			throw new NetworkError(`Could not reach ${base}${path}.`, cause);
		}

		switch (classifyResponse(response)) {
			case 'unauthorized': {
				// Only a request that actually carried a credential can have
				// had a session expire. A 401 answering a login attempt, or
				// any other call made with nothing to send, means "you are
				// not signed in" — running the expiry path there clears a
				// credential that was never there and announces a session
				// ending that never began, which is how a mistyped password
				// came to report "Session expired."
				if (token) this.handleSessionExpired(server.id);
				const body = await safeJson(response);
				throw new UnauthorizedError(errorBodyMessage(body) ?? undefined);
			}
			case 'forbidden': {
				// A 403 is authenticated-but-not-permitted, never a logout.
				// It becomes a typed error a caller can turn into
				// a message; it must never touch the credential or
				// connection state. The server's own reason (e.g. "choose a
				// registry handle before publishing") is read the same way
				// the 401 branch above reads its own — a generic "you do not
				// have permission" here is exactly the class of bug this
				// project already fixed once for a rejected login showing
				// "Session expired".
				const body = await safeJson(response);
				throw new ForbiddenError(errorBodyMessage(body) ?? undefined);
			}
			case 'http-error': {
				const body = await safeJson(response);
				throw new HttpError(`Request to ${path} failed with ${response.status}.`, response.status, body);
			}
			case 'no-content':
				return undefined as T;
			case 'ok':
				return (await safeJson(response)) as T;
		}
	}

	private handleSessionExpired(serverId: string): void {
		this.sessionExpiryGuard.fire(() => {
			endSession(serverId);
			setConnectionState('disconnected', 'Session expired.');
			sessionExpiredHandler();
		});
	}

	/** Re-arm the session-expiry guard. Call after establishing a fresh
	 * session (`login`, below) so a *later* expiry of the new session isn't
	 * silently swallowed by the previous one's guard. */
	resetSessionExpiry(): void {
		this.sessionExpiryGuard.reset();
	}

	/** `GET /api/health` — needs no credential; used by the heartbeat while
	 * no session exists for the active server (see `heartbeatTick`) and by
	 * anything else that just wants to know "is a server there at all". */
	async health(): Promise<HealthResponse> {
		return this.request<HealthResponse>('/api/health');
	}

	/** `GET /api/me` — the caller's own profile. Used by the heartbeat once
	 * a credential exists, so `'authenticated'` means an actually-valid
	 * session rather than merely "the server responded" (see
	 * `heartbeatTick`'s doc). */
	async me(): Promise<MeResponse> {
		return this.request<MeResponse>('/api/me');
	}

	/** `GET /api/me/zone` — the caller's own stored display (timezone) zone,
	 * or `null` if this account has never chosen one. The browser's own
	 * detected zone (`$lib/time/zone.ts`'s `detectBrowserZone`) is only ever
	 * a client-side suggestion for that `null` case — never a substitute
	 * server-side, and never used once a real value comes back here. */
	async getZone(): Promise<UserZoneResponse> {
		return this.request<UserZoneResponse>('/api/me/zone');
	}

	/** `PUT /api/me/zone` — sets the caller's own display zone. Rejected with
	 * a typed `HttpError` (400) if `zone` is not an id the server's bundled
	 * time zone database recognises. */
	async setZone(zone: string): Promise<UserZoneResponse> {
		const body: SetZoneRequest = { zone };
		return this.request<UserZoneResponse>('/api/me/zone', {
			method: 'PUT',
			body: JSON.stringify(body)
		});
	}

	/**
	 * `POST /api/login`. Stores the returned token for the active server,
	 * re-arms the session-expiry guard (a stale guard from a previous,
	 * now-dead session must not swallow this new session's eventual
	 * expiry), and restarts the heartbeat so `connectionStore` reflects the
	 * fresh session on its very next tick instead of waiting out whatever
	 * is left of the current interval.
	 */
	async login(email: string, password: string): Promise<void> {
		const server = activeServer();
		const { token } = await this.request<LoginResponse>('/api/login', {
			method: 'POST',
			body: JSON.stringify({ email, password })
		});
		establishSession(server.id, token);
		this.resetSessionExpiry();
		this.startHeartbeat();
	}

	/**
	 * `POST /api/logout`. Best-effort: the session is cleared locally even
	 * if the request itself fails (an already-dead session, or the server
	 * being briefly unreachable) — a user asking to log out expects to be
	 * logged out on this device regardless of whether the server could also
	 * delete its side of an already-useless session row.
	 */
	async logout(): Promise<void> {
		const server = activeServer();
		try {
			await this.request<void>('/api/logout', { method: 'POST' });
		} catch {
			// See doc comment above — clearing the local credential below
			// happens unconditionally either way.
		} finally {
			endSession(server.id);
			this.resetSessionExpiry();
			setConnectionState('disconnected');
			this.startHeartbeat();
		}
	}

	/**
	 * `POST /api/set-password`, self-service path: changes the *caller's
	 * own* password. Requires an existing session — the server ignores any
	 * `email` on this path (`crates/api/src/identity_handlers.rs`), so none
	 * is sent. This invalidates every other session for the account; the
	 * caller's own session (the one making this request) survives, so no
	 * local credential change is needed here.
	 */
	async setPassword(newPassword: string): Promise<void> {
		await this.request<void>('/api/set-password', {
			method: 'POST',
			body: JSON.stringify({ new_password: newPassword })
		});
	}

	/**
	 * `POST /api/set-password`, anonymous first-run path: the
	 * only way a fenced account — which cannot log in — ever gets a
	 * password. `options.anonymous` (see `request`'s doc) guarantees this
	 * never rides along on some other, unrelated session token this browser
	 * happens to have stored for the active server.
	 */
	async setPasswordAnonymous(email: string, newPassword: string): Promise<void> {
		await this.request<void>(
			'/api/set-password',
			{ method: 'POST', body: JSON.stringify({ email, new_password: newPassword }) },
			{ anonymous: true }
		);
	}

	/**
	 * `GET /api/users`. Scoped by
	 * `IdentityStore::list_users` itself — an ordinary user gets
	 * back only their own row, an admin gets everyone, and `total` already
	 * respects that scope.
	 */
	async listUsers(limit: number, offset: number): Promise<UsersPage> {
		return this.request<UsersPage>(`/api/users?limit=${limit}&offset=${offset}`);
	}

	/** `POST /api/users`. */
	async createUser(body: CreateUserRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/users', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `GET /api/roles`. Scoped by
	 * `IdentityStore::list_roles` itself, the same way `listUsers` is. */
	async listRoles(limit: number, offset: number): Promise<RolesPage> {
		return this.request<RolesPage>(`/api/roles?limit=${limit}&offset=${offset}`);
	}

	/** `POST /api/roles`. */
	async createRole(body: CreateRoleRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/roles', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `POST /api/users/{user_id}/roles`: assigns an existing
	 * role to an existing user. */
	async assignRole(userId: string, roleId: string): Promise<void> {
		await this.request<void>(`/api/users/${encodeURIComponent(userId)}/roles`, {
			method: 'POST',
			body: JSON.stringify({ role_id: roleId })
		});
	}

	/** `POST /api/users/{user_id}/grants`: attaches a direct
	 * `(Action, Resource, Scope)` grant to a user, independent of any role. */
	async grantDirect(userId: string, grant: GrantDto): Promise<void> {
		await this.request<void>(`/api/users/${encodeURIComponent(userId)}/grants`, {
			method: 'POST',
			body: JSON.stringify(grant)
		});
	}

	/** `POST /api/users/{user_id}/grants/revoke` — the inverse of `grantDirect`. */
	async revokeDirect(userId: string, grant: GrantDto): Promise<void> {
		await this.request<void>(`/api/users/${encodeURIComponent(userId)}/grants/revoke`, {
			method: 'POST',
			body: JSON.stringify(grant)
		});
	}

	/**
	 * `GET /api/alerts` (mounted). Scoped by
	 * `senken_alerts::AlertStore::list_alerts` itself, the same way
	 * `listUsers`/`listRoles` are scoped by their own stores — an ordinary
	 * user gets back only their own alerts, `total` already respects that.
	 */
	async listAlerts(limit: number, offset: number): Promise<AlertsPage> {
		return this.request<AlertsPage>(`/api/alerts?limit=${limit}&offset=${offset}`);
	}

	/** `GET /api/alerts/{id}` (mounted). */
	async getAlert(id: string): Promise<AlertDto> {
		return this.request<AlertDto>(`/api/alerts/${encodeURIComponent(id)}`);
	}

	/** `POST /api/alerts` (mounted). */
	async createAlert(body: CreateAlertRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/alerts', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `DELETE /api/alerts/{id}` (mounted). */
	async deleteAlert(id: string): Promise<void> {
		await this.request<void>(`/api/alerts/${encodeURIComponent(id)}`, { method: 'DELETE' });
	}

	/**
	 * `GET /api/instruments`: ranked, multi-source instrument search over
	 * the server's own cached catalog — no venue request, cache hit or not.
	 * `query` may be free text or `source:term` to narrow to one venue, the
	 * same grammar `senken-cli`'s own `search` subcommand accepts.
	 *
	 * Not yet the charts page's symbol picker's data source — see
	 * `InstrumentsPage`'s own doc in `types.ts`.
	 */
	async searchInstruments(query: string, limit = 20, offset = 0): Promise<InstrumentsPage> {
		const params = new URLSearchParams({ q: query, limit: String(limit), offset: String(offset) });
		return this.request<InstrumentsPage>(`/api/instruments?${params.toString()}`);
	}

	/** `GET /api/sources`: every registered source, ordered by id, with
	 * whether it has a bar source and whether it also has a live feed pool
	 * — never fetched per-caller; see `$lib/api/sources.svelte.ts`,
	 * which caches this for the life of the tab. */
	async listSources(): Promise<SourcesResponse> {
		return this.request<SourcesResponse>('/api/sources');
	}

	// ------------------------------------------------------------------
	// Workspaces, layouts, panes and layers. `senken_workspace::WorkspaceStore` performs its own
	// per-account scoping the same way `listUsers`/
	// `listAlerts` do — a plain, valid session is all a caller needs to
	// reach these, ownership is enforced store-side.
	// ------------------------------------------------------------------

	/** `GET /api/workspaces`. */
	async listWorkspaces(limit: number, offset: number): Promise<WorkspacesPage> {
		return this.request<WorkspacesPage>(`/api/workspaces?limit=${limit}&offset=${offset}`);
	}

	/** `POST /api/workspaces`. */
	async createWorkspace(name: string): Promise<IdResponse> {
		return this.request<IdResponse>('/api/workspaces', {
			method: 'POST',
			body: JSON.stringify({ name } satisfies CreateWorkspaceRequest)
		});
	}

	/**
	 * `GET /api/workspaces/default` ("default-on-first-open belongs on the server"). Creates the caller's default workspace and
	 * its one layout on the very first call for this account, and returns
	 * the same pair on every later one — the call the charts page makes
	 * whenever it has no workspace already selected.
	 */
	async defaultWorkspace(): Promise<DefaultWorkspaceResponse> {
		return this.request<DefaultWorkspaceResponse>('/api/workspaces/default');
	}

	/** `PATCH /api/workspaces/{id}`. */
	async renameWorkspace(id: string, name: string): Promise<void> {
		await this.request<void>(`/api/workspaces/${encodeURIComponent(id)}`, {
			method: 'PATCH',
			body: JSON.stringify({ name })
		});
	}

	/** `DELETE /api/workspaces/{id}`. */
	async deleteWorkspace(id: string): Promise<void> {
		await this.request<void>(`/api/workspaces/${encodeURIComponent(id)}`, { method: 'DELETE' });
	}

	/** `PATCH /api/workspaces/{id}/settings`. `settings` is opaque JSON-object
	 * text this client never interprets — the server validates only that it
	 * parses as a JSON object. */
	async updateWorkspaceSettings(id: string, settings: string): Promise<void> {
		await this.request<void>(`/api/workspaces/${encodeURIComponent(id)}/settings`, {
			method: 'PATCH',
			body: JSON.stringify({ settings })
		});
	}

	/** `GET /api/workspaces/{id}/layouts`. */
	async listLayouts(workspaceId: string): Promise<LayoutSummaryDto[]> {
		return this.request<LayoutSummaryDto[]>(`/api/workspaces/${encodeURIComponent(workspaceId)}/layouts`);
	}

	/** `GET /api/layouts/{id}`: one layout with its full nested pane/layer
	 * structure — the shape the charts page renders a workspace from. */
	async getLayout(layoutId: string): Promise<LayoutDetailDto> {
		return this.request<LayoutDetailDto>(`/api/layouts/${encodeURIComponent(layoutId)}`);
	}

	/**
	 * `PUT /api/layouts/{id}`: replaces a layout's whole
	 * pane/layer structure in one transaction. `WorkspaceStore` has no
	 * per-layer patch method (the implementation notes), so every
	 * pane/layer mutation the charts page makes — a new pane, a changed
	 * instrument or timeframe, an added/removed/toggled layer — goes
	 * through this same call with the layout's *entire* new shape.
	 */
	async replaceLayout(layoutId: string, body: ReplaceLayoutRequest): Promise<void> {
		await this.request<void>(`/api/layouts/${encodeURIComponent(layoutId)}`, {
			method: 'PUT',
			body: JSON.stringify(body)
		});
	}

	/** `PATCH /api/layers/{id}`. */
	async updateLayer(layerId: string, body: LayerInputDto): Promise<void> {
		await this.request<void>(`/api/layers/${encodeURIComponent(layerId)}`, {
			method: 'PATCH',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/layers/{id}`. */
	async deleteLayer(layerId: string): Promise<void> {
		await this.request<void>(`/api/layers/${encodeURIComponent(layerId)}`, { method: 'DELETE' });
	}

	/** `PATCH /api/drawings/{id}`. */
	async updateDrawing(drawingId: string, body: DrawingInputDto): Promise<void> {
		await this.request<void>(`/api/drawings/${encodeURIComponent(drawingId)}`, {
			method: 'PATCH',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/drawings/{id}`. */
	async deleteDrawing(drawingId: string): Promise<void> {
		await this.request<void>(`/api/drawings/${encodeURIComponent(drawingId)}`, { method: 'DELETE' });
	}


	// ------------------------------------------------------------------
	// Bars.
	// `plan()`/`ensure()` stay two different calls: `plan`
	// never starts work, so the charts page can show what is missing and
	// roughly how long before committing to a fetch; `ensure` enqueues and
	// returns a job reference to poll, never blocking the request itself.
	// `range` is the read path a chart actually renders from — resolvable
	// entirely from the cache/store/aggregation ladder, so
	// opening the same range twice costs nothing.
	// ------------------------------------------------------------------

	/** `GET /api/bars/plan`: pure inspection, starts no work. */
	async planBars(instrument: string, spec: string, from: number, to: number): Promise<BarsRequirementDto> {
		const params = new URLSearchParams({ instrument, spec, from: String(from), to: String(to) });
		return this.request<BarsRequirementDto>(`/api/bars/plan?${params}`);
	}

	/** `GET /api/bars/range`: whatever is already resolvable right now. */
	async rangeBars(instrument: string, spec: string, from: number, to: number): Promise<BarRangeResponse> {
		const params = new URLSearchParams({ instrument, spec, from: String(from), to: String(to) });
		return this.request<BarRangeResponse>(`/api/bars/range?${params}`);
	}

	/** `POST /api/bars/ensure`: enqueues whatever `planBars` would report as
	 * missing and returns immediately with a job reference — poll it with
	 * `barJobStatus`. */
	async ensureBars(body: EnsureBarsRequest): Promise<EnsureBarsResponse> {
		return this.request<EnsureBarsResponse>('/api/bars/ensure', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `GET /api/bars/jobs/{job_id}`. */
	async barJobStatus(jobId: string): Promise<BarJobDto> {
		return this.request<BarJobDto>(`/api/bars/jobs/${encodeURIComponent(jobId)}`);
	}

	// ------------------------------------------------------------------
	// Indicators. the // whole point: the browser never computes these itself any more — every
	// value a chart layer plots comes from this same catalogue/compute pair,
	// the same `senken-indicators` engine an alert evaluates against.
	// ------------------------------------------------------------------

	/** `GET /api/indicators`: the ten built-ins `senken-indicators`
	 * implements, with the parameter keys and reported value keys each one
	 * needs. */
	async listIndicators(): Promise<IndicatorCatalogEntry[]> {
		return this.request<IndicatorCatalogEntry[]>('/api/indicators');
	}

	/** `POST /api/indicators/compute`: replays whatever bars are already
	 * resolvable for `instrument`/`spec`/`from`/`to` through the named
	 * indicator — the caller must already have `ensureBars`d the range, this
	 * endpoint fetches nothing itself (mirrors `rangeBars`). */
	async computeIndicator(body: ComputeIndicatorRequest): Promise<ComputeIndicatorResponse> {
		return this.request<ComputeIndicatorResponse>('/api/indicators/compute', {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `GET /api/indicators/plugins`: every dynamic indicator ever
	 * registered, enabled or not — unlike `listIndicators`, which only ever
	 * reports what a chart may place right now. The unified Plugins page
	 * still reads this (matched by id against `listPlugins`' own rows) for
	 * the runtime health and log detail `GET /api/plugins/{id}` does not
	 * carry yet; installing, enabling and disabling a plugin go through
	 * `installPlugin`/`setPluginEnabled` below instead of this route's own
	 * `POST`/`enabled` calls, which nothing in this app calls any more. */
	async listIndicatorPlugins(): Promise<IndicatorPluginDto[]> {
		return this.request<IndicatorPluginDto[]>('/api/indicators/plugins');
	}

	// ------------------------------------------------------------------
	// The unified plugin system: one list for every venue plugin compiled
	// into the server and every package discovered on disk, plus install,
	// uninstall, refresh and the one switch to enable or disable one.
	// `crates/api/src/plugin_handlers.rs` is the server side.
	// ------------------------------------------------------------------

	/** `GET /api/plugins`: every plugin this server knows about, static and
	 * package alike. Needs no grant beyond a valid session — every signed-in
	 * caller needs to know which venues are active. */
	async listPlugins(): Promise<PluginDto[]> {
		const response = await this.request<PluginListResponse>('/api/plugins');
		return response.plugins;
	}

	/** `GET /api/plugins/{id}`: one plugin's row from the same catalog
	 * `listPlugins` reads. */
	async getPlugin(id: string): Promise<PluginDto> {
		return this.request<PluginDto>(`/api/plugins/${encodeURIComponent(id)}`);
	}

	/** `POST /api/plugins/{id}/enabled`: enables or disables one plugin.
	 * Requires `Action::Edit` on `Resource::Plugin` at `Scope::All` — an
	 * ordinary account gets a 403. A package's toggle takes effect
	 * immediately; a static plugin's is only recorded, and the returned
	 * `needs_restart: true` says so rather than pretending it already
	 * applied. */
	async setPluginEnabled(id: string, enabled: boolean): Promise<PluginDto> {
		const body: SetPluginEnabledRequest = { enabled };
		return this.request<PluginDto>(`/api/plugins/${encodeURIComponent(id)}/enabled`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `POST /api/plugins`: installs a package from the raw bytes of a zip
	 * archive (`manifest.json` at its root), or a bare compiled `.wasm`
	 * indicator component — wrapped into a generated package automatically,
	 * the same shape `uploadIndicatorPlugin` above has always accepted.
	 * Requires `Action::Create` on `Resource::Plugin` at `Scope::All`. */
	async installPlugin(bytes: Uint8Array | ArrayBuffer): Promise<InstallPluginResponse> {
		return this.request<InstallPluginResponse>('/api/plugins', {
			method: 'POST',
			headers: { 'Content-Type': 'application/octet-stream' },
			body: bytes instanceof ArrayBuffer ? bytes : new Uint8Array(bytes)
		});
	}

	/** `DELETE /api/plugins/{id}`: uninstalls a package's files entirely. A
	 * built-in, statically compiled plugin has nothing to remove and answers
	 * `409` instead — see this call's own error, not a silently-ignored
	 * request. Requires `Action::Delete` on `Resource::Plugin` at
	 * `Scope::All`. */
	async uninstallPlugin(id: string): Promise<void> {
		await this.request<void>(`/api/plugins/${encodeURIComponent(id)}`, {
			method: 'DELETE'
		});
	}

	/** `POST /api/plugins/refresh`: an explicit rescan of the plugin package
	 * directory, for a package dropped directly on disk rather than
	 * uploaded — never a filesystem watcher (one can fire mid-copy and read
	 * a half-written file). Requires `Action::View` on `Resource::Plugin` at
	 * `Scope::All`. */
	async refreshPlugins(): Promise<PluginDto[]> {
		const response = await this.request<PluginListResponse>('/api/plugins/refresh', {
			method: 'POST'
		});
		return response.plugins;
	}

	// ------------------------------------------------------------------
	// Indicators an account wrote and compiled themselves. Every one of
	// these funnels through `senken_indicator_registry::UserIndicatorStore`
	// on the server, the same per-account scoping `listNotes`/`getNote`
	// use below. A failed compile is still a `200` — see
	// `SaveUserIndicatorResponse`'s own fields for how a mistake in the
	// author's own Rust is reported.
	// ------------------------------------------------------------------

	myIndicators = {
		/** `GET /api/my/indicators`: summaries only, never the source. */
		list: (): Promise<UserIndicatorSummaryDto[]> =>
			this.request<UserIndicatorSummaryDto[]>('/api/my/indicators'),

		/** `GET /api/my/indicators/{id}`: the full row, source included. */
		get: (id: string): Promise<UserIndicatorDto> =>
			this.request<UserIndicatorDto>(`/api/my/indicators/${encodeURIComponent(id)}`),

		/** `POST /api/my/indicators`: creates a new indicator, then compiles
		 * it immediately. */
		create: (body: CreateUserIndicatorRequest): Promise<SaveUserIndicatorResponse> =>
			this.request<SaveUserIndicatorResponse>('/api/my/indicators', {
				method: 'POST',
				body: JSON.stringify(body)
			}),

		/** `PUT /api/my/indicators/{id}`: saves a new title/source, then
		 * compiles. */
		update: (id: string, body: UpdateUserIndicatorRequest): Promise<SaveUserIndicatorResponse> =>
			this.request<SaveUserIndicatorResponse>(`/api/my/indicators/${encodeURIComponent(id)}`, {
				method: 'PUT',
				body: JSON.stringify(body)
			}),

		/** `POST /api/my/indicators/{id}/compile`: recompiles the source
		 * already on file, without changing it. */
		compile: (id: string): Promise<SaveUserIndicatorResponse> =>
			this.request<SaveUserIndicatorResponse>(`/api/my/indicators/${encodeURIComponent(id)}/compile`, {
				method: 'POST'
			}),

		/** `DELETE /api/my/indicators/{id}`. */
		remove: (id: string): Promise<void> =>
			this.request<void>(`/api/my/indicators/${encodeURIComponent(id)}`, { method: 'DELETE' }),

		/** `GET /api/my/indicators/toolchain`: whether this server can
		 * compile a Rust indicator right now, so an authoring panel can
		 * disable Save with a reason instead of letting a save fail with no
		 * explanation. */
		toolchain: (): Promise<IndicatorToolchainStatusResponse> =>
			this.request<IndicatorToolchainStatusResponse>('/api/my/indicators/toolchain')
	};

	// ------------------------------------------------------------------
	// Watchlists: a user-owned group of watched instruments and its
	// membership. `senken_watchlist::WatchlistStore` performs its own
	// per-account scoping the same way `listWorkspaces`/`listAlerts` do.
	// ------------------------------------------------------------------

	/** `GET /api/watchlists`. */
	async listWatchlistGroups(limit: number, offset: number): Promise<WatchlistGroupsPage> {
		return this.request<WatchlistGroupsPage>(`/api/watchlists?limit=${limit}&offset=${offset}`);
	}

	/** `POST /api/watchlists`. */
	async createWatchlistGroup(body: CreateWatchlistGroupRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/watchlists', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `PATCH /api/watchlists/{id}`. */
	async renameWatchlistGroup(id: string, name: string): Promise<void> {
		await this.request<void>(`/api/watchlists/${encodeURIComponent(id)}`, {
			method: 'PATCH',
			body: JSON.stringify({ name })
		});
	}

	/** `DELETE /api/watchlists/{id}`. */
	async deleteWatchlistGroup(id: string): Promise<void> {
		await this.request<void>(`/api/watchlists/${encodeURIComponent(id)}`, { method: 'DELETE' });
	}

	/** `POST /api/watchlists/reorder`: `ids[0]` becomes the first group,
	 * `ids[1]` the second, and so on. */
	async reorderWatchlistGroups(ids: string[]): Promise<void> {
		await this.request<void>('/api/watchlists/reorder', { method: 'POST', body: JSON.stringify({ ids }) });
	}

	/** `GET /api/watchlists/{id}/members`. */
	async listWatchlistMembers(groupId: string): Promise<WatchlistMemberDto[]> {
		return this.request<WatchlistMemberDto[]>(`/api/watchlists/${encodeURIComponent(groupId)}/members`);
	}

	/** `POST /api/watchlists/{id}/members`. Adding an instrument the group
	 * already holds is idempotent server-side — this always resolves with
	 * that member's id, never a conflict. */
	async addWatchlistMember(groupId: string, body: AddWatchlistMemberRequest): Promise<IdResponse> {
		return this.request<IdResponse>(`/api/watchlists/${encodeURIComponent(groupId)}/members`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/watchlist-members/{id}` — a member's own id already
	 * uniquely identifies it, so this is not nested under its group's path. */
	async removeWatchlistMember(memberId: string): Promise<void> {
		await this.request<void>(`/api/watchlist-members/${encodeURIComponent(memberId)}`, { method: 'DELETE' });
	}

	/** `POST /api/watchlists/{id}/members/reorder`. */
	async reorderWatchlistMembers(groupId: string, ids: string[]): Promise<void> {
		await this.request<void>(`/api/watchlists/${encodeURIComponent(groupId)}/members/reorder`, {
			method: 'POST',
			body: JSON.stringify({ ids })
		});
	}

	// ------------------------------------------------------------------
	// Notes: a user-owned freeform note. `senken_notes::NoteStore`
	// performs its own per-account scoping the same way the stores above
	// do. `listNotes` never carries a note's body (`NoteSummaryDto`) — only
	// `getNote` (`NoteDto`) does.
	// ------------------------------------------------------------------

	/** `GET /api/notes`. */
	async listNotes(limit: number, offset: number): Promise<NotesPage> {
		return this.request<NotesPage>(`/api/notes?limit=${limit}&offset=${offset}`);
	}

	/** `POST /api/notes`. */
	async createNote(body: CreateNoteRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/notes', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `GET /api/notes/{id}`: the full note, body included. */
	async getNote(id: string): Promise<NoteDto> {
		return this.request<NoteDto>(`/api/notes/${encodeURIComponent(id)}`);
	}

	/** `PUT /api/notes/{id}`: replaces both title and body. */
	async updateNote(id: string, body: UpdateNoteRequest): Promise<void> {
		await this.request<void>(`/api/notes/${encodeURIComponent(id)}`, {
			method: 'PUT',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/notes/{id}`. */
	async deleteNote(id: string): Promise<void> {
		await this.request<void>(`/api/notes/${encodeURIComponent(id)}`, { method: 'DELETE' });
	}

	// ------------------------------------------------------------------
	// Storage: what this server is holding on disk, and reclaiming it.
	// `senken_store::Store` has no notion of a user, so both endpoints check
	// `Resource::Storage` at `Scope::All` themselves rather than delegating
	// to a per-account guarded store the way every block above does.
	// ------------------------------------------------------------------

	/** `GET /api/storage`. */
	async storageReport(): Promise<StorageReportDto> {
		return this.request<StorageReportDto>('/api/storage');
	}

	/** `POST /api/storage/delete`. Naming only `source_id` deletes the whole
	 * source; adding `symbol` narrows to one instrument; adding `series_id`
	 * too narrows to one series. */
	async deleteStorage(body: DeleteStorageRequest): Promise<DeleteStorageResponse> {
		return this.request<DeleteStorageResponse>('/api/storage/delete', {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	// ------------------------------------------------------------------
	// Trade engine: the adapters a plugin registered, the accounts a user
	// attached to them, and the orders, positions and balances read back
	// through those adapters.
	//
	// Nothing here is cached. A broker is the system of record for its own
	// account, so every figure on screen is what the adapter answered on
	// this request — the server does not keep a copy, and neither does
	// this client.
	// ------------------------------------------------------------------

	/** `GET /api/trade/adapters`. */
	async listTradeAdapters(): Promise<AdaptersResponse> {
		return this.request<AdaptersResponse>('/api/trade/adapters');
	}

	/** `GET /api/trade/accounts`. */
	async listTradeAccounts(limit = 100, offset = 0): Promise<TradeAccountsPage> {
		return this.request<TradeAccountsPage>(
			`/api/trade/accounts?limit=${limit}&offset=${offset}`
		);
	}

	/** `POST /api/trade/accounts`. */
	async createTradeAccount(body: CreateTradeAccountRequest): Promise<IdResponse> {
		return this.request<IdResponse>('/api/trade/accounts', {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `GET /api/trade/accounts/{id}`: the account, its resolved access and
	 * its health, in one round trip. */
	async tradeAccountState(id: string): Promise<TradeAccountStateDto> {
		return this.request<TradeAccountStateDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}`
		);
	}

	/** `PATCH /api/trade/accounts/{id}`: rename, or enable/disable. */
	async updateTradeAccount(id: string, body: UpdateTradeAccountRequest): Promise<void> {
		await this.request<void>(`/api/trade/accounts/${encodeURIComponent(id)}`, {
			method: 'PATCH',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/trade/accounts/{id}`. */
	async deleteTradeAccount(id: string): Promise<void> {
		await this.request<void>(`/api/trade/accounts/${encodeURIComponent(id)}`, {
			method: 'DELETE'
		});
	}

	/** `GET /api/trade/accounts/{id}/settings`. Credentials come back as
	 * `null`; `secrets_set` says which of them actually hold one. */
	async tradeAccountSettings(id: string): Promise<TradeAccountSettingsDto> {
		return this.request<TradeAccountSettingsDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}/settings`
		);
	}

	/** `PUT /api/trade/accounts/{id}/settings`. A secret left blank keeps
	 * whatever is stored — the server applies that before validating, so a
	 * required credential already on file does not have to be re-typed. */
	async replaceTradeAccountSettings(
		id: string,
		body: ReplaceSettingsRequest
	): Promise<TradeAccountSettingsDto> {
		return this.request<TradeAccountSettingsDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}/settings`,
			{ method: 'PUT', body: JSON.stringify(body) }
		);
	}

	/** `GET /api/trade/accounts/{id}/health`. */
	async tradeAccountHealth(id: string): Promise<HealthDto> {
		return this.request<HealthDto>(`/api/trade/accounts/${encodeURIComponent(id)}/health`);
	}

	/** `GET /api/trade/accounts/{id}/balances`. */
	async tradeAccountBalances(id: string): Promise<BalancesDto> {
		return this.request<BalancesDto>(`/api/trade/accounts/${encodeURIComponent(id)}/balances`);
	}

	/** `GET /api/trade/accounts/{id}/positions`. */
	async tradeAccountPositions(id: string): Promise<PositionDto[]> {
		return this.request<PositionDto[]>(`/api/trade/accounts/${encodeURIComponent(id)}/positions`);
	}

	/** `GET /api/trade/accounts/{id}/orders`. */
	async tradeAccountOrders(id: string, status: 'open' | 'all' = 'open'): Promise<OrderDto[]> {
		return this.request<OrderDto[]>(
			`/api/trade/accounts/${encodeURIComponent(id)}/orders?status=${status}`
		);
	}

	/** `GET /api/trade/accounts/{id}/fills`. */
	async tradeAccountFills(id: string): Promise<FillDto[]> {
		return this.request<FillDto[]>(`/api/trade/accounts/${encodeURIComponent(id)}/fills`);
	}

	/** `POST /api/trade/accounts/{id}/orders`. */
	async placeOrder(id: string, body: PlaceOrderRequest): Promise<OrderDto> {
		return this.request<OrderDto>(`/api/trade/accounts/${encodeURIComponent(id)}/orders`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `DELETE /api/trade/accounts/{id}/orders/{orderId}`. */
	async cancelOrder(id: string, orderId: string): Promise<OrderDto> {
		return this.request<OrderDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}/orders/${encodeURIComponent(orderId)}`,
			{ method: 'DELETE' }
		);
	}

	/** `PATCH /api/trade/accounts/{id}/orders/{orderId}`: amends a resting
	 * order's size, limit price or trigger price in place. */
	async amendOrder(id: string, orderId: string, body: AmendOrderRequest): Promise<OrderDto> {
		return this.request<OrderDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}/orders/${encodeURIComponent(orderId)}`,
			{ method: 'PATCH', body: JSON.stringify(body) }
		);
	}

	/** `POST /api/trade/accounts/{id}/close`: closes an open position by
	 * sending an opposite market order for exactly the size the adapter
	 * reports held right now — never a size this client chose. */
	async closePosition(id: string, body: CloseRequest): Promise<OrderDto> {
		return this.request<OrderDto>(`/api/trade/accounts/${encodeURIComponent(id)}/close`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `POST /api/trade/accounts/{id}/actions/{actionId}`. */
	async runAdapterAction(
		id: string,
		actionId: string,
		body: RunActionRequest
	): Promise<ActionOutcomeDto> {
		return this.request<ActionOutcomeDto>(
			`/api/trade/accounts/${encodeURIComponent(id)}/actions/${encodeURIComponent(actionId)}`,
			{ method: 'POST', body: JSON.stringify(body) }
		);
	}

	/**
	 * Starts polling to keep `connectionStore` (the
	 * disconnected/connecting/authenticated/reconnecting machine) accurate,
	 * so the UI and the WS layer can read connection health without
	 * either of them polling anything themselves.
	 *
	 * Polls `GET /api/me` when a credential is stored for the active server
	 * (so `'authenticated'` means an actually-resolvable session — a 401
	 * here drives `connectionStore` to `'disconnected'` via
	 * `handleSessionExpired`, same as any other request), and falls back to
	 * `GET /api/health` when there is none (the login page, before the user
	 * has signed in — there is nothing to poll with credentials at that
	 * point) so the UI can still show whether the chosen server is reachable
	 * at all.
	 *
	 * Idempotent and safe to call again after `selectServer` — a prior loop
	 * is always stopped first, so switching servers can't leave two
	 * heartbeats racing against different base URLs.
	 */
	startHeartbeat(): void {
		this.stopHeartbeat();
		this.sessionExpiryGuard.reset();
		const generation = ++this.heartbeatGeneration;
		this.heartbeatAttempt = 0;
		setConnectionState('connecting');
		void this.heartbeatTick(generation);
	}

	stopHeartbeat(): void {
		this.heartbeatGeneration++;
		if (this.heartbeatTimer !== null) {
			clearTimeout(this.heartbeatTimer);
			this.heartbeatTimer = null;
		}
	}

	private async heartbeatTick(generation: number): Promise<void> {
		try {
			const server = activeServer();
			if (credentialReader.get(server.id)) {
				await this.me();
			} else {
				await this.health();
			}
			if (generation !== this.heartbeatGeneration) return; // superseded by a server switch
			this.heartbeatAttempt = 0;
			setConnectionState('authenticated');
			this.heartbeatTimer = setTimeout(() => void this.heartbeatTick(generation), HEARTBEAT_INTERVAL_MS);
		} catch (error) {
			if (generation !== this.heartbeatGeneration) return;
			// A 401 already drove state to 'disconnected' and cleared the
			// credential via `handleSessionExpired` — polling a server that
			// just rejected our token would only 401 again forever, so stop
			// rather than backing off pointlessly. A future login restarts
			// the heartbeat itself.
			if (error instanceof UnauthorizedError) return;
			// A `ForbiddenError` here means `/api/me` 403'd — a valid session
			// whose account is *still* fenced (the fence is a property of
			// the account, not the session, so a token minted before a
			// password was set stays fenced even if the password is later
			// cleared again by some other path). That is not "session gone" —
			// a 403 must never log anyone out — but retrying will also keep
			// 403'ing until the account is unfenced. Left as a fall-through to
			// the generic retry below rather than a third branch: this state
			// should not arise on the primary first-run-then-login path this
			// milestone builds (login only succeeds once `set-password` has
			// already cleared the fence), and a dedicated UI for it would be
			// speculative.
			const wasConnected =
				connectionStore.state === 'authenticated' || connectionStore.state === 'reconnecting';
			setConnectionState(wasConnected ? 'reconnecting' : 'connecting', describeError(error));
			const delay = backoffDelay(this.heartbeatAttempt++);
			this.heartbeatTimer = setTimeout(() => void this.heartbeatTick(generation), delay);
		}
	}
}

async function safeJson(response: Response): Promise<unknown> {
	const text = await response.text();
	if (!text) return undefined;
	try {
		return JSON.parse(text);
	} catch {
		return text;
	}
}

export const apiClient = new ApiClient();

/** Switch the active server and restart the heartbeat against it — the
 * pairing its done-criterion asks for ("point at a different server and
 * back without a restart"). Exported from here, not `servers.svelte.ts`,
 * so that module doesn't need to import `apiClient` back (it would create
 * a cycle: `client.ts` already imports from `servers.svelte.ts`).
 *
 * Also refreshes `sessionStore` (the auth gate reads it): each
 * server has its own credential, so switching servers can flip
 * whether a session exists at all. */
export function switchServer(id: string): void {
	selectServer(id);
	refreshSessionPresence();
	apiClient.startHeartbeat();
}
