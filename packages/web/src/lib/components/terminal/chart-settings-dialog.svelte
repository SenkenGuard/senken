<script lang="ts">
	// Chart settings dialog. Domain-specific (edits the active pane's `ChartSettings`), so it
	// lives in `terminal/` per Part C. Reachable only from the charts
	// route's "CHART SETTINGS" toolbar button and, for the primary
	// instrument layer, its chip's settings icon (see
	// `pane-header.svelte`'s header comment) — `routes/charts/+page.svelte`
	// owns the `chartSettings` object and `csOpen`/`csSection` state
	// locally, same reasoning as `layer-dialog.svelte`.
	//
	// Reuses shadcn `dialog` plus `ui/color-picker` for every
	// color field. Settings apply live (no separate "Apply" step) — CANCEL
	// and OK both just close, matching the reference (`closeChartSettings`
	// on both, lines 1096/1099).
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { ColorPicker } from '$lib/components/ui/color-picker/index.js';
	import { cn } from '$lib/utils.js';
	import { mode } from 'mode-watcher';
	import {
		CS_SECTIONS,
		buildSettingsRows,
		chartThemeDefaults,
		type ChartSettings,
		type CsSectionKey
	} from '$lib/mock/chart-settings';
	import CheckIcon from '@lucide/svelte/icons/check';
	import MinusIcon from '@lucide/svelte/icons/minus';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		open,
		settings,
		scopeLabel,
		onClose,
		onChange,
		initialSection = 'symbol',
		quotesSupported
	}: {
		open: boolean;
		settings: ChartSettings;
		/** reference: `csScope`, "PANE 1 · BTCUSDT". */
		scopeLabel: string;
		onClose: () => void;
		onChange: (patch: Partial<ChartSettings>) => void;
		/** Which section to land on. The price-scale menu's "More settings"
		 * opens straight into scales, which is what the reader asking for it
		 * was already looking at. */
		initialSection?: CsSectionKey;
		/** `GET /api/sources` is authoritative: a live trade feed does not
		 * imply that this source can report both sides of a quote. */
		quotesSupported?: boolean;
	} = $props();

	// Seeded on each open rather than at construction: this dialog is never
	// unmounted, so without that it would reopen wherever the last visit
	// ended — and "More settings" would not land where it says it does.
	let section = $state<CsSectionKey>('symbol');
	$effect(() => {
		if (open) section = initialSection;
	});

	const theme = $derived(chartThemeDefaults(mode.current === 'light'));
	const built = $derived(buildSettingsRows(section, settings, onChange, theme, { quotes: quotesSupported }));
</script>

<Dialog.Root
	{open}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		class="top-[66px] flex max-h-[78%] w-[592px] max-w-[94%] translate-y-0 flex-col gap-0 overflow-hidden border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 border-b border-ink/10 px-4 py-[13px]">
			<Dialog.Title class="text-[13px] font-medium tracking-[0.06em] text-foreground">CHART SETTINGS</Dialog.Title>
			<Dialog.Close class="flex-none cursor-pointer text-dim2" aria-label="Close">
				<XIcon class="size-[14px]" />
			</Dialog.Close>
		</Dialog.Header>
		<Dialog.Description class="sr-only">Chart display and scale settings</Dialog.Description>

		<div class="flex min-h-0 flex-1">
			<div class="w-[178px] flex-none border-r border-ink/9 py-2">
				{#each CS_SECTIONS as sec (sec.key)}
					<button
						type="button"
						class={cn(
							'flex w-full cursor-pointer items-center gap-2.5 px-3.5 py-2.5 text-left',
							section === sec.key ? 'bg-ink/7' : ''
						)}
						onclick={() => (section = sec.key)}
					>
						<sec.icon class={cn('size-3.5', section === sec.key ? 'text-foreground' : 'text-dim2')} />
						<span class={cn('font-mono text-[9.5px] tracking-[0.14em]', section === sec.key ? 'text-foreground' : 'text-dim2')}>
							{sec.label}
						</span>
					</button>
				{/each}
			</div>

			<div class="min-w-0 flex-1 overflow-auto px-4 pt-[14px] pb-[18px]">
				<span class="mb-3 block font-mono text-[8px] tracking-[0.24em] text-dim">{built.groupLabel}</span>
				<div class="flex flex-col gap-3">
					{#each built.rows as row (row.label + (row.isGroup ? '-group' : ''))}
						{#if row.isGroup}
							<span class="pt-1.5 font-mono text-[8px] tracking-[0.24em] text-dim">{row.label}</span>
						{:else}
							<div class="flex items-center gap-3">
								{#if row.hasCheck}
									<button
										type="button"
									class={cn(
											'flex size-[15px] flex-none items-center justify-center border',
											row.disabled ? 'cursor-not-allowed opacity-45' : 'cursor-pointer',
											row.checked ? 'border-foreground bg-foreground' : 'border-ink/24'
										)}
									onclick={row.onToggle}
									disabled={row.disabled}
									title={row.disabledReason}
									aria-label={row.checked ? `Disable ${row.label}` : `Enable ${row.label}`}
								>
										{#if row.checked}<CheckIcon class="size-2.5 text-inv" />{/if}
									</button>
								{/if}
								<span class="min-w-0 flex-1 truncate font-mono text-[10px] tracking-[0.1em] text-secondary-foreground">
									{row.label}
								</span>
								{#if row.disabledReason}
									<span class="max-w-[132px] text-right font-mono text-[8px] leading-tight tracking-[0.06em] text-dim" data-capability-reason={row.label}>
										{row.disabledReason}
									</span>
								{/if}
								{#if row.hasColor && row.colorValue !== undefined && row.onColorChange}
									<ColorPicker value={row.colorValue} fallback={row.colorFallback ?? '#f2f2ef'} onChange={row.onColorChange} />
								{/if}
								{#if row.hasOptions}
									<div class="flex flex-none gap-[5px]">
										{#each row.options ?? [] as o (o.label)}
											<button
												type="button"
												class={cn('cursor-pointer border px-2.5 py-1', o.active ? 'border-foreground bg-foreground' : 'border-ink/14')}
												onclick={o.onPick}
											>
												<span class={cn('font-mono text-[8.5px] tracking-[0.14em]', o.active ? 'text-inv' : 'text-dim2')}>{o.label}</span>
											</button>
										{/each}
									</div>
								{/if}
								{#if row.hasNumber}
									<div class="flex flex-none items-center gap-1.5">
										<button type="button" class="flex size-[22px] cursor-pointer items-center justify-center border border-ink/14 text-dim2" onclick={row.onDec} aria-label="Decrease">
											<MinusIcon class="size-2.5" />
										</button>
										<div class="flex h-6 min-w-[62px] items-center justify-center gap-1 border border-ink/14">
											<span class="font-mono text-[10.5px] text-foreground">{row.value}</span>
											<span class="font-mono text-[8px] text-dim">{row.unit}</span>
										</div>
										<button type="button" class="flex size-[22px] cursor-pointer items-center justify-center border border-ink/14 text-dim2" onclick={row.onInc} aria-label="Increase">
											<PlusIcon class="size-2.5" />
										</button>
									</div>
								{/if}
							</div>
						{/if}
					{/each}
				</div>
			</div>
		</div>

		<div class="flex flex-none items-center justify-between border-t border-ink/10 px-4 py-[11px]">
			<span class="font-mono text-[8.5px] tracking-[0.18em] text-dim">APPLIES TO {scopeLabel}</span>
			<div class="flex gap-[7px]">
				<button type="button" class="cursor-pointer border border-ink/14 px-4 py-[7px]" onclick={onClose}>
					<span class="font-mono text-[9px] tracking-[0.18em] text-dim2">CANCEL</span>
				</button>
				<button type="button" class="cursor-pointer bg-foreground px-[18px] py-[7px]" onclick={onClose}>
					<span class="font-mono text-[9px] font-semibold tracking-[0.18em] text-inv">OK</span>
				</button>
			</div>
		</div>
	</Dialog.Content>
</Dialog.Root>
