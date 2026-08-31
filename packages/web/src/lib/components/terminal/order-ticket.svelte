<script lang="ts">
	// BUY/SELL order ticket: side toggle,
	// order-type row, price/size/TIF fields, size presets (display-only in
	// the reference too — its `sizePresets` row carries no onClick either),
	// and the place-order action.
	import { cn } from '$lib/utils.js';
	import { ORDER_TYPES, SIZE_PRESETS, type OrderSide, type OrderType } from './trade-demo';
	import { fmtDecimal, parseInstrumentId } from './chart-config';

	let {
		instrument,
		mid,
		ccy,
		leverage,
		onPlace
	}: {
		instrument: string;
		mid: number;
		ccy: string;
		leverage: string;
		onPlace: (order: { side: OrderSide; type: OrderType; qty: number; price: number }) => void;
	} = $props();

	let side = $state<OrderSide>('BUY');
	let type = $state<OrderType>('MARKET');

	const qty = 0.25;
	const price = $derived(type === 'MARKET' ? mid : mid * (side === 'BUY' ? 0.997 : 1.003));
	const fields = $derived([
		{ label: 'PRICE', value: fmtDecimal(price), unit: ccy },
		{ label: 'SIZE', value: qty.toFixed(3), unit: parseInstrumentId(instrument).ticker.slice(0, 3) },
		{ label: 'TIME IN FORCE', value: 'GTC', unit: leverage }
	]);
</script>

<div class="flex flex-col gap-2.5 border-b border-border p-3">
	<div class="flex gap-px border border-ink/12">
		<button
			type="button"
			class={cn(
				'flex-1 cursor-pointer py-2 text-center font-mono text-[10px] tracking-[0.18em]',
				side === 'BUY' ? 'bg-gain/12 text-gain' : 'text-dim2'
			)}
			onclick={() => (side = 'BUY')}
		>
			BUY
		</button>
		<button
			type="button"
			class={cn(
				'flex-1 cursor-pointer py-2 text-center font-mono text-[10px] tracking-[0.18em]',
				side === 'SELL' ? 'bg-loss/12 text-loss' : 'text-dim2'
			)}
			onclick={() => (side = 'SELL')}
		>
			SELL
		</button>
	</div>

	<div class="flex gap-1.5">
		{#each ORDER_TYPES as t (t)}
			<button
				type="button"
				class={cn(
					'flex-1 cursor-pointer border py-1.5 text-center font-mono text-[8.5px] tracking-[0.14em]',
					type === t ? 'border-foreground bg-ink/10 text-foreground' : 'border-ink/10 text-dim'
				)}
				onclick={() => (type = t)}
			>
				{t}
			</button>
		{/each}
	</div>

	{#each fields as f (f.label)}
		<div class="flex flex-col gap-1.5">
			<span class="font-mono text-[8px] tracking-[0.22em] text-dim">{f.label}</span>
			<div class="flex h-[30px] items-center justify-between border border-ink/11 px-2.5">
				<span class="font-mono text-[11.5px] text-secondary-foreground">{f.value}</span>
				<span class="font-mono text-[8.5px] text-dim">{f.unit}</span>
			</div>
		</div>
	{/each}

	<div class="flex gap-1.5">
		{#each SIZE_PRESETS as s (s)}
			<div class="flex-1 border border-ink/10 py-1.5 text-center font-mono text-[9px] text-dim2">{s}</div>
		{/each}
	</div>

	<button
		type="button"
		class="cursor-pointer bg-foreground py-2.5 text-center font-mono text-[10px] font-semibold tracking-[0.2em] text-inv"
		onclick={() => onPlace({ side, type, qty, price })}
	>
		PLACE {side} ORDER
	</button>
</div>
