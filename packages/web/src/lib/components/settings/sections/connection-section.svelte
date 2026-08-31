<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Servers',
			rowId: 'server-list',
			rowLabel: 'Connected servers',
			rowDescription: 'Add, remove, and switch between Senken servers.'
		},
		{
			groupHeading: 'Connection',
			rowId: 'connection-status',
			rowLabel: 'Status',
			rowDescription: 'Whether this device is currently connected.'
		}
	];
</script>

<script lang="ts">
	// Connection / server settings (the "connection/server"
	// section). Entirely real — everything here is the
	// already-built connection layer (`$lib/api/servers.svelte.ts`,
	// `connection.svelte.ts`, `credential-store.ts`), just surfaced as UI.
	// No new state is introduced here; this section reads and drives the
	// same module-level stores the WS layer and the connection-status
	// indicator already read.
	import {
		serversStore,
		activeServer,
		addServer,
		removeServer,
		resolveBaseUrl,
		isSecureConnection,
		EMBEDDED_SERVER,
		type ServerConfig
	} from '$lib/api/servers.svelte';
	import { switchServer } from '$lib/api/client';
	import { connectionStore, type ConnectionState } from '$lib/api/connection.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import TrashIcon from '@lucide/svelte/icons/trash-2';
	import PlusIcon from '@lucide/svelte/icons/plus';

	let addOpen = $state(false);
	let newLabel = $state('');
	let newUrl = $state('');
	let addError = $state<string | null>(null);

	const stateLabel: Record<ConnectionState, string> = {
		disconnected: 'Disconnected',
		connecting: 'Connecting…',
		authenticated: 'Connected',
		reconnecting: 'Reconnecting…'
	};

	const stateTone: Record<ConnectionState, string> = {
		disconnected: 'text-loss',
		connecting: 'text-dim2',
		authenticated: 'text-gain',
		reconnecting: 'text-dim2'
	};

	function displayUrl(server: ServerConfig): string {
		return server.baseUrl === '' ? `${resolveBaseUrl(server)} (this device)` : server.baseUrl;
	}

	function pick(server: ServerConfig) {
		if (server.id === serversStore.activeId) return;
		switchServer(server.id);
	}

	function remove(server: ServerConfig) {
		removeServer(server.id);
	}

	function submitAdd(event: SubmitEvent) {
		event.preventDefault();
		addError = null;
		const label = newLabel.trim();
		const url = newUrl.trim();
		if (label === '' || url === '') {
			addError = 'Both a label and a URL are required.';
			return;
		}
		let normalized: string;
		try {
			normalized = new URL(url).origin;
		} catch {
			addError = 'Enter a full URL, e.g. https://trading.example.com:4206.';
			return;
		}
		const server = addServer({ label, baseUrl: normalized });
		newLabel = '';
		newUrl = '';
		addOpen = false;
		switchServer(server.id);
	}
</script>

{#snippet statusControl()}
	<span class={`font-mono text-[11px] tracking-[0.08em] ${stateTone[connectionStore.state]}`}>
		{stateLabel[connectionStore.state]}
	</span>
{/snippet}

<div class="flex flex-col gap-6">
	<section class="flex flex-col">
		<header class="mb-1 flex items-center justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Servers</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Connecting to a different Senken server does not require a restart.
				</p>
			</div>
			<Button
				variant="outline"
				size="sm"
				onclick={() => {
					addOpen = !addOpen;
					addError = null;
				}}
			>
				<PlusIcon class="size-3.5" />
				Add server
			</Button>
		</header>

		{#if addOpen}
			<form onsubmit={submitAdd} class="mb-2 flex flex-col gap-2 border border-ink/14 bg-card2 p-3">
				<div class="flex flex-wrap gap-2">
					<Input placeholder="Label, e.g. Office server" bind:value={newLabel} class="w-[200px]" />
					<Input placeholder="https://host:port" bind:value={newUrl} class="w-[260px]" />
					<Button type="submit" size="sm">Connect</Button>
				</div>
				{#if addError}<p class="text-[11.5px] text-destructive">{addError}</p>{/if}
			</form>
		{/if}

		<div class="divide-y divide-border">
			{#each serversStore.list as server (server.id)}
				{@const active = server.id === serversStore.activeId}
				{@const secure = isSecureConnection(server)}
				<div class="flex items-start justify-between gap-6 py-3">
					<div class="flex min-w-0 flex-1 flex-col gap-0.5 pt-0.5">
						<span class="flex items-center gap-2 text-[13px] font-medium text-foreground">
							{server.label}
							{#if active}
								<span
									class="border border-foreground/30 px-1.5 py-[1px] font-mono text-[8.5px] tracking-[0.14em] text-dim2"
								>
									ACTIVE
								</span>
							{/if}
						</span>
						<span class="font-mono text-[12px] text-dim2">{displayUrl(server)}</span>
						{#if !secure}
							<span class="mt-0.5 flex items-center gap-1.5 text-[11.5px] text-destructive">
								<TriangleAlertIcon class="size-3.5 flex-none" />
								Not loopback or HTTPS — credentials for this server travel in the clear.
							</span>
						{/if}
					</div>
					<div class="flex flex-none items-center gap-2">
						{#if !active}
							<Button variant="outline" size="sm" onclick={() => pick(server)}>Connect</Button>
						{/if}
						{#if serversStore.list.length > 1}
							<Button
								variant="ghost"
								size="icon-sm"
								onclick={() => remove(server)}
								aria-label={`Remove ${server.label}`}
							>
								<TrashIcon class="size-3.5 text-dim2" />
							</Button>
						{/if}
					</div>
				</div>
			{/each}
		</div>
		{#if serversStore.list.length === 1 && serversStore.list[0].id === EMBEDDED_SERVER.id}
			<p class="mt-1 text-[11.5px] text-dim">
				There must always be at least one server — the last one cannot be removed.
			</p>
		{/if}
	</section>

	<section class="flex flex-col">
		<header class="mb-1">
			<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Connection</h3>
			<p class="mt-0.5 text-[12px] text-dim2">Live status for the active server.</p>
		</header>
		<div class="divide-y divide-border">
			<div class="flex items-start justify-between gap-6 py-3">
				<div class="flex min-w-0 flex-1 flex-col gap-0.5 pt-0.5">
					<span class="text-[13px] font-medium text-foreground">Status</span>
					<span class="text-[12px] leading-snug text-dim2">
						{connectionStore.lastError ?? `Talking to ${activeServer().label}.`}
					</span>
				</div>
				<div class="flex flex-none items-center gap-2">{@render statusControl()}</div>
			</div>
		</div>
	</section>
</div>
