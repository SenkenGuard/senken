<script lang="ts">
	// "ADD WIDGET…" picker: lists the effective catalog served by
	// `GET /api/dashboard/widgets/catalog`, never a hardcoded list — a
	// widget contributed later by a plugin appears here for free the
	// moment the server's own registry reports it.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import type { DashboardWidgetDefinition } from './api';

	let {
		open,
		catalog,
		onClose,
		onPick
	}: {
		open: boolean;
		catalog: DashboardWidgetDefinition[];
		onClose: () => void;
		onPick: (definition: DashboardWidgetDefinition) => void;
	} = $props();
</script>

<Dialog.Root
	{open}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={true}
		class="top-[30%] w-[420px] max-w-[92%] gap-0 border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
			<Dialog.Title class="text-[13px] font-medium tracking-[0.03em] text-foreground">
				Add widget
			</Dialog.Title>
		</Dialog.Header>
		<div class="flex max-h-[360px] flex-col overflow-y-auto border-t border-ink/7">
			{#each catalog as definition (definition.widget_type_id)}
				<button
					type="button"
					class="flex flex-col items-start gap-0.5 border-b border-ink/[0.045] px-4 py-2.5 text-left hover:bg-ink/7"
					onclick={() => {
						onPick(definition);
						onClose();
					}}
				>
					<span class="font-mono text-[10px] tracking-[0.12em] text-foreground uppercase">
						{definition.title}
					</span>
					<span class="font-mono text-[8.5px] tracking-[0.1em] text-dim">
						{definition.description}
					</span>
				</button>
			{:else}
				<div class="px-4 py-6 text-center font-mono text-[9px] tracking-[0.12em] text-dim uppercase">
					No widgets available
				</div>
			{/each}
		</div>
	</Dialog.Content>
</Dialog.Root>
