<script lang="ts">
	// The order ticket, rendered from the account's own resolved access.
	//
	// Which order kinds appear, whether there is a time-in-force row, and
	// whether reduce-only exists at all are read from `access.capabilities`
	// — the adapter's own declared capabilities, **narrowed to this
	// account** — so a spot account shows no leverage controls, an adapter
	// with no stop orders shows no STOP button, and a read-only login (an
	// MT5 investor password, an exchange key with no trade scope) shows
	// exactly the controls it would if it could trade, just disabled: a
	// form that accepts a size it will never send is a lie.
	//
	// # What the user types is what is sent
	//
	// A price and a size go out as the digits typed, at the precision typed,
	// never through a `Number`. The server rounds them down onto the
	// instrument's real tick and step — which this client does not know —
	// and refuses anything it cannot express.
	import { cn } from '$lib/utils.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import type {
		AccountAccessDto,
		OrderKindTagDto,
		OrderSideDto,
		PlaceOrderRequest,
		TimeInForceDto,
		TradeAccountDto
	} from '$lib/api/types';
	import { parseDecimalInput } from '$lib/trade/scaled';

	let {
		instrument,
		account,
		access,
		mid,
		submitting = false,
		onPlace
	}: {
		instrument: string;
		account: TradeAccountDto | null;
		/** This account's resolved access — `null` while it is still
		 * loading, which is read the same as unrestricted rather than
		 * blocking the ticket on a lookup that has not answered yet. */
		access: AccountAccessDto | null;
		/** The pane's last close, used only to prefill a limit price. It is
		 * a display value from the chart; the value that trades is whatever
		 * the user leaves in the box. */
		mid: number;
		submitting?: boolean;
		onPlace: (request: PlaceOrderRequest) => void;
	} = $props();

	let side = $state<OrderSideDto>('buy');
	let kind = $state<OrderKindTagDto>('market');
	let quantity = $state('');
	let limitPrice = $state('');
	let triggerPrice = $state('');
	let timeInForce = $state<TimeInForceDto>('gtc');
	let reduceOnly = $state(false);
	let localError = $state<string | null>(null);

	const readOnly = $derived(access?.level === 'read_only');
	const kinds = $derived<OrderKindTagDto[]>(access?.capabilities.order_kinds ?? ['market']);
	const tifs = $derived<TimeInForceDto[]>(access?.capabilities.time_in_force ?? ['gtc']);
	const quantityUnit = $derived(access?.capabilities.quantity_unit ?? 'base');
	const supportsReduceOnly = $derived(
		access?.capabilities.features.includes('reduce_only') ?? false
	);
	const needsLimit = $derived(kind === 'limit' || kind === 'stop_limit');
	const needsTrigger = $derived(kind === 'stop' || kind === 'stop_limit');
	const tradable = $derived(
		!!account && account.owned && account.enabled && !readOnly && !!instrument
	);

	const unitLabel: Record<string, string> = {
		base: 'UNITS',
		contracts: 'CONTRACTS',
		lots: 'LOTS',
		quote_notional: 'NOTIONAL'
	};

	// Keep the selected kind inside what this adapter accepts: switching to
	// an account whose adapter has no stop orders must not leave STOP
	// selected and unsendable.
	$effect(() => {
		if (!kinds.includes(kind)) kind = kinds[0] ?? 'market';
	});
	$effect(() => {
		if (!tifs.includes(timeInForce)) timeInForce = tifs[0] ?? 'gtc';
	});

	function prefillLimit() {
		if (limitPrice === '' && mid > 0) limitPrice = String(mid);
	}

	function submit() {
		localError = null;
		const parsedQuantity = parseDecimalInput(quantity);
		if (!parsedQuantity) {
			localError = 'Enter a size.';
			return;
		}
		const request: PlaceOrderRequest = {
			instrument,
			side,
			kind,
			quantity: parsedQuantity,
			time_in_force: timeInForce,
			reduce_only: reduceOnly && supportsReduceOnly,
			post_only: false
		};
		if (needsLimit) {
			const price = parseDecimalInput(limitPrice);
			if (!price) {
				localError = 'Enter a limit price.';
				return;
			}
			request.limit_price = price;
		}
		if (needsTrigger) {
			const trigger = parseDecimalInput(triggerPrice);
			if (!trigger) {
				localError = 'Enter a trigger price.';
				return;
			}
			request.trigger_price = trigger;
		}
		onPlace(request);
	}

	const fieldClass =
		'h-[30px] rounded-none border-ink/16 bg-transparent font-mono text-[11.5px] shadow-none';
	const labelClass = 'font-mono text-[8px] tracking-[0.22em] text-dim';
</script>

<div class="flex flex-col gap-2.5 border-b border-border p-3">
	<div class="flex gap-px border border-ink/12">
		{#each [{ value: 'buy' as const, label: 'BUY' }, { value: 'sell' as const, label: 'SELL' }] as option (option.value)}
			<button
				type="button"
				disabled={readOnly}
				class={cn(
					'flex-1 py-2 text-center font-mono text-[10px] tracking-[0.18em]',
					readOnly ? 'cursor-not-allowed' : 'cursor-pointer',
					side === option.value
						? option.value === 'buy'
							? 'bg-gain/12 text-gain'
							: 'bg-loss/12 text-loss'
						: 'text-dim2'
				)}
				onclick={() => (side = option.value)}
			>
				{option.label}
			</button>
		{/each}
	</div>

	<div class="flex gap-1.5">
		{#each kinds as candidate (candidate)}
			<button
				type="button"
				disabled={readOnly}
				class={cn(
					'flex-1 border py-1.5 text-center font-mono text-[8.5px] tracking-[0.14em]',
					readOnly ? 'cursor-not-allowed' : 'cursor-pointer',
					kind === candidate ? 'border-foreground bg-ink/10 text-foreground' : 'border-ink/10 text-dim'
				)}
				onclick={() => {
					kind = candidate;
					if (candidate === 'limit' || candidate === 'stop_limit') prefillLimit();
				}}
			>
				{candidate.replace('_', '-').toUpperCase()}
			</button>
		{/each}
	</div>

	{#if needsTrigger}
		<div class="flex flex-col gap-1.5">
			<span class={labelClass}>TRIGGER</span>
			<Input
				bind:value={triggerPrice}
				disabled={readOnly}
				inputmode="decimal"
				class={fieldClass}
				placeholder="0.00"
			/>
		</div>
	{/if}

	{#if needsLimit}
		<div class="flex flex-col gap-1.5">
			<span class={labelClass}>LIMIT PRICE</span>
			<Input
				bind:value={limitPrice}
				disabled={readOnly}
				inputmode="decimal"
				class={fieldClass}
				placeholder="0.00"
			/>
		</div>
	{/if}

	<div class="flex flex-col gap-1.5">
		<span class={labelClass}>SIZE · {unitLabel[quantityUnit] ?? 'UNITS'}</span>
		<Input
			bind:value={quantity}
			disabled={readOnly}
			inputmode="decimal"
			class={fieldClass}
			placeholder="0.00"
		/>
	</div>

	{#if tifs.length > 1}
		<div class="flex flex-col gap-1.5">
			<span class={labelClass}>TIME IN FORCE</span>
			<select
				bind:value={timeInForce}
				disabled={readOnly}
				class="h-[30px] w-full appearance-none border border-ink/16 bg-transparent px-2.5 font-mono text-[11.5px] text-foreground outline-none"
			>
				{#each tifs as tif (tif)}
					<option value={tif}>{tif.toUpperCase()}</option>
				{/each}
			</select>
		</div>
	{/if}

	{#if supportsReduceOnly}
		<label class={cn('flex items-center gap-2', readOnly ? 'cursor-not-allowed' : 'cursor-pointer')}>
			<input
				type="checkbox"
				bind:checked={reduceOnly}
				disabled={readOnly}
				class="size-3 accent-current"
			/>
			<span class="font-mono text-[9px] tracking-[0.14em] text-dim2">REDUCE ONLY</span>
		</label>
	{/if}

	{#if localError}
		<span class="font-mono text-[9px] text-loss" data-ticket-error>{localError}</span>
	{/if}

	<button
		type="button"
		disabled={!tradable || submitting}
		class={cn(
			'cursor-pointer py-2.5 text-center font-mono text-[10px] font-semibold tracking-[0.2em]',
			tradable && !submitting ? 'bg-foreground text-inv' : 'bg-ink/12 text-dim'
		)}
		onclick={submit}
	>
		{#if submitting}
			PLACING…
		{:else if !account}
			NO ACCOUNT ATTACHED
		{:else if !account.owned}
			NOT YOUR ACCOUNT
		{:else if !account.enabled}
			ACCOUNT DISABLED
		{:else if readOnly}
			READ-ONLY ACCOUNT
		{:else}
			PLACE {side.toUpperCase()} ORDER
		{/if}
	</button>

	{#if readOnly && access?.note}
		<span class="font-mono text-[9px] leading-[1.6] text-dim" data-ticket-readonly-note>
			{access.note}
		</span>
	{/if}
</div>
