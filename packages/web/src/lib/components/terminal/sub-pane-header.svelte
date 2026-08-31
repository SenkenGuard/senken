<script lang="ts">
	// One sub-pane's chip row: icon, name,
	// params, and — on hover — the same toggle-visibility / settings / remove
	// action set pane-header.svelte's overlay chips carry. The
	// reference also offers a resize handle (line 519, `onResize`) and a
	// fourth "···" menu for reorder/merge-to-main actions (`menuItems`, lines
	// 2414-2420); the resize handle is provided by the `Resizable.Handle`
	// pane-cell.svelte already places above this component, and the menu is
	// out of its scope (toggle/settings/remove only).
	import { cn } from '$lib/utils.js';
	import { LAYER_KIND_ICON } from './chart-config';
	import type { LayerRuntime } from '$lib/charts/pane-runtime';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';
	import SettingsIcon from '@lucide/svelte/icons/settings-2';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		layer,
		onToggleLayer,
		onOpenSettings,
		onRemoveLayer
	}: {
		layer: LayerRuntime;
		onToggleLayer: (id: string) => void;
		onOpenSettings: (layerId: string) => void;
		onRemoveLayer: (layerId: string) => void;
	} = $props();

	const Icon = $derived(LAYER_KIND_ICON[layer.kind]);
	const label = $derived(layer.indicatorName ?? '');
	const paramsLabel = $derived(Object.values(layer.params).join(' · '));
</script>

<div class="pointer-events-none absolute top-[5px] left-2 z-[6] flex items-center">
	<div class="group/chip pointer-events-auto flex items-center gap-1.5 bg-transparent py-0.5 pr-1.5 pl-0.5 hover:bg-ink/7">
		<Icon class={cn('size-[11px] flex-none', layer.visible ? 'text-secondary-foreground' : 'text-dim')} />
		<span class={cn('font-mono text-[10px] font-medium tracking-[0.04em] whitespace-nowrap', layer.visible ? 'text-foreground' : 'text-dim')}>
			{label}
		</span>
		<span class="font-mono text-[8.5px] whitespace-nowrap text-dim">{paramsLabel}</span>
		<button
			type="button"
			class="hidden flex-none cursor-pointer pl-0.5 text-dim2 group-hover/chip:block"
			onclick={() => onToggleLayer(layer.id)}
			aria-label={layer.visible ? `Hide ${label}` : `Show ${label}`}
		>
			{#if layer.visible}
				<EyeIcon class="size-[11px]" />
			{:else}
				<EyeOffIcon class="size-[11px]" />
			{/if}
		</button>
		<button
			type="button"
			class="hidden flex-none cursor-pointer text-dim group-hover/chip:block"
			onclick={() => onOpenSettings(layer.id)}
			aria-label={`${label} settings`}
		>
			<SettingsIcon class="size-[11px]" />
		</button>
		<button
			type="button"
			class="hidden flex-none cursor-pointer text-dim group-hover/chip:block"
			onclick={() => onRemoveLayer(layer.id)}
			aria-label={`Remove ${label}`}
		>
			<XIcon class="size-[11px]" />
		</button>
	</div>
</div>
