<script lang="ts">
	// The dock's action row. Every action here is an explicit click — no
	// inline toolbar inputs; all input goes through a dialog. Buttons that
	// cannot currently do anything stay visible and `aria-disabled`, with the
	// reason in `title`, rather than disappearing or silently doing nothing
	// on click.
	import { cn } from '$lib/utils.js';
	import type { IndicatorToolchainStatusResponse, UserIndicatorSummaryDto } from '$lib/api/types';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import SaveIcon from '@lucide/svelte/icons/save';
	import LayoutTemplateIcon from '@lucide/svelte/icons/layout-template';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		active,
		dirty,
		saving,
		diagnosticsCount,
		toolchain,
		hasEverCompiled,
		onNew,
		onSave,
		onAddToChart,
		onRename,
		onDelete,
		onClose
	}: {
		active: UserIndicatorSummaryDto | null;
		dirty: boolean;
		saving: boolean;
		diagnosticsCount: number;
		toolchain: IndicatorToolchainStatusResponse | null;
		/** Whether *any* of this account's indicators has ever compiled —
		 * used only to decide which "still building" copy to show, since
		 * this dock has no direct signal for "the compile cache is cold". */
		hasEverCompiled: boolean;
		onNew: () => void;
		onSave: () => void;
		onAddToChart: () => void;
		onRename: () => void;
		onDelete: () => void;
		onClose: () => void;
	} = $props();

	const toolchainReady = $derived(toolchain?.available ?? false);
	const saveDisabled = $derived(!active || saving || !toolchainReady || (!dirty && !!active?.compiled));
	const saveDisabledReason = $derived(
		!active
			? 'Open an indicator first.'
			: !toolchainReady
				? (toolchain?.reason ?? 'Rust toolchain not found on this machine.')
				: !dirty && active?.compiled
					? 'No changes to save.'
					: undefined
	);
	const addDisabled = $derived(!active || saving);

	const statusText = $derived.by(() => {
		if (saving) return hasEverCompiled ? 'Compiling…' : 'First build warms the cache — this can take a minute.';
		if (!active) return '';
		if (diagnosticsCount > 0) return `Build failed: ${diagnosticsCount} error${diagnosticsCount === 1 ? '' : 's'}`;
		if (active.compiled) return 'Compiled';
		return 'Not compiled yet';
	});
	const statusTone = $derived(diagnosticsCount > 0 ? 'text-loss' : active?.compiled ? 'text-gain' : 'text-dim2');
</script>

<div data-indicator-toolbar class="flex flex-none flex-col border-b border-ink/10">
	{#if toolchain && !toolchain.available}
		<div data-toolchain-banner class="border-b border-ink/10 bg-loss/8 px-3 py-1.5">
			<span class="font-mono text-[9.5px] tracking-[0.04em] text-loss">
				Rust toolchain not found on this machine — {toolchain.reason ?? 'compiling indicators is unavailable.'}
			</span>
		</div>
	{/if}
	<div class="flex items-center gap-1.5 px-2.5 py-1.5">
		<button
			type="button"
			class="flex h-[26px] cursor-pointer items-center gap-1.5 border border-ink/14 px-2.5 text-dim2 hover:text-foreground"
			onclick={onNew}
		>
			<PlusIcon class="size-3" />
			<span class="font-mono text-[9px] tracking-[0.14em]">NEW</span>
		</button>
		<button
			type="button"
			aria-disabled={saveDisabled}
			title={saveDisabledReason}
			class={cn(
				'flex h-[26px] items-center gap-1.5 border px-2.5',
				saveDisabled ? 'cursor-not-allowed border-ink/10 text-dim' : 'cursor-pointer border-ink/14 text-dim2 hover:text-foreground'
			)}
			onclick={() => {
				if (!saveDisabled) onSave();
			}}
		>
			<SaveIcon class="size-3" />
			<span class="font-mono text-[9px] tracking-[0.14em]">SAVE</span>
		</button>
		<button
			type="button"
			aria-disabled={addDisabled}
			title={!active ? 'Open an indicator first.' : undefined}
			class={cn(
				'flex h-[26px] items-center gap-1.5 border px-2.5',
				addDisabled ? 'cursor-not-allowed border-ink/10 text-dim' : 'cursor-pointer border-ink/14 text-dim2 hover:text-foreground'
			)}
			onclick={() => {
				if (!addDisabled) onAddToChart();
			}}
		>
			<LayoutTemplateIcon class="size-3" />
			<span class="font-mono text-[9px] tracking-[0.14em]">ADD TO CHART</span>
		</button>
		<button
			type="button"
			aria-disabled={!active}
			title={!active ? 'Open an indicator first.' : undefined}
			class={cn(
				'flex size-7 items-center justify-center border',
				!active ? 'cursor-not-allowed border-ink/10 text-dim' : 'cursor-pointer border-ink/14 text-dim2 hover:text-foreground'
			)}
			onclick={() => {
				if (active) onRename();
			}}
			aria-label="Rename indicator"
		>
			<PencilIcon class="size-3" />
		</button>
		<button
			type="button"
			aria-disabled={!active}
			title={!active ? 'Open an indicator first.' : undefined}
			class={cn(
				'flex size-7 items-center justify-center border',
				!active ? 'cursor-not-allowed border-ink/10 text-dim' : 'cursor-pointer border-ink/14 text-dim2 hover:text-loss'
			)}
			onclick={() => {
				if (active) onDelete();
			}}
			aria-label="Delete indicator"
		>
			<Trash2Icon class="size-3" />
		</button>

		<span data-indicator-status class={cn('ml-1.5 font-mono text-[9.5px] tracking-[0.04em]', statusTone)}>{statusText}</span>

		<div class="flex-1"></div>

		<button
			type="button"
			class="flex size-7 cursor-pointer items-center justify-center text-dim2 hover:text-foreground"
			onclick={onClose}
			aria-label="Close indicator editor"
		>
			<XIcon class="size-3.5" />
		</button>
	</div>
</div>
