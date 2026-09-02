<script lang="ts">
	// Amending a resting order: only the fields it actually has (quantity
	// always; limit price if it has one; trigger if it has one), pre-filled
	// from the order as it currently stands.
	//
	// The digits typed are what is sent, never through a `Number` — the
	// server rounds them onto the instrument's real tick and step, the same
	// rule the order ticket follows for a new order.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { AmendOrderRequest, OrderDto } from '$lib/api/types';
	import { formatScaled, parseDecimalInput } from '$lib/trade/scaled';

	let {
		open = $bindable(false),
		accountId,
		order,
		onamended
	}: {
		open: boolean;
		accountId: string;
		order: OrderDto;
		onamended: () => void;
	} = $props();

	let quantity = $state('');
	let limitPrice = $state('');
	let triggerPrice = $state('');
	let submitting = $state(false);
	let failure = $state<string | null>(null);
	let loaded = $state(false);

	const hasLimit = $derived(order.limit_price != null);
	const hasTrigger = $derived(order.trigger_price != null);

	$effect(() => {
		if (!open) {
			loaded = false;
			return;
		}
		if (loaded) return;
		loaded = true;
		failure = null;
		quantity = formatScaled(order.quantity);
		limitPrice = order.limit_price ? formatScaled(order.limit_price) : '';
		triggerPrice = order.trigger_price ? formatScaled(order.trigger_price) : '';
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		failure = null;

		const parsedQuantity = parseDecimalInput(quantity);
		if (!parsedQuantity) {
			failure = 'Enter a size.';
			return;
		}
		const body: AmendOrderRequest = { quantity: parsedQuantity };

		if (hasLimit) {
			const price = parseDecimalInput(limitPrice);
			if (!price) {
				failure = 'Enter a limit price.';
				return;
			}
			body.limit_price = price;
		}
		if (hasTrigger) {
			const trigger = parseDecimalInput(triggerPrice);
			if (!trigger) {
				failure = 'Enter a trigger price.';
				return;
			}
			body.trigger_price = trigger;
		}

		submitting = true;
		try {
			await apiClient.amendOrder(accountId, order.id, body);
			open = false;
			onamended();
		} catch (error) {
			failure = getErrorMessage(error, 'Could not amend this order.');
		} finally {
			submitting = false;
		}
	}

	const fieldClass =
		'h-[30px] rounded-none border-ink/16 bg-transparent font-mono text-[11.5px] shadow-none';
	const labelClass = 'font-mono text-[8px] tracking-[0.22em] text-dim';
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-w-md gap-0 rounded-none border-ink/14 p-0">
		<Dialog.Header class="border-b border-ink/9 px-4 py-3">
			<Dialog.Title class="font-mono text-[11px] tracking-[0.16em] uppercase">
				Amend order
			</Dialog.Title>
			<Dialog.Description class="font-mono text-[9.5px] tracking-[0.04em] text-dim">
				{order.side.toUpperCase()}
				{order.kind.replace('_', '-').toUpperCase()}
				{order.instrument}
			</Dialog.Description>
		</Dialog.Header>

		<form onsubmit={submit} class="flex flex-col gap-4 px-4 py-4">
			<div class="flex flex-col gap-2.5">
				{#if hasTrigger}
					<div class="flex flex-col gap-1.5">
						<span class={labelClass}>TRIGGER</span>
						<Input
							bind:value={triggerPrice}
							disabled={submitting}
							inputmode="decimal"
							class={fieldClass}
						/>
					</div>
				{/if}
				{#if hasLimit}
					<div class="flex flex-col gap-1.5">
						<span class={labelClass}>LIMIT PRICE</span>
						<Input
							bind:value={limitPrice}
							disabled={submitting}
							inputmode="decimal"
							class={fieldClass}
						/>
					</div>
				{/if}
				<div class="flex flex-col gap-1.5">
					<span class={labelClass}>SIZE</span>
					<Input bind:value={quantity} disabled={submitting} inputmode="decimal" class={fieldClass} />
				</div>
			</div>

			{#if failure}
				<p
					class="border border-loss/40 bg-loss/8 px-2.5 py-2 font-mono text-[9.5px] text-loss"
					data-dialog-error
				>
					{failure}
				</p>
			{/if}

			<div class="flex justify-end gap-2 border-t border-ink/9 pt-3">
				<Button
					type="button"
					variant="ghost"
					class="h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]"
					onclick={() => (open = false)}
				>
					CANCEL
				</Button>
				<Button
					type="submit"
					disabled={submitting}
					class="h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]"
				>
					{submitting ? 'AMENDING…' : 'AMEND'}
				</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>
