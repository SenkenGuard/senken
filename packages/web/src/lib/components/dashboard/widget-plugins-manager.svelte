<script lang="ts">
	// Install, list, enable/disable, refresh and remove dynamic widget UI
	// packages. Lives here (a dashboard component) rather than in
	// Settings → Plugins: that page manages a different kind of plugin
	// entirely (compiled `.wasm` indicators, `crates/api/src/indicator_handlers.rs`),
	// and this platform's widget UI packages are a deliberately different
	// shape — a manifest plus a static bundle, never compiled or executed
	// by this server at all (see `senken_plugin::widget_package`'s own
	// module docs). The upload/list/enable/disable shape below mirrors that
	// page's own conventions on purpose, since a plugin author or an admin
	// managing plugins should not have to learn a second interaction
	// pattern for a second kind of plugin.
	import { toast } from 'svelte-sonner';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import { getErrorMessage } from '$lib/api/errors';
	import {
		installWidgetPlugin,
		listWidgetPlugins,
		refreshWidgetPlugins,
		setWidgetPluginEnabled,
		uninstallWidgetPlugin,
		type WidgetPluginPackage
	} from './api';
	import UploadIcon from '@lucide/svelte/icons/upload';
	import RefreshIcon from '@lucide/svelte/icons/refresh-cw';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import PuzzleIcon from '@lucide/svelte/icons/puzzle';

	let {
		open,
		onClose,
		/** Called after any mutation (install/enable/disable/uninstall) that
		 * could change the effective catalog, so the caller can re-fetch it
		 * and re-register plugin widget renderers. */
		onCatalogChanged
	}: {
		open: boolean;
		onClose: () => void;
		onCatalogChanged: () => void;
	} = $props();

	let packages = $state<WidgetPluginPackage[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let uploading = $state(false);
	let uploadError = $state<string | null>(null);
	let togglingId = $state<string | null>(null);
	let removingId = $state<string | null>(null);
	let fileInput = $state<HTMLInputElement | null>(null);

	async function load(): Promise<void> {
		loading = true;
		error = null;
		try {
			const response = await listWidgetPlugins();
			packages = response.packages;
		} catch (cause) {
			error = getErrorMessage(cause, 'Could not read the installed widget plugins.');
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		if (open) void load();
	});

	function statusLabel(pkg: WidgetPluginPackage): string {
		if (!pkg.enabled) return 'Disabled';
		if (pkg.status.state === 'failed') return 'Failed';
		return 'Active';
	}

	function statusVariant(pkg: WidgetPluginPackage): 'secondary' | 'outline' | 'destructive' {
		if (pkg.status.state === 'failed') return 'destructive';
		return pkg.enabled ? 'secondary' : 'outline';
	}

	async function toggle(pkg: WidgetPluginPackage): Promise<void> {
		togglingId = pkg.id;
		error = null;
		try {
			await setWidgetPluginEnabled(pkg.id, !pkg.enabled);
			await load();
			onCatalogChanged();
		} catch (cause) {
			error = getErrorMessage(cause, `Could not ${pkg.enabled ? 'disable' : 'enable'} ${pkg.id}.`);
		} finally {
			togglingId = null;
		}
	}

	async function remove(pkg: WidgetPluginPackage): Promise<void> {
		removingId = pkg.id;
		error = null;
		try {
			await uninstallWidgetPlugin(pkg.id);
			await load();
			onCatalogChanged();
		} catch (cause) {
			error = getErrorMessage(cause, `Could not remove ${pkg.id}.`);
		} finally {
			removingId = null;
		}
	}

	async function refresh(): Promise<void> {
		loading = true;
		error = null;
		try {
			const response = await refreshWidgetPlugins();
			packages = response.packages;
			onCatalogChanged();
		} catch (cause) {
			error = getErrorMessage(cause, 'Could not refresh the widget plugin directory.');
		} finally {
			loading = false;
		}
	}

	function pickFile(): void {
		fileInput?.click();
	}

	async function onFileChosen(event: Event): Promise<void> {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file) return;
		uploading = true;
		uploadError = null;
		try {
			const bytes = await file.arrayBuffer();
			await installWidgetPlugin(bytes);
			await load();
			onCatalogChanged();
			toast.success(`Installed ${file.name}.`);
		} catch (cause) {
			uploadError = getErrorMessage(cause, `Could not install ${file.name}.`);
		} finally {
			uploading = false;
		}
	}
</script>

<Dialog.Root
	{open}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={true}
		class="w-[560px] max-w-[92%] gap-0 border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
			<Dialog.Title class="text-[13px] font-medium tracking-[0.03em] text-foreground">
				Widget plugins
			</Dialog.Title>
			<div class="mr-6 flex flex-none items-center gap-2">
				<input
					bind:this={fileInput}
					type="file"
					accept=".zip"
					class="hidden"
					onchange={onFileChosen}
				/>
				<Button variant="outline" size="sm" onclick={pickFile} disabled={uploading}>
					{#if uploading}
						<Spinner class="size-3.5" />
					{:else}
						<UploadIcon class="size-3.5" />
					{/if}
					Install
				</Button>
				<Button variant="outline" size="sm" onclick={() => void refresh()} disabled={loading}>
					<RefreshIcon class="size-3.5" />
					Refresh
				</Button>
			</div>
		</Dialog.Header>

		{#if uploadError}
			<p data-testid="widget-plugins-upload-error" class="border-t border-ink/7 px-4 py-2 text-[12px] text-destructive">
				{uploadError}
			</p>
		{/if}

		<div class="max-h-[420px] overflow-y-auto border-t border-ink/7">
			{#if loading && packages.length === 0}
				<div class="flex items-center gap-2 px-4 py-8">
					<Spinner class="size-3.5" />
					<span class="font-mono text-[11px] tracking-[0.14em] text-dim2">LOADING…</span>
				</div>
			{:else if error}
				<p data-testid="widget-plugins-error" class="px-4 py-4 text-[12.5px] text-destructive">
					{error}
				</p>
			{:else if packages.length === 0}
				<div class="flex flex-col items-center justify-center gap-3 px-4 py-12 text-center">
					<PuzzleIcon class="size-8 text-dim" />
					<p class="max-w-sm text-[13px] text-dim2">
						No widget plugin package has been installed on this server yet.
					</p>
				</div>
			{:else}
				<div data-testid="widget-plugins-list">
					{#each packages as pkg (pkg.id)}
						<div
							class="flex items-center justify-between gap-3 border-b border-ink/[0.045] px-4 py-2.5 last:border-b-0"
							data-testid={`widget-plugin-row-${pkg.id}`}
						>
							<div class="flex min-w-0 flex-1 flex-col gap-0.5">
								<div class="flex items-center gap-2">
									<span class="truncate text-[13px] font-medium text-foreground">{pkg.name}</span>
									<Badge variant={statusVariant(pkg)} data-testid={`widget-plugin-state-${pkg.id}`}>
										{statusLabel(pkg)}
									</Badge>
								</div>
								<span class="truncate font-mono text-[11px] text-dim2">
									{pkg.id}@{pkg.version} · {pkg.widget_count}
									{pkg.widget_count === 1 ? 'widget' : 'widgets'}
								</span>
								{#if pkg.status.state === 'failed'}
									<span class="truncate text-[11px] text-destructive">{pkg.status.reason}</span>
								{/if}
							</div>
							<div class="flex flex-none items-center gap-2">
								{#if togglingId === pkg.id}
									<Spinner class="size-3.5" />
								{:else}
									<Switch
										size="sm"
										checked={pkg.enabled}
										onCheckedChange={() => void toggle(pkg)}
										data-testid={`widget-plugin-toggle-${pkg.id}`}
									/>
								{/if}
								<Button
									variant="ghost"
									size="icon-sm"
									aria-label={`Remove ${pkg.name}`}
									onclick={() => void remove(pkg)}
									disabled={removingId === pkg.id}
								>
									{#if removingId === pkg.id}
										<Spinner class="size-3.5" />
									{:else}
										<Trash2Icon class="size-3.5" />
									{/if}
								</Button>
							</div>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</Dialog.Content>
</Dialog.Root>
