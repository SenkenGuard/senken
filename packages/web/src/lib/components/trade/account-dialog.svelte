<script lang="ts">
	// Attaching an account to an adapter, and editing one that exists.
	//
	// One dialog for both, because they are the same form: the difference
	// is where the initial values come from and which endpoint the submit
	// goes to. The fields themselves are whatever the adapter declared —
	// this file names none of them.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { cn } from '$lib/utils.js';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { AdapterDto, TradeAccountDto } from '$lib/api/types';
	import SchemaForm from './schema-form.svelte';
	import {
		initialFormState,
		toSubmission,
		validateForm,
		type FormErrors,
		type FormState
	} from '$lib/trade/form';

	let {
		open = $bindable(false),
		adapter,
		account = null,
		onsaved
	}: {
		open: boolean;
		adapter: AdapterDto;
		/** `null` attaches a new account; otherwise this one is edited. */
		account?: TradeAccountDto | null;
		onsaved: () => void;
	} = $props();

	let label = $state('');
	let form = $state<FormState>({});
	let errors = $state<FormErrors>({});
	let secretsSet = $state<Record<string, boolean>>({});
	let submitting = $state(false);
	let failure = $state<string | null>(null);
	let loaded = $state(false);

	const editing = $derived(account !== null);
	const title = $derived(editing ? `Edit ${account?.label}` : `Attach ${adapter.name}`);

	// Loads whenever the dialog opens, and re-derives from whatever the
	// server currently holds rather than from anything this component was
	// last given: an account edited in another tab must not be saved back
	// from a stale copy.
	$effect(() => {
		if (!open) {
			loaded = false;
			return;
		}
		if (loaded) return;
		loaded = true;
		failure = null;
		errors = {};
		label = account?.label ?? defaultLabel();
		if (!account) {
			form = initialFormState(adapter.settings_schema);
			secretsSet = {};
			return;
		}
		const id = account.id;
		void apiClient
			.tradeAccountSettings(id)
			.then((stored) => {
				form = initialFormState(adapter.settings_schema, stored.settings);
				secretsSet = stored.secrets_set;
			})
			.catch((error) => {
				failure = getErrorMessage(error, "Could not load this account's settings.");
			});
	});

	function defaultLabel(): string {
		return adapter.name;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		failure = null;
		errors = validateForm(adapter.settings_schema, form);
		if (Object.keys(errors).length > 0) return;
		if (label.trim() === '') {
			failure = 'This account needs a name.';
			return;
		}

		submitting = true;
		try {
			const settings = toSubmission(adapter.settings_schema, form);
			if (account) {
				await apiClient.updateTradeAccount(account.id, { label: label.trim() });
				await apiClient.replaceTradeAccountSettings(account.id, { settings });
			} else {
				await apiClient.createTradeAccount({
					adapter_id: adapter.id,
					label: label.trim(),
					settings
				});
			}
			open = false;
			onsaved();
		} catch (error) {
			failure = getErrorMessage(
				error,
				editing ? 'Could not save this account.' : 'Could not attach this account.'
			);
		} finally {
			submitting = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-w-lg gap-0 rounded-none border-ink/14 p-0">
		<Dialog.Header class="border-b border-ink/9 px-4 py-3">
			<Dialog.Title class="font-mono text-[11px] tracking-[0.16em] uppercase">
				{title}
			</Dialog.Title>
			<Dialog.Description class="font-mono text-[9.5px] tracking-[0.04em] text-dim">
				{adapter.trades_real_money
					? 'Orders on this account reach a real venue and move real money.'
					: 'Simulated. No order on this account leaves this machine.'}
			</Dialog.Description>
		</Dialog.Header>

		<form onsubmit={submit} class="flex max-h-[60vh] flex-col gap-4 overflow-auto px-4 py-4">
			<div class="flex flex-col gap-1.5">
				<label
					for="account-label"
					class="font-mono text-[8.5px] tracking-[0.2em] text-dim uppercase"
				>
					Account name
				</label>
				<Input
					id="account-label"
					bind:value={label}
					class="h-8 rounded-none border-ink/16 bg-transparent font-mono text-[11.5px] shadow-none"
				/>
				<span class="font-mono text-[9px] tracking-[0.04em] text-dim">
					What this account is called in your own account list.
				</span>
			</div>

			<SchemaForm
				schema={adapter.settings_schema}
				bind:state={form}
				{errors}
				{secretsSet}
				disabled={submitting}
			/>

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
					class={cn('h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]')}
					onclick={() => (open = false)}
				>
					CANCEL
				</Button>
				<Button
					type="submit"
					disabled={submitting}
					class="h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]"
				>
					{submitting ? 'SAVING…' : editing ? 'SAVE' : 'ATTACH'}
				</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>
