<script lang="ts">
	// Generic color picker popover.
	//
	// The design reference's line map labels this range "Symbol picker", but
	// the markup at those lines is unambiguously a color picker: a `COLOR`
	// header, a 12x5 hue/lightness swatch grid, a gray ramp, quick swatches,
	// a native `<input type="color">`, a hex text field and a "THEME" reset
	// button (`pickGrid`/`pickGrays`/`pickQuick`/`pickHex`/`onPickAuto`,
	// lines 3300-3311) — there is no symbol search or list here. The
	// reference's actual symbol picker reuses the general command palette in
	// its `cmd === 'symbol'` mode (line 2716), sharing that markup (lines
	// 1141-1177) rather than having a dialog of its own. See
	// `layout/command-palette.svelte` for that piece, and its implementation
	// report for this divergence.
	//
	// A generic value/fallback/onChange popover with no Senken-specific
	// knowledge, so it lives in `ui/` per Part C. Every settings/layer-dialog
	// color field (`terminal/chart-settings-dialog.svelte`,
	// `terminal/layer-dialog.svelte`) composes one of these instead of the
	// reference's single global floating picker positioned by click
	// coordinates — bits-ui's `Popover` already anchors to its trigger, so
	// there is no need to reproduce that positioning math.
	import * as Popover from '$lib/components/ui/popover/index.js';
	import { cn } from '$lib/utils.js';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		value,
		fallback,
		onChange,
		quickSwatches = ['#f2f2ef', '#9aa0a6', '#7de0a3', '#e8836f', '#7aa7e8', '#e8c87a']
	}: {
		/** Current setting — a 6-digit hex string, or `'auto'` to follow the
		 * theme (reference: `colorField`'s `cur`, line 2499). */
		value: string;
		/** The theme-resolved color to show/apply when `value` is `'auto'`. */
		fallback: string;
		onChange: (hex: string) => void;
		quickSwatches?: string[];
	} = $props();

	let open = $state(false);

	const isAuto = $derived(!value || value === 'auto');
	const display = $derived(isAuto ? fallback : value);
	const label = $derived(isAuto ? 'THEME' : value.toUpperCase());

	// reference: `hslToHex`, lines 2521-2529.
	function hslToHex(h: number, s: number, l: number): string {
		const sa = s / 100;
		const li = l / 100;
		const k = (n: number) => (n + h / 30) % 12;
		const a = sa * Math.min(li, 1 - li);
		const f = (n: number) => Math.round(255 * (li - a * Math.max(-1, Math.min(Math.min(k(n) - 3, 9 - k(n)), 1))));
		return '#' + [f(0), f(8), f(4)].map((v) => v.toString(16).padStart(2, '0')).join('');
	}

	// reference: `gridColors` / `grayColors`, lines 2516-2520.
	const GRID_HUES = [0, 20, 40, 60, 90, 120, 160, 190, 215, 250, 285, 320];
	const GRID_LIGHTS = [78, 64, 52, 40, 28];
	const GRID_COLORS = GRID_HUES.flatMap((h) => GRID_LIGHTS.map((l) => hslToHex(h, 62, l)));
	const GRAY_COLORS = [98, 90, 80, 70, 60, 50, 42, 34, 26, 18, 11, 5].map((l) => hslToHex(220, 4, l));

	function pick(hex: string) {
		onChange(hex);
	}

	function onHexInput(e: Event) {
		const v = (e.currentTarget as HTMLInputElement).value.trim();
		if (/^#[0-9a-fA-F]{6}$/.test(v)) onChange(v);
	}
</script>

<Popover.Root bind:open>
	<Popover.Trigger
		class="flex flex-none items-center gap-[7px] border border-ink/16 py-0.5 pr-[7px] pl-[3px]"
	>
		<div class="size-4" style="background: {display};"></div>
		<span class="font-mono text-[9px] text-dim2">{label}</span>
	</Popover.Trigger>
	<Popover.Content align="start" sideOffset={6} class="w-[244px] rounded-none border-ink/20 bg-popover p-3 shadow-[0_26px_60px_rgba(0,0,0,0.7)]">
		<div class="mb-2.5 flex items-center justify-between">
			<span class="font-mono text-[8px] tracking-[0.22em] text-dim">COLOR</span>
			<button type="button" class="cursor-pointer text-dim" onclick={() => (open = false)} aria-label="Close color picker">
				<XIcon class="size-[11px]" />
			</button>
		</div>
		<div class="mb-2 grid grid-cols-12 gap-0.5">
			{#each GRID_COLORS as c (c)}
				<button
					type="button"
					class="aspect-square border border-ink/12"
					style="background: {c};"
					onclick={() => pick(c)}
					aria-label={c}
				></button>
			{/each}
		</div>
		<div class="mb-[11px] grid grid-cols-12 gap-0.5">
			{#each GRAY_COLORS as c (c)}
				<button
					type="button"
					class="aspect-square border border-ink/12"
					style="background: {c};"
					onclick={() => pick(c)}
					aria-label={c}
				></button>
			{/each}
		</div>
		<div class="mb-2.5 flex gap-1">
			{#each quickSwatches as c (c)}
				<button
					type="button"
					class={cn('h-[17px] flex-1 border', !isAuto && value === c ? 'border-foreground' : 'border-ink/16')}
					style="background: {c};"
					onclick={() => pick(c)}
					aria-label={c}
				></button>
			{/each}
		</div>
		<div class="flex items-center gap-1.5">
			<input
				type="color"
				value={display}
				oninput={onHexInput}
				onchange={onHexInput}
				class="h-[26px] w-[30px] cursor-pointer border border-ink/16 bg-transparent p-0"
			/>
			<input
				value={isAuto ? '' : display.toUpperCase()}
				onchange={onHexInput}
				placeholder="THEME"
				class="h-[26px] min-w-0 flex-1 border border-ink/16 bg-transparent px-2 font-mono text-[10.5px] tracking-[0.06em] text-foreground outline-none placeholder:text-dim"
			/>
			<button
				type="button"
				class="flex h-[26px] cursor-pointer items-center border border-ink/16 px-[9px]"
				onclick={() => {
					onChange('auto');
					open = false;
				}}
			>
				<span class="font-mono text-[8px] tracking-[0.14em] text-dim2">THEME</span>
			</button>
		</div>
	</Popover.Content>
</Popover.Root>
