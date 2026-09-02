<script lang="ts">
	// Position-size widget body: balance, risk percent, entry and stop price
	// in, an exact position size out. See `position-size-calc.ts` for why
	// this never touches an `f64` — the answer is a quantity, held to the
	// same scaled-integer rule as money.
	//
	// Unlike the equity/positions/risk widgets beside it in the catalog,
	// this one reads no trade-engine state at all: `tradeStore` is never
	// imported here. Its only state is what the user typed, optionally
	// round-tripped through this placed instance's own `config` — the same
	// mechanism a dynamic widget plugin's iframe uses via `config.patch`,
	// wired here directly since a built-in has no iframe boundary to cross.
	import { cn } from '$lib/utils.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { calculatePositionSize, formatExact, type PositionSizeFields } from './position-size-calc';

	let {
		config = '{}',
		onConfigChange
	}: {
		config?: string;
		onConfigChange?: (config: string) => void;
	} = $props();

	function loadedFields(raw: string): PositionSizeFields {
		const fallback: PositionSizeFields = {
			balance: '',
			riskPercent: '',
			entryPrice: '',
			stopPrice: '',
			sizeDecimals: ''
		};
		try {
			const parsed = JSON.parse(raw) as Partial<PositionSizeFields>;
			return { ...fallback, ...parsed };
		} catch {
			return fallback;
		}
	}

	// Read once, at mount, deliberately — the same reasoning
	// `plugin-widget-frame.svelte`'s own `currentConfig` documents: this
	// component's own edits are what changes `config` afterwards (through
	// `persist()` below, echoed back by the parent), so re-reading it on
	// every prop change would overwrite whatever the user just typed with
	// the caller's last-known copy of it. `dashboard-grid.svelte` keys each
	// widget's render on its own `widget.id`, so a genuinely different
	// placed instance gets a fresh mount and a fresh read here, never a
	// stale one.
	const initial = loadedFields(config);
	let balance = $state(initial.balance);
	let riskPercent = $state(initial.riskPercent);
	let entryPrice = $state(initial.entryPrice);
	let stopPrice = $state(initial.stopPrice);
	let sizeDecimals = $state(initial.sizeDecimals);

	const outcome = $derived(
		calculatePositionSize({ balance, riskPercent, entryPrice, stopPrice, sizeDecimals })
	);

	function persist() {
		onConfigChange?.(
			JSON.stringify({ balance, riskPercent, entryPrice, stopPrice, sizeDecimals })
		);
	}

	const fieldClass =
		'h-[26px] rounded-none border-ink/16 bg-transparent font-mono text-[11px] shadow-none';
	const labelClass = 'font-mono text-[8px] tracking-[0.2em] text-dim';
</script>

<div class="flex h-full flex-col gap-2.5 overflow-auto">
	<div class="flex gap-2">
		<label class="flex flex-1 flex-col gap-1">
			<span class={labelClass}>BALANCE</span>
			<Input
				bind:value={balance}
				oninput={persist}
				inputmode="decimal"
				class={fieldClass}
				placeholder="10000.00"
				data-testid="position-size-balance"
			/>
		</label>
		<label class="flex flex-1 flex-col gap-1">
			<span class={labelClass}>RISK %</span>
			<Input
				bind:value={riskPercent}
				oninput={persist}
				inputmode="decimal"
				class={fieldClass}
				placeholder="1"
				data-testid="position-size-risk-percent"
			/>
		</label>
	</div>
	<div class="flex gap-2">
		<label class="flex flex-1 flex-col gap-1">
			<span class={labelClass}>ENTRY PRICE</span>
			<Input
				bind:value={entryPrice}
				oninput={persist}
				inputmode="decimal"
				class={fieldClass}
				placeholder="100.00"
				data-testid="position-size-entry-price"
			/>
		</label>
		<label class="flex flex-1 flex-col gap-1">
			<span class={labelClass}>STOP PRICE</span>
			<Input
				bind:value={stopPrice}
				oninput={persist}
				inputmode="decimal"
				class={fieldClass}
				placeholder="95.00"
				data-testid="position-size-stop-price"
			/>
		</label>
	</div>
	<label class="flex flex-col gap-1">
		<span class={labelClass}>SIZE DECIMALS</span>
		<Input
			bind:value={sizeDecimals}
			oninput={persist}
			inputmode="numeric"
			class={fieldClass}
			placeholder="2"
			data-testid="position-size-decimals"
		/>
	</label>

	<div class="mt-0.5 flex flex-col gap-1.5 border border-ink/12 bg-ink/[0.03] px-2.5 py-2">
		<div class="flex items-baseline justify-between">
			<span class={labelClass}>AMOUNT AT RISK</span>
			<span class="font-mono text-[13px] text-foreground" data-testid="position-size-risk-amount">
				{outcome.kind === 'ok' ? formatExact(outcome.result.riskAmount) : '—'}
			</span>
		</div>
		<div class="flex items-baseline justify-between">
			<span class={labelClass}>POSITION SIZE</span>
			<span
				class="font-mono text-[15px] font-semibold text-foreground"
				data-testid="position-size-result"
			>
				{outcome.kind === 'ok' ? formatExact(outcome.result.positionSize) : '—'}
			</span>
		</div>
		<p
			class={cn('font-mono text-[10px] leading-snug', outcome.kind === 'error' ? 'text-loss' : 'text-dim')}
			data-testid="position-size-note"
		>
			{#if outcome.kind === 'empty'}
				Enter a balance, a risk percent, an entry price and a stop price.
			{:else if outcome.kind === 'error'}
				{outcome.message}
			{:else}
				Truncated, never rounded up — never larger than the exact result.
			{/if}
		</p>
	</div>
</div>
