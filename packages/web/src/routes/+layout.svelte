<script lang="ts">
	import './layout.css';
	import favicon from '$lib/assets/favicon.svg';
	import { ModeWatcher } from 'mode-watcher';
	import { AppShell } from '$lib/components/layout/index.js';

	let { children } = $props();
</script>

<svelte:head><link rel="icon" href={favicon} /></svelte:head>

<!-- Applies the persisted light/dark class before paint, so the terminal
     does not flash white on load. -->
<ModeWatcher defaultMode="dark" />
<AppShell>
	{@render children()}
</AppShell>
