<script lang="ts">
	// The server picker: "login must let the user choose
	// which server to connect to, add one, and switch." Lives inside
	// `routes/login/` rather than `$lib/components/layout/` because nothing
	// outside the login page needs it — the authenticated shell shows the
	// *current* server via `connection-status.svelte`, but changing it is a
	// setup-time action, not a running-session one.
	//
	// Reads and mutates `serversStore` directly (`$lib/api/servers.svelte`)
	// and switches through `switchServer` (`$lib/api/client`) — the same
	// two entry points Q3 built and documented as the only way to change
	// which server the app talks to. This component adds no new state of
	// its own beyond the popover's open flag and the add-server form's
	// draft fields.
	import { serversStore, activeServer, addServer, removeServer, isSecureConnection, serverAddressLabel } from '$lib/api/servers.svelte';
	import { switchServer } from '$lib/api/client';
	import { cn } from '$lib/utils.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';
	import CheckIcon from '@lucide/svelte/icons/check';

	let open = $state(false);
	let adding = $state(false);
	let draftLabel = $state('');
	let draftUrl = $state('');
	let addError = $state<string | null>(null);

	const current = $derived(activeServer());

	function select(id: string) {
		switchServer(id);
		open = false;
	}

	function startAdd() {
		adding = true;
		draftLabel = '';
		draftUrl = '';
		addError = null;
	}

	function submitAdd(event: SubmitEvent) {
		event.preventDefault();
		const label = draftLabel.trim();
		const baseUrl = draftUrl.trim();
		if (!label || !baseUrl) {
			addError = 'Both a name and a URL are required.';
			return;
		}
		// `resolveBaseUrl`/`isSecureUrl` (servers.svelte.ts, security.ts) both
		// parse this with `new URL(...)`, so an unparsable value fails there,
		// silently, the same way an unreachable one would only fail at the
		// next heartbeat tick. Failing here, immediately, with a message the
		// user can act on is worth the duplicate parse.
		try {
			void new URL(baseUrl);
		} catch {
			addError = 'Enter a full URL, e.g. https://trading.example.com:4180.';
			return;
		}
		const server = addServer({ label, baseUrl });
		adding = false;
		select(server.id);
	}

	function cancelAdd() {
		adding = false;
		addError = null;
	}
</script>

<Popover.Root bind:open>
	<Popover.Trigger
		data-testid="server-picker-trigger"
		class={cn(
			'flex w-full items-center justify-between border border-ink/16 bg-transparent px-3 py-2 text-left transition-colors hover:bg-ink/5',
			open && 'border-foreground'
		)}
	>
		<span class="flex min-w-0 items-center gap-2">
			<span
				class={cn('size-1.5 flex-none rounded-full', isSecureConnection(current) ? 'bg-gain' : 'bg-loss')}
			></span>
			<span class="truncate font-mono text-[11px] tracking-[0.06em] text-foreground">
				{current.label}
			</span>
		</span>
		<ChevronDownIcon class="size-3.5 flex-none text-dim2" />
	</Popover.Trigger>
	<Popover.Content align="start" sideOffset={6} class="w-[320px] border border-ink/18 bg-popover p-0">
		<div class="border-b border-ink/7 px-3 py-2.5 font-mono text-[8px] tracking-[0.24em] text-dim">
			CONNECT TO
		</div>
		<div class="max-h-[220px] overflow-y-auto">
			{#each serversStore.list as server (server.id)}
				{@const active = server.id === serversStore.activeId}
				<div
					class={cn(
						'group flex items-center gap-2 border-b border-ink/[4.5%] px-3 py-2 last:border-b-0',
						active ? 'bg-ink/6' : 'hover:bg-ink/[3.5%]'
					)}
				>
					<button
						type="button"
						class="flex min-w-0 flex-1 items-center gap-2 text-left"
						onclick={() => select(server.id)}
					>
						<span class="flex size-3.5 flex-none items-center justify-center">
							{#if active}
								<CheckIcon class="size-3 text-foreground" />
							{/if}
						</span>
						<span class="min-w-0 flex-1">
							<span class="block truncate font-mono text-[11px] tracking-[0.04em] text-foreground">
								{server.label}
							</span>
							<span class="block truncate font-mono text-[9px] text-dim2">
								{serverAddressLabel(server)}
							</span>
						</span>
						<span
							class={cn('size-1.5 flex-none rounded-full', isSecureConnection(server) ? 'bg-gain' : 'bg-loss')}
							title={isSecureConnection(server) ? 'Secure' : 'Insecure — neither loopback nor HTTPS'}
						></span>
					</button>
					{#if serversStore.list.length > 1}
						<button
							type="button"
							class="flex size-5 flex-none items-center justify-center text-dim2 opacity-0 transition-opacity hover:text-loss group-hover:opacity-100"
							title="Remove this server"
							onclick={() => removeServer(server.id)}
						>
							<XIcon class="size-3" />
						</button>
					{/if}
				</div>
			{/each}
		</div>
		{#if adding}
			<form class="border-t border-ink/7 p-3" onsubmit={submitAdd}>
				<div class="flex flex-col gap-2">
					<div class="flex flex-col gap-1">
						<Label for="server-label" class="font-mono text-[9px] tracking-[0.14em] text-dim2">NAME</Label>
						<Input
							id="server-label"
							bind:value={draftLabel}
							placeholder="Trading desk"
							class="h-7 rounded-none border-ink/16 font-mono text-[11px]"
						/>
					</div>
					<div class="flex flex-col gap-1">
						<Label for="server-url" class="font-mono text-[9px] tracking-[0.14em] text-dim2">SERVER URL</Label>
						<Input
							id="server-url"
							bind:value={draftUrl}
							placeholder="https://trading.example.com:4180"
							class="h-7 rounded-none border-ink/16 font-mono text-[11px]"
						/>
					</div>
					{#if addError}
						<p class="font-mono text-[10px] text-loss">{addError}</p>
					{/if}
					<div class="flex justify-end gap-2 pt-1">
						<Button type="button" variant="ghost" size="xs" onclick={cancelAdd}>Cancel</Button>
						<Button type="submit" variant="secondary" size="xs">Add & connect</Button>
					</div>
				</div>
			</form>
		{:else}
			<button
				type="button"
				class="flex w-full items-center gap-2 border-t border-ink/7 px-3 py-2.5 text-left font-mono text-[10px] tracking-[0.1em] text-dim2 hover:bg-ink/[3.5%] hover:text-foreground"
				onclick={startAdd}
			>
				<PlusIcon class="size-3" />
				ADD SERVER
			</button>
		{/if}
	</Popover.Content>
</Popover.Root>
