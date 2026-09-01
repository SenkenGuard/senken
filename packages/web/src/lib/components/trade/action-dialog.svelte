<script lang="ts">
	// Running one of an adapter's own operations.
	//
	// The parameters are, again, whatever the adapter declared — the same
	// `SchemaForm` the settings dialog uses. A destructive action is
	// confirmed here rather than run on one click.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { apiClient } from '$lib/api/client';
	import { describeError } from '$lib/api/errors';
	import type { AdapterActionDto } from '$lib/api/types';
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
		action,
		accountId,
		accountLabel,
		onran
	}: {
		open: boolean;
		action: AdapterActionDto;
		accountId: string;
		accountLabel: string;
		onran: (message: string) => void;
	} = $props();

	let form = $state<FormState>({});
	let errors = $state<FormErrors>({});
	let submitting = $state(false);
	let failure = $state<string | null>(null);
	let loaded = $state(false);

	$effect(() => {
		if (!open) {
			loaded = false;
			return;
		}
		if (loaded) return;
		loaded = true;
		failure = null;
		errors = {};
		form = initialFormState(action.form);
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		failure = null;
		errors = validateForm(action.form, form);
		if (Object.keys(errors).length > 0) return;

		submitting = true;
		try {
			const outcome = await apiClient.runAdapterAction(accountId, action.id, {
				params: toSubmission(action.form, form)
			});
			open = false;
			onran(outcome.message);
		} catch (error) {
			failure = describeError(error);
		} finally {
			submitting = false;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="max-w-md gap-0 rounded-none border-ink/14 p-0">
		<Dialog.Header class="border-b border-ink/9 px-4 py-3">
			<Dialog.Title class="font-mono text-[11px] tracking-[0.16em] uppercase">
				{action.label}
			</Dialog.Title>
			<Dialog.Description class="font-mono text-[9.5px] tracking-[0.04em] text-dim">
				{action.description || `Runs on ${accountLabel}.`}
			</Dialog.Description>
		</Dialog.Header>

		<form onsubmit={submit} class="flex flex-col gap-4 px-4 py-4">
			<SchemaForm schema={action.form} bind:state={form} {errors} disabled={submitting} />

			{#if action.destructive}
				<p class="border border-loss/40 bg-loss/8 px-2.5 py-2 font-mono text-[9.5px] text-loss">
					This cannot be undone.
				</p>
			{/if}

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
					variant={action.destructive ? 'destructive' : 'default'}
					class="h-8 rounded-none font-mono text-[9.5px] tracking-[0.16em]"
				>
					{submitting ? 'RUNNING…' : action.label.toUpperCase()}
				</Button>
			</div>
		</form>
	</Dialog.Content>
</Dialog.Root>
