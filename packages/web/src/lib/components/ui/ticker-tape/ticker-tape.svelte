<script lang="ts" generics="T">
	import { cn } from "$lib/utils.js";
	import type { Snippet } from "svelte";

	let {
		items,
		item,
		durationSeconds = 46,
		class: className,
	}: {
		items: T[];
		item: Snippet<[T]>;
		durationSeconds?: number;
		class?: string;
	} = $props();

	// Doubling the list and animating exactly -50% is what makes the loop
	// seamless — the second copy is already in place when the first scrolls
	// out of view.
	const looped = $derived([...items, ...items]);
</script>

<div data-slot="ticker-tape" class={cn("flex min-w-0 flex-1 items-center overflow-hidden", className)}>
	<div
		class="flex items-center gap-[26px] whitespace-nowrap"
		style="animation: ticker-tape-scroll {durationSeconds}s linear infinite;"
	>
		{#each looped as entry, i (i)}
			{@render item(entry)}
		{/each}
	</div>
</div>

<style>
	@keyframes ticker-tape-scroll {
		from {
			transform: translateX(0);
		}
		to {
			transform: translateX(-50%);
		}
	}
</style>
