<script lang="ts">
	// Shown only when the workspace has more than one pane — "Add to chart"
	// with a single pane always means that pane, no dialog needed. Picking
	// a pane is a destination choice, not a form value, so this is a plain
	// list of buttons rather than a select.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { cn } from '$lib/utils.js';

	let {
		open,
		paneCount,
		onPick,
		onOpenChange
	}: {
		open: boolean;
		paneCount: number;
		onPick: (paneIndex: number) => void;
		onOpenChange: (open: boolean) => void;
	} = $props();
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content class="w-[300px] max-w-[92%] gap-4 border-ink/18 bg-card2">
		<Dialog.Header>
			<Dialog.Title class="font-mono text-[13px] tracking-[0.03em]">Add to which pane?</Dialog.Title>
			<Dialog.Description class="sr-only">Choose a chart pane to place this indicator on</Dialog.Description>
		</Dialog.Header>
		<div class="flex flex-col gap-1.5">
			{#each Array.from({ length: paneCount }) as _, i (i)}
				<button
					type="button"
					class={cn(
						'cursor-pointer border border-ink/14 px-3 py-2 text-left font-mono text-[11px] tracking-[0.06em] text-foreground hover:bg-ink/7'
					)}
					onclick={() => onPick(i)}
				>
					Pane {i + 1}
				</button>
			{/each}
		</div>
	</Dialog.Content>
</Dialog.Root>
