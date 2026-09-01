<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Market data',
			rowId: 'storage-market-data',
			rowLabel: 'Downloaded market data',
			rowDescription: 'How much disk each source, instrument and series is using, and free it.'
		},
		{
			groupHeading: 'Database',
			rowId: 'storage-database',
			rowLabel: 'Accounts database',
			rowDescription: 'Users, roles, workspaces, alerts, watchlists and notes.'
		}
	];
</script>

<script lang="ts">
	// What this server is holding on disk, and the one place it can be
	// reclaimed.
	//
	// Only market data gets a tree, because only market data is stored as
	// files this app lays out itself — `senken-store` writes one Parquet file
	// per fetched range under `sources/{source}/instruments/{symbol}/…`, so
	// every level of that path is a real thing with a real size. Everything
	// else lives inside one SQLite database, where a per-feature breakdown
	// would be invented rather than measured: reporting it as one figure is
	// the honest shape of what is actually known.
	//
	// Deletion is real and immediate — the point of the panel is to free
	// disk, which a soft delete would not do. It is also safe in the one way
	// that matters: market data is re-fetchable, so what is lost is time and
	// venue requests, never a user's own work. Nothing user-authored is
	// reachable from here.
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import {
		describeStorageDeletion,
		formatBytes,
		formatFiles,
		shareOfTotal,
		storageNodeKey,
		type StorageNodeId
	} from '$lib/storage/usage';
	import type { StorageReportDto } from '$lib/api/types';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import * as AlertDialog from '$lib/components/ui/alert-dialog/index.js';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import TrashIcon from '@lucide/svelte/icons/trash-2';
	import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
	import DatabaseIcon from '@lucide/svelte/icons/database';
	import HardDriveIcon from '@lucide/svelte/icons/hard-drive';

	let report = $state<StorageReportDto | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	/** Which tree nodes are open, by `storageNodeKey`. Everything starts
	 * collapsed: a server that has been running a while holds hundreds of
	 * instruments, and opening on all of them buries the totals the reader
	 * came for. */
	let expanded = $state<string[]>([]);
	let pending = $state<{ id: StorageNodeId; message: string } | null>(null);
	let deleting = $state(false);

	async function load(): Promise<void> {
		loading = true;
		error = null;
		try {
			report = await apiClient.storageReport();
		} catch (cause) {
			// A 403 here means this account may not administer storage. It
			// shows as a message and never as a logout — the session is fine.
			error = getErrorMessage(cause, 'Could not read what this server has stored.');
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
	});

	function isOpen(id: StorageNodeId): boolean {
		return expanded.includes(storageNodeKey(id));
	}

	function toggle(id: StorageNodeId): void {
		const key = storageNodeKey(id);
		expanded = expanded.includes(key) ? expanded.filter((k) => k !== key) : [...expanded, key];
	}

	function ask(id: StorageNodeId, bytes: number, files: number, label?: string): void {
		pending = { id, message: describeStorageDeletion(id, { bytes, files }, label) };
	}

	async function confirmDelete(): Promise<void> {
		const target = pending;
		if (!target) return;
		deleting = true;
		try {
			// The wire shape is the server's, not the tree's: mapped here
			// rather than making `StorageNodeId` snake_case, so the UI's own
			// node identity stays a UI concern.
			await apiClient.deleteStorage({
				source_id: target.id.sourceId,
				symbol: target.id.symbol,
				series_id: target.id.seriesId
			});
			pending = null;
			// Re-read rather than patching the tree in memory: the freed
			// figure the server reports is the truth, and an instrument whose
			// last series just went takes its own row with it.
			await load();
		} catch (cause) {
			error = getErrorMessage(cause, 'Could not delete that.');
			pending = null;
		} finally {
			deleting = false;
		}
	}

	const marketTotal = $derived(report?.market_data.total_bytes ?? 0);
</script>

{#snippet sizeCell(bytes: number, files: number)}
	<div class="flex flex-none flex-col items-end gap-0.5">
		<span class="font-mono text-[12px] text-foreground">{formatBytes(bytes)}</span>
		<span class="font-mono text-[10px] text-dim">{formatFiles(files)}</span>
	</div>
{/snippet}

{#snippet shareBar(bytes: number)}
	<!-- Width is set from a computed percentage rather than a Tailwind class:
	     an arbitrary-value class built from a runtime number is not in the
	     stylesheet and would silently do nothing. -->
	<div class="h-[3px] w-full bg-ink/8">
		<div class="h-full bg-foreground/60" style={`width: ${shareOfTotal(bytes, marketTotal)}%`}></div>
	</div>
{/snippet}

<div class="flex flex-col gap-6">
	<section class="flex flex-col">
		<header class="mb-2 flex items-start justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">
					Market data
				</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Bars and trades downloaded from venues. Deleting any of it frees the disk immediately; it
					can be downloaded again.
				</p>
			</div>
			<Button variant="outline" size="sm" onclick={() => void load()} disabled={loading}>
				<RefreshIcon class="size-3.5" />
				Refresh
			</Button>
		</header>

		{#if loading && !report}
			<div class="flex items-center gap-2 py-8">
				<Spinner class="size-3.5" />
				<span class="font-mono text-[11px] tracking-[0.14em] text-dim2">MEASURING…</span>
			</div>
		{:else if error}
			<p data-testid="storage-error" class="py-4 text-[12.5px] text-destructive">{error}</p>
		{:else if report}
			<div class="flex items-center justify-between gap-4 border-y border-border py-2.5">
				<span class="flex items-center gap-2 text-[13px] font-medium text-foreground">
					<HardDriveIcon class="size-3.5 text-dim2" />
					All sources
				</span>
				{@render sizeCell(report.market_data.total_bytes, report.market_data.total_files)}
			</div>

			{#if report.market_data.sources.length === 0}
				<p class="py-6 text-[12.5px] text-dim2">
					Nothing has been downloaded yet. Opening a chart fetches what it needs, and it will show
					up here.
				</p>
			{/if}

			{#each report.market_data.sources as source (source.source_id)}
				{@const sourceId = { sourceId: source.source_id }}
				<div class="border-b border-border">
					<div class="flex items-center gap-3 py-2.5">
						<button
							type="button"
							class="flex min-w-0 flex-1 items-center gap-2 text-left"
							onclick={() => toggle(sourceId)}
							aria-expanded={isOpen(sourceId)}
						>
							{#if isOpen(sourceId)}
								<ChevronDownIcon class="size-3.5 flex-none text-dim2" />
							{:else}
								<ChevronRightIcon class="size-3.5 flex-none text-dim2" />
							{/if}
							<span class="min-w-0 flex-1">
								<span class="block truncate text-[13px] font-medium text-foreground">
									{source.source_id}
								</span>
								{@render shareBar(source.bytes)}
							</span>
						</button>
						{@render sizeCell(source.bytes, source.files)}
						<Button
							variant="ghost"
							size="icon-sm"
							onclick={() => ask(sourceId, source.bytes, source.files)}
							aria-label={`Delete everything stored for ${source.source_id}`}
						>
							<TrashIcon class="size-3.5 text-dim2" />
						</Button>
					</div>

					{#if isOpen(sourceId)}
						{#each source.instruments as instrument (instrument.symbol)}
							{@const symbolId = { sourceId: source.source_id, symbol: instrument.symbol }}
							<div class="border-t border-border/60 pl-6">
								<div class="flex items-center gap-3 py-2">
									<button
										type="button"
										class="flex min-w-0 flex-1 items-center gap-2 text-left"
										onclick={() => toggle(symbolId)}
										aria-expanded={isOpen(symbolId)}
									>
										{#if isOpen(symbolId)}
											<ChevronDownIcon class="size-3 flex-none text-dim2" />
										{:else}
											<ChevronRightIcon class="size-3 flex-none text-dim2" />
										{/if}
										<span class="truncate font-mono text-[12px] text-secondary-foreground">
											{instrument.symbol}
										</span>
									</button>
									{@render sizeCell(instrument.bytes, instrument.files)}
									<Button
										variant="ghost"
										size="icon-sm"
										onclick={() => ask(symbolId, instrument.bytes, instrument.files)}
										aria-label={`Delete every stored series for ${instrument.symbol}`}
									>
										<TrashIcon class="size-3.5 text-dim2" />
									</Button>
								</div>

								{#if isOpen(symbolId)}
									{#each instrument.series as series (series.id)}
										{@const seriesNode = {
											sourceId: source.source_id,
											symbol: instrument.symbol,
											seriesId: series.id
										}}
										<div class="flex items-center gap-3 border-t border-border/40 py-1.5 pl-5">
											<span class="min-w-0 flex-1 truncate font-mono text-[11.5px] text-dim2">
												{series.label}
											</span>
											{@render sizeCell(series.bytes, series.files)}
											<Button
												variant="ghost"
												size="icon-sm"
												onclick={() => ask(seriesNode, series.bytes, series.files, series.label)}
												aria-label={`Delete ${series.label} for ${instrument.symbol}`}
											>
												<TrashIcon class="size-3.5 text-dim2" />
											</Button>
										</div>
									{/each}
								{/if}
							</div>
						{/each}
					{/if}
				</div>
			{/each}
		{/if}
	</section>

	{#if report}
		<section class="flex flex-col">
			<header class="mb-1">
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">
					Database
				</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Users, roles, workspaces, drawings, alerts, watchlists and notes share one database. It
					is not broken down per feature here because that figure would be estimated rather than
					measured, and none of it is deletable from a storage panel — it is your own work, not a
					cache.
				</p>
			</header>
			<div class="divide-y divide-border">
				{#each report.databases as database (database.path)}
					<div class="flex items-start justify-between gap-6 py-3">
						<div class="flex min-w-0 flex-1 flex-col gap-0.5">
							<span class="flex items-center gap-2 text-[13px] font-medium text-foreground">
								<DatabaseIcon class="size-3.5 text-dim2" />
								{database.label}
							</span>
							<span class="truncate font-mono text-[11px] text-dim">{database.path}</span>
						</div>
						<span class="flex-none font-mono text-[12px] text-foreground">
							{formatBytes(database.bytes)}
						</span>
					</div>
				{/each}
			</div>
		</section>
	{/if}
</div>

<AlertDialog.Root
	open={pending !== null}
	onOpenChange={(open) => {
		if (!open) pending = null;
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Delete stored data</AlertDialog.Title>
			<AlertDialog.Description data-testid="storage-delete-confirm">
				{pending?.message ?? ''}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel disabled={deleting}>Cancel</AlertDialog.Cancel>
			<Button variant="destructive" disabled={deleting} onclick={() => void confirmDelete()}>
				{#if deleting}
					<Spinner class="size-3.5" />
				{/if}
				Delete
			</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
