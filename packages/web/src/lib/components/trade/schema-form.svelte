<script lang="ts">
	// The dynamic form builder: renders whatever an adapter declared, with
	// no adapter-specific code anywhere in this file.
	//
	// A plugin describes its settings as data and this reads that data.
	// It deliberately cannot render anything a plugin sends beyond the six
	// field kinds `senken-trade` defines — a plugin that could inject
	// markup or script here would have the session of every user who opened
	// its settings screen, and no amount of review makes that safe.
	import { cn } from '$lib/utils.js';
	import type { SettingFieldDto, SettingsSchemaDto } from '$lib/api/types';
	import type { FormErrors, FormState } from '$lib/trade/form';
	import { secretPlaceholder } from '$lib/trade/form';
	import { Input } from '$lib/components/ui/input/index.js';

	let {
		schema,
		state = $bindable(),
		errors = {},
		secretsSet = {},
		disabled = false
	}: {
		schema: SettingsSchemaDto;
		state: FormState;
		errors?: FormErrors;
		secretsSet?: Record<string, boolean>;
		disabled?: boolean;
	} = $props();

	const inputClass =
		'h-8 rounded-none border-ink/16 bg-transparent font-mono text-[11.5px] shadow-none';

	/** Only two of the six kinds carry a placeholder at all, so this narrows
	 * to them rather than reading a field that does not exist on the rest. */
	function placeholderFor(field: SettingFieldDto): string {
		if (field.type === 'secret') return secretPlaceholder(field, secretsSet);
		return field.type === 'text' ? (field.placeholder ?? '') : '';
	}
</script>

{#if schema.fields.length === 0}
	<p class="font-mono text-[10px] tracking-[0.06em] text-dim">
		This adapter needs no settings.
	</p>
{:else}
	<div class="flex flex-col gap-3.5">
		{#each schema.fields as field (field.key)}
			{@const error = errors[field.key]}
			<div class="flex flex-col gap-1.5" data-field={field.key} data-field-type={field.type}>
				<div class="flex items-baseline justify-between gap-2">
					<label
						for={`field-${field.key}`}
						class="font-mono text-[8.5px] tracking-[0.2em] text-dim uppercase"
					>
						{field.label}
					</label>
					{#if !field.required}
						<span class="font-mono text-[8px] tracking-[0.14em] text-dim">OPTIONAL</span>
					{/if}
				</div>

				{#if field.type === 'toggle'}
					<!-- A plain `role="switch"` button rather than the shared
					     `ui/switch`: that one is a bits-ui primitive whose
					     runtime cannot render outside a browser, and this
					     form is the one place in the app whose exact output
					     is asserted against. A control that cannot be
					     rendered in a test is a control nothing checks. -->
					<button
						type="button"
						id={`field-${field.key}`}
						role="switch"
						aria-checked={state[field.key] === true}
						{disabled}
						class={cn(
							'flex h-8 w-fit cursor-pointer items-center gap-2 border border-ink/16 px-2.5',
							disabled && 'opacity-50'
						)}
						onclick={() => (state[field.key] = state[field.key] !== true)}
					>
						<span
							class={cn(
								'size-2.5 flex-none',
								state[field.key] === true ? 'bg-gain' : 'bg-ink/25'
							)}
						></span>
						<span class="font-mono text-[9.5px] tracking-[0.14em] text-dim2">
							{state[field.key] === true ? 'ON' : 'OFF'}
						</span>
					</button>
				{:else if field.type === 'choice'}
					<select
						id={`field-${field.key}`}
						{disabled}
						class={cn(
							'h-8 w-full appearance-none border border-ink/16 bg-transparent px-2.5 font-mono text-[11.5px] text-foreground outline-none',
							error && 'border-loss'
						)}
						value={String(state[field.key] ?? '')}
						onchange={(event) => (state[field.key] = event.currentTarget.value)}
					>
						{#each field.options as option (option.value)}
							<option value={option.value}>{option.label}</option>
						{/each}
					</select>
				{:else}
					<div class="flex items-center gap-2">
						<Input
							id={`field-${field.key}`}
							{disabled}
							type={field.type === 'secret' ? 'password' : 'text'}
							inputmode={field.type === 'number' || field.type === 'decimal'
								? 'decimal'
								: undefined}
							autocomplete={field.type === 'secret' ? 'off' : undefined}
							placeholder={placeholderFor(field)}
							class={cn(inputClass, error && 'border-loss')}
							value={String(state[field.key] ?? '')}
							oninput={(event) => (state[field.key] = event.currentTarget.value)}
						/>
						{#if (field.type === 'number' || field.type === 'decimal') && field.unit}
							<span class="font-mono text-[9px] tracking-[0.14em] text-dim">{field.unit}</span>
						{/if}
					</div>
				{/if}

				{#if error}
					<span class="font-mono text-[9px] tracking-[0.04em] text-loss" data-field-error>
						{error}
					</span>
				{:else if field.help}
					<span class="font-mono text-[9px] tracking-[0.04em] text-dim">{field.help}</span>
				{/if}
			</div>
		{/each}
	</div>
{/if}
