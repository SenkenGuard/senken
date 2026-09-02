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
	SetIndicatorPluginEnabledRequest,
	CompileIndicatorRequest,
	CompileIndicatorErrorDto,
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
	RegistryPage,
	IndicatorEntryDto,
	PublishIndicatorRequest,
	SetHandleRequest,
	HandleResponse,
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

/** `installIndicator`'s result — the compiled `wasm32-wasip2` component's
 * raw bytes plus the language version they were compiled against. Not a
 * generated DTO: `install_indicator`'s success body is
 * `application/wasm`, not JSON, so there is no `serde` struct for
 * `openapi-typescript` to have derived this shape from. */
export interface InstalledIndicator {
	wasm: ArrayBuffer;
	languageVersion: string;
}

// B16 point 2: "route to login exactly once." Q5 plugs in the real
// navigation with one call to `setSessionExpiredHandler` (`app-shell.svelte`,
// the auth gate); until that call happens the default no-op is fine because
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
	// Guards B16 point 2's "exactly once" — see `once-guard.ts` for why this
	// is a separate, unit-tested primitive rather than an inline flag.
	private readonly sessionExpiryGuard = new OnceGuard();
	private heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
	private heartbeatAttempt = 0;
	private heartbeatGeneration = 0;

	/**
	 * B16 point 1 (attach credential + base URL) and 2/3 (translate
	 * transport failures into typed errors). This is the only function in
	 * the app that calls `fetch`.
	 *
	 * `options.anonymous` skips attaching a stored credential even if one
	 * exists for the active server — needed for exactly one call
	 * (`setPasswordAnonymous`, the B4 first-run bootstrap path): that
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
				// B16 point 3: 403 is authenticated-but-not-permitted, never
				// a logout. It becomes a typed error a caller can turn into
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
	 * is sent. Per B13 this invalidates every other session for the
	 * account; the caller's own session (the one making this request)
	 * survives, so no local credential change is needed here.
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

	/** `POST /api/indicators/compile`: compiles indicator-lang source and
	 * registers the result the same way `uploadIndicatorPlugin` registers a
	 * compiled `.wasm` component. A `400` here carries a
	 * `CompileIndicatorErrorDto` body (`line`/`column`/`message`), not the
	 * crate-wide `{error}` shape — pass `HttpError.body` to
	 * `readCompileIndicatorError` below rather than `getErrorMessage`, which
	 * only ever looks for `error`. */
	async compileIndicator(source: string): Promise<IndicatorCatalogEntry> {
		const body: CompileIndicatorRequest = { source };
		return this.request<IndicatorCatalogEntry>('/api/indicators/compile', {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

	/** `POST /api/indicators/plugins`: registers a compiled `wasm32-wasip2`
	 * component (the raw component bytes, not JSON) as a dynamic indicator.
	 * Requires the caller to hold `Action::Create` on `Resource::Indicator`
	 * at `Scope::All` — an ordinary account gets a 403, the same as any
	 * other storage-wide administrative call. */
	async uploadIndicatorPlugin(wasm: Uint8Array | ArrayBuffer): Promise<IndicatorCatalogEntry> {
		return this.request<IndicatorCatalogEntry>('/api/indicators/plugins', {
			method: 'POST',
			headers: { 'Content-Type': 'application/wasm' },
			body: wasm instanceof ArrayBuffer ? wasm : new Uint8Array(wasm)
		});
	}

	/** `GET /api/indicators/plugins`: every dynamic indicator ever
	 * registered, enabled or not — unlike `listIndicators`, which only ever
	 * reports what a chart may place right now. */
	async listIndicatorPlugins(): Promise<IndicatorPluginDto[]> {
		return this.request<IndicatorPluginDto[]>('/api/indicators/plugins');
	}

	/** `POST /api/indicators/plugins/{id}/enabled`: flips whether
	 * `listIndicators` currently offers this dynamic indicator, without
	 * discarding the loaded component. A chart layer already plotting it
	 * keeps its own stored parameters regardless — disabling only removes
	 * it from the catalogue, so a client shows a placeholder rather than
	 * dropping the layer, and enabling restores the real plot.
	 *
	 * `id` is `IndicatorPluginDto.id` — present for every entry regardless
	 * of state, unlike its flattened catalogue fields (`name`, `title`, …)
	 * which are only there once a descriptor has actually been read.
	 * Passing `enabled: true` also closes this plugin's circuit breaker, so
	 * this same call re-enables one the runtime auto-disabled after
	 * repeated traps. */
	async setIndicatorPluginEnabled(id: string, enabled: boolean): Promise<void> {
		const body: SetIndicatorPluginEnabledRequest = { enabled };
		await this.request<void>(`/api/indicators/plugins/${encodeURIComponent(id)}/enabled`, {
			method: 'POST',
			body: JSON.stringify(body)
		});
	}

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
	// The indicator registry: publish, search, install indicator-lang
	// source. `senken_indicator_registry::RegistryStore` performs its own
	// guarded check on `publish`/`listMyIndicators`; `searchIndicators` and
	// `getRegistryIndicator` need no session — a published indicator is
	// public by design (`crates/api/src/registry_handlers.rs`'s own doc).
	// ------------------------------------------------------------------

	/** `POST /api/registry/indicators`: publishes `source` under `name` in
	 * the caller's own namespace, replacing an earlier publish of the same
	 * name. */
	async publishIndicator(name: string, source: string): Promise<IdResponse> {
		const body: PublishIndicatorRequest = { name, source };
		return this.request<IdResponse>('/api/registry/indicators', { method: 'POST', body: JSON.stringify(body) });
	}

	/** `GET /api/registry/indicators`: the public catalog, across every
	 * namespace — no session required. `query` matches on indicator name;
	 * an empty string lists everything published, newest first. */
	async searchIndicators(query: string, limit = 50, offset = 0): Promise<RegistryPage> {
		const params = new URLSearchParams({ limit: String(limit), offset: String(offset) });
		if (query) params.set('query', query);
		return this.request<RegistryPage>(`/api/registry/indicators?${params.toString()}`);
	}

	/** `GET /api/registry/indicators/mine`: every indicator the caller has
	 * published. */
	async listMyIndicators(limit: number, offset: number): Promise<RegistryPage> {
		return this.request<RegistryPage>(`/api/registry/indicators/mine?limit=${limit}&offset=${offset}`);
	}

	/** `GET /api/registry/indicators/{namespace}/{name}`: the full published
	 * entry, source included. */
	async getRegistryIndicator(namespace: string, name: string): Promise<IndicatorEntryDto> {
		return this.request<IndicatorEntryDto>(
			`/api/registry/indicators/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}`
		);
	}

	/** `GET /api/registry/handle`: the caller's own claimed registry handle,
	 * or `null` if they have not chosen one yet — `publishIndicator` 403s
	 * with "choose a registry handle before publishing" until they have. */
	async getRegistryHandle(): Promise<HandleResponse> {
		return this.request<HandleResponse>('/api/registry/handle');
	}

	/** `PUT /api/registry/handle`: claims, or replaces, the caller's own
	 * registry handle — the human-readable address other users type instead
	 * of the caller's raw account id. */
	async setRegistryHandle(handle: string): Promise<void> {
		const body: SetHandleRequest = { handle };
		await this.request<void>('/api/registry/handle', { method: 'PUT', body: JSON.stringify(body) });
	}

	/**
	 * `POST /api/registry/indicators/{namespace}/{name}/install`: fetches
	 * the published source and compiles it right here on this host, the
	 * same "what you read is what you run" guarantee
	 * `registry_handlers.rs`'s own doc describes. No session required — see
	 * that same doc for why install, like search and get, is public.
	 *
	 * This cannot go through `request` above: a successful response body is
	 * the compiled `application/wasm` component's raw bytes, not JSON, so
	 * `safeJson` would misparse it. Everything else — attaching a
	 * credential when one exists, the network/401/403/other-error
	 * classification — mirrors `request` exactly.
	 */
	async installIndicator(namespace: string, name: string): Promise<InstalledIndicator> {
		const server = activeServer();
		const base = resolveBaseUrl(server);
		const token = credentialReader.get(server.id);
		const path = `/api/registry/indicators/${encodeURIComponent(namespace)}/${encodeURIComponent(name)}/install`;
		const headers = new Headers();
		if (token) headers.set('Authorization', `Bearer ${token}`);

		let response: Response;
		try {
			response = await fetch(`${base}${path}`, { method: 'POST', headers });
		} catch (cause) {
			throw new NetworkError(`Could not reach ${base}${path}.`, cause);
		}

		switch (classifyResponse(response)) {
			case 'unauthorized': {
				if (token) this.handleSessionExpired(server.id);
				const body = await safeJson(response);
				throw new UnauthorizedError(errorBodyMessage(body) ?? undefined);
			}
			case 'forbidden': {
				// Same reasoning as `request`'s own 403 branch above: read
				// the server's own message rather than a generic default.
				const body = await safeJson(response);
				throw new ForbiddenError(errorBodyMessage(body) ?? undefined);
			}
			case 'http-error': {
				const body = await safeJson(response);
				throw new HttpError(`Request to ${path} failed with ${response.status}.`, response.status, body);
			}
			case 'no-content':
				throw new HttpError(`Request to ${path} returned no body.`, response.status);
			case 'ok':
				return {
					wasm: await response.arrayBuffer(),
					languageVersion: response.headers.get('X-Indicator-Language-Version') ?? ''
				};
		}
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
			// cleared again by some other path). That is not "session gone"
			// (never a logout point 3), but retrying will also keep
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

/** Narrows an `HttpError.body` from `compileIndicator` into its
 * `CompileIndicatorErrorDto` shape, or `null` if the body is the crate-wide
 * `{error}` shape instead (a registration failure past the compiler, or an
 * unrelated error) — the two are deliberately different shapes so a caller
 * can tell "highlight this line" apart from "show this message" without
 * guessing from field presence alone. */
export function readCompileIndicatorError(body: unknown): CompileIndicatorErrorDto | null {
	if (!body || typeof body !== 'object') return null;
	const candidate = body as Partial<CompileIndicatorErrorDto>;
	return typeof candidate.line === 'number' &&
		typeof candidate.column === 'number' &&
		typeof candidate.message === 'string'
		? (candidate as CompileIndicatorErrorDto)
		: null;
}

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
