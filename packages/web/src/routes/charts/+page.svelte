<script lang="ts">
	// Charts route (the
	// largest of the three pages). Top toolbar (lines 311-425): symbol
	// readout, timeframe strip, indicator/settings menu buttons, replay
	// control, refresh/reset-view, workspace switcher, layout menu. Below it
	// (lines 428-714): the drawing toolbar, the resizable pane grid, and the
	// right panel (trade engine / order book / watchlist / objects / alerts /
	// notes) with its own vertical tab rail. The panel starts closed — a
	// first look at this page is the chart and nothing else.
	//
	// this page now reads and writes real state — workspaces,
	// layouts, panes, layers, bars and indicator values all come from
	// `senken-api` (through `$lib/charts/workspace-store.svelte.ts` and
	// `$lib/charts/bars.ts`), never `$lib/mock/charts.ts` (deleted along
	// with `$lib/indicators/browser-compute.ts` —). What is
	// still local-only, and why, is documented at each such spot below:
	// chart settings (no server field yet) and per-plot style (same). The
	// trade panel is no longer among them — it places real orders through
	// `/api/trade`, against whichever account is active in
	// `$lib/state/trade.svelte`.
	import { onDestroy, onMount, untrack } from 'svelte';
	import { cn } from '$lib/utils.js';
	import {
		TF_LIST,
		type Timeframe,
		type ToolKey,
		LAYOUTS,
		LAYOUT_GROUPS,
		type LayoutId,
		PANEL_TABS,
		type ChartPanelKey,
		CHART_MENUS,
		parseInstrumentId,
		fmtDecimal,
		tfLabel
	} from '$lib/components/terminal/chart-config';
	import { apiClient } from '$lib/api/client';
	import { describeError } from '$lib/api/errors';
	import type { InstrumentSummaryDto } from '$lib/api/types';
	import type { MarketStatus } from '$lib/charts/live-state';
	import { setIndicatorDescriptors, type IndicatorDescriptorClient } from '$lib/charts/layer-style';
	import { ensureSourcesLoaded, hasBarSource, hasQuoteFeed } from '$lib/api/sources.svelte';
	import { drawingsForInstrument, objectTree, type ObjectTreeItem } from '$lib/charts/pane-runtime';
	import {
		chartWorkspaceStore,
		initChartWorkspaces,
		selectWorkspace,
		createNewWorkspace,
		renameWorkspace,
		deleteWorkspace,
		setLayoutPreset,
		setPaneInstrument,
		setAllPanesInstrument,
		setAllPanesTimeframe,
		setPaneTimeframe,
		toggleLayerVisible,
		toggleDrawingVisible,
		swapLayerOrder,
		swapDrawingOrder,
		editLayerParams,
		addInstrumentOverlay,
		addIndicatorLayer,
		removeLayer,
		addDrawing,
		updateDrawingStyle,
		updateDrawingText,
		removeDrawing,
		clearPaneDrawings,
		dismissWorkspaceError,
		persistLayerStyle,
		updateDrawingGeometry,
		updatePaneSettings
	} from '$lib/charts/workspace-store.svelte';
	import type { IndicatorCatalogItem } from '$lib/charts/workspace-store.svelte';
	import type { DrawingRuntime } from '$lib/charts/pane-runtime';
	import { defaultChartSettings, type ChartSettings, type CsSectionKey } from '$lib/mock/chart-settings';
	import ChartContextMenu, { type MenuItem } from '$lib/components/terminal/chart-context-menu.svelte';
	import { commandPalette, openCommand, closeCommand } from '$lib/state/command-palette.svelte';
	import PaneGrid from '$lib/components/terminal/pane-grid.svelte';
	import DrawToolbar from '$lib/components/terminal/draw-toolbar.svelte';
	import OrderTicket from '$lib/components/terminal/order-ticket.svelte';
	import OrderBook from '$lib/components/terminal/order-book.svelte';
	import ObjectTree from '$lib/components/terminal/object-tree.svelte';
	import ChartSettingsDialog from '$lib/components/terminal/chart-settings-dialog.svelte';
	import LayerDialog from '$lib/components/terminal/layer-dialog.svelte';
	import IndicatorPanel from '$lib/components/terminal/indicator-panel.svelte';
	import DateJumpDialog from '$lib/components/terminal/date-jump-dialog.svelte';
	import DrawingToolbar from '$lib/components/terminal/drawing-toolbar.svelte';
	import AlertsPanel from '$lib/components/terminal/alerts-panel.svelte';
	import {
		afterMutation,
		loadTrade,
		refreshPortfolios,
		selectAccount,
		tradeStore,
		watchTrade
	} from '$lib/state/trade.svelte';
	import { iconForKind, isAccountTradable, isWorking, modeOf, formatTime } from '$lib/trade/view';
	import { formatScaledForDisplay } from '$lib/trade/scaled';
	import type { OrderDto, PlaceOrderRequest } from '$lib/api/types';
	import AmendDialog from '$lib/components/trade/amend-dialog.svelte';
	import WatchlistTab from '$lib/components/terminal/watchlist-tab.svelte';
	import NotesTab from '$lib/components/terminal/notes-tab.svelte';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { syncTargets } from '$lib/charts/chart-sync';
	import { shouldResetActivePaneView } from '$lib/charts/shortcuts';
	import type { IChartApi, LogicalRange, MouseEventParams } from 'lightweight-charts';
	import SearchIcon from '@lucide/svelte/icons/search';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import Columns3Icon from '@lucide/svelte/icons/columns-3';
	import Maximize2Icon from '@lucide/svelte/icons/maximize-2';
	import PlayIcon from '@lucide/svelte/icons/play';
	import PauseIcon from '@lucide/svelte/icons/pause';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import ScanIcon from '@lucide/svelte/icons/scan';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';
	import CoinsIcon from '@lucide/svelte/icons/coins';
	import SigmaIcon from '@lucide/svelte/icons/sigma';
	import { toast } from 'svelte-sonner';

	let indicatorCatalog = $state<IndicatorDescriptorClient[]>([]);

	onMount(() => {
		// The instrument pickers below refuse a source that cannot serve
		// bars, so the capability response has to be on its way before the
		// first picker opens — the panes warm the same shared cache, but a
		// reader can reach a picker before any pane has mounted.
		void ensureSourcesLoaded();
		void initChartWorkspaces();
		void apiClient.listIndicators().then((items) => {
			indicatorCatalog = items as unknown as IndicatorDescriptorClient[];
			setIndicatorDescriptors(indicatorCatalog);
		});
		// The trade panel needs the adapter catalogue before it can draw a
		// ticket at all — which controls exist is read from the active
		// account's adapter, not assumed. Every account's portfolio (not
		// just this one) is fetched here too: `tradeStore.portfolios` is
		// shared with the engine page, so this page's own order list is
		// simply derived from it below rather than fetched separately.
		void loadTrade().then(() => refreshPortfolios());
		// 'active': this panel shows one account's ticket and its orders, and
		// a trader sits on this page for hours. Refreshing the other nine
		// accounts from here would be four venue requests each, every five
		// seconds, for data nothing on screen is showing.
		return watchTrade('active');
	});

	const layout = $derived(chartWorkspaceStore.layout);
	const panes = $derived(layout?.panes ?? []);

	// A rejected mutation on an already-open layout used to render an inline
	// banner right here — now routed to the app's one toaster instead, so a
	// save failure reads the same way as any other transient problem rather
	// than growing a second place errors show up. Only once a layout has
	// actually loaded: before that, `chartWorkspaceStore.error` means
	// "nothing to show at all" and the full-page fallback below reads it
	// directly, which needs the value left in place, not dismissed here.
	$effect(() => {
		const message = chartWorkspaceStore.error;
		if (!message || !layout) return;
		toast.error(message);
		dismissWorkspaceError();
	});

	let activeIndex = $state(0);
	const activePaneIndex = $derived(panes.length === 0 ? 0 : Math.min(activeIndex, panes.length - 1));
	const activePane = $derived(panes[activePaneIndex]);
	// Search results are the catalog status source. The server supplies this
	// field from its cached venue catalog; keep the short-lived browser cache
	// separate from persisted pane settings because a venue's state is market
	// data, never a user preference. A key's presence means this instrument
	// has been resolved at all — `pickSymbol` and the effect below both write
	// `{ status: 'trading' }`, not just `{ status: 'closed' }`, so that
	// presence is enough to mean "already resolved" without a second map to
	// track it (`deriveLiveState` only ever looks at `status === 'closed'`,
	// so recording the non-closed case changes nothing it reads).
	let marketStatuses = $state<Record<string, MarketStatus>>({});
	let marketStatusToken = 0;

	// A pane loaded from a saved workspace — the normal case on every page
	// load — never went through `pickSymbol`, so nothing has ever populated
	// `marketStatuses` for its instrument, and its market-closed state never
	// reaches the chart even when the catalog knows it. This resolves every
	// pane's instrument the same way `pickSymbol` does, whenever the set of
	// panes changes, skipping anything already resolved. `panes` is read
	// outside `untrack` (it is what this effect must react to); the cache
	// read/write is inside `untrack` for the same reason the WATCH tab's
	// price effect uses it below — this effect's own writes to
	// `marketStatuses` must not make it re-run against itself.
	$effect(() => {
		const currentPanes = panes;
		const token = ++marketStatusToken;
		untrack(() => {
			const instrumentIds = Array.from(
				new Set(currentPanes.map((p) => p.instrument).filter((id) => !(id in marketStatuses)))
			);
			if (instrumentIds.length === 0) return;
			void Promise.all(
				instrumentIds.map(async (id) => {
					try {
						// `id` is already `source:symbol`, the same grammar
						// `searchInstruments` accepts to narrow to one venue —
						// reused as the query rather than re-deriving it, then
						// matched by exact id since a narrowed search can still
						// return more than one row.
						const page = await apiClient.searchInstruments(id, 5);
						const row = page.rows.find((r) => r.id === id);
						if (!row) return null;
						const status: MarketStatus = row.status === 'closed' ? { status: 'closed' } : { status: 'trading' };
						return [id, status] as const;
					} catch {
						// Leaves the pane exactly as unresolved as it was — no
						// entry is written, so it behaves as it does today
						// (and is retried next time the pane set changes).
						return null;
					}
				})
			).then((results) => {
				if (token !== marketStatusToken) return;
				const resolved = results.filter((r): r is readonly [string, MarketStatus] => r !== null);
				if (resolved.length === 0) return;
				marketStatuses = { ...marketStatuses, ...Object.fromEntries(resolved) };
			});
		});
	});

	let tool = $state<ToolKey>('cursor');
	let clearToken = $state(0);

	// Closed by default: a first look at this page is the chart and nothing
	// else. Clicking a rail tab opens that panel, same as it always has.
	let panelOpen = $state(false);
	let chartPanel = $state<ChartPanelKey>('engine');

	// The trade panel's own UI state. The order list itself is not here —
	// it is derived from `tradeStore.portfolios` below (see `tradeAccount`'s
	// neighbouring declarations), the shared copy the engine page also
	// reads, so a fill or a cancel neither screen caused still shows up
	// here without a local list of this page's own to go stale.
	let placing = $state(false);
	let orderError = $state<string | null>(null);

	let wsMenuOpen = $state(false);
	let layoutMenuOpen = $state(false);
	// No server field yet, so this does not persist — plain cosmetic UI
	// state, page-local like the rest of this stage's additions. `interval`
	// defaults on (not off, like its siblings) because that already was this
	// page's real behaviour before the toggle did anything at all — every
	// pane's timeframe changed together, unconditionally.
	let sync = $state({ symbol: false, interval: true, crosshair: true, time: false });
	let newWorkspaceName = $state('');
	let renamingWorkspaceId = $state<string | null>(null);
	let renameDraft = $state('');

	// Overlay state — local to this page.
	// Each pane now persists its own chart settings server-side
	// (`PaneDto.settings`); the dialog edits whichever pane is active, so
	// this reads that pane's own settings rather than one page-level object
	// every pane used to share (and none of them actually kept).
	const chartSettings = $derived(activePane?.settings ?? defaultChartSettings());
	const activePaneQuotesSupported = $derived(hasQuoteFeed(activePane?.instrument ?? ''));
	let csOpen = $state(false);
	/** Which settings section the dialog should open on. The price-scale
	 * menu's "More settings" points straight at scales. */
	let csSection = $state<CsSectionKey>('symbol');
	/** The open context menu, if any: where it was raised, which pane, and
	 * which part of that pane. */
	let ctxMenu = $state<{
		x: number;
		y: number;
		pane: number;
		region: 'chart' | 'price-scale' | 'time-scale';
	} | null>(null);
	/** One reload counter per pane, exactly like `resetTokens` below: the
	 * context menu's "REFRESH CHART DATA" bumps only the pane it was raised
	 * on, and the toolbar's reload button bumps every pane at once. */
	let reloadTokens = $state<number[]>([]);
	/** One counter per pane: resetting pane 2's view must not disturb the
	 * frame the user set on pane 1. */
	let resetTokens = $state<number[]>([]);

	function reloadPane(index: number) {
		const next = [...reloadTokens];
		next[index] = (next[index] ?? 0) + 1;
		reloadTokens = next;
	}

	function reloadAllPanes() {
		reloadTokens = panes.map((_, i) => (reloadTokens[i] ?? 0) + 1);
	}

	function resetPaneView(index: number) {
		const next = [...resetTokens];
		next[index] = (next[index] ?? 0) + 1;
		resetTokens = next;
	}
	/** One "go to date" request per pane — same reasoning as `resetTokens`:
	 * resetting/jumping pane 2 must not touch pane 1's own frame. */
	let jumpTargets = $state<({ token: number; targetNanos: number } | null)[]>([]);
	/** Which pane the open "go to date" dialog targets, or `null` while it is
	 * closed. */
	let dateJumpDlg = $state<{ pane: number } | null>(null);

	function jumpPaneToDate(index: number, targetNanos: number) {
		const next = [...jumpTargets];
		const previousToken = next[index]?.token ?? 0;
		next[index] = { token: previousToken + 1, targetNanos };
		jumpTargets = next;
	}

	// --- Cross-pane sync (layout menu's SYMBOL/INTERVAL/CROSSHAIR/TIME) -----
	//
	// SYMBOL and INTERVAL are plain branches in `pickSymbol`/`pickTf` above —
	// each already had an all-panes and a single-pane store call to choose
	// between. CROSSHAIR and TIME have no server-side notion at all: they
	// only mean anything against a live `IChartApi`, one per pane, reported
	// up through `onChartApi` (`pane-cell.svelte`/`pane-grid.svelte` forward
	// `chart-pane.svelte`'s own prop of that name).
	/** One `IChartApi` per pane, or `undefined` where the pane has not
	 * mounted a chart yet (or has just torn one down). */
	let chartApis = $state<(IChartApi | undefined)[]>([]);

	function onChartApiFromPane(paneIndex: number, api: IChartApi | undefined) {
		const next = [...chartApis];
		next[paneIndex] = api;
		chartApis = next;
	}

	/** TIME sync: panning or zooming one pane's time scale applies the same
	 * logical range to every other pane. Re-subscribes whenever the toggle
	 * flips or the set of live chart APIs changes, and tears every
	 * subscription down on either — the effect's own return is the only
	 * place any of this unsubscribes.
	 *
	 * The re-entrancy guard has to come first: applying a range to pane B
	 * fires B's own change handler, which would apply it straight back to
	 * pane A (and everyone else) without one — an infinite synchronous loop
	 * between two panes, not merely a redundant extra call. */
	$effect(() => {
		if (!sync.time) return;
		const apis = chartApis;
		let applying = false;
		const subs: { api: IChartApi; handler: (range: LogicalRange | null) => void }[] = [];
		apis.forEach((api, sourceIndex) => {
			if (!api) return;
			const handler = (range: LogicalRange | null) => {
				if (applying || range === null) return;
				applying = true;
				try {
					for (const targetIndex of syncTargets(sourceIndex, apis.length)) {
						apis[targetIndex]?.timeScale().setVisibleLogicalRange(range);
					}
				} finally {
					applying = false;
				}
			};
			api.timeScale().subscribeVisibleLogicalRangeChange(handler);
			subs.push({ api, handler });
		});
		return () => {
			for (const { api, handler } of subs) api.timeScale().unsubscribeVisibleLogicalRangeChange(handler);
		};
	});

	/** CROSSHAIR sync: the hovered pane's crosshair draws in every other
	 * pane at the same time and the same vertical position (each target
	 * converts that position through its own price scale, since panes can
	 * show differently priced instruments). Same re-entrancy guard as TIME
	 * sync, for the same reason — `setCrosshairPosition` on pane B must not
	 * re-trigger a fan-out from B. */
	$effect(() => {
		if (!sync.crosshair) return;
		const apis = chartApis;
		let applying = false;
		const subs: { api: IChartApi; handler: (param: MouseEventParams) => void }[] = [];
		apis.forEach((api, sourceIndex) => {
			if (!api) return;
			const handler = (param: MouseEventParams) => {
				if (applying) return;
				applying = true;
				try {
					for (const targetIndex of syncTargets(sourceIndex, apis.length)) {
						const target = apis[targetIndex];
						if (!target) continue;
						const series = target.panes()[0]?.getSeries()[0];
						const price = param.point && series ? series.coordinateToPrice(param.point.y) : null;
						if (!series || param.time === undefined || price === null) {
							target.clearCrosshairPosition();
							continue;
						}
						target.setCrosshairPosition(price, param.time, series);
					}
				} finally {
					applying = false;
				}
			};
			api.subscribeCrosshairMove(handler);
			subs.push({ api, handler });
		});
		return () => {
			for (const { api, handler } of subs) api.unsubscribeCrosshairMove(handler);
		};
	});

	/** The window-level `Alt+R` handler behind the context menu's own
	 * "RESET CHART VIEW · ALT R" hint — `shouldResetActivePaneView` decides
	 * whether this particular keydown should fire it at all (typing in a
	 * field, or a dialog open, must not have the chart jump underneath). */
	function handleGlobalKeydown(e: KeyboardEvent) {
		const dialogOpen = csOpen || !!layerDlg || !!dateJumpDlg;
		if (!shouldResetActivePaneView(e, e.target as { tagName?: string; isContentEditable?: boolean } | null, dialogOpen)) return;
		e.preventDefault();
		resetPaneView(activePaneIndex);
	}

	/** Which layer's settings are open, tracked by its *position* rather
	 * than its id: saving a layout replaces every layer row server-side, so
	 * the ids change on each write and a dialog keyed on one would close
	 * itself the moment the user edited anything. */
	let layerDlg = $state<{ pane: number; position: number } | null>(null);
	let addMenuKind = $state<'instrument' | 'indicator'>('indicator');
	/** Which pane the indicator-authoring panel is placing onto, or `null`
	 * while it is closed. */
	let indicatorPanelPane = $state<number | null>(null);

	// Drawing objects: which one is selected, if any — set by
	// `chart-pane.svelte`'s cursor-tool click handler, cleared by closing the
	// properties dialog or clicking empty chart space.
	let selectedDrawing = $state<{ pane: number; id: string } | null>(null);

	function handleCreateDrawing(paneIndex: number, drawing: Omit<DrawingRuntime, 'id' | 'position'>) {
		void addDrawing(paneIndex, drawing);
	}
	function handleSelectDrawing(paneIndex: number, id: string | null) {
		selectedDrawing = id ? { pane: paneIndex, id } : null;
	}
	function handleToolConsumed() {
		tool = 'cursor';
	}
	function clearDrawings() {
		selectedDrawing = null;
		clearToken++;
		void clearPaneDrawings(activePaneIndex);
	}

	// The last real close per pane (`chart-pane.svelte`'s `onLastClose`).
	// The order ticket uses it to prefill a limit price and the depth panel
	// to centre its ladder; neither treats it as a price to trade at — what
	// trades is what the user leaves in the box, and the server marks a
	// market order from its own price source.
	let lastClose = $state<Record<number, number>>({});

	// `defaultBarWindow`'s own default bar count (`chart-config.ts`) — kept
	// in sync with it the same way the former mock's replay logic was
	// tightly coupled to `genCandles`'s fixed 320-bar length.
	const REPLAY_BAR_COUNT = 300;
	let replaying = $state(false);
	let replayIdx = $state(0);
	let replayTimer: ReturnType<typeof setInterval> | undefined;
	const replayPct = $derived(Math.round(Math.min(100, (replayIdx / REPLAY_BAR_COUNT) * 100)) + '%');
	const replayCut = $derived(replaying || replayIdx > 0 ? replayIdx : null);

	onDestroy(() => clearInterval(replayTimer));

	function toggleReplay() {
		if (replaying) {
			clearInterval(replayTimer);
			replaying = false;
			return;
		}
		replaying = true;
		if (replayIdx === 0) replayIdx = 60;
		replayTimer = setInterval(() => {
			const next = replayIdx + 1;
			if (next >= REPLAY_BAR_COUNT) {
				clearInterval(replayTimer);
				replayIdx = REPLAY_BAR_COUNT;
				replaying = false;
				return;
			}
			replayIdx = next;
		}, 220);
	}
	function resetReplay() {
		clearInterval(replayTimer);
		replaying = false;
		replayIdx = 0;
	}

	function onFocusPane(i: number) {
		activeIndex = i;
	}

	function pickTf(t: Timeframe) {
		if (sync.interval) void setAllPanesTimeframe(t);
		else void setPaneTimeframe(activePaneIndex, t);
	}

	/** The pane-targeting core every symbol picker shares: SYMBOL sync decides
	 * whether the active pane alone, or every pane, gets the new instrument.
	 * Split out of `pickSymbol` so the WATCHLIST tab — whose rows are plain
	 * instrument ids, not full `InstrumentSummaryDto` search hits — can reach
	 * the same targeting logic without a status field it does not have. */
	function pickPaneInstrument(instrument: string) {
		if (sync.symbol) void setAllPanesInstrument(instrument);
		else void setPaneInstrument(activePaneIndex, instrument);
	}

	function pickSymbol(instrument: InstrumentSummaryDto) {
		// Recorded either way — see `marketStatuses`' own doc above on why a
		// resolved-but-trading instrument is written too, not just closed ones.
		marketStatuses = {
			...marketStatuses,
			[instrument.id]: instrument.status === 'closed' ? { status: 'closed' } : { status: 'trading' }
		};
		pickPaneInstrument(instrument.id);
	}

	function onLastCloseFromPane(paneIndex: number, price: number) {
		lastClose = { ...lastClose, [paneIndex]: price };
	}

	function setLayout(id: LayoutId) {
		void setLayoutPreset(id);
		layoutMenuOpen = false;
	}

	function pickWorkspace(id: string) {
		void selectWorkspace(id);
		activeIndex = 0;
		wsMenuOpen = false;
	}

	function submitNewWorkspace(event: SubmitEvent) {
		event.preventDefault();
		const name = newWorkspaceName.trim();
		if (!name) return;
		void createNewWorkspace(name);
		newWorkspaceName = '';
		wsMenuOpen = false;
	}

	function startRename(id: string, currentName: string) {
		renamingWorkspaceId = id;
		renameDraft = currentName;
	}

	function submitRename(event: SubmitEvent) {
		event.preventDefault();
		const id = renamingWorkspaceId;
		const name = renameDraft.trim();
		renamingWorkspaceId = null;
		if (!id || !name) return;
		void renameWorkspace(id, name);
	}

	function confirmDeleteWorkspace(id: string) {
		if (chartWorkspaceStore.workspaces.length <= 1) return;
		void deleteWorkspace(id);
	}

	function toggleFullscreen() {
		try {
			if (document.fullscreenElement) document.exitFullscreen();
			else document.documentElement.requestFullscreen();
		} catch {
			// fullscreen isn't available in every embedding context
		}
	}

	const tradeAccount = $derived(tradeStore.active ?? null);
	const tradeAdapter = $derived(
		tradeAccount ? (tradeStore.adapter(tradeAccount.adapter_id) ?? null) : null
	);
	const tradeAccess = $derived(tradeAccount ? (tradeStore.access[tradeAccount.id] ?? null) : null);
	const tradeAccountTradable = $derived(
		tradeAccount ? isAccountTradable(tradeAccount, tradeStore.access) : false
	);
	/** Whatever the account's adapter last reported, read out of the shared
	 * store rather than a list this page appends to itself — a resting
	 * order that filled while nobody was looking on this page still shows
	 * as filled, because it is the same portfolio the engine page saw fill
	 * it. */
	const orders = $derived(tradeAccount ? (tradeStore.portfolios[tradeAccount.id]?.orders ?? []) : []);
	/** `true` while the active account's portfolio has never been fetched —
	 * distinct from a fetch that came back with genuinely no orders, so the
	 * panel does not claim "no orders" before it has actually asked. */
	const ordersLoading = $derived(
		tradeAccount ? tradeStore.portfolios[tradeAccount.id] === undefined : false
	);

	let amendDialogOpen = $state(false);
	let amendDialogOrder = $state<OrderDto | null>(null);

	function openAmend(order: OrderDto) {
		amendDialogOrder = order;
		amendDialogOpen = true;
	}

	async function placeOrder(request: PlaceOrderRequest) {
		const account = tradeStore.active;
		if (!account) return;
		placing = true;
		orderError = null;
		try {
			await apiClient.placeOrder(account.id, request);
			await afterMutation(account.id);
		} catch (error) {
			orderError = describeError(error);
		} finally {
			placing = false;
		}
	}

	async function cancelOrder(orderId: string) {
		const account = tradeStore.active;
		if (!account) return;
		orderError = null;
		try {
			await apiClient.cancelOrder(account.id, orderId);
			await afterMutation(account.id);
		} catch (error) {
			orderError = describeError(error);
		}
	}

	function openTradeAccountPicker() {
		openCommand({
			mode: 'account',
			placeholder: 'Search accounts…',
			footer: 'ACTIVE TRADING ACCOUNT',
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return tradeStore.accounts
					.filter((account) => !q || account.label.toLowerCase().includes(q))
					.map((account) => {
						const adapter = tradeStore.adapter(account.adapter_id);
						return {
							icon: iconForKind(adapter?.kind ?? 'exchange'),
							title: account.label,
							sub: `${adapter?.name ?? account.adapter_id} · ${modeOf(adapter)}`,
							meta: account.owned ? modeOf(adapter) : 'NOT YOURS',
							metaTone: adapter?.trades_real_money ? 'loss' : 'gain',
							onPick: () => {
								// No fetch needed here: `tradeStore.portfolios` already
								// holds every account's orders (the shared refresh
								// covers all of them), so switching the active account
								// just changes which key `orders` above reads.
								selectAccount(account.id);
								closeCommand();
							}
						};
					});
			}
		});
	}

	// Real instrument search, replacing the former fixed
	// `INSTRUMENT_CATALOG` for both pickers below: `apiClient.searchInstruments`
	// re-runs on every keystroke into the command palette while it is open in
	// a mode that names instruments (`'symbol'`, or `'layer'`'s own
	// INSTRUMENT tab) — an empty query already ranks and returns real hits
	// (`instrument_handlers.rs`'s own "an empty query lists something" test),
	// so the picker opens with real rows even before the user types anything.
	let instrumentResults = $state<InstrumentSummaryDto[]>([]);
	// The first open of a picker has no rows yet and a request in flight.
	// Without this the palette renders its "NO MATCH" empty state over a
	// catalogue it has not finished asking for — which is why the instrument
	// list looked empty until it was closed and reopened, the second open
	// finding `instrumentResults` still populated from the first.
	let instrumentSearchBusy = $state(false);
	let instrumentSearchToken = 0;
	$effect(() => {
		const request = commandPalette.request;
		const wantsInstruments =
			commandPalette.open && !!request && (request.mode === 'symbol' || (request.mode === 'layer' && addMenuKind === 'instrument'));
		if (!wantsInstruments) return;
		const query = commandPalette.query;
		const token = ++instrumentSearchToken;
		instrumentSearchBusy = true;
		apiClient
			.searchInstruments(query, 20)
			.then((page) => {
				if (token === instrumentSearchToken) instrumentResults = page.rows;
			})
			.catch(() => {
				if (token === instrumentSearchToken) instrumentResults = [];
			})
			.finally(() => {
				if (token === instrumentSearchToken) instrumentSearchBusy = false;
			});
	});

	/** One instrument row for a picker.
	 *
	 * A source with no bar source registered cannot chart at all — picking
	 * one leaves the pane blank while `GET /api/bars/plan` refuses the
	 * request, which is what the catalogue's ~47 registered sources make
	 * easy to do by accident. The row stays in the list, because dropping it
	 * would read as "no such instrument"; it is simply not selectable, and
	 * says why. `undefined` (the capability response has not arrived yet) is
	 * never treated as "no": nothing is disabled on an answer we have not
	 * been given. */
	function instrumentRow(x: InstrumentSummaryDto, meta: string, onPick: () => void) {
		const chartable = hasBarSource(x.id) !== false;
		return {
			icon: CoinsIcon,
			title: `${x.source_id.toUpperCase()}:${x.symbol}`,
			sub: x.name,
			meta: chartable ? meta : 'NO BAR DATA',
			metaTone: 'dim' as const,
			disabled: !chartable,
			onPick
		};
	}

	// reference: the charts toolbar's symbol readout, `toggleSymbolPicker`
	// (line 3253) — opens the command palette in `'symbol'` mode.
	function openSymbolPicker() {
		openCommand({
			mode: 'symbol',
			placeholder: 'Search instruments by ticker or venue…',
			footer: `MAIN INSTRUMENT · PANE ${activePaneIndex + 1}`,
			busy: () => instrumentSearchBusy,
			rows: () =>
				instrumentResults.map((x) =>
					instrumentRow(x, x.kind.toUpperCase(), () => {
						pickSymbol(x);
						closeCommand();
					})
				)
		});
	}

	// reference: the toolbar's "INDICATORS & LAYERS" button (line 3275) —
	// opens the command palette in `'layer'` mode. the
	// INSTRUMENT tab adds an overlay instrument (`addInstrumentOverlay`),
	// the INDICATOR tab adds a real indicator layer, placed overlay or
	// The API descriptor selects an allowed placement. There is no
	// STRATEGY tab any more — `senken_workspace::LayerKind` has no such
	// variant.
	function openLayerPicker() {
		const paneTarget = activePaneIndex;
		openCommand({
			mode: 'layer',
			placeholder: 'Search instruments or indicators…',
			footer: `ADDING TO PANE ${paneTarget + 1}`,
			busy: () => addMenuKind === 'instrument' && instrumentSearchBusy,
			kindTabs: [
				...(['instrument', 'indicator'] as const).map((k) => ({
					label: k.toUpperCase(),
					active: () => addMenuKind === k,
					onClick: () => (addMenuKind = k)
				})),
				// Not a third `addMenuKind` state: picking it opens the
				// indicator-authoring panel directly (its own dialog, not
				// another row of this palette) and leaves `addMenuKind` alone.
				{
					label: 'CUSTOM',
					active: () => false,
					onClick: () => {
						closeCommand();
						indicatorPanelPane = paneTarget;
					}
				}
			],
			rows: (query) => {
				if (addMenuKind === 'instrument') {
					return instrumentResults.map((x) =>
						instrumentRow(x, 'OVERLAY', () => {
							void addInstrumentOverlay(paneTarget, x.id);
							closeCommand();
						})
					);
				}
				const q = query.trim().toLowerCase();
				return indicatorCatalog.filter((def) => !q || `${def.title} ${def.legend}`.toLowerCase().includes(q)).map((def) => ({
					icon: SigmaIcon,
					title: def.short_title,
					sub: def.legend,
					meta: def.placement === 'sub_pane' ? 'SUB-PANE' : 'OVERLAY',
					metaTone: 'dim' as const,
					onPick: () => {
						const item: IndicatorCatalogItem = {
							name: def.name,
							defaultParams: Object.fromEntries(def.params.map((param) => [param.name, param.default.value])),
							placement: def.placement
						};
						void addIndicatorLayer(paneTarget, item);
						closeCommand();
					}
				}));
			}
		});
	}

	// reference: a layer chip's "OPTIONS…" (line 2431) — the main
	// instrument's settings live in chart settings; every other layer opens
	// the layer dialog.
	function openMainSettings(paneIndex: number) {
		activeIndex = paneIndex;
		csOpen = true;
	}
	function openLayerSettings(paneIndex: number, layerId: string) {
		const found = panes[paneIndex]?.layers.find((l) => l.id === layerId);
		if (!found) return;
		layerDlg = { pane: paneIndex, position: found.position };
	}

	const activePanel = $derived(PANEL_TABS.find((p) => p.key === chartPanel) ?? PANEL_TABS[0]);
	/** The drawings the active pane is actually showing — the ones made on
	 * whatever instrument it currently displays. The pane keeps the rest,
	 * and switching back to that instrument brings them straight back, but a
	 * tree listing objects the chart is not drawing would be a list of
	 * things the reader cannot see or click. */
	const activePaneDrawings = $derived(
		activePane ? drawingsForInstrument(activePane.drawings, activePane.instrument) : []
	);
	const treeGroups = $derived(objectTree(activePane?.layers ?? [], activePaneDrawings));
	/** Only a *drawing* selection ever marks a row in the object tree — see
	 * `object-tree.svelte`'s own `selected` check — and only while that
	 * selection belongs to the pane the tree is currently showing. */
	const treeSelectedId = $derived(selectedDrawing?.pane === activePaneIndex ? selectedDrawing.id : null);

	/** Clicking a row selects it. Only a drawing has anything for "selected"
	 * to mean on this page (it opens the floating `DrawingToolbar` — see
	 * `handleSelectDrawing`); clicking a layer row clears any drawing
	 * selection instead of leaving a stale one pointing at a different row. */
	function handleTreeSelect(item: ObjectTreeItem) {
		handleSelectDrawing(activePaneIndex, item.kind === 'drawing' ? item.id : null);
	}
	function handleTreeToggle(item: ObjectTreeItem) {
		if (item.kind === 'layer') void toggleLayerVisible(activePaneIndex, item.id);
		else void toggleDrawingVisible(activePaneIndex, item.id);
	}
	/** A layer's options open `LayerDialog`; a drawing has no separate
	 * dialog — its properties are the floating `DrawingToolbar` that
	 * selecting it already opens, so "options" on a drawing just selects it. */
	function handleTreeOptions(item: ObjectTreeItem) {
		if (item.kind === 'layer') openLayerSettings(activePaneIndex, item.id);
		else handleSelectDrawing(activePaneIndex, item.id);
	}
	function handleTreeRemove(item: ObjectTreeItem) {
		if (item.kind === 'layer') void removeLayer(activePaneIndex, item.id);
		else void removeDrawing(activePaneIndex, item.id);
	}
	function handleTreeMove(item: ObjectTreeItem, neighborId: string) {
		if (item.kind === 'layer') void swapLayerOrder(activePaneIndex, item.id, neighborId);
		else void swapDrawingOrder(activePaneIndex, item.id, neighborId);
	}
	const dialogDrawing = $derived.by(() => {
		const sel = selectedDrawing;
		if (!sel) return null;
		const pane = panes[sel.pane];
		if (!pane) return null;
		// Only among the drawings that pane is showing: switching its
		// instrument hides the others, and a properties bar floating over a
		// chart that is no longer drawing the object it edits is a control
		// pointed at nothing.
		return drawingsForInstrument(pane.drawings, pane.instrument).find((d) => d.id === sel.id) ?? null;
	});
	/** Where the floating toolbar sits. Anchored to the top of the chart
	 * area rather than to the object itself: a bar that follows the drawing
	 * ends up under the pointer mid-drag, which is the one moment it must
	 * not be in the way. */
	/** Where the floating toolbar sits inside the chart area: near the top,
	 * clear of the drawing rail on the left and the price scale on the
	 * right, and above the object rather than under the pointer that is
	 * dragging it. */
	const drawingToolbarAnchor = { x: 420, y: 54 };

	const mid = $derived(lastClose[activePaneIndex] ?? 0);

	const syncToggleKeys = ['symbol', 'interval', 'crosshair', 'time'] as const;

	const dialogLayer = $derived.by(() => {
		const dlg = layerDlg;
		if (!dlg) return null;
		return panes[dlg.pane]?.layers.find((l) => l.position === dlg.position) ?? null;
	});
	/** Patches the settings of whichever pane was right-clicked, so a menu
	 * raised on pane 2 does not silently edit pane 1. */
	function patchPane(index: number, patch: Partial<ChartSettings>) {
		void updatePaneSettings(index, patch);
	}

	function contextMenuItems(menu: NonNullable<typeof ctxMenu>): MenuItem[] {
		const cs = panes[menu.pane]?.settings ?? defaultChartSettings();
		if (menu.region === 'price-scale') {
			const mode = (m: ChartSettings['priceScaleMode'], label: string): MenuItem => ({
				kind: 'toggle',
				label,
				checked: cs.priceScaleMode === m,
				onSelect: () => patchPane(menu.pane, { priceScaleMode: m })
			});
			return [
				{
					kind: 'toggle',
					label: 'AUTO (FITS DATA TO SCREEN)',
					checked: cs.autoScale,
					onSelect: () => patchPane(menu.pane, { autoScale: !cs.autoScale })
				},
				{
					kind: 'toggle',
					label: 'INVERT SCALE',
					checked: cs.invertScale,
					onSelect: () => patchPane(menu.pane, { invertScale: !cs.invertScale })
				},
				{ kind: 'separator' },
				mode('REGULAR', 'REGULAR'),
				mode('PERCENT', 'PERCENT'),
				mode('INDEXED', 'INDEXED TO 100'),
				mode('LOGARITHMIC', 'LOGARITHMIC'),
				{ kind: 'separator' },
				{
					kind: 'action',
					label: cs.placement === 'LEFT' ? 'MOVE SCALE TO RIGHT' : 'MOVE SCALE TO LEFT',
					onSelect: () =>
						patchPane(menu.pane, { placement: cs.placement === 'LEFT' ? 'RIGHT' : 'LEFT' })
				},
				{
					kind: 'toggle',
					label: 'COUNTDOWN TO BAR CLOSE',
					checked: cs.countdown,
					onSelect: () => patchPane(menu.pane, { countdown: !cs.countdown })
				},
				{ kind: 'separator' },
				{
					kind: 'action',
					label: 'MORE SETTINGS…',
					onSelect: () => {
						onFocusPane(menu.pane);
						csSection = 'scales';
						csOpen = true;
					}
				}
			];
		}
		return [
			{ kind: 'action', label: 'RESET CHART VIEW', hint: 'ALT R', onSelect: () => resetPaneView(menu.pane) },
			{ kind: 'action', label: 'REFRESH CHART DATA', onSelect: () => reloadPane(menu.pane) },
			{ kind: 'action', label: 'GO TO DATE…', onSelect: () => (dateJumpDlg = { pane: menu.pane }) },
			{ kind: 'separator' },
			{
				kind: 'toggle',
				label: 'HIGH AND LOW LINES',
				checked: cs.highLow,
				onSelect: () => patchPane(menu.pane, { highLow: !cs.highLow })
			},
			{
				kind: 'toggle',
				label: 'WATERMARK',
				checked: cs.watermark === 'VISIBLE',
				onSelect: () =>
					patchPane(menu.pane, { watermark: cs.watermark === 'VISIBLE' ? 'HIDDEN' : 'VISIBLE' })
			},
			{ kind: 'separator' },
			{
				kind: 'action',
				label: 'SETTINGS…',
				onSelect: () => {
					onFocusPane(menu.pane);
					csSection = 'symbol';
					csOpen = true;
				}
			}
		];
	}

	const settingsScopeLabel = $derived(
		`PANE ${activePaneIndex + 1} · ${activePane ? parseInstrumentId(activePane.instrument).ticker : ''}`
	);
</script>

<svelte:window onkeydown={handleGlobalKeydown} />

{#snippet layoutIcon(id: LayoutId, active: boolean)}
	{@const L = LAYOUTS[id]}
	<div
		class={cn('grid gap-px', active ? '[&>div]:border-foreground' : '[&>div]:border-dim2')}
		style="width: 15px; height: 12px; grid-template-columns: {L.cols}; grid-template-rows: {L.rows};"
	>
		{#each Array.from({ length: L.n }) as _, i (i)}
			<div class="border"></div>
		{/each}
	</div>
{/snippet}

{#if chartWorkspaceStore.loading}
	<div class="flex min-h-0 flex-1 items-center justify-center">
		<span class="font-mono text-[10px] tracking-[0.2em] text-dim">LOADING WORKSPACE…</span>
	</div>
{:else if layout}
	<div class="flex min-h-0 flex-1 flex-col">
		<!-- Top toolbar (lines 311-425) -->
		<div class="flex h-11 flex-none items-center gap-2.5 border-b border-border bg-chrome px-2.5">
			<button
				type="button"
				class="flex h-[27px] flex-none cursor-pointer items-center gap-2.5 border border-ink/18 px-2.5"
				onclick={openSymbolPicker}
			>
				<SearchIcon class="size-3 text-dim" />
				<span class="font-mono text-xs font-medium tracking-[0.06em] text-dim2">
					{parseInstrumentId(activePane?.instrument ?? '').venue}:<span class="text-foreground">{parseInstrumentId(activePane?.instrument ?? '').ticker}</span>
				</span>
				<ChevronDownIcon class="size-[11px] text-dim2" />
			</button>

			<div class="flex flex-none items-center gap-px border border-ink/10">
				{#each TF_LIST as t (t)}
					<button
						type="button"
						class={cn(
							'flex h-[25px] min-w-[34px] cursor-pointer items-center justify-center font-mono text-[10px] tracking-[0.1em]',
							activePane?.timeframe === t ? 'bg-foreground text-inv' : 'text-dim2'
						)}
						onclick={() => pickTf(t)}
					>
						{tfLabel(t)}
					</button>
				{/each}
			</div>

			<div class="h-5 w-px flex-none bg-ink/10"></div>

			<Tooltip.Provider>
				<div class="flex flex-none items-center gap-1.5">
					{#each CHART_MENUS as m (m.key)}
						<Tooltip.Root>
							<Tooltip.Trigger
								class="flex h-[25px] min-w-7 cursor-pointer items-center justify-center border border-transparent px-2 text-dim2"
								aria-label={m.label}
								onclick={() => (m.key === 'ind' ? openLayerPicker() : (csOpen = true))}
							>
								<m.icon class="size-[15px]" />
							</Tooltip.Trigger>
							<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
								<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">{m.label}</span>
							</Tooltip.Content>
						</Tooltip.Root>
					{/each}
				</div>

				<div class="flex-1"></div>

				<div class="flex flex-none items-center gap-1.5">
					<Tooltip.Root>
						<Tooltip.Trigger
							class={cn(
								'flex h-[27px] w-[30px] cursor-pointer items-center justify-center border',
								replaying ? 'border-foreground bg-foreground text-inv' : 'border-dim text-secondary-foreground'
							)}
							onclick={toggleReplay}
						>
							{#if replaying}
								<PauseIcon class="size-[14px]" />
							{:else}
								<PlayIcon class="size-[14px]" />
							{/if}
						</Tooltip.Trigger>
						<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
							<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">BAR REPLAY {replayPct}</span>
						</Tooltip.Content>
					</Tooltip.Root>
					<Tooltip.Root>
						<Tooltip.Trigger
							class="flex h-[27px] w-7 cursor-pointer items-center justify-center border border-ink/10 text-dim2"
							onclick={resetReplay}
							aria-label="Reset replay"
						>
							<RotateCcwIcon class="size-[14px]" />
						</Tooltip.Trigger>
						<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
							<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">RESET REPLAY</span>
						</Tooltip.Content>
					</Tooltip.Root>
				</div>

				<div class="h-5 w-px flex-none bg-ink/10"></div>

				<!-- Refresh and reset-view: the manual way out of a pane that has
				     ended up somewhere the data or the frame cannot explain.
				     Refresh reloads every pane; reset-view snaps the active pane
				     back to its default range (also `Alt R` — see
				     `handleGlobalKeydown`). -->
				<div class="flex flex-none items-center gap-1.5">
					<Tooltip.Root>
						<Tooltip.Trigger
							class="flex h-[27px] w-7 cursor-pointer items-center justify-center border border-ink/10 text-dim2"
							onclick={reloadAllPanes}
							aria-label="Refresh chart data"
						>
							<RefreshCwIcon class="size-[14px]" />
						</Tooltip.Trigger>
						<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
							<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">REFRESH CHART DATA</span>
						</Tooltip.Content>
					</Tooltip.Root>
					<Tooltip.Root>
						<Tooltip.Trigger
							class="flex h-[27px] w-7 cursor-pointer items-center justify-center border border-ink/10 text-dim2"
							onclick={() => resetPaneView(activePaneIndex)}
							aria-label="Reset chart view"
						>
							<ScanIcon class="size-[14px]" />
						</Tooltip.Trigger>
						<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
							<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">RESET CHART VIEW · ALT R</span>
						</Tooltip.Content>
					</Tooltip.Root>
				</div>

				<div class="h-5 w-px flex-none bg-ink/10"></div>

				<Popover.Root bind:open={wsMenuOpen}>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<span {...props} class="contents">
								<Popover.Trigger
									class={cn(
										'flex h-[27px] flex-none cursor-pointer items-center gap-2 border px-2.5',
										wsMenuOpen ? 'border-foreground bg-ink/8' : 'border-ink/14'
									)}
								>
									<Columns3Icon class="size-3 text-dim2" />
									<span class="max-w-[72px] overflow-hidden font-medium text-ellipsis whitespace-nowrap uppercase text-foreground text-[11px] tracking-[0.1em]">
										{chartWorkspaceStore.workspaces.find((w) => w.id === chartWorkspaceStore.activeWorkspaceId)?.name ?? '—'}
									</span>
									<ChevronDownIcon class="size-[11px] text-dim2" />
								</Popover.Trigger>
							</span>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
						<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">WORKSPACES</span>
					</Tooltip.Content>
				</Tooltip.Root>
				<Popover.Content align="end" sideOffset={10} class="w-[280px] rounded-none border-ink/16 bg-popover p-0">
					<form onsubmit={submitNewWorkspace} class="flex items-center gap-1.5 border-b border-ink/7 px-3 py-2.5">
						<Input placeholder="New workspace name…" bind:value={newWorkspaceName} class="h-7 flex-1 text-[11px]" />
						<Tooltip.Root>
							<Tooltip.Trigger
								type="submit"
								class="flex size-7 flex-none cursor-pointer items-center justify-center border border-ink/14 text-dim2"
								aria-label="Create workspace"
							>
								<PlusIcon class="size-3" />
							</Tooltip.Trigger>
							<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
								<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">CREATE WORKSPACE</span>
							</Tooltip.Content>
						</Tooltip.Root>
					</form>
					<div class="px-3 pt-2.5 pb-1.5 font-mono text-[8px] tracking-[0.26em] text-dim">WORKSPACES</div>
					{#each chartWorkspaceStore.workspaces as w (w.id)}
						{@const active = w.id === chartWorkspaceStore.activeWorkspaceId}
						<div class={cn('mx-1.5 mb-1 flex items-center gap-1.5 border px-2 py-1.5', active ? 'border-foreground bg-foreground' : 'border-transparent')}>
							{#if renamingWorkspaceId === w.id}
								<form onsubmit={submitRename} class="flex flex-1 items-center gap-1">
									<Input bind:value={renameDraft} class="h-6 flex-1 text-[11px]" autofocus />
									<button type="submit" class="font-mono text-[9px] text-dim2">OK</button>
								</form>
							{:else}
								<button type="button" class="min-w-0 flex-1 cursor-pointer text-left" onclick={() => pickWorkspace(w.id)}>
									<span class={cn('block truncate text-[11.5px] font-medium tracking-[0.1em] uppercase', active ? 'text-inv' : 'text-foreground')}>{w.name}</span>
								</button>
								<Tooltip.Root>
									<Tooltip.Trigger
										class={cn('flex-none cursor-pointer', active ? 'text-inv/70' : 'text-dim2')}
										onclick={() => startRename(w.id, w.name)}
										aria-label="Rename workspace"
									>
										<PencilIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">RENAME WORKSPACE</span>
									</Tooltip.Content>
								</Tooltip.Root>
								<Tooltip.Root>
									<Tooltip.Trigger
										class={cn('flex-none cursor-pointer', active ? 'text-inv/70' : 'text-dim2')}
										disabled={chartWorkspaceStore.workspaces.length <= 1}
										onclick={() => confirmDeleteWorkspace(w.id)}
										aria-label="Delete workspace"
									>
										<Trash2Icon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">DELETE WORKSPACE</span>
									</Tooltip.Content>
								</Tooltip.Root>
							{/if}
						</div>
					{/each}
				</Popover.Content>
			</Popover.Root>

			<Popover.Root bind:open={layoutMenuOpen}>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<span {...props} class="contents">
								<Popover.Trigger
									class={cn(
										'flex h-[27px] flex-none cursor-pointer items-center gap-2 border px-2',
										layoutMenuOpen ? 'border-foreground bg-ink/8' : 'border-ink/14'
									)}
								>
									{@render layoutIcon(layout.preset, true)}
									<ChevronDownIcon class="size-[11px] text-dim2" />
								</Popover.Trigger>
							</span>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
						<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">CHART LAYOUT</span>
					</Tooltip.Content>
				</Tooltip.Root>
				<Popover.Content align="end" sideOffset={10} class="w-[250px] rounded-none border-ink/16 bg-popover p-0">
					<div class="px-3 pt-2.5 pb-1.5 font-mono text-[8px] tracking-[0.26em] text-dim">CHART LAYOUT</div>
					{#each LAYOUT_GROUPS as g (g.count)}
						<div class="flex items-center gap-2.5 border-t border-ink/6 px-3 py-2">
							<span class="w-3 font-mono text-[10px] text-dim">{g.count}</span>
							<div class="flex gap-1.5">
								{#each g.ids as id (id)}
									<button
										type="button"
										class={cn(
											'flex h-6 w-[30px] cursor-pointer items-center justify-center border',
											layout.preset === id ? 'border-foreground bg-ink/12' : 'border-ink/14'
										)}
										onclick={() => setLayout(id)}
									>
										{@render layoutIcon(id, layout.preset === id)}
									</button>
								{/each}
							</div>
						</div>
					{/each}
					<div class="border-t border-ink/7 px-3 pt-2.5 pb-1.5 font-mono text-[8px] tracking-[0.26em] text-dim">SYNC IN LAYOUT</div>
					{#each syncToggleKeys as key (key)}
						<div class="flex items-center justify-between px-3 py-1.5">
							<span class="font-mono text-[10px] tracking-[0.14em] text-secondary-foreground uppercase">{key}</span>
							<Switch size="sm" checked={sync[key]} onCheckedChange={(v) => (sync = { ...sync, [key]: v })} />
						</div>
					{/each}
				</Popover.Content>
			</Popover.Root>

			<Tooltip.Root>
				<Tooltip.Trigger
					class="flex h-[27px] w-[30px] flex-none cursor-pointer items-center justify-center border border-ink/10 text-dim2"
					onclick={toggleFullscreen}
					aria-label="Toggle fullscreen"
				>
					<Maximize2Icon class="size-[14px]" />
				</Tooltip.Trigger>
				<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
					<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">TOGGLE FULLSCREEN</span>
				</Tooltip.Content>
			</Tooltip.Root>
		</Tooltip.Provider>
		</div>

		<!-- Toolbar + pane grid + right panel (lines 428-714) -->
		<div class="relative flex min-h-0 flex-1">
			<!-- Selecting a drawing puts it in move mode — draggable handles on the
	     chart, and this toolbar for the properties. A modal here would sit
	     over the very object being adjusted, which is why it is not one. -->
			{#if dialogDrawing}
				<DrawingToolbar
			x={drawingToolbarAnchor.x}
			y={drawingToolbarAnchor.y}
			kind={dialogDrawing.kind}
			text={dialogDrawing.text}
			visible={dialogDrawing.visible}
			color={dialogDrawing.color}
			width={dialogDrawing.width}
			lineStyle={dialogDrawing.lineStyle}
			onChangeStyle={(patch) =>
				selectedDrawing && void updateDrawingStyle(selectedDrawing.pane, selectedDrawing.id, patch)}
			onChangeText={(text) =>
				selectedDrawing && void updateDrawingText(selectedDrawing.pane, selectedDrawing.id, text)}
			onToggleVisible={() =>
				selectedDrawing && void toggleDrawingVisible(selectedDrawing.pane, selectedDrawing.id)}
			onDelete={() => {
				if (selectedDrawing) void removeDrawing(selectedDrawing.pane, selectedDrawing.id);
				selectedDrawing = null;
			}}
		/>
			{/if}

			<DrawToolbar {tool} onPick={(t) => (tool = t)} onClear={clearDrawings} />

			<PaneGrid
				{panes}
				layout={layout.preset}
				activeIndex={activePaneIndex}
				{tool}
				{replaying}
				{replayCut}
				{replayPct}
				{clearToken}
				{selectedDrawing}
				{reloadTokens}
				{resetTokens}
				{jumpTargets}
				{marketStatuses}
				onContextMenu={(pane, e) => (ctxMenu = { ...e, pane })}
				onMoveDrawing={(pane, id, patch) => void updateDrawingGeometry(pane, id, patch)}
				onPatchSettings={(pane, patch) => patchPane(pane, patch)}
				onFocus={onFocusPane}
				onToggleLayer={(i, id) => void toggleLayerVisible(i, id)}
				onOpenMainSettings={openMainSettings}
				onOpenLayerSettings={openLayerSettings}
				onRemoveLayer={(i, id) => void removeLayer(i, id)}
				onLastClose={onLastCloseFromPane}
				onCreateDrawing={handleCreateDrawing}
				onSelectDrawing={handleSelectDrawing}
				onToolConsumed={handleToolConsumed}
				onChartApi={onChartApiFromPane}
			/>

			{#if panelOpen}
				<div class="flex w-[262px] min-h-0 flex-none flex-col border-l border-border bg-chrome">
					<div class="flex h-[30px] flex-none items-center justify-between border-b border-ink/7 px-3">
						<span class="font-mono text-[8px] tracking-[0.26em] text-dim">{activePanel.label}</span>
						<Tooltip.Provider>
							<Tooltip.Root>
								<Tooltip.Trigger class="cursor-pointer text-dim" onclick={() => (panelOpen = false)} aria-label="Close panel">
									<XIcon class="size-3" />
								</Tooltip.Trigger>
								<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
									<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">CLOSE PANEL</span>
								</Tooltip.Content>
							</Tooltip.Root>
						</Tooltip.Provider>
					</div>

					{#if chartPanel === 'engine'}
						<div class="flex min-h-0 flex-1 flex-col">
							{#if tradeAdapter && !tradeAdapter.trades_real_money}
								<div class="flex-none border-b border-ink/7 px-3 py-2">
									<span class="font-mono text-[8.5px] tracking-[0.1em] text-dim">
										Simulated account — no order placed here leaves this machine.
									</span>
								</div>
							{:else if tradeAdapter}
								<div class="flex-none border-b border-ink/7 bg-loss/8 px-3 py-2">
									<span class="font-mono text-[8.5px] tracking-[0.1em] text-loss">
										Live account — orders placed here reach {tradeAdapter.name} and move real
										money.
									</span>
								</div>
							{/if}

							<button
								type="button"
								class="flex flex-none items-center justify-between gap-2 border-b border-ink/7 px-3 py-2.5 text-left"
								onclick={openTradeAccountPicker}
							>
								<div class="flex min-w-0 items-center gap-2">
									{#if tradeAdapter}
										{@const AccountIcon = iconForKind(tradeAdapter.kind)}
										<AccountIcon class="size-[13px] flex-none text-dim2" />
									{/if}
									<span
										class="overflow-hidden font-mono text-[10px] tracking-[0.04em] whitespace-nowrap text-foreground"
									>
										{tradeAccount?.label ?? 'NO ACCOUNT ATTACHED'}
									</span>
								</div>
								<span
									class="flex-none border border-ink/12 px-1 font-mono text-[7.5px] tracking-[0.14em] text-dim2"
								>
									{modeOf(tradeAdapter ?? undefined)}
								</span>
							</button>

							<OrderTicket
								instrument={activePane?.instrument ?? ''}
								account={tradeAccount}
								access={tradeAccess}
								{mid}
								submitting={placing}
								onPlace={(request) => void placeOrder(request)}
							/>

							{#if orderError}
								<div class="flex-none border-b border-ink/7 bg-loss/8 px-3 py-2">
									<span class="font-mono text-[9px] text-loss" data-order-error>{orderError}</span>
								</div>
							{/if}

							<div class="min-h-0 flex-1 overflow-auto">
								{#if orders.length === 0}
									<div class="py-5.5 text-center font-mono text-[9px] tracking-[0.16em] text-dim">
										{ordersLoading ? 'LOADING…' : 'NO ORDERS ON THIS ACCOUNT'}
									</div>
								{:else}
									{#each orders as o (o.id)}
										<div class="flex flex-col gap-0.5 border-b border-ink/5 px-3 py-2">
											<div class="flex items-center justify-between gap-2">
												<span
													class={cn(
														'font-mono text-[10px] tracking-[0.1em]',
														o.side === 'buy' ? 'text-gain' : 'text-loss'
													)}
												>
													{o.side.toUpperCase()}
													{o.kind.replace('_', '-').toUpperCase()}
												</span>
												<div class="flex items-center gap-2">
													<span
														class={cn(
															'font-mono text-[8.5px] tracking-[0.14em]',
															o.status === 'filled'
																? 'text-gain'
																: isWorking(o)
																	? 'text-secondary-foreground'
																	: 'text-dim'
														)}
													>
														{o.status.replace('_', ' ').toUpperCase()}
													</span>
													{#if isWorking(o) && tradeAccountTradable}
														<button
															type="button"
															class="cursor-pointer font-mono text-[8.5px] tracking-[0.14em] text-dim hover:text-foreground"
															onclick={() => openAmend(o)}
														>
															AMEND
														</button>
														<button
															type="button"
															class="cursor-pointer font-mono text-[8.5px] tracking-[0.14em] text-dim hover:text-loss"
															onclick={() => void cancelOrder(o.id)}
														>
															CANCEL
														</button>
													{/if}
												</div>
											</div>
											<div class="flex items-center justify-between gap-2">
												<span
													class="overflow-hidden font-mono text-[9px] text-ellipsis whitespace-nowrap text-dim"
												>
													{formatTime(o.submitted_at)} · {parseInstrumentId(o.instrument).ticker}
												</span>
												<span class="font-mono text-[9.5px] text-secondary-foreground">
													{formatScaledForDisplay(o.quantity, 8)} @ {formatScaledForDisplay(
														o.limit_price ?? o.trigger_price ?? o.average_price,
														8
													)}
												</span>
											</div>
										</div>
									{/each}
								{/if}
							</div>
						</div>
					{:else if chartPanel === 'depth'}
						<div class="min-h-0 flex-1 overflow-auto">
							<OrderBook instrument={activePane?.instrument ?? ''} />
						</div>
					{:else if chartPanel === 'watch'}
						<WatchlistTab onPickInstrument={pickPaneInstrument} />
					{:else if chartPanel === 'alerts'}
						<AlertsPanel paneInstrument={activePane?.instrument ?? ''} paneTimeframe={activePane?.timeframe ?? '1h'} />
					{:else if chartPanel === 'objects'}
						<div class="min-h-0 flex-1 overflow-auto">
							<div class="border-b border-ink/7 px-3 py-2 font-mono text-[8.5px] tracking-[0.2em] text-secondary-foreground">
								PANE {activePaneIndex + 1} · {activePane ? parseInstrumentId(activePane.instrument).venue + ':' + parseInstrumentId(activePane.instrument).ticker : ''}
							</div>
							<ObjectTree
								groups={treeGroups}
								selectedId={treeSelectedId}
								onSelect={handleTreeSelect}
								onToggle={handleTreeToggle}
								onOptions={handleTreeOptions}
								onRemove={handleTreeRemove}
								onMove={handleTreeMove}
							/>
						</div>
					{:else if chartPanel === 'notes'}
						<NotesTab />
					{/if}
				</div>
			{/if}

			<Tooltip.Provider>
				<div class="flex w-10 min-h-0 flex-none flex-col items-center gap-0.5 border-l border-border bg-chrome py-1.5">
					{#each PANEL_TABS as t (t.key)}
						{@const active = panelOpen && chartPanel === t.key}
						<Tooltip.Root>
							<Tooltip.Trigger
								class={cn(
									'flex size-7 cursor-pointer items-center justify-center border',
									active ? 'border-foreground bg-foreground text-inv' : 'border-transparent text-dim2 hover:bg-ink/7 hover:text-foreground'
								)}
								onclick={() => {
									panelOpen = chartPanel === t.key ? !panelOpen : true;
									chartPanel = t.key;
								}}
								aria-label={t.label}
							>
								<t.icon class="size-[15px]" />
							</Tooltip.Trigger>
							<Tooltip.Content side="left" sideOffset={10} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
								<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">{t.label}</span>
							</Tooltip.Content>
						</Tooltip.Root>
					{/each}
				</div>
			</Tooltip.Provider>
		</div>
	</div>

	{#if ctxMenu}
		<ChartContextMenu
			x={ctxMenu.x}
			y={ctxMenu.y}
			items={contextMenuItems(ctxMenu)}
			onClose={() => (ctxMenu = null)}
		/>
	{/if}

	<ChartSettingsDialog
		open={csOpen}
		initialSection={csSection}
		settings={chartSettings}
		quotesSupported={activePaneQuotesSupported}
		scopeLabel={settingsScopeLabel}
		onClose={() => (csOpen = false)}
		onChange={(patch) => void updatePaneSettings(activePaneIndex, patch)}
	/>

	<LayerDialog
		layer={dialogLayer}
		onClose={() => (layerDlg = null)}
		onEditParams={(patch) => dialogLayer && layerDlg && void editLayerParams(layerDlg.pane, dialogLayer.id, patch)}
		onStyleChanged={() => dialogLayer && void persistLayerStyle(dialogLayer.id)}
	/>

	<IndicatorPanel paneIndex={indicatorPanelPane} onClose={() => (indicatorPanelPane = null)} />

	<DateJumpDialog
		open={!!dateJumpDlg}
		onClose={() => (dateJumpDlg = null)}
		onJump={(targetNanos) => dateJumpDlg && jumpPaneToDate(dateJumpDlg.pane, targetNanos)}
	/>

	{#if amendDialogOrder && tradeAccount}
		{@const amendAccountId = tradeAccount.id}
		<AmendDialog
			bind:open={amendDialogOpen}
			accountId={amendAccountId}
			order={amendDialogOrder}
			onamended={() => void afterMutation(amendAccountId)}
		/>
	{/if}

{:else}
	<!-- No layout ever loaded (the initial `defaultWorkspace`/`getLayout`
	     round-trip itself failed) — nothing to render a chart into, so this
	     is the one case that still fills the whole route instead of an
	     inline banner. -->
	<div class="flex min-h-0 flex-1 items-center justify-center">
		<span class="font-mono text-[10px] tracking-[0.2em] text-destructive">
			{chartWorkspaceStore.error ?? 'Could not load your workspace.'}
		</span>
	</div>
{/if}
