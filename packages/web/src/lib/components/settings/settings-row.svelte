<script lang="ts">
	// One settings row: label + description on the left, a control on the
	// right, and a reset-to-default affordance that only
	// appears once the row's live value has actually diverged from its
	// default. This is the second of its two "worth copying deliberately"
	// details: "in a shared instance 'why is this different for me' is a
	// real support question," so the indicator has to be genuinely wired to
	// a live value, not decorative.
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import type { Snippet } from 'svelte';

	let {
		id,
		label,
		description,
		control,
		changed,
		onReset
	}: {
		id: string;
		label: string;
		description: string;
		control: Snippet;
		changed?: () => boolean;
		onReset?: () => void;
	} = $props();

	const isChanged = $derived(changed?.() ?? false);
</script>

<div data-settings-row={id} class="flex items-start justify-between gap-6 py-3">
	<div class="flex min-w-0 flex-1 flex-col gap-0.5 pt-0.5">
		<span class="text-[13px] font-medium text-foreground">{label}</span>
		<span class="text-[12px] leading-snug text-dim2">{description}</span>
	</div>
	<div class="flex flex-none items-center gap-2">
		{#if isChanged && onReset}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<button
							{...props}
							type="button"
							onclick={onReset}
							aria-label={`Reset ${label} to its default`}
							class="flex size-6 cursor-pointer items-center justify-center text-dim2 hover:text-foreground"
						>
							<RotateCcwIcon class="size-3.5" />
						</button>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content side="top" class="font-mono text-[9.5px] tracking-[0.1em]">
					RESET TO DEFAULT
				</Tooltip.Content>
			</Tooltip.Root>
		{/if}
		{@render control()}
	</div>
</div>
