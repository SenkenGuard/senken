<script lang="ts">
	// Dashboard route: the workspace switcher, and a 12-column grid of
	// configurable widgets. The layout scaffolding (workspaces, span, the
	// widget catalog) is still local component state — nothing about which
	// widgets are placed where is persisted. The `equity`, `positions` and
	// `risk` widgets are not: they read the shared trade store like the
	// engine page does, through `watchTrade('all')`, because this page shows
	// every account the user owns rather than one at a time.
	import { onMount } from 'svelte';
	import {
		INITIAL_WORKSPACES,
		SPAN_STEPS,
		SPAN_CLASS,
		CATALOG,
		catalogEntry,
		WATCHLIST,
		HEAT_CELLS,
		SIGNALS,
		FLOW_BARS,
		FEED,
		type Workspace,
		type WidgetType
	} from '$lib/mock/dashboard';
	import { openCommand, closeCommand } from '$lib/state/command-palette.svelte';
	import { loadTrade, refreshPortfolios, tradeStore, watchTrade } from '$lib/state/trade.svelte';
	import { userZoneStore } from '$lib/state/user-zone.svelte';
	import { dashboardEquity, dashboardPositions, dashboardRisk } from '$lib/trade/view';
	import WorkspaceBar from '$lib/components/terminal/workspace-bar.svelte';
	import DashboardWidget from '$lib/components/terminal/dashboard-widget.svelte';
	import EquityCard from '$lib/components/terminal/equity-card.svelte';
	import WatchlistPanel from '$lib/components/terminal/watchlist-panel.svelte';
	import PositionsPanel from '$lib/components/terminal/positions-panel.svelte';
	import RiskPanel from '$lib/components/terminal/risk-panel.svelte';
	import HeatPanel from '$lib/components/terminal/heat-panel.svelte';
	import SignalFeed from '$lib/components/terminal/signal-feed.svelte';
	import FlowPanel from '$lib/components/terminal/flow-panel.svelte';
	import NewsFeed from '$lib/components/terminal/news-feed.svelte';
	import PlusIcon from '@lucide/svelte/icons/plus';

	let workspaces = $state<Workspace[]>(structuredClone(INITIAL_WORKSPACES));
	let activeWs = $state(0);
	let seq = 100;
	const nextId = () => `w${seq++}`;

	const active = $derived(workspaces[activeWs] ?? workspaces[0]);

	// The account widgets' view models, rebuilt from whatever
	// `tradeStore.portfolios` currently holds rather than a snapshot taken
	// once on mount — `watchTrade('all')` below keeps that store current, and
	// `tradeStore.ownAccounts` is read fresh on every recompute so an account
	// attached or removed elsewhere is reflected without this page doing
	// anything special.
	const equityData = $derived(dashboardEquity(tradeStore.ownAccounts, tradeStore.portfolios));
	const positionsTable = $derived(
		dashboardPositions(tradeStore.ownAccounts, tradeStore.portfolios, userZoneStore.zone)
	);
	const riskData = $derived(dashboardRisk(tradeStore.ownAccounts, tradeStore.portfolios));

	async function reload() {
		await loadTrade();
		await refreshPortfolios();
	}

	onMount(() => {
		void reload();
		// 'all': this page totals every owned account, so it is a broad
		// watcher exactly like the engine page's own OVERVIEW.
		return watchTrade('all');
	});

	function mutateActive(fn: (w: Workspace) => Workspace) {
		workspaces = workspaces.map((w, i) => (i === activeWs ? fn(w) : w));
	}

	function selectWorkspace(i: number) {
		activeWs = i;
	}

	// reference: `addWorkspace`, line 3075.
	function addWorkspace() {
		workspaces = [
			...workspaces,
			{
				name: `Layout ${workspaces.length + 1}`,
				widgets: [
					{ id: nextId(), type: 'watchlist', span: 4 },
					{ id: nextId(), type: 'equity', span: 8 }
				]
			}
		];
		activeWs = workspaces.length - 1;
	}

	// reference: `addWidget`, line 2131.
	function addWidget(type: WidgetType) {
		const cat = catalogEntry(type);
		mutateActive((w) => ({ ...w, widgets: [...w.widgets, { id: nextId(), type, span: cat.span }] }));
	}

	// reference: "ADD WIDGET…" opens the command palette in `'widget'` mode
	// (line 3086); its rows come from `CATALOG` (line 2738).
	function openAddWidget() {
		openCommand({
			mode: 'widget',
			placeholder: 'Search dashboard widgets…',
			footer: `ADDING TO ${active.name.toUpperCase()}`,
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return CATALOG.filter((c) => !q || `${c.title} ${c.hint}`.toLowerCase().includes(q)).map((c) => ({
					icon: PlusIcon,
					title: c.title.toUpperCase(),
					sub: c.hint.toUpperCase(),
					meta: `SPAN ${c.span}`,
					metaTone: 'dim',
					onPick: () => {
						addWidget(c.type);
						closeCommand();
					}
				}));
			}
		});
	}

	// reference: `removeWidget`, line 2139.
	function removeWidget(id: string) {
		mutateActive((w) => ({ ...w, widgets: w.widgets.filter((x) => x.id !== id) }));
	}

	// reference: `cycleWidget`, line 2143.
	function cycleWidget(id: string) {
		mutateActive((w) => ({
			...w,
			widgets: w.widgets.map((x) =>
				x.id === id ? { ...x, span: SPAN_STEPS[(SPAN_STEPS.indexOf(x.span) + 1) % SPAN_STEPS.length] } : x
			)
		}));
	}

	// reference: dashMenuItems "DUPLICATE WORKSPACE", line 3089.
	function duplicateWorkspace() {
		const copy: Workspace = {
			name: `${active.name} Copy`,
			widgets: active.widgets.map((w) => ({ ...w, id: nextId() }))
		};
		workspaces = [...workspaces, copy];
		activeWs = workspaces.length - 1;
	}

	// reference: dashMenuItems "RESET LAYOUT", line 3093.
	function resetLayout() {
		mutateActive((w) => ({ ...w, widgets: [] }));
	}

	// reference: dashMenuItems "DELETE WORKSPACE", line 3096 — no-ops below
	// two workspaces, same as the reference.
	function deleteWorkspace() {
		if (workspaces.length < 2) return;
		workspaces = workspaces.filter((_, i) => i !== activeWs);
		activeWs = Math.max(0, activeWs - 1);
	}

	function widgetMeta(type: WidgetType, span: number): string {
		return span > 3 ? catalogEntry(type).hint.toUpperCase() : '';
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<WorkspaceBar
		{workspaces}
		activeIndex={activeWs}
		onSelect={selectWorkspace}
		onAdd={addWorkspace}
		onOpenAddWidget={openAddWidget}
		onDuplicate={duplicateWorkspace}
		onReset={resetLayout}
		onDelete={deleteWorkspace}
	/>

	<div class="min-h-0 flex-1 overflow-auto bg-background p-3">
		<div class="grid grid-cols-12 items-start gap-3">
			{#each active.widgets as w (w.id)}
				{@const cat = catalogEntry(w.type)}
				<DashboardWidget
					title={cat.title}
					meta={widgetMeta(w.type, w.span)}
					spanClass={SPAN_CLASS[w.span] ?? 'col-span-3'}
					minHeightClass={cat.minHeightClass}
					onCycle={() => cycleWidget(w.id)}
					onRemove={() => removeWidget(w.id)}
				>
					{#if w.type === 'equity'}
						<EquityCard data={equityData} />
					{:else if w.type === 'watchlist'}
						<WatchlistPanel rows={WATCHLIST} />
					{:else if w.type === 'positions'}
						<PositionsPanel table={positionsTable} />
					{:else if w.type === 'risk'}
						<RiskPanel data={riskData} />
					{:else if w.type === 'heatmap'}
						<HeatPanel cells={HEAT_CELLS} />
					{:else if w.type === 'signals'}
						<SignalFeed signals={SIGNALS} />
					{:else if w.type === 'flow'}
						<FlowPanel bars={FLOW_BARS} />
					{:else if w.type === 'feed'}
						<NewsFeed items={FEED} />
					{/if}
				</DashboardWidget>
			{/each}
		</div>
	</div>
</div>
