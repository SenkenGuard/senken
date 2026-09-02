<script lang="ts">
	// The trade engine page, on live data.
	//
	// Three views over one set of state: OVERVIEW (every account this user
	// can see), ACCOUNTS (one at a time, with its own settings and the
	// adapter's own actions), and ADAPTERS (what is registered, and what to
	// attach an account to).
	//
	// # Nothing here is cached
	//
	// Balances, positions, orders and fills are read through each adapter on
	// every load. The broker behind an account is the system of record for
	// them, so this page shows what the adapter answered rather than a copy
	// that could disagree. `refreshPortfolios` re-reads from the account
	// list *as it currently is*, never from the ids the page opened with —
	// an account attached or removed since then would otherwise be missed or
	// asked about forever.
	import { onMount } from 'svelte';
	import { cn } from '$lib/utils.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import PlugIcon from '@lucide/svelte/icons/plug';
	import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
	import WalletIcon from '@lucide/svelte/icons/wallet';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SettingsIcon from '@lucide/svelte/icons/settings';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import { apiClient } from '$lib/api/client';
	import { describeError } from '$lib/api/errors';
	import type { AdapterActionDto, AdapterDto, TradeAccountDto } from '$lib/api/types';
	import {
		loadTrade,
		refreshAccounts,
		selectAccount,
		tradeStore
	} from '$lib/state/trade.svelte';
	import {
		accountCards,
		accountStats,
		adapterCards,
		aggregateStats,
		emptyPortfolio,
		engineTable,
		exposureByAdapter,
		iconForKind,
		modeOf,
		type EngineTableTab,
		type Portfolio
	} from '$lib/trade/view';
	import StatStrip from '$lib/components/terminal/stat-strip.svelte';
	import ExposurePanel from '$lib/components/terminal/exposure-panel.svelte';
	import AccountCard from '$lib/components/terminal/account-card.svelte';
	import AdapterCard from '$lib/components/terminal/adapter-card.svelte';
	import ExecutionsTable from '$lib/components/terminal/executions-table.svelte';
	import AccountDialog from '$lib/components/trade/account-dialog.svelte';
	import ActionDialog from '$lib/components/trade/action-dialog.svelte';
	import { openCommand, closeCommand } from '$lib/state/command-palette.svelte';

	type EngineView = 'overview' | 'accounts' | 'adapters';

	const ENGINE_VIEWS = [
		{ key: 'overview' as const, label: 'OVERVIEW', icon: LayoutGridIcon },
		{ key: 'accounts' as const, label: 'ACCOUNTS', icon: WalletIcon },
		{ key: 'adapters' as const, label: 'ADAPTERS', icon: PlugIcon }
	];

	let engineView = $state<EngineView>('overview');
	let engineTab = $state<EngineTableTab>('positions');
	let portfolios = $state<Record<string, Portfolio>>({});
	let refreshing = $state(false);
	let notice = $state<string | null>(null);

	let accountDialogOpen = $state(false);
	let accountDialogAdapter = $state<AdapterDto | null>(null);
	let accountDialogAccount = $state<TradeAccountDto | null>(null);
	let actionDialogOpen = $state(false);
	let actionDialogAction = $state<AdapterActionDto | null>(null);

	const isOverview = $derived(engineView === 'overview');
	const isAccountsView = $derived(engineView === 'accounts');
	const isAdaptersView = $derived(engineView === 'adapters');
	const showTable = $derived(engineView !== 'adapters');

	const accounts = $derived(tradeStore.accounts);
	const adapters = $derived(tradeStore.adapters);
	const activeAccount = $derived(tradeStore.active);
	const activeAdapter = $derived(
		activeAccount ? tradeStore.adapter(activeAccount.adapter_id) : undefined
	);
	const distinctAdapterCount = $derived(new Set(accounts.map((a) => a.adapter_id)).size);

	/** The rows the table shows: every visible account in OVERVIEW, just the
	 * selected one otherwise. Each row carries its account's label so the
	 * ACCOUNT column does not need a second lookup. */
	const scoped = $derived.by(() => {
		const inScope = isOverview ? accounts : activeAccount ? [activeAccount] : [];
		const positions = inScope.flatMap((account) =>
			(portfolios[account.id]?.positions ?? []).map((row) => ({
				...row,
				accountLabel: account.label
			}))
		);
		const orders = inScope.flatMap((account) =>
			(portfolios[account.id]?.orders ?? []).map((row) => ({
				...row,
				accountLabel: account.label
			}))
		);
		const fills = inScope.flatMap((account) =>
			(portfolios[account.id]?.fills ?? []).map((row) => ({
				...row,
				accountLabel: account.label
			}))
		);
		return { positions, orders, fills };
	});

	const currentTable = $derived(
		engineTable(
			engineTab,
			isOverview ? 'all' : 'account',
			scoped.positions,
			scoped.orders,
			scoped.fills
		)
	);

	onMount(() => {
		void reload();
	});

	async function reload() {
		await loadTrade();
		await refreshPortfolios();
	}

	/** Re-reads every visible account's portfolio, from the account list as
	 * it currently is. */
	async function refreshPortfolios() {
		refreshing = true;
		const next: Record<string, Portfolio> = {};
		await Promise.all(
			tradeStore.accounts.map(async (account) => {
				const portfolio = emptyPortfolio();
				try {
					const [balances, positions, orders, fills] = await Promise.all([
						apiClient.tradeAccountBalances(account.id),
						apiClient.tradeAccountPositions(account.id),
						apiClient.tradeAccountOrders(account.id, 'all'),
						apiClient.tradeAccountFills(account.id)
					]);
					portfolio.balances = balances;
					portfolio.positions = positions;
					portfolio.orders = orders;
					portfolio.fills = fills;
				} catch (error) {
					// One account's adapter being unreachable must not blank
					// the whole page; the failure is kept against that
					// account and shown on its own card.
					portfolio.error = describeError(error);
				}
				next[account.id] = portfolio;
			})
		);
		portfolios = next;
		refreshing = false;
	}

	async function detach(account: TradeAccountDto) {
		try {
			await apiClient.deleteTradeAccount(account.id);
			await refreshAccounts();
			await refreshPortfolios();
			notice = `${account.label} detached.`;
		} catch (error) {
			notice = describeError(error);
		}
	}

	function openAttach(adapter: AdapterDto) {
		accountDialogAdapter = adapter;
		accountDialogAccount = null;
		accountDialogOpen = true;
	}

	function openEdit(account: TradeAccountDto) {
		const adapter = tradeStore.adapter(account.adapter_id);
		if (!adapter) return;
		accountDialogAdapter = adapter;
		accountDialogAccount = account;
		accountDialogOpen = true;
	}

	function openAction(action: AdapterActionDto) {
		actionDialogAction = action;
		actionDialogOpen = true;
	}

	async function onAccountSaved() {
		await refreshAccounts();
		await refreshPortfolios();
	}

	function openAccountPicker() {
		openCommand({
			mode: 'account',
			placeholder: 'Search accounts…',
			footer: 'ACTIVE TRADING ACCOUNT',
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return accounts
					.filter((account) => {
						const adapter = tradeStore.adapter(account.adapter_id);
						return (
							!q || `${account.label} ${adapter?.name ?? ''}`.toLowerCase().includes(q)
						);
					})
					.map((account) => {
						const adapter = tradeStore.adapter(account.adapter_id);
						return {
							icon: iconForKind(adapter?.kind ?? 'exchange'),
							title: account.label,
							sub: `${adapter?.name ?? account.adapter_id} · ${modeOf(adapter)}`,
							meta: portfolios[account.id]?.balances
								? formatEquity(account.id)
								: '—',
							metaTone: account.id === tradeStore.activeId ? 'gain' : 'dim2',
							onPick: () => {
								selectAccount(account.id);
								closeCommand();
							}
						};
					});
			}
		});
	}

	function formatEquity(accountId: string): string {
		const balances = portfolios[accountId]?.balances;
		if (!balances) return '—';
		return `${accountStats(portfolios[accountId]).find((s) => s.label === 'EQUITY')?.value ?? '—'} ${balances.currency}`;
	}

	function openAdapterPicker() {
		openCommand({
			mode: 'adapter',
			placeholder: 'Search adapters to attach an account…',
			footer: 'ATTACH A NEW ACCOUNT',
			rows: (query) => {
				const q = query.trim().toLowerCase();
				return adapters
					.filter((adapter) => !q || `${adapter.name} ${adapter.kind}`.toLowerCase().includes(q))
					.map((adapter) => ({
						icon: iconForKind(adapter.kind),
						title: adapter.name,
						sub: adapter.description,
						meta: adapter.trades_real_money ? 'LIVE' : 'SIMULATED',
						metaTone: adapter.trades_real_money ? 'loss' : 'gain',
						onPick: () => {
							openAttach(adapter);
							closeCommand();
						}
					}));
			}
		});
	}

	function selectView(view: string) {
		engineView = view as EngineView;
		if (view === 'adapters') engineTab = 'positions';
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

		<div class="flex items-center px-2.5">
			<button
				type="button"
				class="flex h-[26px] cursor-pointer items-center gap-[7px] border border-ink/14 px-[11px] text-dim2"
				onclick={() => void refreshPortfolios()}
				disabled={refreshing}
				data-refresh-portfolios
			>
				<RefreshCwIcon class={cn('size-[12px]', refreshing && 'animate-spin')} />
				<span class="font-mono text-[9px] tracking-[0.18em]">REFRESH</span>
			</button>
		</div>

		{#if isAdaptersView}
			<div class="flex items-center pr-2.5">
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

		{#if isAccountsView && activeAccount}
			{@const AcctIcon = iconForKind(activeAdapter?.kind ?? 'exchange')}
			<div class="flex items-center gap-[9px] border-l border-ink/6 px-3">
				<span class="font-mono text-[8px] tracking-[0.2em] text-dim">ACCOUNT</span>
				<button
					type="button"
					class="flex h-[26px] cursor-pointer items-center gap-2 border border-ink/18 px-[11px]"
					onclick={openAccountPicker}
				>
					<AcctIcon class="size-3 text-dim2" />
					<span
						class="max-w-32 overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10.5px] text-foreground"
					>
						{activeAccount.label}
					</span>
					<Badge
						class={cn(
							'h-auto flex-none rounded-none px-[4px] py-[1px] font-mono text-[7.5px] tracking-[0.14em]',
							activeAdapter?.trades_real_money ? 'bg-loss text-inv' : 'bg-ink/12 text-dim2'
						)}
					>
						{modeOf(activeAdapter)}
					</Badge>
					<ChevronDownIcon class="size-2.5 text-dim" />
				</button>
			</div>
		{/if}
	</div>

	{#if tradeStore.load === 'error'}
		<div class="flex flex-1 items-center justify-center px-6">
			<p class="font-mono text-[11px] text-loss">{tradeStore.error}</p>
		</div>
	{:else if tradeStore.load === 'loading'}
		<div class="flex flex-1 items-center justify-center px-6">
			<p class="font-mono text-[10px] tracking-[0.18em] text-dim">LOADING TRADE ENGINE…</p>
		</div>
	{:else if accounts.length === 0 && !isAdaptersView}
		<div class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
			<p class="font-mono text-[11px] tracking-[0.08em] text-dim2">
				No trading account is attached yet.
			</p>
			<p class="max-w-md font-mono text-[9.5px] leading-[1.7] text-dim">
				An account connects Senken to a broker, an exchange, or the built-in simulator. The
				simulator trades every instrument on the platform and no order it takes leaves this
				machine.
			</p>
			<Button
				class="h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]"
				onclick={() => selectView('adapters')}
			>
				CHOOSE AN ADAPTER
			</Button>
		</div>
	{:else}
		{#if notice}
			<div class="flex-none border-b border-ink/9 bg-bg2 px-4 py-2">
				<span class="font-mono text-[9.5px] tracking-[0.04em] text-dim2">{notice}</span>
			</div>
		{/if}

		{#if isOverview}
			<StatStrip stats={aggregateStats(accounts, portfolios)} size="lg" />
			<ExposurePanel
				items={exposureByAdapter(accounts, adapters, portfolios)}
				accounts={accounts.length}
				adapters={distinctAdapterCount}
			/>
		{/if}

		{#if isAccountsView}
			<div
				class="flex flex-none items-stretch gap-[9px] overflow-x-auto border-b border-ink/9 bg-background px-3 py-2.5"
			>
				{#each accountCards(accounts, adapters, portfolios, activeAccount?.id ?? null) as card (card.id)}
					<AccountCard {card} onclick={() => selectAccount(card.id)} />
				{/each}
			</div>
			<StatStrip stats={accountStats(portfolios[activeAccount?.id ?? ''])} size="md" />

			{#if activeAccount}
				{@const portfolio = portfolios[activeAccount.id]}
				{#if portfolio?.error}
					<div class="flex-none border-b border-ink/9 bg-loss/8 px-4 py-2">
						<span class="font-mono text-[9.5px] text-loss">{portfolio.error}</span>
					</div>
				{/if}
				{#if activeAccount.owned}
					<div
						class="flex flex-none flex-wrap items-center gap-2 border-b border-ink/9 bg-background px-3 py-2"
					>
						<button
							type="button"
							class="flex h-[26px] cursor-pointer items-center gap-[7px] border border-ink/14 px-[11px] text-dim2"
							onclick={() => openEdit(activeAccount)}
						>
							<SettingsIcon class="size-[12px]" />
							<span class="font-mono text-[9px] tracking-[0.18em]">SETTINGS</span>
						</button>
						{#each activeAdapter?.actions ?? [] as action (action.id)}
							<button
								type="button"
								class={cn(
									'flex h-[26px] cursor-pointer items-center border px-[11px] font-mono text-[9px] tracking-[0.18em]',
									action.destructive
										? 'border-loss/50 text-loss'
										: 'border-ink/14 text-dim2'
								)}
								onclick={() => openAction(action)}
							>
								{action.label.toUpperCase()}
							</button>
						{/each}
						<div class="flex-1"></div>
						<button
							type="button"
							class="flex h-[26px] cursor-pointer items-center gap-[7px] border border-loss/50 px-[11px] text-loss"
							onclick={() => void detach(activeAccount)}
						>
							<Trash2Icon class="size-[12px]" />
							<span class="font-mono text-[9px] tracking-[0.18em]">DETACH</span>
						</button>
					</div>
				{:else}
					<div class="flex-none border-b border-ink/9 bg-bg2 px-4 py-2">
						<span class="font-mono text-[9.5px] tracking-[0.04em] text-dim">
							This account belongs to another user. You can see that it exists; its
							credentials and its orders are not yours to reach.
						</span>
					</div>
				{/if}
			{/if}
		{/if}

		{#if isAdaptersView}
			<div class="min-h-0 flex-1 overflow-auto bg-background p-3.5">
				<div class="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-3">
					{#each adapterCards(adapters, accounts, portfolios, activeAccount?.id ?? null) as card (card.key)}
						<AdapterCard
							adapter={card}
							onSelectAccount={(id) => {
								selectAccount(id);
								selectView('accounts');
							}}
							onAttach={(key) => {
								const adapter = tradeStore.adapter(key);
								if (adapter) openAttach(adapter);
							}}
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
	{/if}
</div>

{#if accountDialogAdapter}
	<AccountDialog
		bind:open={accountDialogOpen}
		adapter={accountDialogAdapter}
		account={accountDialogAccount}
		onsaved={() => void onAccountSaved()}
	/>
{/if}

{#if actionDialogAction && activeAccount}
	<ActionDialog
		bind:open={actionDialogOpen}
		action={actionDialogAction}
		accountId={activeAccount.id}
		accountLabel={activeAccount.label}
		onran={(message) => {
			notice = message;
			void refreshPortfolios();
		}}
	/>
{/if}
