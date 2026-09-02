<script lang="ts">
	// Workspace switcher + its "..." menu, the dashboard's own counterpart
	// to the chart terminal's `workspace-bar.svelte` — same look, real
	// server-backed workspaces instead of local-only state.
	import { cn } from '$lib/utils.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import type { DashboardWorkspaceDto } from './api';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import EllipsisVerticalIcon from '@lucide/svelte/icons/ellipsis-vertical';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import PuzzleIcon from '@lucide/svelte/icons/puzzle';

	let {
		workspaces,
		activeId,
		onSelect,
		onAdd,
		onOpenAddWidget,
		onOpenWidgetPlugins,
		onRename,
		onDelete
	}: {
		workspaces: DashboardWorkspaceDto[];
		activeId: string;
		onSelect: (id: string) => void;
		onAdd: () => void;
		onOpenAddWidget: () => void;
		/** Opens the widget plugin package manager — install, enable/disable,
		 * refresh, remove. Separate from `onOpenAddWidget`'s picker, which
		 * only ever places an already-active widget type. */
		onOpenWidgetPlugins: () => void;
		onRename: (id: string, newName: string) => void;
		onDelete: (id: string) => void;
	} = $props();

	let menuOpen = $state(false);

	const active = $derived(workspaces.find((w) => w.id === activeId) ?? workspaces[0]);

	const itemClass = 'gap-2.5 rounded-none border-b border-ink/[0.045] px-3 py-2 focus:bg-ink/7';

	function renameActive() {
		if (!active) return;
		const next = prompt('Rename workspace', active.name);
		if (next && next.trim() && next.trim() !== active.name) onRename(active.id, next.trim());
	}
</script>

<div class="flex h-10 flex-none items-stretch border-b border-border bg-secondary">
	<div class="flex min-w-0 flex-1 items-stretch overflow-x-auto overflow-y-hidden">
		{#each workspaces as w, i (w.id)}
			<button
				type="button"
				class={cn(
					'flex flex-none cursor-pointer items-center gap-2 border-r border-ink/6 px-4',
					w.id === activeId ? 'bg-foreground text-inv' : 'text-secondary-foreground'
				)}
				onclick={() => onSelect(w.id)}
			>
				<span class={cn('font-mono text-[9px]', w.id === activeId ? 'text-inv/65' : 'text-dim')}>
					{String(i + 1).padStart(2, '0')}
				</span>
				<span class="text-[11px] font-medium tracking-[0.12em] uppercase">{w.name}</span>
			</button>
		{/each}
		<button
			type="button"
			class="flex flex-none cursor-pointer items-center border-r border-ink/6 px-3.5 text-secondary-foreground"
			aria-label="New workspace"
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
					{(active?.name ?? '').toUpperCase()}
				</div>
				<DropdownMenu.Item class={itemClass} onSelect={onOpenAddWidget}>
					<PlusIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-foreground">ADD WIDGET…</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass} onSelect={onOpenWidgetPlugins}>
					<PuzzleIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-foreground">
						WIDGET PLUGINS…
					</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item class={itemClass} onSelect={renameActive}>
					<PencilIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">
						RENAME WORKSPACE…
					</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item
					variant="destructive"
					class="gap-2.5 rounded-none px-3 py-2"
					onSelect={() => active && onDelete(active.id)}
				>
					<Trash2Icon class="size-[13px]" />
					<span class="font-mono text-[10px] tracking-[0.12em]">DELETE WORKSPACE</span>
				</DropdownMenu.Item>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
</div>
