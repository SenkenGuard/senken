<script lang="ts">
	// Global AI panel: floating panel + its closed launcher chip, and the sidebar variant
	// (lines 1234-1263). Mounted once in `AppShell` per Part C, driven by
	// `$lib/state/ai-panel.svelte`.
	//
	// The float/launcher pair is positioned `fixed` to the viewport (bottom
	// right) rather than `position: absolute` inside the content column the
	// way the reference does it — the reference needs `absolute` because
	// everything in it lives in one file with one relatively-positioned
	// wrapper; here, mounting a single global component means `fixed`
	// reaches the same visual corner without also having to reach back into
	// `AppShell`'s layout to add a relative wrapper. The sidebar variant is
	// a real flex sibling of the rest of the shell (see `app-shell.svelte`)
	// since it has to take up layout width, not float over it.
	import { cn } from '$lib/utils.js';
	import { aiPanel } from '$lib/state/ai-panel.svelte';
	import { AI_PROMPTS, buildAiMessages } from '$lib/mock/ai';
	import SparkleIcon from '@lucide/svelte/icons/sparkle';
	import PictureInPicture2Icon from '@lucide/svelte/icons/picture-in-picture-2';
	import PanelRightIcon from '@lucide/svelte/icons/panel-right';
	import MinusIcon from '@lucide/svelte/icons/minus';
	import XIcon from '@lucide/svelte/icons/x';
	import SendIcon from '@lucide/svelte/icons/send';

	const messages = buildAiMessages();

	function setFloat() {
		aiPanel.mode = 'float';
		aiPanel.open = true;
	}
	function setSide() {
		aiPanel.mode = 'sidebar';
		aiPanel.open = true;
	}
	function collapse() {
		aiPanel.open = false;
	}
</script>

{#snippet modeButtons()}
	<button
		type="button"
		class={cn(
			'flex size-[22px] cursor-pointer items-center justify-center border border-ink/12',
			aiPanel.mode === 'float' ? 'bg-foreground text-inv' : 'text-dim2'
		)}
		onclick={setFloat}
		aria-label="Float the AI panel"
	>
		<PictureInPicture2Icon class="size-3" />
	</button>
	<button
		type="button"
		class={cn(
			'flex size-[22px] cursor-pointer items-center justify-center border border-ink/12',
			aiPanel.mode === 'sidebar' ? 'bg-foreground text-inv' : 'text-dim2'
		)}
		onclick={setSide}
		aria-label="Dock the AI panel to the sidebar"
	>
		<PanelRightIcon class="size-3" />
	</button>
{/snippet}

{#snippet transcript()}
	<div class="flex min-h-0 flex-1 flex-col gap-[9px] overflow-auto p-3">
		{#each messages as m, i (i)}
			<div
				class={cn(
					'max-w-[92%] px-[11px] py-[9px] font-mono text-[10px] leading-[1.6]',
					m.align === 'user'
						? 'self-end border border-ink/10 bg-ink/7 text-secondary-foreground'
						: 'self-start border border-foreground bg-foreground text-inv'
				)}
			>
				{m.text}
			</div>
		{/each}
	</div>
	<div class="flex flex-none flex-col gap-2 border-t border-ink/8 p-3">
		<div class="flex gap-[5px]">
			{#each AI_PROMPTS as p (p)}
				<span class="cursor-pointer border border-ink/12 px-2 py-1 font-mono text-[8px] tracking-[0.14em] text-dim2">
					{p}
				</span>
			{/each}
		</div>
		<div class="flex items-center justify-between border border-ink/14 px-[11px] py-[9px] font-mono text-[10px] text-dim">
			<span>Ask the desk…</span>
			<SendIcon class="size-[13px] text-dim2" />
		</div>
	</div>
{/snippet}

{#if !aiPanel.open}
	<button
		type="button"
		class="fixed right-5 bottom-[38px] z-[95] flex cursor-pointer items-center gap-[9px] bg-foreground px-[14px] py-[9px] text-inv shadow-[0_18px_44px_rgba(0,0,0,0.6)]"
		onclick={() => (aiPanel.open = true)}
	>
		<SparkleIcon class="size-[14px]" />
		<span class="font-mono text-[9px] font-semibold tracking-[0.2em]">SENKEN AI</span>
	</button>
{:else if aiPanel.mode === 'float'}
	<div
		class="fixed right-5 bottom-[38px] z-[95] flex h-[430px] w-[344px] flex-col border border-ink/16 bg-card2 shadow-[0_26px_60px_rgba(0,0,0,0.7)]"
	>
		<div class="flex h-[34px] flex-none items-center justify-between border-b border-ink/8 px-3">
			<div class="flex items-center gap-2">
				<SparkleIcon class="size-[13px] text-foreground" />
				<span class="font-mono text-[8.5px] tracking-[0.24em] text-dim2">SENKEN AI</span>
			</div>
			<div class="flex items-center gap-1">
				{@render modeButtons()}
				<button type="button" class="ml-1 cursor-pointer text-dim2" onclick={collapse} aria-label="Collapse the AI panel">
					<MinusIcon class="size-3" />
				</button>
			</div>
		</div>
		{@render transcript()}
	</div>
{/if}

{#if aiPanel.open && aiPanel.mode === 'sidebar'}
	<div class="flex w-[min(26vw,316px)] min-w-[240px] flex-none flex-col border-l border-border bg-chrome">
		<div class="flex h-[46px] flex-none items-center justify-between border-b border-ink/9 px-3">
			<div class="flex items-center gap-2">
				<SparkleIcon class="size-[14px] text-foreground" />
				<span class="font-mono text-[8.5px] tracking-[0.24em] text-dim2">SENKEN AI</span>
			</div>
			<div class="flex items-center gap-1">
				{@render modeButtons()}
				<button type="button" class="ml-1 cursor-pointer text-dim2" onclick={collapse} aria-label="Close the AI panel">
					<XIcon class="size-3" />
				</button>
			</div>
		</div>
		{@render transcript()}
	</div>
{/if}
