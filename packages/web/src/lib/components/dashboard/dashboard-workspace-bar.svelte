<script lang="ts">
	// Workspace switcher + its "..." menu, the dashboard's own counterpart
	// to the chart terminal's `workspace-bar.svelte` — same look, real
	// server-backed workspaces instead of local-only state.
	import { cn } from '$lib/utils.js';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import RenameWorkspaceDialog from './rename-workspace-dialog.svelte';
	import type { DashboardWorkspaceDto } from './api';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import EllipsisVerticalIcon from '@lucide/svelte/icons/ellipsis-vertical';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	let {
		workspaces,
		activeId,
		activeWidgetCount,
		onSelect,
		onAdd,
		onOpenAddWidget,
		onRename,
		onDelete
	}: {
		workspaces: DashboardWorkspaceDto[];
		activeId: string;
		/** The active workspace's current widget count, for the delete
		 * confirmation's own sentence — the caller's grid, not
		 * `workspaces`, is the source of truth for this (see
		 * `dashboard-grid.svelte`'s `widgetCount`). */
		activeWidgetCount: number;
		onSelect: (id: string) => void;
		onAdd: () => void;
		onOpenAddWidget: () => void;
		onRename: (id: string, newName: string) => void;
		onDelete: (id: string) => void;
	} = $props();

	let menuOpen = $state(false);
	let renameOpen = $state(false);
	let deleteOpen = $state(false);

	const active = $derived(workspaces.find((w) => w.id === activeId) ?? workspaces[0]);
	// The last remaining workspace can never be deleted — this only makes
	// that existing rule (`routes/dashboard/+page.svelte`'s own guard)
	// visible before the request round-trips, rather than a toast after the
	// fact.
	const isLastWorkspace = $derived(workspaces.length < 2);

	const itemClass = 'gap-2.5 rounded-none border-b border-ink/[0.045] px-3 py-2 focus:bg-ink/7';
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
				aria-label="Workspace menu"
				class={cn(
					'flex size-7 cursor-pointer items-center justify-center border transition-colors',
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
				<DropdownMenu.Item class={itemClass} onSelect={() => (renameOpen = true)}>
					<PencilIcon class="size-[13px] text-secondary-foreground" />
					<span class="font-mono text-[10px] tracking-[0.12em] text-secondary-foreground">
						RENAME WORKSPACE…
					</span>
				</DropdownMenu.Item>
				<DropdownMenu.Item
					variant="destructive"
					class="gap-2.5 rounded-none px-3 py-2"
					onSelect={() => (deleteOpen = true)}
				>
					<Trash2Icon class="size-[13px]" />
					<span class="font-mono text-[10px] tracking-[0.12em]">DELETE WORKSPACE</span>
				</DropdownMenu.Item>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
</div>

{#if active}
	<RenameWorkspaceDialog
		open={renameOpen}
		currentName={active.name}
		onOpenChange={(v) => (renameOpen = v)}
		onRename={(newName) => {
			onRename(active.id, newName);
			renameOpen = false;
		}}
	/>
	<ConfirmDialog
		open={deleteOpen}
		title="Delete workspace"
		description={`Delete workspace ${active.name}? Its ${activeWidgetCount} widget${activeWidgetCount === 1 ? '' : 's'} will be removed.`}
		confirmDisabled={isLastWorkspace}
		confirmDisabledReason="You need at least one workspace"
		onOpenChange={(v) => (deleteOpen = v)}
		onConfirm={() => {
			onDelete(active.id);
			deleteOpen = false;
		}}
	/>
{/if}
