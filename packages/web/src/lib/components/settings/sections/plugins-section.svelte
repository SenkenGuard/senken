<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Indicator plugins',
			rowId: 'plugins-list',
			rowLabel: 'Installed plugins',
			rowDescription: 'Enable, disable, or inspect a compiled indicator plugin.'
		},
		{
			groupHeading: 'Indicator plugins',
			rowId: 'plugins-upload',
			rowLabel: 'Upload a plugin',
			rowDescription: 'Register a compiled .wasm indicator component.'
		}
	];
</script>

<script lang="ts">
	// Manages the dynamic-indicator catalogue (`senken_runtime::
	// DynamicIndicators`) — indicators loaded at runtime from an uploaded
	// `.wasm` component, alongside the ten built-ins every account already
	// gets from the chart's own indicator picker. This is the one place a
	// plugin author can find out *why* an upload did not show up on a
	// chart: without a visible log, "why is my indicator missing" is a
	// guess.
	//
	// Every request here still gets checked for real by
	// `crates/api/src/indicator_handlers.rs` regardless of who this section
	// is shown to (`register-core-sections.ts` hides the nav entry itself
	// from an account with no grant on `Indicator`, but that is cosmetic —
	// hiding a control is never the enforcement).
	//
	// Five states, each demanding a different action from the reader:
	// active (disable), disabled (enable), incompatible (update the plugin
	// or Senken itself), failed to load (read why, replace the file) and
	// auto-disabled (read why, re-enable). Neither `incompatible` nor
	// `failed_to_load` ever got far enough to have a descriptor read from
	// it, so `IndicatorPluginDto`'s catalogue fields (`name`, `title`,
	// `params`, `plots`, `placement`, `warmup_bars`, …) are simply absent
	// on the wire for those two states — this file never treats their
	// presence as guaranteed by `state` alone; every read of one goes
	// through `hasDescriptor` below first, so a plugin that failed still
	// renders a readable row instead of one with holes in it.
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { IndicatorPluginDto } from '$lib/api/types';
	import { formatBytes } from '$lib/storage/usage';
	import { formatInstant } from '$lib/time';
	import { userZoneStore } from '$lib/state/user-zone.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import { Badge, type BadgeVariant } from '$lib/components/ui/badge/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
	import UploadIcon from '@lucide/svelte/icons/upload';
	import PuzzleIcon from '@lucide/svelte/icons/puzzle';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import OctagonXIcon from '@lucide/svelte/icons/octagon-x';
	import ZapOffIcon from '@lucide/svelte/icons/zap-off';

	let plugins = $state<IndicatorPluginDto[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	/** Which rows are expanded, by plugin id. Everything starts collapsed,
	 * the same reasoning `storage-section.svelte` gives for its own tree:
	 * the list is the thing a reader came for first. */
	let expanded = $state<string[]>([]);
	/** The plugin id currently mid-mutation, so only its own control shows
	 * a pending state rather than freezing the whole list for one request. */
	let togglingId = $state<string | null>(null);
	let uploading = $state(false);
	let uploadError = $state<string | null>(null);
	let fileInput = $state<HTMLInputElement | null>(null);

	async function load(): Promise<void> {
		loading = true;
		error = null;
		try {
			plugins = await apiClient.listIndicatorPlugins();
		} catch (cause) {
			// A 403 here means this account may not administer plugins. It
			// shows as a message and never as a logout — the session is fine.
			error = getErrorMessage(cause, 'Could not read the installed plugins.');
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		void load();
	});

	function isOpen(id: string): boolean {
		return expanded.includes(id);
	}

	function toggleOpen(id: string): void {
		expanded = isOpen(id) ? expanded.filter((n) => n !== id) : [...expanded, id];
	}

	/** Whether `plugin` carries a real catalogue descriptor — true for
	 * `active`, `disabled` and `auto_disabled` (all three loaded the
	 * component far enough to read one), false for `incompatible` and
	 * `failed_to_load` (which never did). Mirrors
	 * `crates/runtime/src/plugin_host.rs`'s own `status_for`: those are
	 * exactly the two states built from an entry that never reached
	 * `DynamicIndicatorEntry::Loaded`. */
	function hasDescriptor(plugin: IndicatorPluginDto): boolean {
		return plugin.state === 'active' || plugin.state === 'disabled' || plugin.state === 'auto_disabled';
	}

	interface StateDisplay {
		label: string;
		variant: BadgeVariant;
		class?: string;
	}

	const WARNING_BADGE_CLASS = 'border-amber-500/40 bg-amber-500/10 text-amber-300';

	function stateDisplay(plugin: IndicatorPluginDto): StateDisplay {
		switch (plugin.state) {
			case 'active':
				return { label: 'Active', variant: 'secondary' };
			case 'disabled':
				return { label: 'Disabled', variant: 'outline' };
			case 'incompatible':
				return { label: 'Incompatible', variant: 'outline', class: WARNING_BADGE_CLASS };
			case 'failed_to_load':
				return { label: 'Failed to load', variant: 'destructive' };
			case 'auto_disabled':
				return { label: 'Auto-disabled', variant: 'outline', class: WARNING_BADGE_CLASS };
		}
	}

	function originLabel(origin: IndicatorPluginDto['origin']): string {
		switch (origin) {
			case 'built_in':
				return 'Built-in';
			case 'uploaded':
				return 'Uploaded';
			case 'data_directory':
				return 'Found in data directory';
		}
	}

	async function setEnabled(plugin: IndicatorPluginDto, enabled: boolean): Promise<void> {
		togglingId = plugin.id;
		error = null;
		const label = hasDescriptor(plugin) ? plugin.title : plugin.id;
		try {
			await apiClient.setIndicatorPluginEnabled(plugin.id, enabled);
			// Re-read rather than flipping the flag in memory: the server's
			// own state is the truth, the same reasoning
			// `storage-section.svelte`'s `confirmDelete` gives for reloading
			// after a mutation rather than patching a local copy. It also
			// matters more here than it did before: enabling closes this
			// plugin's circuit breaker too, so a stale local patch would
			// keep showing `auto_disabled` after the server has already
			// moved it back to `active`.
			await load();
		} catch (cause) {
			error = getErrorMessage(cause, `Could not ${enabled ? 'enable' : 'disable'} ${label}.`);
		} finally {
			togglingId = null;
		}
	}

	function pickFile(): void {
		fileInput?.click();
	}

	async function onFileChosen(event: Event): Promise<void> {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		// Cleared immediately so choosing the same file again (e.g. after
		// fixing a build and re-exporting to the same path) still fires a
		// `change` event.
		input.value = '';
		if (!file) return;
		uploading = true;
		uploadError = null;
		try {
			const bytes = await file.arrayBuffer();
			await apiClient.uploadIndicatorPlugin(bytes);
			await load();
		} catch (cause) {
			uploadError = getErrorMessage(cause, `Could not register ${file.name}.`);
		} finally {
			uploading = false;
		}
	}
</script>

<div class="flex flex-col gap-6">
	<section class="flex flex-col">
		<header class="mb-2 flex items-start justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">
					Installed plugins
				</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Compiled indicator components registered on this server. Disabling one removes it from
					every chart's indicator picker without discarding the upload.
				</p>
			</div>
			<div class="flex flex-none items-center gap-2">
				<input
					bind:this={fileInput}
					type="file"
					accept=".wasm"
					class="hidden"
					onchange={onFileChosen}
				/>
				<Button variant="outline" size="sm" onclick={pickFile} disabled={uploading}>
					{#if uploading}
						<Spinner class="size-3.5" />
					{:else}
						<UploadIcon class="size-3.5" />
					{/if}
					Upload
				</Button>
				<Button variant="outline" size="sm" onclick={() => void load()} disabled={loading}>
					<RefreshIcon class="size-3.5" />
					Refresh
				</Button>
			</div>
		</header>

		{#if uploadError}
			<p data-testid="plugins-upload-error" class="mb-2 text-[12.5px] text-destructive">
				{uploadError}
			</p>
		{/if}

		{#if loading && plugins.length === 0}
			<div class="flex items-center gap-2 py-8">
				<Spinner class="size-3.5" />
				<span class="font-mono text-[11px] tracking-[0.14em] text-dim2">LOADING…</span>
			</div>
		{:else if error}
			<p data-testid="plugins-error" class="py-4 text-[12.5px] text-destructive">{error}</p>
		{:else if plugins.length === 0}
			<div class="flex flex-col items-center justify-center gap-3 border border-border py-12 text-center">
				<PuzzleIcon class="size-8 text-dim" />
				<p class="max-w-sm text-[13px] text-dim2">
					No plugin has been uploaded to this server yet. The ten built-in indicators are always
					available and are not listed here.
				</p>
			</div>
		{:else}
			<div class="border border-border" data-testid="plugins-list">
				{#each plugins as plugin (plugin.id)}
					{@const descriptor = hasDescriptor(plugin)}
					{@const state = stateDisplay(plugin)}
					<div class="border-b border-border last:border-b-0">
						<div class="flex items-center justify-between gap-3 px-3 py-2.5">
							<button
								type="button"
								class="flex min-w-0 flex-1 items-center gap-2 text-left"
								onclick={() => toggleOpen(plugin.id)}
								data-testid={`plugin-row-${plugin.id}`}
							>
								{#if isOpen(plugin.id)}
									<ChevronDownIcon class="size-3.5 flex-none text-dim2" />
								{:else}
									<ChevronRightIcon class="size-3.5 flex-none text-dim2" />
								{/if}
								<span class="flex min-w-0 flex-col">
									<span class="truncate text-[13px] font-medium text-foreground">
										{descriptor ? plugin.title : 'Uploaded plugin'}
									</span>
									<span class="truncate font-mono text-[11px] text-dim2">
										{descriptor ? plugin.name : plugin.id}
									</span>
								</span>
								<Badge
									variant={state.variant}
									class={state.class}
									data-testid={`plugin-state-${plugin.id}`}
								>
									{state.label}
								</Badge>
								<Badge variant="outline" data-testid={`plugin-origin-${plugin.id}`}>
									{originLabel(plugin.origin)}
								</Badge>
							</button>
							<div class="flex flex-none items-center gap-2">
								{#if plugin.state === 'active' || plugin.state === 'disabled'}
									{#if togglingId === plugin.id}
										<Spinner class="size-3.5" />
									{:else}
										<Switch
											size="sm"
											checked={plugin.state === 'active'}
											onCheckedChange={() => void setEnabled(plugin, plugin.state !== 'active')}
											data-testid={`plugin-toggle-${plugin.id}`}
										/>
									{/if}
								{:else if plugin.state === 'auto_disabled'}
									<Button
										variant="outline"
										size="sm"
										onclick={() => void setEnabled(plugin, true)}
										disabled={togglingId === plugin.id}
										data-testid={`plugin-reenable-${plugin.id}`}
									>
										{#if togglingId === plugin.id}
											<Spinner class="size-3.5" />
										{:else}
											<ZapOffIcon class="size-3.5" />
										{/if}
										Re-enable
									</Button>
								{/if}
							</div>
						</div>

						{#if isOpen(plugin.id)}
							<div
								class="flex flex-col gap-4 border-t border-border bg-ink/2 px-3 py-3"
								data-testid={`plugin-detail-${plugin.id}`}
							>
								{#if plugin.state === 'incompatible'}
									<div
										class="flex items-start gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300"
										data-testid={`plugin-reason-${plugin.id}`}
									>
										<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
										<div>
											<div class="font-medium">This plugin's API version is not supported.</div>
											<div class="mt-0.5 text-amber-300/80">
												Found <span class="font-mono">{plugin.found_version}</span>, this server
												supports <span class="font-mono">{plugin.supported_version}</span>. Update
												the plugin, or update Senken.
											</div>
										</div>
									</div>
								{:else if plugin.state === 'failed_to_load'}
									<div
										class="flex items-start gap-2 border border-destructive/40 bg-destructive/10 px-2.5 py-2 text-[12px] text-destructive"
										data-testid={`plugin-reason-${plugin.id}`}
									>
										<OctagonXIcon class="mt-0.5 size-3.5 flex-none" />
										<div>
											<div class="font-medium">This plugin failed to load.</div>
											<div class="mt-0.5">{plugin.reason}</div>
										</div>
									</div>
								{:else if plugin.state === 'auto_disabled'}
									<div
										class="flex items-start gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300"
										data-testid={`plugin-reason-${plugin.id}`}
									>
										<ZapOffIcon class="mt-0.5 size-3.5 flex-none" />
										<div>
											<div class="font-medium">
												Turned off automatically after repeated failures.
											</div>
											<div class="mt-0.5 text-amber-300/80">{plugin.reason}</div>
										</div>
									</div>
								{/if}

								{#if descriptor}
									<div class="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] sm:grid-cols-3">
										<div>
											<div class="text-dim2">Short title</div>
											<div class="font-mono text-foreground">{plugin.short_title}</div>
										</div>
										<div>
											<div class="text-dim2">Placement</div>
											<div class="text-foreground">{plugin.placement}</div>
										</div>
										<div>
											<div class="text-dim2">Warm-up bars</div>
											<div class="text-foreground">{plugin.warmup_bars}</div>
										</div>
									</div>

									<div>
										<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Parameters
										</div>
										{#if plugin.params.length === 0}
											<p class="text-[12px] text-dim2">This indicator takes no parameters.</p>
										{:else}
											<ul class="flex flex-col gap-0.5">
												{#each plugin.params as param (param.name)}
													<li class="font-mono text-[12px] text-foreground">
														{param.name}
														<span class="text-dim2">({param.kind})</span>
													</li>
												{/each}
											</ul>
										{/if}
									</div>

									<div>
										<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Plots
										</div>
										<ul class="flex flex-col gap-0.5">
											{#each plugin.plots as plot (plot.field)}
												<li class="flex items-center gap-1.5 font-mono text-[12px] text-foreground">
													<span
														class="size-2 flex-none rounded-full"
														style={`background-color: ${plot.color}`}
													></span>
													{plot.label}
													<span class="text-dim2">({plot.field})</span>
												</li>
											{/each}
										</ul>
									</div>
								{/if}

								<div>
									<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
										Runtime health
									</div>
									{#if plugin.health}
										<div class="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] sm:grid-cols-4">
											<div>
												<div class="text-dim2">Traps</div>
												<div class="font-mono text-foreground">{plugin.health.trap_count}</div>
											</div>
											<div>
												<div class="text-dim2">Deadlines exceeded</div>
												<div class="font-mono text-foreground">
													{plugin.health.deadline_exceeded_count}
												</div>
											</div>
											<div>
												<div class="text-dim2">Peak memory</div>
												<div class="font-mono text-foreground">
													{formatBytes(plugin.health.peak_memory_bytes)}
												</div>
											</div>
											<div>
												<div class="text-dim2">Circuit breaker</div>
												<div class="text-foreground">
													{plugin.health.circuit.state === 'open' ? 'Open' : 'Closed'}
												</div>
											</div>
										</div>
										{#if plugin.health.circuit.state === 'open'}
											<p class="mt-1.5 text-[12px] text-amber-300">{plugin.health.circuit.reason}</p>
										{/if}
									{:else}
										<p class="text-[12px] text-dim2">
											This plugin never finished loading, so it has no runtime health to report.
										</p>
									{/if}
								</div>

								<div>
									<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
										Logs
									</div>
									{#if plugin.logs.length === 0}
										<p class="text-[12px] text-dim2" data-testid={`plugin-logs-empty-${plugin.id}`}>
											No log lines recorded for this plugin yet.
										</p>
									{:else}
										<p class="mb-1 text-[11px] text-dim2">
											Times shown in {formatInstant(plugin.logs[0].timestamp, userZoneStore.zone)
												.zoneLabel} ({userZoneStore.zone}).
										</p>
										<ul class="flex flex-col gap-1" data-testid={`plugin-logs-${plugin.id}`}>
											{#each plugin.logs as line, index (index)}
												<li class="flex items-start gap-2 font-mono text-[11.5px]">
													<span class="flex-none text-dim2">
														{formatInstant(line.timestamp, userZoneStore.zone).text}
													</span>
													<span
														class={line.severity === 'warn'
															? 'flex-none text-amber-300'
															: 'flex-none text-dim2'}
													>
														{line.severity === 'warn' ? 'WARN' : 'INFO'}
													</span>
													<span class="text-foreground">{line.message}</span>
												</li>
											{/each}
										</ul>
									{/if}
								</div>
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</section>
</div>
