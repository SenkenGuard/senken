<script lang="ts">
	// Charts route (the
	// largest of the three pages). Top toolbar (lines 311-425): symbol
	// readout, timeframe strip, indicator/settings menu buttons, replay
	// control, workspace switcher, layout menu. Below it (lines 428-714):
	// the drawing toolbar, the resizable pane grid, and the right panel
	// (watchlist / objects / alerts / notes / engine) with its own vertical
	// tab rail.
	//
	// this page now reads and writes real state — workspaces,
	// layouts, panes, layers, bars and indicator values all come from
	// `senken-api` (through `$lib/charts/workspace-store.svelte.ts` and
	// `$lib/charts/bars.ts`), never `$lib/mock/charts.ts` (deleted along
	// with `$lib/indicators/browser-compute.ts` —). What is
	// still local-only, and why, is documented at each such spot below and
	// in the current report: chart settings (no server field yet),
	// per-plot style (same), and the trade-engine furniture (`terminal/
	// trade-demo.ts` — a real trade engine is out of
	// scope for the whole plan, not just this stage).
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
		INDICATOR_CATALOG,
		parseInstrumentId,
		fmtDecimal,
		tfLabel
	} from '$lib/components/terminal/chart-config';
	import { apiClient } from '$lib/api/client';
	import type { InstrumentSummaryDto } from '$lib/api/types';
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore, priceFromEvent } from '$lib/api/ws-events.svelte';
	import { objectTree } from '$lib/charts/pane-runtime';
	import {
		chartWorkspaceStore,
		initChartWorkspaces,
		selectWorkspace,
		createNewWorkspace,
		renameWorkspace,
		deleteWorkspace,
		setLayoutPreset,
		setPaneInstrument,
		setAllPanesTimeframe,
		toggleLayerVisible,
		editLayerParams,
		addInstrumentOverlay,
		addIndicatorLayer,
		removeLayer,
		addDrawing,
		updateDrawingStyle,
		removeDrawing,
		clearAllDrawings,
		dismissWorkspaceError,
		persistLayerStyle,
		updatePaneSettings
	} from '$lib/charts/workspace-store.svelte';
	import type { DrawingRuntime } from '$lib/charts/pane-runtime';
	import { defaultChartSettings, type ChartSettings, type CsSectionKey } from '$lib/mock/chart-settings';
	import ChartContextMenu, { type MenuItem } from '$lib/components/terminal/chart-context-menu.svelte';
	import { commandPalette, openCommand, closeCommand } from '$lib/state/command-palette.svelte';
	import PaneGrid from '$lib/components/terminal/pane-grid.svelte';
	import DrawToolbar from '$lib/components/terminal/draw-toolbar.svelte';
	import OrderTicket from '$lib/components/terminal/order-ticket.svelte';
	import OrderBook from '$lib/components/terminal/order-book.svelte';
	import SymbolTree from '$lib/components/terminal/symbol-tree.svelte';
	import ChartSettingsDialog from '$lib/components/terminal/chart-settings-dialog.svelte';
	import LayerDialog from '$lib/components/terminal/layer-dialog.svelte';
	import DrawingDialog from '$lib/components/terminal/drawing-dialog.svelte';
	import AlertsPanel from '$lib/components/terminal/alerts-panel.svelte';
	import {
		DEFAULT_ACCOUNT,
		ACCOUNT_ICON,
		ENGINE_SUB_TABS,
		genOrderBook,
		NOTE_TAGS,
		ACTIVE_NOTE,
		type OrderRow,
		type OrderSide,
		type OrderType
	} from '$lib/components/terminal/trade-demo';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import SearchIcon from '@lucide/svelte/icons/search';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import Columns3Icon from '@lucide/svelte/icons/columns-3';
	import Maximize2Icon from '@lucide/svelte/icons/maximize-2';
	import PlayIcon from '@lucide/svelte/icons/play';
	import PauseIcon from '@lucide/svelte/icons/pause';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';
	import CoinsIcon from '@lucide/svelte/icons/coins';
	import SigmaIcon from '@lucide/svelte/icons/sigma';
	import { toast } from 'svelte-sonner';

	onMount(() => {
		void initChartWorkspaces();
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

	let tool = $state<ToolKey>('cursor');
	let clearToken = $state(0);

	let panelOpen = $state(true);
	let chartPanel = $state<ChartPanelKey>('engine');
	let engineSub = $state<'book' | 'orders'>('book');
	let orders = $state<OrderRow[]>([]);

	let wsMenuOpen = $state(false);
	let layoutMenuOpen = $state(false);
	// Neither of these has a server field yet, so neither persists — kept as
	// plain cosmetic UI state, same as before this stage.
	let autosave = $state(true);
	let sync = $state({ symbol: false, interval: false, crosshair: true, time: false });
	let newWorkspaceName = $state('');
	let renamingWorkspaceId = $state<string | null>(null);
	let renameDraft = $state('');

	// Overlay state — local to this page.
	// Each pane now persists its own chart settings server-side
	// (`PaneDto.settings`); the dialog edits whichever pane is active, so
	// this reads that pane's own settings rather than one page-level object
	// every pane used to share (and none of them actually kept).
	const chartSettings = $derived(activePane?.settings ?? defaultChartSettings());
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
	/** Bumped by the toolbar's reload button and by "Refresh chart data". */
	let reloadToken = $state(0);
	/** One counter per pane: resetting pane 2's view must not disturb the
	 * frame the user set on pane 1. */
	let resetTokens = $state<number[]>([]);

	function resetPaneView(index: number) {
		const next = [...resetTokens];
		next[index] = (next[index] ?? 0) + 1;
		resetTokens = next;
	}
	/** Which layer's settings are open, tracked by its *position* rather
	 * than its id: saving a layout replaces every layer row server-side, so
	 * the ids change on each write and a dialog keyed on one would close
	 * itself the moment the user edited anything. */
	let layerDlg = $state<{ pane: number; position: number } | null>(null);
	let addMenuKind = $state<'instrument' | 'indicator'>('indicator');

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
		void clearAllDrawings();
	}

	// The last real close per pane (`chart-pane.svelte`'s `onLastClose`),
	// used by the trade-engine demo ticket/book (a real number, even though that widget itself stays out of scope — see `terminal/trade-demo.ts`'s header comment).
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
		void setAllPanesTimeframe(t);
	}

	function pickSymbol(instrumentId: string) {
		void setPaneInstrument(activePaneIndex, instrumentId);
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

	function placeOrder(order: { side: OrderSide; type: OrderType; qty: number; price: number }) {
		const d = new Date();
		const ts = [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':');
		orders = [
			{
				id: 'O-' + (1042 + orders.length),
				ts,
				instrument: activePane?.instrument ?? '',
				side: order.side,
				type: order.type,
				qty: order.qty,
				price: order.price,
				status: order.type === 'MARKET' ? 'FILLED' : 'WORKING'
			},
			...orders
		];
	}

	// Real instrument search, replacing the former fixed
	// `INSTRUMENT_CATALOG` for both pickers below: `apiClient.searchInstruments`
	// re-runs on every keystroke into the command palette while it is open in
	// a mode that names instruments (`'symbol'`, or `'layer'`'s own
	// INSTRUMENT tab) — an empty query already ranks and returns real hits
	// (`instrument_handlers.rs`'s own "an empty query lists something" test),
	// so the picker opens with real rows even before the user types anything.
	let instrumentResults = $state<InstrumentSummaryDto[]>([]);
	let instrumentSearchToken = 0;
	$effect(() => {
		const request = commandPalette.request;
		const wantsInstruments =
			commandPalette.open && !!request && (request.mode === 'symbol' || (request.mode === 'layer' && addMenuKind === 'instrument'));
		if (!wantsInstruments) return;
		const query = commandPalette.query;
		const token = ++instrumentSearchToken;
		apiClient
			.searchInstruments(query, 20)
			.then((page) => {
				if (token === instrumentSearchToken) instrumentResults = page.rows;
			})
			.catch(() => {
				if (token === instrumentSearchToken) instrumentResults = [];
			});
	});

	function instrumentRow(x: InstrumentSummaryDto, meta: string, onPick: () => void) {
		return {
			icon: CoinsIcon,
			title: `${x.source_id.toUpperCase()}:${x.symbol}`,
			sub: x.name,
			meta,
			metaTone: 'dim' as const,
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
			rows: () =>
				instrumentResults.map((x) =>
					instrumentRow(x, x.kind.toUpperCase(), () => {
						pickSymbol(x.id);
						closeCommand();
					})
				)
		});
	}

	// reference: the toolbar's "INDICATORS & LAYERS" button (line 3275) —
	// opens the command palette in `'layer'` mode. the
	// INSTRUMENT tab adds an overlay instrument (`addInstrumentOverlay`),
	// the INDICATOR tab adds a real indicator layer, placed overlay or
	// sub-pane per `INDICATOR_CATALOG`'s own `subPane` flag. There is no
	// STRATEGY tab any more — `senken_workspace::LayerKind` has no such
	// variant.
	function openLayerPicker() {
		const paneTarget = activePaneIndex;
		openCommand({
			mode: 'layer',
			placeholder: 'Search instruments or indicators…',
			footer: `ADDING TO PANE ${paneTarget + 1}`,
			kindTabs: (['instrument', 'indicator'] as const).map((k) => ({
				label: k.toUpperCase(),
				active: () => addMenuKind === k,
				onClick: () => (addMenuKind = k)
			})),
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
				return INDICATOR_CATALOG.filter((def) => !q || `${def.label} ${def.paramsLabel}`.toLowerCase().includes(q)).map((def) => ({
					icon: SigmaIcon,
					title: def.label,
					sub: def.paramsLabel,
					meta: def.subPane ? 'SUB-PANE' : 'OVERLAY',
					metaTone: 'dim' as const,
					onPick: () => {
						void addIndicatorLayer(paneTarget, def);
						closeCommand();
					}
				}));
			}
		});
	}

	// The right panel's WATCH tab ("a watchlist row ... take[s] a lease", same as a chart pane, an alert or a position). Real search,
	// same endpoint the pickers above use; each listed row leases its own
	// instrument through the shared `wsClient` for as long as it stays
	// listed, so the whole visible set moves together whenever the tab
	// opens, closes, or a new query narrows or widens it.
	let watchQuery = $state('');
	let watchResults = $state<InstrumentSummaryDto[]>([]);
	let watchSearchToken = 0;
	$effect(() => {
		if (!(panelOpen && chartPanel === 'watch')) return;
		const query = watchQuery;
		const token = ++watchSearchToken;
		apiClient
			.searchInstruments(query, 20)
			.then((page) => {
				if (token === watchSearchToken) watchResults = page.rows;
			})
			.catch(() => {
				if (token === watchSearchToken) watchResults = [];
			});
	});

	// The lease itself: subscribing here and unsubscribing in this effect's
	// own cleanup (fired again whenever `watchResults` changes, and once
	// more on destroy) is the whole release mechanism — nobody has to
	// remember to call anything, the same discipline `chart-pane.svelte`'s
	// own live-price effect follows for a pane.
	$effect(() => {
		if (!(panelOpen && chartPanel === 'watch')) return;
		const ids = watchResults.map((r) => r.id);
		for (const id of ids) wsClient.subscribe(id);
		return () => {
			for (const id of ids) wsClient.unsubscribe(id);
		};
	});

	let watchPrices = $state<Record<string, number>>({});
	$effect(() => {
		const event = wsEventsStore.last;
		if (!event) return;
		const rows = watchResults;
		// `watchPrices` is read and written inside `untrack` deliberately —
		// this effect's only reactive dependencies are the incoming WS event
		// and the currently-listed rows, exactly the reasoning
		// `chart-pane.svelte`'s own live-price effect documents for the same
		// shape of update.
		untrack(() => {
			let next: Record<string, number> | null = null;
			for (const row of rows) {
				const price = priceFromEvent(event, row.id);
				if (price != null && watchPrices[row.id] !== price) {
					next ??= { ...watchPrices };
					next[row.id] = price;
				}
			}
			if (next) watchPrices = next;
		});
	});

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
	const treeGroups = $derived(objectTree(activePane?.layers ?? [], activePane?.drawings ?? []));
	const dialogDrawing = $derived.by(() => {
		const sel = selectedDrawing;
		if (!sel) return null;
		return panes[sel.pane]?.drawings.find((d) => d.id === sel.id) ?? null;
	});
	const mid = $derived(lastClose[activePaneIndex] ?? 0);
	const bookRows = $derived(genOrderBook(mid));

	const syncToggleKeys = ['symbol', 'interval', 'crosshair', 'time'] as const;
	const AccountIcon = ACCOUNT_ICON;

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
			{ kind: 'action', label: 'REFRESH CHART DATA', onSelect: () => (reloadToken += 1) },
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
					<button
						type="button"
						class="flex h-[27px] w-7 cursor-pointer items-center justify-center border border-ink/10 text-dim2"
						onclick={resetReplay}
					>
						<RotateCcwIcon class="size-[14px]" />
					</button>
				</div>
			</Tooltip.Provider>

			<div class="h-5 w-px flex-none bg-ink/10"></div>

			<Popover.Root bind:open={wsMenuOpen}>
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
				<Popover.Content align="end" sideOffset={10} class="w-[280px] rounded-none border-ink/16 bg-popover p-0">
					<div class="flex items-center justify-between border-b border-ink/7 px-3 py-2.5">
						<span class="font-mono text-[10px] tracking-[0.16em] text-secondary-foreground">AUTOSAVE</span>
						<Switch size="sm" checked={autosave} onCheckedChange={(v) => (autosave = v)} />
					</div>
					<form onsubmit={submitNewWorkspace} class="flex items-center gap-1.5 border-b border-ink/7 px-3 py-2.5">
						<Input placeholder="New workspace name…" bind:value={newWorkspaceName} class="h-7 flex-1 text-[11px]" />
						<button type="submit" class="flex size-7 flex-none cursor-pointer items-center justify-center border border-ink/14 text-dim2" aria-label="Create workspace">
							<PlusIcon class="size-3" />
						</button>
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
								<button type="button" class={cn('flex-none cursor-pointer', active ? 'text-inv/70' : 'text-dim2')} onclick={() => startRename(w.id, w.name)} aria-label="Rename workspace">
									<PencilIcon class="size-3" />
								</button>
								<button
									type="button"
									class={cn('flex-none cursor-pointer', active ? 'text-inv/70' : 'text-dim2')}
									disabled={chartWorkspaceStore.workspaces.length <= 1}
									onclick={() => confirmDeleteWorkspace(w.id)}
									aria-label="Delete workspace"
								>
									<Trash2Icon class="size-3" />
								</button>
							{/if}
						</div>
					{/each}
				</Popover.Content>
			</Popover.Root>

			<Popover.Root bind:open={layoutMenuOpen}>
				<Popover.Trigger
					class={cn(
						'flex h-[27px] flex-none cursor-pointer items-center gap-2 border px-2',
						layoutMenuOpen ? 'border-foreground bg-ink/8' : 'border-ink/14'
					)}
				>
					{@render layoutIcon(layout.preset, true)}
					<ChevronDownIcon class="size-[11px] text-dim2" />
				</Popover.Trigger>
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

			<button
				type="button"
				class="flex h-[27px] w-[30px] flex-none cursor-pointer items-center justify-center border border-ink/10 text-dim2"
				onclick={toggleFullscreen}
				aria-label="Toggle fullscreen"
			>
				<Maximize2Icon class="size-[14px]" />
			</button>
		</div>

		<!-- Toolbar + pane grid + right panel (lines 428-714) -->
		<div class="flex min-h-0 flex-1">
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
				{reloadToken}
				{resetTokens}
				onContextMenu={(pane, e) => (ctxMenu = { ...e, pane })}
				onFocus={onFocusPane}
				onToggleLayer={(i, id) => void toggleLayerVisible(i, id)}
				onOpenMainSettings={openMainSettings}
				onOpenLayerSettings={openLayerSettings}
				onRemoveLayer={(i, id) => void removeLayer(i, id)}
				onLastClose={onLastCloseFromPane}
				onCreateDrawing={handleCreateDrawing}
				onSelectDrawing={handleSelectDrawing}
				onToolConsumed={handleToolConsumed}
			/>

			{#if panelOpen}
				<div class="flex w-[262px] min-h-0 flex-none flex-col border-l border-border bg-chrome">
					<div class="flex h-[30px] flex-none items-center justify-between border-b border-ink/7 px-3">
						<span class="font-mono text-[8px] tracking-[0.26em] text-dim">{activePanel.label}</span>
						<button type="button" class="cursor-pointer text-dim" onclick={() => (panelOpen = false)} aria-label="Close panel">
							<XIcon class="size-3" />
						</button>
					</div>

					{#if chartPanel === 'engine'}
						<div class="flex min-h-0 flex-1 flex-col">
							<div class="flex items-center justify-between gap-2 border-b border-ink/7 px-3 py-2.5">
								<div class="flex min-w-0 items-center gap-2">
									<AccountIcon class="size-[13px] flex-none text-dim2" />
									<span class="font-mono text-[10px] tracking-[0.04em] whitespace-nowrap text-foreground">{DEFAULT_ACCOUNT.label}</span>
								</div>
								<span class="flex-none border border-ink/12 px-1 font-mono text-[7.5px] tracking-[0.14em] text-dim2">{DEFAULT_ACCOUNT.mode}</span>
							</div>

							<OrderTicket instrument={activePane?.instrument ?? ''} {mid} ccy={DEFAULT_ACCOUNT.ccy} leverage={DEFAULT_ACCOUNT.leverage} onPlace={placeOrder} />

							<div class="min-h-0 flex-1 overflow-auto">
								<div class="flex h-7 items-stretch border-b border-ink/7">
									{#each ENGINE_SUB_TABS as t (t.key)}
										<button
											type="button"
											class={cn(
												'flex cursor-pointer items-center gap-1.5 border-r border-ink/6 px-3',
												engineSub === t.key ? 'bg-foreground text-inv' : 'text-dim'
											)}
											onclick={() => (engineSub = t.key)}
										>
											<span class="font-mono text-[8px] tracking-[0.22em]">{t.label}</span>
											{#if t.key === 'orders'}
												<span class="font-mono text-[8.5px] text-dim">{orders.length}</span>
											{/if}
										</button>
									{/each}
								</div>
								{#if engineSub === 'book'}
									<OrderBook rows={bookRows} />
								{:else if orders.length === 0}
									<div class="py-5.5 text-center font-mono text-[9px] tracking-[0.16em] text-dim">NO ORDERS ON THIS ACCOUNT</div>
								{:else}
									{#each orders as o (o.id)}
										<div class="flex flex-col gap-0.5 border-b border-ink/5 px-3 py-2">
											<div class="flex items-center justify-between gap-2">
												<span class={cn('font-mono text-[10px] tracking-[0.1em]', o.side === 'BUY' ? 'text-gain' : 'text-loss')}>{o.side} {o.type}</span>
												<span class={cn('font-mono text-[8.5px] tracking-[0.14em]', o.status === 'WORKING' ? 'text-secondary-foreground' : o.status === 'FILLED' ? 'text-gain' : 'text-dim')}>{o.status}</span>
											</div>
											<div class="flex items-center justify-between gap-2">
												<span class="overflow-hidden font-mono text-[9px] text-ellipsis whitespace-nowrap text-dim">{o.ts} · {parseInstrumentId(o.instrument).ticker}</span>
												<span class="font-mono text-[9.5px] text-secondary-foreground">{o.qty} @ {fmtDecimal(o.price)}</span>
											</div>
										</div>
									{/each}
								{/if}
							</div>
						</div>
					{:else if chartPanel === 'watch'}
						<div class="flex min-h-0 flex-1 flex-col">
							<div class="flex-none border-b border-ink/7 p-2">
								<Input
									bind:value={watchQuery}
									placeholder="Search instruments…"
									class="h-7 border-ink/14 bg-transparent font-mono text-[10.5px]"
								/>
							</div>
							<div class="min-h-0 flex-1 overflow-auto">
								{#each watchResults as r (r.id)}
									<button
										type="button"
										class="flex w-full cursor-pointer items-center justify-between gap-2 border-b border-ink/5 px-3 py-2.5"
										onclick={() => pickSymbol(r.id)}
									>
										<div class="flex min-w-0 items-center gap-2.5">
											<div class="flex size-[18px] flex-none items-center justify-center border border-ink/14 font-mono text-[8.5px] text-secondary-foreground">{r.symbol.slice(0, 1)}</div>
											<span class="truncate font-mono text-[10.5px] tracking-[0.06em] text-foreground">{r.source_id.toUpperCase()}:{r.symbol}</span>
										</div>
										<span class="flex-none font-mono text-[10px] text-secondary-foreground">
											{watchPrices[r.id] != null ? fmtDecimal(watchPrices[r.id]) : ''}
										</span>
									</button>
								{/each}
								{#if watchResults.length === 0}
									<div class="py-5.5 text-center font-mono text-[9px] tracking-[0.16em] text-dim">NO MATCH</div>
								{/if}
							</div>
						</div>
					{:else if chartPanel === 'alerts'}
						<AlertsPanel paneInstrument={activePane?.instrument ?? ''} paneTimeframe={activePane?.timeframe ?? '1h'} />
					{:else if chartPanel === 'objects'}
						<div class="min-h-0 flex-1 overflow-auto">
							<div class="border-b border-ink/7 px-3 py-2 font-mono text-[8.5px] tracking-[0.2em] text-secondary-foreground">
								PANE {activePaneIndex + 1} · {activePane ? parseInstrumentId(activePane.instrument).venue + ':' + parseInstrumentId(activePane.instrument).ticker : ''}
							</div>
							<SymbolTree groups={treeGroups} />
						</div>
					{:else if chartPanel === 'notes'}
						<div class="flex min-h-0 flex-1 flex-col gap-2.5 overflow-auto p-3">
							<div class="border border-ink/10 p-2.5 font-mono text-[10px] leading-relaxed text-secondary-foreground">{ACTIVE_NOTE}</div>
							<div class="flex flex-wrap gap-1.5">
								{#each NOTE_TAGS as t (t)}
									<span class="border border-ink/12 px-2.5 py-1 font-mono text-[8.5px] tracking-[0.16em] text-dim2">{t}</span>
								{/each}
							</div>
						</div>
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
		scopeLabel={settingsScopeLabel}
		onClose={() => (csOpen = false)}
		onChange={(patch) => void updatePaneSettings(activePaneIndex, patch)}
	/>

	<LayerDialog
		layer={dialogLayer}
		onClose={() => (layerDlg = null)}
		onEditParams={(patch) => dialogLayer && layerDlg && void editLayerParams(layerDlg.pane, dialogLayer.id, patch)}
		onStyleChanged={() => void persistLayerStyle()}
	/>

	<DrawingDialog
		drawing={dialogDrawing}
		onClose={() => (selectedDrawing = null)}
		onChangeStyle={(patch) => selectedDrawing && void updateDrawingStyle(selectedDrawing.pane, selectedDrawing.id, patch)}
		onDelete={() => selectedDrawing && void removeDrawing(selectedDrawing.pane, selectedDrawing.id)}
	/>
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
