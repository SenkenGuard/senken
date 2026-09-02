<script lang="ts">
	// The header + body frame every placed dashboard widget sits in.
	//
	// Two properties the plan is explicit about, both enforced here rather
	// than left to whatever widget happens to be mounted inside:
	//
	// - The mockup label is drawn by this host frame, never by the widget
	//   itself — so a widget can never suppress its own "this is a fixture"
	//   label by omission. A widget only ever reports its `dataSource`; this
	//   component decides whether that means a label appears.
	// - A placeholder is a property of the *cell*, not the widget: when no
	//   renderer is registered for a placed widget's type (its provider is
	//   disabled, failed to load, or simply not installed), this frame
	//   still renders at the placed size, with the placed title unavailable
	//   — the caller passes `isPlaceholder` instead of `children` in that
	//   case, and nothing about the widget's stored config or geometry is
	//   touched by that decision.
	//
	// Props-in/markup-out only: no `onMount`, no browser API calls, no
	// lifecycle — this keeps it renderable through this project's
	// server-only Svelte test harness, which is how the two properties
	// above are proven in the DOM rather than asserted by description.
	import { cn } from '$lib/utils.js';
	import GripIcon from '@lucide/svelte/icons/grip';
	import MoveDiagonalIcon from '@lucide/svelte/icons/move-diagonal-2';
	import XIcon from '@lucide/svelte/icons/x';
	import type { Snippet } from 'svelte';

	let {
		title,
		dataSource,
		isPlaceholder = false,
		onRemove,
		onDragHandlePointerDown,
		onResizeHandlePointerDown,
		children
	}: {
		title: string;
		/** `undefined` when the widget's type is not in the effective
		 * catalog at all — a placeholder never claims a data source it
		 * cannot know. */
		dataSource?: 'live' | 'mock';
		isPlaceholder?: boolean;
		onRemove: () => void;
		onDragHandlePointerDown?: (event: PointerEvent) => void;
		onResizeHandlePointerDown?: (event: PointerEvent) => void;
		children?: Snippet;
	} = $props();
</script>

<div
	class="flex h-full flex-col border border-border bg-card"
	data-dashboard-widget-frame
	data-placeholder={isPlaceholder ? 'true' : undefined}
>
	<div class="flex h-8 flex-none items-center justify-between border-b border-ink/7 px-[11px]">
		<div class="flex min-w-0 items-center gap-2.5">
			<button
				type="button"
				class="flex size-5 flex-none cursor-grab items-center justify-center text-dim active:cursor-grabbing"
				aria-label={`Move ${title}`}
				onpointerdown={onDragHandlePointerDown}
			>
				<GripIcon class="size-3" />
			</button>
			<span
				class="flex-none text-[10.5px] font-semibold tracking-[0.2em] whitespace-nowrap text-secondary-foreground uppercase"
			>
				{title}
			</span>
			{#if dataSource === 'mock'}
				<span
					class="flex-none rounded-sm border border-ink/20 px-1 py-px font-mono text-[8px] font-semibold tracking-[0.12em] text-dim uppercase"
					data-mockup-label
					title="This widget renders seeded example data, not a real account."
				>
					Mock
				</span>
			{/if}
		</div>
		<button
			type="button"
			class="flex size-5 flex-none items-center justify-center text-dim"
			aria-label={`Remove ${title}`}
			onclick={onRemove}
		>
			<XIcon class="size-3" />
		</button>
	</div>
	<div class="relative min-h-0 flex-1 p-3">
		{#if isPlaceholder}
			<div
				class="flex h-full flex-col items-center justify-center gap-1 text-center"
				data-widget-placeholder
			>
				<span class="font-mono text-[9px] tracking-[0.14em] text-dim uppercase">
					Widget unavailable
				</span>
				<span class="max-w-[220px] font-mono text-[8px] tracking-[0.1em] text-dim/70">
					Its provider is disabled or not installed. Its position and settings are kept.
				</span>
			</div>
		{:else if children}
			{@render children()}
		{/if}
		<button
			type="button"
			class={cn(
				'absolute right-0.5 bottom-0.5 flex size-4 cursor-nwse-resize items-center justify-center text-dim/60'
			)}
			aria-label={`Resize ${title}`}
			onpointerdown={onResizeHandlePointerDown}
		>
			<MoveDiagonalIcon class="size-3" />
		</button>
	</div>
</div>
