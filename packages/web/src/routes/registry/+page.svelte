<script lang="ts">
	// The indicator registry: search every published indicator, read its
	// source, fork it into your own publish, or install it straight onto
	// this server. `senken_indicator_registry::RegistryStore`
	// (`crates/api/src/registry_handlers.rs`) is source-based rather than
	// binary — publishing always uploads indicator-lang text, never a
	// compiled component — so "install" here is a real, visible two-step
	// pipeline: fetch the source-derived `.wasm` this host just compiled
	// (`apiClient.installIndicator`) and register it into the same dynamic
	// catalogue a manual `.wasm` upload would
	// (`apiClient.uploadIndicatorPlugin`, `settings/sections/
	// plugins-section.svelte`'s own path). What you read in this screen is
	// what ends up running, because the wasm never comes from anywhere else.
	//
	// "Fork" does not need a server-side copy operation at all: publishing
	// has no `namespace` field (`PublishIndicatorRequest`'s own doc) — it
	// always lands in the caller's own account — so reading someone else's
	// source into this screen's publish form and submitting it under a name
	// of your choosing *is* the fork.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { IndicatorEntryDto, IndicatorSummaryDto, RegistryPage } from '$lib/api/types';
	import { formatInstant } from '$lib/time';
	import { userZoneStore } from '$lib/state/user-zone.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import SearchIcon from '@lucide/svelte/icons/search';
	import DownloadIcon from '@lucide/svelte/icons/download';
	import GitForkIcon from '@lucide/svelte/icons/git-fork';
	import SendIcon from '@lucide/svelte/icons/send';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import PackageSearchIcon from '@lucide/svelte/icons/package-search';
	import AtSignIcon from '@lucide/svelte/icons/at-sign';

	/** A starting point for "publish new", not a design decision this
	 * screen is inventing — the exact source `registry_handlers.rs`'s own
	 * test suite compiles and installs, so it is known-valid indicator-lang
	 * rather than a guess at the language's surface. */
	const BLANK_TEMPLATE = 'let fast = ema(close, 5)\nplot fast\n';

	type Tab = 'browse' | 'mine';

	let tab = $state<Tab>('browse');
	let query = $state('');
	let page = $state<RegistryPage>({ rows: [], total: 0 });
	let searchLoading = $state(true);
	let searchError = $state<string | null>(null);

	let selectedKey = $state<string | null>(null);
	let selectedEntry = $state<IndicatorEntryDto | null>(null);
	let entryLoading = $state(false);
	let entryError = $state<string | null>(null);

	let installingKey = $state<string | null>(null);

	let publishOpen = $state(false);
	let publishName = $state('');
	let publishSource = $state('');
	let publishing = $state(false);
	let publishError = $state<string | null>(null);

	/** The caller's own claimed registry handle (`alice` in
	 * `alice/supertrend`), or `null` before they have chosen one —
	 * `PUT /api/registry/handle`. `publishIndicator` refuses with "choose a
	 * registry handle before publishing" until this is set, so the publish
	 * form below offers claiming one right at the point that otherwise
	 * dead-ends. */
	let handle = $state<string | null>(null);
	let handleLoading = $state(true);
	let claimValue = $state('');
	let claiming = $state(false);
	let claimError = $state<string | null>(null);

	async function loadHandle(): Promise<void> {
		handleLoading = true;
		try {
			const response = await apiClient.getRegistryHandle();
			handle = response.handle ?? null;
		} catch {
			// Read-only status check — a failure here does not block
			// publishing itself; the publish attempt's own error (now the
			// server's real one, not a generic 403) is the one that matters.
			handle = null;
		} finally {
			handleLoading = false;
		}
	}

	async function claimHandle(): Promise<void> {
		const wanted = claimValue.trim();
		if (!wanted) {
			claimError = 'Choose a handle first.';
			return;
		}
		claiming = true;
		claimError = null;
		try {
			await apiClient.setRegistryHandle(wanted);
			handle = wanted;
			claimValue = '';
			toast.success(`Claimed the handle ${wanted}.`);
		} catch (cause) {
			claimError = getErrorMessage(cause, `Could not claim ${wanted}.`);
		} finally {
			claiming = false;
		}
	}

	function onClaimSubmit(event: SubmitEvent): void {
		event.preventDefault();
		void claimHandle();
	}

	function rowKey(namespace: string, name: string): string {
		return `${namespace}/${name}`;
	}

	async function runSearch(): Promise<void> {
		searchLoading = true;
		searchError = null;
		try {
			page = tab === 'mine' ? await apiClient.listMyIndicators(100, 0) : await apiClient.searchIndicators(query, 100, 0);
		} catch (cause) {
			searchError = getErrorMessage(cause, 'Could not search the registry.');
		} finally {
			searchLoading = false;
		}
	}

	onMount(() => {
		void runSearch();
		void loadHandle();
	});

	function selectTab(next: Tab): void {
		if (tab === next) return;
		tab = next;
		query = '';
		void runSearch();
	}

	function onSearchSubmit(event: SubmitEvent): void {
		event.preventDefault();
		void runSearch();
	}

	async function openEntry(entry: IndicatorSummaryDto): Promise<void> {
		const key = rowKey(entry.namespace, entry.name);
		selectedKey = key;
		publishOpen = false;
		publishError = null;
		entryLoading = true;
		entryError = null;
		selectedEntry = null;
		try {
			selectedEntry = await apiClient.getRegistryIndicator(entry.namespace, entry.name);
		} catch (cause) {
			entryError = getErrorMessage(cause, 'Could not load this indicator’s source.');
		} finally {
			entryLoading = false;
		}
	}

	function startPublish(): void {
		selectedKey = null;
		selectedEntry = null;
		entryError = null;
		publishOpen = true;
		publishName = '';
		publishSource = BLANK_TEMPLATE;
		publishError = null;
	}

	/** Copies the currently viewed source into the publish form under its
	 * own name, editable before it goes out under the caller's account —
	 * this screen's whole "read it, then run it" pitch depends on the
	 * source landing here exactly as published, not a summary of it. */
	function forkSelected(): void {
		if (!selectedEntry) return;
		publishOpen = true;
		publishName = selectedEntry.name;
		publishSource = selectedEntry.source;
		publishError = null;
	}

	function cancelPublish(): void {
		publishOpen = false;
		publishError = null;
	}

	async function submitPublish(): Promise<void> {
		const name = publishName.trim();
		if (!name || !publishSource.trim()) {
			publishError = 'Both a name and source are required.';
			return;
		}
		publishing = true;
		publishError = null;
		try {
			await apiClient.publishIndicator(name, publishSource);
			toast.success(`Published ${name}.`);
			publishOpen = false;
			if (tab === 'mine') await runSearch();
		} catch (cause) {
			publishError = getErrorMessage(cause, `Could not publish ${name}.`);
		} finally {
			publishing = false;
		}
	}

	/** Fetches the source-compiled component this host just built
	 * (`installIndicator`) and registers it into the dynamic indicator
	 * catalogue the same way a manual `.wasm` upload would
	 * (`uploadIndicatorPlugin`) — the two steps "install" means end to end
	 * for a chart to actually offer this indicator afterward. */
	async function install(entry: { namespace: string; name: string }): Promise<void> {
		const key = rowKey(entry.namespace, entry.name);
		installingKey = key;
		try {
			const { wasm } = await apiClient.installIndicator(entry.namespace, entry.name);
			await apiClient.uploadIndicatorPlugin(wasm);
			toast.success(`Installed ${entry.name} — it now appears in every chart’s indicator picker.`);
		} catch (cause) {
			toast.error(getErrorMessage(cause, `Could not install ${entry.name}.`));
		} finally {
			installingKey = null;
		}
	}

	function updatedLabel(unixSeconds: number): string {
		return formatInstant(unixSeconds * 1_000_000_000, userZoneStore.zone, { seconds: false }).text;
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<div class="flex flex-none items-center justify-between gap-3 border-b border-ink/9 bg-bg2 px-4 py-3">
		<div>
			<h1 class="text-[13px] font-semibold tracking-[0.08em] text-foreground uppercase">Indicator registry</h1>
			<p class="mt-0.5 text-[12px] text-dim2">
				Every published indicator is source, not a binary — read it, fork it, or install it as-is.
			</p>
		</div>
		<div class="flex flex-none items-center gap-3">
			{#if !handleLoading && handle}
				<span class="font-mono text-[11px] text-dim2" data-testid="registry-handle-status">
					Publishing as <span class="text-foreground">{handle}</span>
				</span>
			{/if}
			<Button size="sm" onclick={startPublish}>
				<PlusIcon class="size-3.5" />
				Publish new
			</Button>
		</div>
	</div>

	<div class="flex min-h-0 flex-1">
		<div class="flex w-[360px] flex-none flex-col border-r border-ink/9">
			<div class="flex flex-none border-b border-ink/9">
				<button
					type="button"
					class="flex-1 border-r border-ink/9 px-3 py-2 font-mono text-[9.5px] tracking-[0.16em] uppercase {tab === 'browse'
						? 'bg-foreground text-inv'
						: 'text-dim2 hover:text-foreground'}"
					onclick={() => selectTab('browse')}
				>
					Browse all
				</button>
				<button
					type="button"
					class="flex-1 px-3 py-2 font-mono text-[9.5px] tracking-[0.16em] uppercase {tab === 'mine'
						? 'bg-foreground text-inv'
						: 'text-dim2 hover:text-foreground'}"
					onclick={() => selectTab('mine')}
				>
					Published by me
				</button>
			</div>

			{#if tab === 'browse'}
				<form class="flex flex-none items-center gap-2 border-b border-ink/9 px-3 py-2.5" onsubmit={onSearchSubmit}>
					<SearchIcon class="size-3.5 flex-none text-dim2" />
					<Input
						bind:value={query}
						placeholder="Search indicator names…"
						class="h-7 border-none bg-transparent px-0 text-[12.5px] shadow-none focus-visible:ring-0"
					/>
				</form>
			{/if}

			<div class="min-h-0 flex-1 overflow-auto">
				{#if searchLoading}
					<div class="flex items-center justify-center gap-2 py-10">
						<Spinner class="size-3.5" />
						<span class="font-mono text-[10px] tracking-[0.14em] text-dim2 uppercase">Loading…</span>
					</div>
				{:else if searchError}
					<p class="m-3 border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-[12px] text-destructive">
						{searchError}
					</p>
				{:else if page.rows.length === 0}
					<div class="flex flex-col items-center gap-2 px-4 py-10 text-center">
						<PackageSearchIcon class="size-6 text-dim" />
						<p class="text-[12.5px] text-dim2">
							{tab === 'mine' ? 'You have not published an indicator yet.' : 'No published indicator matches this search.'}
						</p>
					</div>
				{:else}
					{#each page.rows as entry (entry.id)}
						{@const key = rowKey(entry.namespace, entry.name)}
						<button
							type="button"
							data-testid={`registry-row-${entry.name}`}
							class="flex w-full flex-col gap-1 border-b border-ink/[6%] px-3 py-2.5 text-left {selectedKey === key
								? 'bg-ink/[6%]'
								: 'hover:bg-ink/[3%]'}"
							onclick={() => openEntry(entry)}
						>
							<div class="flex items-center justify-between gap-2">
								<span class="truncate text-[13px] font-medium text-foreground">{entry.name}</span>
								<Badge variant="outline" class="flex-none font-mono text-[9.5px]">v{entry.language_version}</Badge>
							</div>
							<div class="flex items-center justify-between gap-2">
								<span class="truncate font-mono text-[10.5px] text-dim2">{entry.namespace}</span>
								<span class="flex-none font-mono text-[10px] text-dim">{updatedLabel(entry.updated_at)}</span>
							</div>
						</button>
					{/each}
				{/if}
			</div>
		</div>

		<div class="min-h-0 flex-1 overflow-auto p-4">
			{#if publishOpen}
				<div class="mx-auto flex max-w-2xl flex-col gap-3">
					<h2 class="text-[13px] font-semibold tracking-[0.04em] text-foreground">
						{selectedEntry ? 'Fork and publish' : 'Publish new indicator'}
					</h2>
					<label class="flex flex-col gap-1">
						<span class="font-mono text-[9px] tracking-[0.2em] text-dim uppercase">Name</span>
						<Input bind:value={publishName} placeholder="my-indicator" disabled={publishing} />
					</label>
					<label class="flex flex-col gap-1">
						<span class="font-mono text-[9px] tracking-[0.2em] text-dim uppercase">Source</span>
						<Textarea
							bind:value={publishSource}
							rows={16}
							disabled={publishing}
							class="font-mono text-[12.5px] leading-relaxed"
						/>
					</label>
					{#if publishError}
						<p class="border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-[12px] text-destructive">
							{publishError}
						</p>
					{/if}
					{#if !handleLoading && !handle}
						<div class="flex flex-col gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300">
							<p>
								You need a registry handle before you can publish — it's the name your indicators
								appear under ({claimValue || 'yourhandle'}/{publishName || 'indicator-name'}).
							</p>
							<form class="flex items-center gap-1.5" onsubmit={onClaimSubmit}>
								<AtSignIcon class="size-3.5 flex-none" />
								<Input
									bind:value={claimValue}
									placeholder="your-handle"
									disabled={claiming}
									class="h-7 flex-1 border-amber-500/40 bg-transparent text-[12px] text-foreground"
								/>
								<Button size="sm" type="submit" disabled={claiming}>
									{#if claiming}
										<Spinner class="size-3.5" />
									{:else}
										Claim
									{/if}
								</Button>
							</form>
							{#if claimError}
								<p class="text-destructive">{claimError}</p>
							{/if}
						</div>
					{/if}
					<div class="flex items-center gap-2">
						<Button onclick={submitPublish} disabled={publishing}>
							{#if publishing}
								<Spinner class="size-3.5" />
							{:else}
								<SendIcon class="size-3.5" />
							{/if}
							Publish
						</Button>
						<Button variant="outline" onclick={cancelPublish} disabled={publishing}>Cancel</Button>
					</div>
					<p class="text-[11.5px] text-dim2">
						Publishing always lands under your own account — there is no separate namespace field to set.
					</p>
				</div>
			{:else if entryLoading}
				<div class="flex items-center justify-center gap-2 py-16">
					<Spinner class="size-3.5" />
					<span class="font-mono text-[10px] tracking-[0.14em] text-dim2 uppercase">Loading source…</span>
				</div>
			{:else if entryError}
				<p class="mx-auto max-w-2xl border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-[12.5px] text-destructive">
					{entryError}
				</p>
			{:else if selectedEntry}
				{@const entry = selectedEntry}
				<div class="mx-auto flex max-w-3xl flex-col gap-3">
					<div class="flex items-start justify-between gap-3">
						<div>
							<h2 class="text-[15px] font-semibold text-foreground">{entry.name}</h2>
							<p class="font-mono text-[11px] text-dim2">
								{entry.namespace}/{entry.name} · language v{entry.language_version}
							</p>
							<p class="mt-0.5 text-[11px] text-dim">
								Published {updatedLabel(entry.created_at)} · updated {updatedLabel(entry.updated_at)}
							</p>
						</div>
						<div class="flex flex-none items-center gap-2">
							<Button variant="outline" size="sm" onclick={forkSelected}>
								<GitForkIcon class="size-3.5" />
								Fork
							</Button>
							<Button
								size="sm"
								onclick={() => install(entry)}
								disabled={installingKey === rowKey(entry.namespace, entry.name)}
							>
								{#if installingKey === rowKey(entry.namespace, entry.name)}
									<Spinner class="size-3.5" />
								{:else}
									<DownloadIcon class="size-3.5" />
								{/if}
								Install
							</Button>
						</div>
					</div>
					<pre class="overflow-auto border border-ink/12 bg-bg2 p-3 font-mono text-[12.5px] leading-relaxed text-foreground">{entry.source}</pre>
				</div>
			{:else}
				<div class="flex h-full flex-col items-center justify-center gap-2 text-center">
					<PackageSearchIcon class="size-8 text-dim" />
					<p class="max-w-sm text-[13px] text-dim2">
						Pick a published indicator on the left to read its source, or publish your own.
					</p>
				</div>
			{/if}
		</div>
	</div>
</div>
