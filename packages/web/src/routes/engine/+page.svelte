<script lang="ts">
	// Trade engine route: the
	// overview/accounts/adapters view switcher, per-view stat strips,
	// exposure, account cards, the adapter grid and the executions table.
	// All state here is local component state (`$state`), mirroring the
	// reference's own `this.state.engineView` / `engineTab` / `activeAccount`
	// — this page is UI-only, so nothing here is persisted.
	import { cn } from '$lib/utils.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import PlugIcon from '@lucide/svelte/icons/plug';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import {
		ENGINE_VIEWS,
		ORDERS,
		ADAPTERS,
		adapterOf,
		allPositions,
		positionsForAccount,
		aggStats,
		acctStats,
		exposure,
		accountCards,
		adapterCards,
		engineTable,
		type EngineTableTab
	} from '$lib/mock/engine';
	import { accountsStore, attachAccount } from '$lib/state/accounts.svelte';
	import { openCommand, closeCommand } from '$lib/state/command-palette.svelte';
	import StatStrip from '$lib/components/terminal/stat-strip.svelte';
	import ExposurePanel from '$lib/components/terminal/exposure-panel.svelte';
	import AccountCard from '$lib/components/terminal/account-card.svelte';
	import AdapterCard from '$lib/components/terminal/adapter-card.svelte';
	import ExecutionsTable from '$lib/components/terminal/executions-table.svelte';

	type EngineView = 'overview' | 'accounts' | 'adapters';

	let engineView = $state<EngineView>('overview');
	let engineTab = $state<EngineTableTab>('positions');

	const isOverview = $derived(engineView === 'overview');
	const isAccountsView = $derived(engineView === 'accounts');
	const isAdaptersView = $derived(engineView === 'adapters');
	const showTable = $derived(engineView !== 'adapters');
	const showScopeChip = $derived(isAccountsView);

	const accounts = $derived(accountsStore.list);
	const activeAccount = $derived(accounts.find((a) => a.id === accountsStore.activeId) ?? accounts[0]);
	const activeAccountIndex = $derived(accounts.findIndex((a) => a.id === activeAccount.id));
	const activeAdapter = $derived(adapterOf(activeAccount.adapter));

	// reference: `posOf`/`allPositions`/`accOrders`/`scoped`/`scopedOrders`.
	const scopedPositions = $derived(
		isOverview ? allPositions(accounts) : positionsForAccount(activeAccount, Math.max(activeAccountIndex, 0))
	);
	const scopedOrders = $derived(isOverview ? ORDERS : ORDERS.filter((o) => o.account === activeAccount.id));
	const currentTable = $derived(
		engineTable(engineTab, isOverview ? 'all' : 'account', scopedPositions, scopedOrders, accounts)
	);

	const distinctAdapterCount = $derived(new Set(accounts.map((a) => a.adapter)).size);

	// reference: `openAccountCmd` (line 2946) — the scope chip switches the
	// active trading account through the command palette's 'account' mode.
	function openAccountPicker() {
		openCommand({
			mode: 'account',
			placeholder: 'Search accounts…',
			footer: 'ACTIVE TRADING ACCOUNT',
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return accounts
					.filter((a) => !q || `${a.label} ${adapterOf(a.adapter).name}`.toLowerCase().includes(q))
					.map((a) => ({
						icon: adapterOf(a.adapter).icon,
						title: a.label,
						sub: `${adapterOf(a.adapter).name} · ${a.mode} · ${a.leverage}`,
						meta: (a.ccy === 'USD' ? '$' : '') + a.equity.toLocaleString('en-US', { maximumFractionDigits: 2 }),
						metaTone: a.id === accountsStore.activeId ? 'gain' : 'dim2',
						onPick: () => {
							accountsStore.activeId = a.id;
							closeCommand();
						}
					}));
			}
		});
	}

	// reference: `openAdapterCmd` (line 2947) — "ATTACH ACCOUNT" picks which
	// adapter to attach a new account for, then calls `attachAccount`.
	function openAdapterPicker() {
		openCommand({
			mode: 'adapter',
			placeholder: 'Search adapters to attach an account…',
			footer: 'ATTACH A NEW ACCOUNT',
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return ADAPTERS.filter((a) => !q || `${a.name} ${a.kind}`.toLowerCase().includes(q)).map((a) => ({
					icon: a.icon,
					title: a.name,
					sub: `${a.kind} · ${a.symbols === '*' ? 'all instruments' : a.symbols.join(', ')}`,
					meta: a.status,
					metaTone: a.status === 'CONNECTED' ? 'gain' : 'dim2',
					onPick: () => {
						attachAccount(a.key);
						closeCommand();
					}
				}));
			}
		});
	}

	// reference: the engineViews tab's onClick — switching to ADAPTERS resets
	// the (hidden) table sub-tab back to POSITIONS, same as the reference.
	function selectView(v: string) {
		engineView = v as EngineView;
		if (v === 'adapters') engineTab = 'positions';
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<div class="flex h-10 flex-none items-stretch border-b border-ink/9 bg-bg2">
		<Tabs.Root value={engineView} onValueChange={selectView} class="contents">
			<Tabs.List
				class="group-data-horizontal/tabs:h-full flex-none items-stretch justify-start gap-0 rounded-none bg-transparent p-0"
			>
				{#each ENGINE_VIEWS as v (v.key)}
					<Tabs.Trigger
						value={v.key}
						class="h-auto flex-none items-center gap-2 rounded-none border-0 border-r border-ink/6 px-4 py-0 font-mono text-[9.5px] tracking-[0.18em] text-dim2 uppercase shadow-none data-[state=active]:bg-foreground data-[state=active]:text-inv data-[state=active]:shadow-none dark:data-[state=active]:border-transparent dark:data-[state=active]:bg-foreground dark:data-[state=active]:text-inv"
					>
						<v.icon class="size-[13px]" />
						{v.label}
					</Tabs.Trigger>
				{/each}
			</Tabs.List>
		</Tabs.Root>

		<div class="flex-1"></div>

		{#if isAdaptersView}
			<div class="flex items-center px-2.5">
				<button
					type="button"
					class="flex h-[26px] cursor-pointer items-center gap-[7px] border border-ink/14 px-[11px] text-dim2"
					onclick={openAdapterPicker}
				>
					<PlugIcon class="size-[13px]" />
					<span class="font-mono text-[9px] tracking-[0.18em]">ATTACH ACCOUNT</span>
				</button>
			</div>
		{/if}

		{#if showScopeChip}
			{@const AcctIcon = activeAdapter.icon}
			<div class="flex items-center gap-[9px] border-l border-ink/6 px-3">
				<span class="font-mono text-[8px] tracking-[0.2em] text-dim">ACCOUNT</span>
				<button
					type="button"
					class="flex h-[26px] cursor-pointer items-center gap-2 border border-ink/18 px-[11px]"
					onclick={openAccountPicker}
				>
					<AcctIcon class="size-3 text-dim2" />
					<span class="max-w-32 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10.5px] text-foreground">
						{activeAccount.label}
					</span>
					<Badge
						class={cn(
							'h-auto flex-none rounded-none px-[4px] py-[1px] font-mono text-[7.5px] tracking-[0.14em]',
							activeAccount.mode === 'LIVE' ? 'bg-loss text-inv' : 'bg-ink/12 text-dim2'
						)}
					>
						{activeAccount.mode}
					</Badge>
					<ChevronDownIcon class="size-2.5 text-dim" />
				</button>
			</div>
		{/if}
	</div>

	{#if isOverview}
		<StatStrip stats={aggStats(accounts)} size="lg" />
		<ExposurePanel items={exposure(accounts)} accounts={accounts.length} adapters={distinctAdapterCount} />
	{/if}

	{#if isAccountsView}
		<div class="flex flex-none items-stretch gap-[9px] overflow-x-auto border-b border-ink/9 bg-background px-3 py-2.5">
			{#each accountCards(accountsStore.activeId, accounts) as card (card.id)}
				<AccountCard {card} onclick={() => (accountsStore.activeId = card.id)} />
			{/each}
		</div>
		<StatStrip stats={acctStats(activeAccount)} size="md" />
	{/if}

	{#if isAdaptersView}
		<div class="min-h-0 flex-1 overflow-auto bg-background p-3.5">
			<div class="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-3">
				{#each adapterCards(accountsStore.activeId, accounts) as adapter (adapter.name)}
					<AdapterCard
						{adapter}
						onSelectAccount={(id) => (accountsStore.activeId = id)}
						onAttach={(key) => attachAccount(key)}
					/>
				{/each}
			</div>
		</div>
	{/if}

	{#if showTable}
		<ExecutionsTable
			tabs={currentTable.tabs}
			activeTab={engineTab}
			ontabchange={(t) => (engineTab = t)}
			table={currentTable}
		/>
	{/if}
</div>
