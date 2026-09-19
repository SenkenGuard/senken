<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Plugins',
			rowId: 'unified-plugins-list',
			rowLabel: 'All plugins',
			rowDescription: 'Every venue, trade adapter, widget and indicator plugin on this server.'
		},
		{
			groupHeading: 'Plugins',
			rowId: 'unified-plugins-install',
			rowLabel: 'Install a plugin',
			rowDescription: 'Upload a plugin package (.zip) or a compiled indicator component (.wasm).'
		}
	];
</script>

<script lang="ts">
	// One list for every plugin this server knows about — a static venue
	// compiled into the binary, a trade adapter, a dashboard widget UI
	// package, or a compiled indicator component — replacing what used to
	// be two separate upload dialogs (indicator plugins, widget plugins)
	// that showed no venue at all. One page, not one page per plugin shape:
	// this is the only place a plugin author or admin can find out why
	// something they installed is not doing anything, and splitting it by
	// plugin shape meant an account looking for a venue found an empty page
	// and reasonably concluded nothing was installed.
	//
	// Every request here still gets checked for real by
	// `crates/api/src/plugin_handlers.rs` (install/uninstall/enable) or the
	// legacy `indicator_handlers`/`widget_plugin_handlers` routes this page
	// still reads detail from, regardless of who this section is shown to
	// (`register-core-sections.ts` hides the nav entry itself from an
	// account with no grant on `Plugin`, `Indicator` or `WidgetPlugin`, but
	// that is cosmetic — hiding a control is never the enforcement).
	//
	// # Why detail still reads two legacy endpoints
	//
	// `GET /api/plugins`/`GET /api/plugins/{id}` (the unified endpoints)
	// report only identity, contribution and enable state today — no
	// runtime health, log, or manifest detail yet — that is a later backend
	// increment. Rather than show an expanded row
	// with nothing in it, this page still reads
	// `apiClient.listIndicatorPlugins()`/`listWidgetPlugins()` once, purely
	// as **detail sources** matched by id against a unified row, and
	// renders whichever one actually has a match — never as a second
	// visible list of their own. A plugin with no match in either (every
	// static venue, and a package whose id differs from its own loaded
	// component's descriptor id — see `senken_runtime::Runtime::plugin_catalog`'s
	// own docs on why a venue or indicator package can appear under two
	// different ids until a later pass merges them into one row) shows a
	// plain "no additional detail yet" panel instead of guessing.
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { IndicatorPluginDto, PluginDto } from '$lib/api/types';
	import {
		listWidgetPlugins,
		type WidgetPluginPackage
	} from '$lib/components/dashboard/api';
	import { refreshWidgetCatalog } from '$lib/components/dashboard/widget-catalog.svelte';
	import { formatBytes } from '$lib/storage/usage';
	import { formatInstant } from '$lib/time';
	import { userZoneStore } from '$lib/state/user-zone.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import { Badge, type BadgeVariant } from '$lib/components/ui/badge/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import SearchIcon from '@lucide/svelte/icons/search';
	import RefreshCcwDotIcon from '@lucide/svelte/icons/refresh-ccw-dot';
	import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
	import UploadIcon from '@lucide/svelte/icons/upload';
	import PuzzleIcon from '@lucide/svelte/icons/puzzle';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import OctagonXIcon from '@lucide/svelte/icons/octagon-x';
	import ZapOffIcon from '@lucide/svelte/icons/zap-off';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	interface StateDisplay {
		label: string;
		variant: BadgeVariant;
		class?: string;
	}

	const WARNING_BADGE_CLASS = 'border-amber-500/40 bg-amber-500/10 text-amber-300';

	// -----------------------------------------------------------------
	// The unified plugin system (`GET /api/plugins`): one row for every
	// static venue plugin compiled into this server and every package
	// discovered on disk.
	// -----------------------------------------------------------------

	type ContributionKind = PluginDto['contributes'][number];

	let unifiedPlugins = $state<PluginDto[]>([]);
	let unifiedLoading = $state(true);
	let unifiedError = $state<string | null>(null);
	let unifiedTogglingId = $state<string | null>(null);
	let unifiedSearch = $state('');
	let unifiedFilter = $state<'all' | ContributionKind>('all');
	/** Which rows are expanded, by plugin id — everything starts collapsed,
	 * the same reasoning `storage-section.svelte` gives for its own tree:
	 * the list is the thing a reader came for first. */
	let expanded = $state<string[]>([]);

	let installing = $state(false);
	let installError = $state<string | null>(null);
	let fileInput = $state<HTMLInputElement | null>(null);

	let removingId = $state<string | null>(null);
	let confirmTarget = $state<PluginDto | null>(null);

	// Detail-only sources — see this file's own top-of-script docs for why
	// these are never rendered as a second visible list.
	let indicatorDetails = $state<Map<string, IndicatorPluginDto>>(new Map());
	let widgetDetails = $state<Map<string, WidgetPluginPackage>>(new Map());

	const UNIFIED_FILTERS: { value: 'all' | ContributionKind; label: string }[] = [
		{ value: 'all', label: 'All' },
		{ value: 'venue', label: 'Venue & market data' },
		{ value: 'trade_adapter', label: 'Trade engine' },
		{ value: 'dashboard_widget', label: 'Widgets' },
		{ value: 'indicator', label: 'Indicators' }
	];

	async function loadUnifiedPlugins(): Promise<void> {
		unifiedLoading = true;
		unifiedError = null;
		try {
			unifiedPlugins = await apiClient.listPlugins();
		} catch (cause) {
			unifiedError = getErrorMessage(cause, 'Could not read the installed plugins.');
		} finally {
			unifiedLoading = false;
		}
	}

	async function loadIndicatorDetails(): Promise<void> {
		try {
			const plugins = await apiClient.listIndicatorPlugins();
			indicatorDetails = new Map(plugins.map((plugin) => [plugin.id, plugin]));
		} catch {
			// Detail is a nice-to-have layered on top of the unified list —
			// a caller with no grant on `Indicator` still sees every row's
			// name, state and toggle; only the expanded detail is thinner.
			indicatorDetails = new Map();
		}
	}

	async function loadWidgetDetails(): Promise<void> {
		try {
			const response = await listWidgetPlugins();
			widgetDetails = new Map(response.packages.map((pkg) => [pkg.id, pkg]));
		} catch {
			widgetDetails = new Map();
		}
	}

	$effect(() => {
		void loadUnifiedPlugins();
		void loadIndicatorDetails();
		void loadWidgetDetails();
	});

	async function refreshAll(): Promise<void> {
		unifiedLoading = true;
		unifiedError = null;
		try {
			unifiedPlugins = await apiClient.refreshPlugins();
		} catch (cause) {
			unifiedError = getErrorMessage(cause, 'Could not refresh the plugin directory.');
		} finally {
			unifiedLoading = false;
		}
		await loadIndicatorDetails();
		await loadWidgetDetails();
	}

	function isOpen(id: string): boolean {
		return expanded.includes(id);
	}

	function toggleOpen(id: string): void {
		expanded = isOpen(id) ? expanded.filter((n) => n !== id) : [...expanded, id];
	}

	const filteredUnifiedPlugins = $derived(
		unifiedPlugins.filter((plugin) => {
			const matchesFilter =
				unifiedFilter === 'all' || plugin.contributes.includes(unifiedFilter);
			const term = unifiedSearch.trim().toLowerCase();
			const matchesSearch =
				term.length === 0 ||
				plugin.name.toLowerCase().includes(term) ||
				plugin.id.toLowerCase().includes(term);
			return matchesFilter && matchesSearch;
		})
	);

	const anyNeedsRestart = $derived(unifiedPlugins.some((plugin) => plugin.needs_restart));

	function contributionLabel(kind: ContributionKind): string {
		switch (kind) {
			case 'venue':
				return 'Venue';
			case 'trade_adapter':
				return 'Trade adapter';
			case 'dashboard_widget':
				return 'Widget';
			case 'indicator':
				return 'Indicator';
		}
	}

	function kindLabel(plugin: PluginDto): string {
		return plugin.kind === 'static' ? 'Built-in' : 'Installed';
	}

	function unifiedStateDisplay(plugin: PluginDto): StateDisplay {
		if (plugin.state.state === 'failed') {
			return { label: `Failed: ${plugin.state.reason}`, variant: 'destructive' };
		}
		if (plugin.state.state === 'disabled') {
			return { label: 'Disabled', variant: 'outline' };
		}
		return { label: 'Active', variant: 'secondary' };
	}

	/** A built-in cannot be removed at all — a static plugin refuses with
	 * `409` (see `crates/api/src/plugin_handlers.rs::uninstall_plugin`),
	 * and the widget package this server always ships refuses with `400`
	 * (`senken_plugin::widget_package::WidgetPackageError::CannotUninstallBuiltIn`).
	 * Both are worth disabling the control for up front, with a reason,
	 * rather than letting the click round-trip to the server just to learn
	 * that. */
	function undeleteableReason(plugin: PluginDto): string | undefined {
		if (plugin.kind === 'static') {
			return 'Built-in plugins can be disabled but not removed.';
		}
		if (widgetDetails.get(plugin.id)?.is_builtin) {
			return 'This plugin ships with Senken and cannot be removed.';
		}
		return undefined;
	}

	async function setUnifiedEnabled(plugin: PluginDto, enabled: boolean): Promise<void> {
		unifiedTogglingId = plugin.id;
		unifiedError = null;
		try {
			const updated = await apiClient.setPluginEnabled(plugin.id, enabled);
			unifiedPlugins = unifiedPlugins.map((p) => (p.id === updated.id ? updated : p));
			if (plugin.contributes.includes('dashboard_widget')) {
				await refreshWidgetCatalog();
			}
		} catch (cause) {
			unifiedError = getErrorMessage(cause, `Could not ${enabled ? 'enable' : 'disable'} ${plugin.name}.`);
		} finally {
			unifiedTogglingId = null;
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
		installing = true;
		installError = null;
		try {
			const bytes = await file.arrayBuffer();
			await apiClient.installPlugin(bytes);
			await loadUnifiedPlugins();
			await loadIndicatorDetails();
			await loadWidgetDetails();
			await refreshWidgetCatalog();
		} catch (cause) {
			installError = getErrorMessage(cause, `Could not install ${file.name}.`);
		} finally {
			installing = false;
		}
	}

	function requestUninstall(plugin: PluginDto): void {
		confirmTarget = plugin;
	}

	async function confirmUninstall(): Promise<void> {
		const plugin = confirmTarget;
		if (!plugin) return;
		removingId = plugin.id;
		unifiedError = null;
		try {
			await apiClient.uninstallPlugin(plugin.id);
			confirmTarget = null;
			await loadUnifiedPlugins();
			await loadWidgetDetails();
			if (plugin.contributes.includes('dashboard_widget')) {
				await refreshWidgetCatalog();
			}
		} catch (cause) {
			unifiedError = getErrorMessage(cause, `Could not remove ${plugin.name}.`);
			confirmTarget = null;
		} finally {
			removingId = null;
		}
	}

	// -----------------------------------------------------------------
	// Detail sourced from the legacy indicator-plugin catalogue — see this
	// file's own top-of-script docs for why this is matched by id rather
	// than rendered as its own section.
	// -----------------------------------------------------------------

	/** Whether `plugin` carries a real catalogue descriptor — true for
	 * `active`, `disabled` and `auto_disabled` (all three loaded the
	 * component far enough to read one), false for `incompatible` and
	 * `failed_to_load` (which never did). */
	function hasDescriptor(plugin: IndicatorPluginDto): boolean {
		return plugin.state === 'active' || plugin.state === 'disabled' || plugin.state === 'auto_disabled';
	}

	function indicatorStateDisplay(plugin: IndicatorPluginDto): StateDisplay {
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
</script>

<div class="flex flex-col gap-6">
	<section class="flex flex-col" data-testid="unified-plugins-section">
		<header class="mb-2 flex items-start justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">
					Plugins
				</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Every venue, trade adapter, widget and indicator plugin this server knows about — the
					ten built-in indicators from the chart's own picker are not listed here.
				</p>
			</div>
			<div class="flex flex-none items-center gap-2">
				<input
					bind:this={fileInput}
					type="file"
					accept=".zip,.wasm"
					class="hidden"
					onchange={onFileChosen}
					data-testid="unified-plugins-file-input"
				/>
				<Button
					variant="outline"
					size="sm"
					onclick={pickFile}
					disabled={installing}
					data-testid="unified-plugins-install"
				>
					{#if installing}
						<Spinner class="size-3.5" />
					{:else}
						<UploadIcon class="size-3.5" />
					{/if}
					Install…
				</Button>
				<Button
					variant="outline"
					size="sm"
					onclick={() => void refreshAll()}
					disabled={unifiedLoading}
				>
					<RefreshIcon class="size-3.5" />
					Refresh
				</Button>
			</div>
		</header>

		{#if installError}
			<p data-testid="unified-plugins-install-error" class="mb-2 text-[12.5px] text-destructive">
				{installError}
			</p>
		{/if}

		{#if anyNeedsRestart}
			<div
				class="mb-2 flex items-center gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300"
				data-testid="unified-plugins-restart-banner"
			>
				<RefreshCcwDotIcon class="size-3.5 flex-none" />
				<span class="flex-1">
					Changes to a built-in plugin only take effect after Senken restarts.
				</span>
			</div>
		{/if}

		<div class="mb-2 flex flex-wrap items-center gap-2">
			<div class="relative">
				<SearchIcon class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-dim2" />
				<Input
					bind:value={unifiedSearch}
					placeholder="Search plugins…"
					class="h-8 w-[200px] pl-7 text-[12.5px]"
					data-testid="unified-plugins-search"
				/>
			</div>
			<div class="flex flex-wrap items-center gap-1">
				{#each UNIFIED_FILTERS as filterOption (filterOption.value)}
					<Button
						variant={unifiedFilter === filterOption.value ? 'secondary' : 'ghost'}
						size="sm"
						onclick={() => (unifiedFilter = filterOption.value)}
						data-testid={`unified-plugins-filter-${filterOption.value}`}
					>
						{filterOption.label}
					</Button>
				{/each}
			</div>
			<span class="ml-auto text-[11.5px] text-dim2" data-testid="unified-plugins-count">
				{filteredUnifiedPlugins.length} of {unifiedPlugins.length} shown
			</span>
		</div>

		{#if unifiedError}
			<p data-testid="unified-plugins-error" class="mb-2 text-[12.5px] text-destructive">
				{unifiedError}
			</p>
		{/if}

		{#if unifiedLoading && unifiedPlugins.length === 0}
			<div class="flex items-center gap-2 py-8">
				<Spinner class="size-3.5" />
				<span class="font-mono text-[11px] tracking-[0.14em] text-dim2">LOADING…</span>
			</div>
		{:else if filteredUnifiedPlugins.length === 0}
			<div class="flex flex-col items-center justify-center gap-3 border border-border py-12 text-center">
				<PuzzleIcon class="size-8 text-dim" />
				<p class="max-w-sm text-[13px] text-dim2">
					{unifiedPlugins.length === 0
						? 'No plugin is registered on this server yet.'
						: 'No plugin matches this search or filter.'}
				</p>
			</div>
		{:else}
			<div class="border border-border" data-testid="unified-plugins-list">
				{#each filteredUnifiedPlugins as plugin (plugin.id)}
					{@const state = unifiedStateDisplay(plugin)}
					{@const indicatorDetail = indicatorDetails.get(plugin.id)}
					{@const widgetDetail = widgetDetails.get(plugin.id)}
					{@const deleteReason = undeleteableReason(plugin)}
					<div class="border-b border-border last:border-b-0">
						<div class="flex items-center justify-between gap-3 px-3 py-2.5">
							<button
								type="button"
								class="flex min-w-0 flex-1 items-center gap-2 text-left"
								onclick={() => toggleOpen(plugin.id)}
								data-testid={`unified-plugin-row-${plugin.id}`}
							>
								{#if isOpen(plugin.id)}
									<ChevronDownIcon class="size-3.5 flex-none text-dim2" />
								{:else}
									<ChevronRightIcon class="size-3.5 flex-none text-dim2" />
								{/if}
								<span class="flex min-w-0 flex-col">
									<span class="truncate text-[13px] font-medium text-foreground">{plugin.name}</span>
									<span class="truncate font-mono text-[11px] text-dim2">
										{plugin.id}{plugin.version ? `@${plugin.version}` : ''}
									</span>
								</span>
								<Badge variant={state.variant} class={state.class} data-testid={`unified-plugin-state-${plugin.id}`}>
									{state.label}
								</Badge>
								{#if plugin.needs_restart}
									<Badge
										variant="outline"
										class={WARNING_BADGE_CLASS}
										title={plugin.restart_reason ?? undefined}
										data-testid={`unified-plugin-needs-restart-${plugin.id}`}
									>
										{plugin.restart_reason ?? 'Restart to apply'}
									</Badge>
								{/if}
								<Badge variant="outline" data-testid={`unified-plugin-origin-${plugin.id}`}>
									{kindLabel(plugin)}
								</Badge>
								{#each plugin.contributes as kind (kind)}
									<Badge variant="outline" data-testid={`unified-plugin-contributes-${plugin.id}-${kind}`}>
										{contributionLabel(kind)}
									</Badge>
								{/each}
							</button>
							<div class="flex flex-none items-center gap-2">
								{#if unifiedTogglingId === plugin.id}
									<Spinner class="size-3.5" />
								{:else}
									<Switch
										size="sm"
										checked={plugin.enabled}
										onCheckedChange={() => void setUnifiedEnabled(plugin, !plugin.enabled)}
										aria-label={`${plugin.enabled ? 'Disable' : 'Enable'} ${plugin.name}`}
										data-testid={`unified-plugin-toggle-${plugin.id}`}
									/>
								{/if}
								<Button
									variant="ghost"
									size="icon-sm"
									aria-label={`Uninstall ${plugin.name}`}
									title={deleteReason}
									aria-disabled={Boolean(deleteReason)}
									class={deleteReason ? 'cursor-not-allowed opacity-40' : undefined}
									onclick={() => {
										if (deleteReason) return;
										requestUninstall(plugin);
									}}
									disabled={removingId === plugin.id}
									data-testid={`unified-plugin-uninstall-${plugin.id}`}
								>
									{#if removingId === plugin.id}
										<Spinner class="size-3.5" />
									{:else}
										<Trash2Icon class="size-3.5" />
									{/if}
								</Button>
							</div>
						</div>

						{#if isOpen(plugin.id)}
							<div
								class="flex flex-col gap-4 border-t border-border bg-ink/2 px-3 py-3"
								data-testid={`unified-plugin-detail-${plugin.id}`}
							>
								{#if plugin.state.state === 'failed' && !indicatorDetail && !widgetDetail}
									<div
										class="flex items-start gap-2 border border-destructive/40 bg-destructive/10 px-2.5 py-2 text-[12px] text-destructive"
										data-testid={`unified-plugin-reason-${plugin.id}`}
									>
										<OctagonXIcon class="mt-0.5 size-3.5 flex-none" />
										<div>
											<div class="font-medium">This plugin failed to load.</div>
											<div class="mt-0.5">{plugin.state.reason}</div>
										</div>
									</div>
								{/if}

								{#if indicatorDetail}
									{@const descriptor = hasDescriptor(indicatorDetail)}
									{@const indState = indicatorStateDisplay(indicatorDetail)}
									<div class="flex items-center gap-2">
										<span class="text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Indicator plugin
										</span>
										<Badge variant={indState.variant} class={indState.class}>{indState.label}</Badge>
									</div>

									{#if indicatorDetail.state === 'incompatible'}
										<div
											class="flex items-start gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300"
											data-testid={`unified-plugin-reason-${plugin.id}`}
										>
											<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
											<div>
												<div class="font-medium">This plugin's API version is not supported.</div>
												<div class="mt-0.5 text-amber-300/80">
													Found <span class="font-mono">{indicatorDetail.found_version}</span>,
													this server supports
													<span class="font-mono">{indicatorDetail.supported_version}</span>.
													Update the plugin, or update Senken.
												</div>
											</div>
										</div>
									{:else if indicatorDetail.state === 'failed_to_load'}
										<div
											class="flex items-start gap-2 border border-destructive/40 bg-destructive/10 px-2.5 py-2 text-[12px] text-destructive"
											data-testid={`unified-plugin-reason-${plugin.id}`}
										>
											<OctagonXIcon class="mt-0.5 size-3.5 flex-none" />
											<div>
												<div class="font-medium">This plugin failed to load.</div>
												<div class="mt-0.5">{indicatorDetail.reason}</div>
											</div>
										</div>
									{:else if indicatorDetail.state === 'auto_disabled'}
										<div
											class="flex items-start gap-2 border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 text-[12px] text-amber-300"
											data-testid={`unified-plugin-reason-${plugin.id}`}
										>
											<ZapOffIcon class="mt-0.5 size-3.5 flex-none" />
											<div>
												<div class="font-medium">
													Turned off automatically after repeated failures.
												</div>
												<div class="mt-0.5 text-amber-300/80">{indicatorDetail.reason}</div>
											</div>
										</div>
									{/if}

									{#if descriptor}
										<div class="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] sm:grid-cols-3">
											<div>
												<div class="text-dim2">Short title</div>
												<div class="font-mono text-foreground">{indicatorDetail.short_title}</div>
											</div>
											<div>
												<div class="text-dim2">Placement</div>
												<div class="text-foreground">{indicatorDetail.placement}</div>
											</div>
											<div>
												<div class="text-dim2">Warm-up bars</div>
												<div class="text-foreground">{indicatorDetail.warmup_bars}</div>
											</div>
										</div>

										<div>
											<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
												Parameters
											</div>
											{#if indicatorDetail.params.length === 0}
												<p class="text-[12px] text-dim2">This indicator takes no parameters.</p>
											{:else}
												<ul class="flex flex-col gap-0.5">
													{#each indicatorDetail.params as param (param.name)}
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
												{#each indicatorDetail.plots as plot (plot.field)}
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
										{#if indicatorDetail.health}
											<div class="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] sm:grid-cols-4">
												<div>
													<div class="text-dim2">Traps</div>
													<div class="font-mono text-foreground">{indicatorDetail.health.trap_count}</div>
												</div>
												<div>
													<div class="text-dim2">Deadlines exceeded</div>
													<div class="font-mono text-foreground">
														{indicatorDetail.health.deadline_exceeded_count}
													</div>
												</div>
												<div>
													<div class="text-dim2">Peak memory</div>
													<div class="font-mono text-foreground">
														{formatBytes(indicatorDetail.health.peak_memory_bytes)}
													</div>
												</div>
												<div>
													<div class="text-dim2">Circuit breaker</div>
													<div class="text-foreground">
														{indicatorDetail.health.circuit.state === 'open' ? 'Open' : 'Closed'}
													</div>
												</div>
											</div>
											{#if indicatorDetail.health.circuit.state === 'open'}
												<p class="mt-1.5 text-[12px] text-amber-300">{indicatorDetail.health.circuit.reason}</p>
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
										{#if indicatorDetail.logs.length === 0}
											<p class="text-[12px] text-dim2" data-testid={`unified-plugin-logs-empty-${plugin.id}`}>
												No log lines recorded for this plugin yet.
											</p>
										{:else}
											<p class="mb-1 text-[11px] text-dim2">
												Times shown in {formatInstant(indicatorDetail.logs[0].timestamp, userZoneStore.zone)
													.zoneLabel} ({userZoneStore.zone}).
											</p>
											<ul class="flex flex-col gap-1" data-testid={`unified-plugin-logs-${plugin.id}`}>
												{#each indicatorDetail.logs as line, index (index)}
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
								{:else if widgetDetail}
									<div>
										<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Widget plugin
										</div>
										<div class="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] sm:grid-cols-3">
											<div>
												<div class="text-dim2">Description</div>
												<div class="text-foreground">
													{widgetDetail.description || 'No description given.'}
												</div>
											</div>
											<div>
												<div class="text-dim2">Manifest digest</div>
												<div class="truncate font-mono text-foreground">{widgetDetail.digest}</div>
											</div>
											<div>
												<div class="text-dim2">Widgets contributed</div>
												<div class="text-foreground">{widgetDetail.widget_count}</div>
											</div>
										</div>
									</div>

									<div>
										<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Runtime health
										</div>
										<p class="text-[12px] text-dim2">
											Widget plugins run entirely in your own browser, not on this server — there
											is no runtime health to report yet.
										</p>
									</div>

									<div>
										<div class="mb-1 text-[11px] font-medium tracking-[0.05em] text-dim2 uppercase">
											Logs
										</div>
										<p class="text-[12px] text-dim2" data-testid={`unified-plugin-logs-empty-${plugin.id}`}>
											Widget plugins do not report a log yet.
										</p>
									</div>
								{:else}
									<p class="text-[12px] text-dim2" data-testid={`unified-plugin-no-detail-${plugin.id}`}>
										No additional detail is available for this plugin yet.
									</p>
								{/if}
							</div>
						{/if}
					</div>
				{/each}
			</div>
		{/if}
	</section>
</div>

<ConfirmDialog
	open={confirmTarget !== null}
	title="Uninstall plugin"
	description={confirmTarget
		? `Uninstall ${confirmTarget.name}? Market data already downloaded is kept.`
		: ''}
	confirmLabel="Uninstall"
	onConfirm={() => void confirmUninstall()}
	onOpenChange={(open) => {
		if (!open) confirmTarget = null;
	}}
/>
