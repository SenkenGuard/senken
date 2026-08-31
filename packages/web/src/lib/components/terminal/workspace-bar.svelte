<script lang="ts">
	// Workspace switcher + its "..." menu. Reused shadcn `dropdown-menu` for the menu
	// per the reference ("workspace menu -> dropdown-menu / popover").
	//
	// "ADD WIDGET…" opens the global command palette in `'widget'` mode
	// (reference: line 3086), the overlay — `+page.svelte` builds
	// the request (rows from `CATALOG`, `onPick` calling `onAddWidget`) and
	// passes it as `onOpenAddWidget`. P2 stood this in with a
	// `DropdownMenu.Sub` listing the catalog directly before the palette
	// existed; that stand-in is gone now that P5 built it.
	// "SAVE LAYOUT" and "RENAME WORKSPACE..." have no handler here because
	// the reference itself wires them to nothing but closing the menu
	// (`onClick: () => this.setState({ dashMenuOpen: false })`) — that is
	// what a plain, handler-less dropdown item already does on select.
	import { cn } from '$lib/utils.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import type { Workspace } from '$lib/mock/dashboard';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import EllipsisVerticalIcon from '@lucide/svelte/icons/ellipsis-vertical';
	import SaveIcon from '@lucide/svelte/icons/save';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	let {
		workspaces,
		activeIndex,
		onSelect,
		onAdd,
		onOpenAddWidget,
		onDuplicate,
		onReset,
		onDelete
	}: {
		workspaces: Workspace[];
		activeIndex: number;
		onSelect: (index: number) => void;
		onAdd: () => void;
		onOpenAddWidget: () => void;
		onDuplicate: () => void;
		onReset: () => void;
		onDelete: () => void;
	} = $props();

	let menuOpen = $state(false);

	const itemClass = 'gap-2.5 rounded-none border-b border-ink/[0.045] px-3 py-2 focus:bg-ink/7';
</script>

<div class="flex h-10 flex-none items-stretch border-b border-border bg-secondary">
	<div class="flex min-w-0 flex-1 items-stretch overflow-x-auto overflow-y-hidden">
		{#each workspaces as w, i (w.name + i)}
			<button
				type="button"
				class={cn(
					'flex flex-none cursor-pointer items-center gap-2 border-r border-ink/6 px-4',
					i === activeIndex ? 'bg-foreground text-inv' : 'text-secondary-foreground'
				)}
				onclick={() => onSelect(i)}
			>
				<span class={cn('font-mono text-[9px]', i === activeIndex ? 'text-inv/65' : 'text-dim')}>
					{String(i + 1).padStart(2, '0')}
				</span>
				<span class="text-[11px] font-medium tracking-[0.12em] uppercase">{w.name}</span>
			</button>
		{/each}
		<button
			type="button"
			class="flex flex-none cursor-pointer items-center border-r border-ink/6 px-3.5 text-secondary-foreground"
			onclick={onAdd}
		>
			<PlusIcon class="size-[13px]" />
		</button>
	</div>

	<div class="relative flex flex-none items-center border-l border-ink/6 px-2">
		<DropdownMenu.Root bind:open={menuOpen}>
			<DropdownMenu.Trigger
				class={cn(
					'flex h-[25px] w-[27px] cursor-pointer items-center justify-center border transition-colors',
					menuOpen ? 'border-foreground bg-foreground text-inv' : 'border-dim text-secondary-foreground'
				)}
			>
				<EllipsisVerticalIcon class="size-[13px]" />
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end" sideOffset={8} class="w-[224px] border-ink/18 bg-popover p-0">
				<div class="truncate border-b border-ink/7 px-3 py-2 font-mono text-[8px] tracking-[0.24em] text-dim">
					{(workspaces[activeIndex] ?? workspaces[0]).name.toUpperCase()}
				</div>
				<DropdownMenu.Item class={itemClass} onSelect={onOpenAddWidget}>
					<PlusIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-foreground">ADD WIDGET…</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass}>
					<SaveIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">SAVE LAYOUT</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass}>
					<PencilIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">
						RENAME WORKSPACE…
					</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass} onSelect={onDuplicate}>
					<CopyIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">
						DUPLICATE WORKSPACE
					</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass} onSelect={onReset}>
					<RotateCcwIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">RESET LAYOUT</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item
					variant="destructive"
					class="gap-2.5 rounded-none px-3 py-2"
					onSelect={onDelete}
				>
					<Trash2Icon class="size-[13px]" />
					<span class="font-mono text-[10px] tracking-[0.12em]">DELETE WORKSPACE</span>
				</DropdownMenu.Item>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
</div>
