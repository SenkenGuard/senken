<script lang="ts">
	// Small, real (not mock) connection indicator — the "any small
	// edit to layout/ needed to surface connection state." Deliberately
	// tiny: the settings modal's server picker and the login page
	// are out of scope for now, so this is just enough for the
	// state machine and its insecure-connection warning to be visible
	// somewhere, not a redesign of TopBar/FooterBar's existing (still
	// entirely mock,) status widgets.
	import { connectionStore } from '$lib/api/connection.svelte';
	import { activeServer, isSecureConnection } from '$lib/api/servers.svelte';
	import { cn } from '$lib/utils.js';

	const DOT_CLASS: Record<string, string> = {
		disconnected: 'bg-loss',
		connecting: 'bg-dim2',
		authenticated: 'bg-gain',
		reconnecting: 'bg-dim2 animate-pulse'
	};

	const LABEL: Record<string, string> = {
		disconnected: 'DISCONNECTED',
		connecting: 'CONNECTING',
		authenticated: 'CONNECTED',
		reconnecting: 'RECONNECTING'
	};

	const dotClass = $derived(DOT_CLASS[connectionStore.state]);
	const label = $derived(LABEL[connectionStore.state]);
	// Re-derives off `activeServer()`, which itself reads `serversStore`'s
	// runes — this recomputes whenever the selected server changes, not
	// just once at mount.
	const insecure = $derived(!isSecureConnection(activeServer()));
</script>

<div
	data-tauri-drag-region="false"
	data-testid="connection-status"
	data-connection-state={connectionStore.state}
	class="flex items-center gap-1.5"
	title={connectionStore.lastError ?? label}
>
	<span data-testid="connection-status-dot" class={cn('size-1.5 rounded-full', dotClass)}></span>
	<span class="font-mono text-[9px] tracking-[0.16em] text-dim2">{label}</span>
	{#if insecure}
		<span
			data-testid="connection-status-insecure"
			class="font-mono text-[9px] tracking-[0.16em] text-loss"
			title="This connection is neither loopback nor HTTPS — credentials would be sent in the clear."
		>
			⚠ INSECURE
		</span>
	{/if}
</div>
