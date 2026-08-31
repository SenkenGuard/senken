<script lang="ts">
	// A heading + one-line description + its rows ("the right pane is a scrolling list of settings grouped under a heading with a one-line description"). Purely presentational — a section supplies
	// its own `SettingsGroup` data (see `$lib/state/settings-registry.svelte.ts`)
	// and this just lays it out consistently across every section.
	import SettingsRow from './settings-row.svelte';
	import type { SettingsGroup } from '$lib/state/settings-registry.svelte';

	let { group }: { group: SettingsGroup } = $props();
</script>

<section class="flex flex-col">
	<header class="mb-1">
		<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">{group.heading}</h3>
		<p class="mt-0.5 text-[12px] text-dim2">{group.description}</p>
	</header>
	<div class="divide-y divide-border">
		{#each group.rows as row (row.id)}
			<SettingsRow
				id={row.id}
				label={row.label}
				description={row.description}
				control={row.control}
				changed={row.changed}
				onReset={row.onReset}
			/>
		{/each}
	</div>
</section>
