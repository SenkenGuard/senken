<script lang="ts">
	// The charts page's ALERTS right-panel tab (first built against
	// the static `ALERTS` mock array in `$lib/mock/charts`; it is now
	// it real). List/create/delete all go through `apiClient` — no direct
	// `fetch` anywhere in this file — driven entirely by
	// `senken_alerts::AlertStore`'s real fields: "fired" is `last_fired_at`
	// being non-null, never a hand-set UI flag.
	//
	// `crates/api` mounts `/api/alerts`. Its real
	// `AlertDto` nests the indicator and condition (`indicator: {name,
	// params}`, `condition: {field, comparator, threshold}`) rather than
	// the flat `indicator_name`/`indicator_params` shape this file was
	// written against before that mount existed — updated here to the generated shape now that `$lib/api/types.ts` gets `AlertDto`
	// from `GET /api/openapi.json` instead of a hand guess.
	import { onMount } from 'svelte';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as NativeSelect from '$lib/components/ui/native-select/index.js';
	import { cn } from '$lib/utils.js';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import { ensureSourcesLoaded, hasLiveFeed } from '$lib/api/sources.svelte';
	import { alertFormPrefill } from '$lib/charts/alert-form-prefill';
	import type { AlertDto, ComparatorDto, IndicatorFieldDto } from '$lib/api/types';

	let {
		paneInstrument,
		paneTimeframe
	}: {
		/** The active pane's own instrument/timeframe — the new-alert
		 * form prefills from these, and keeps following them while it stays
		 * open (see the `$effect` below), rather than the fixed
		 * `binance-spot:BTCUSDT`/`1h` literal this used to open with
		 * regardless of which pane it was opened from. */
		paneInstrument: string;
		paneTimeframe: string;
	} = $props();

	/** Mirrors `senken_indicators`' ten built-ins by name —
	 * `senken_alerts::ConcreteIndicator::build` matches these
	 * case-insensitively. */
	const INDICATORS = ['Sma', 'Ema', 'Wma', 'Rsi', 'Macd', 'Stochastic', 'BollingerBands', 'Atr', 'Vwap', 'Volume'] as const;
	/** Mirrors `crates/alerts/src/condition.rs::IndicatorField`. */
	const FIELDS: IndicatorFieldDto[] = [
		'Value',
		'MacdLine',
		'MacdSignal',
		'MacdHistogram',
		'StochasticK',
		'StochasticD',
		'BollingerUpper',
		'BollingerMiddle',
		'BollingerLower'
	];
	/** Mirrors `crates/alerts/src/condition.rs::Comparator`. */
	const COMPARATORS: { value: ComparatorDto; label: string }[] = [
		{ value: 'GreaterThan', label: '>' },
		{ value: 'LessThan', label: '<' },
		{ value: 'CrossesAbove', label: 'crosses above' },
		{ value: 'CrossesBelow', label: 'crosses below' }
	];

	let alerts = $state<AlertDto[]>([]);
	let total = $state(0);
	let loading = $state(true);
	let loadError = $state<string | null>(null);

	async function loadAlerts() {
		loading = true;
		loadError = null;
		try {
			const page = await apiClient.listAlerts(100, 0);
			alerts = page.rows;
			total = page.total;
		} catch (error) {
			loadError = getErrorMessage(error, 'Could not load alerts.');
		} finally {
			loading = false;
		}
	}

	let showCreate = $state(false);
	// Filled from the pane the form was opened on, and re-filled whenever
	// that pane changes instrument or timeframe while the form is open — an
	// alert offered against a symbol the user is no longer looking at is the
	// wrong default. Deliberately empty until then rather than seeded from
	// `paneInstrument`/`paneTimeframe` directly: a `$state` initializer only
	// captures the value at that instant, not a live binding, so reading the
	// prop again inside `alertFormPrefill` on every effect run (not once,
	// here) is what actually keeps this following the pane instead of
	// freezing at whichever one was active when the component first mounted.
	let newInstrument = $state('');
	let newTimeframe = $state('');
	$effect(() => {
		const prefill = alertFormPrefill(showCreate, { instrument: paneInstrument, timeframe: paneTimeframe });
		if (!prefill) return;
		newInstrument = prefill.instrument;
		newTimeframe = prefill.timeframe;
	});
	let newIndicator = $state<(typeof INDICATORS)[number]>('Rsi');
	let newParams = $state('{"period":14}');
	let newField = $state<IndicatorFieldDto>('Value');
	let newComparator = $state<ComparatorDto>('GreaterThan');
	let newThreshold = $state('70');
	let creating = $state(false);
	let createError = $state<string | null>(null);

	const canCreate = $derived(
		newInstrument.trim().length > 0 &&
			newTimeframe.trim().length > 0 &&
			newParams.trim().length > 0 &&
			newThreshold.trim().length > 0 &&
			!Number.isNaN(Number(newThreshold)) &&
			!creating
	);

	async function submitCreate(event: SubmitEvent) {
		event.preventDefault();
		if (!canCreate) return;
		creating = true;
		createError = null;
		try {
			await apiClient.createAlert({
				instrument: newInstrument.trim(),
				timeframe: newTimeframe.trim(),
				indicator: { name: newIndicator, params: newParams.trim() },
				condition: { field: newField, comparator: newComparator, threshold: Number(newThreshold) }
			});
			showCreate = false;
			await loadAlerts();
		} catch (error) {
			createError = getErrorMessage(error, 'Could not create that alert.');
		} finally {
			creating = false;
		}
	}

	let deletingId = $state<string | null>(null);
	let deleteError = $state<string | null>(null);

	async function removeAlert(id: string) {
		deletingId = id;
		deleteError = null;
		try {
			await apiClient.deleteAlert(id);
			await loadAlerts();
		} catch (error) {
			deleteError = getErrorMessage(error, 'Could not delete that alert.');
		} finally {
			deletingId = null;
		}
	}

	/** `c` comes back from the server typed as plain `string` (see this
	 * file's header comment on `ConditionDto`'s generated shape) even though
	 * it is always one of `COMPARATORS`' own values in practice. */
	function comparatorLabel(c: string): string {
		return COMPARATORS.find((x) => x.value === c)?.label ?? c;
	}

	onMount(() => {
		void loadAlerts();
		void ensureSourcesLoaded();
	});
</script>

<div class="flex min-h-0 flex-1 flex-col gap-2 overflow-auto p-3">
	<button
		type="button"
		class="cursor-pointer border border-dashed border-ink/18 py-2.5 text-center font-mono text-[9px] tracking-[0.2em] text-dim2 hover:border-ink/30 hover:text-foreground"
		onclick={() => (showCreate = !showCreate)}
	>
		<PlusIcon class="mr-1 inline size-3 align-[-2px]" /> NEW ALERT
	</button>

	{#if showCreate}
		<form onsubmit={submitCreate} class="flex flex-col gap-1.5 border border-ink/14 bg-card2 p-2.5">
			<Input placeholder="Instrument, e.g. binance-spot:BTCUSDT" bind:value={newInstrument} class="h-7 text-[11px]" />
			<div class="flex gap-1.5">
				<Input placeholder="Timeframe, e.g. 1h" bind:value={newTimeframe} class="h-7 w-[90px] text-[11px]" />
				<NativeSelect.Root bind:value={newIndicator} size="sm" class="h-7 flex-1 text-[11px]">
					{#each INDICATORS as i (i)}
						<NativeSelect.Option value={i}>{i}</NativeSelect.Option>
					{/each}
				</NativeSelect.Root>
			</div>
			<Input placeholder={'Params JSON, e.g. {"period":14}'} bind:value={newParams} class="h-7 font-mono text-[11px]" />
			<div class="flex gap-1.5">
				<NativeSelect.Root bind:value={newField} size="sm" class="h-7 flex-1 text-[11px]">
					{#each FIELDS as f (f)}
						<NativeSelect.Option value={f}>{f}</NativeSelect.Option>
					{/each}
				</NativeSelect.Root>
				<NativeSelect.Root bind:value={newComparator} size="sm" class="h-7 w-[110px] text-[11px]">
					{#each COMPARATORS as c (c.value)}
						<NativeSelect.Option value={c.value}>{c.label}</NativeSelect.Option>
					{/each}
				</NativeSelect.Root>
				<Input placeholder="Threshold" bind:value={newThreshold} class="h-7 w-[80px] text-[11px]" />
			</div>
			<div class="flex items-center gap-2">
				<Button type="submit" size="sm" disabled={!canCreate}>{creating ? 'Creating…' : 'Create alert'}</Button>
				<Button type="button" variant="outline" size="sm" onclick={() => (showCreate = false)}>Cancel</Button>
			</div>
			{#if createError}
				<p class="text-[11px] text-destructive">{createError}</p>
			{/if}
		</form>
	{/if}

	{#if loading}
		<p class="text-center font-mono text-[9px] tracking-[0.16em] text-dim">LOADING…</p>
	{:else if loadError}
		<div class="flex items-start gap-2 border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-destructive">
			<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
			<p class="text-[11px]">{loadError}</p>
		</div>
	{:else}
		{#each alerts as a (a.id)}
			{@const canRun = hasLiveFeed(a.instrument)}
			<div class="flex flex-col gap-1.5 border border-ink/9 bg-inv px-2.5 py-2.5">
				<div class="flex items-center justify-between gap-2">
					<span class="truncate font-mono text-[10.5px] tracking-[0.06em] text-foreground">{a.instrument}</span>
					<div class="flex items-center gap-1.5">
						<span
							class={cn(
								'font-mono text-[8.5px] tracking-[0.18em]',
								!canRun ? 'text-dim' : a.last_fired_at ? 'text-loss' : 'text-gain'
							)}
						>
							{!canRun ? 'CANNOT RUN' : a.last_fired_at ? 'TRIGGERED' : 'ARMED'}
						</span>
						<button
							type="button"
							class="cursor-pointer text-dim2 hover:text-destructive disabled:opacity-40"
							disabled={deletingId === a.id}
							onclick={() => removeAlert(a.id)}
							aria-label="Delete alert"
						>
							<XIcon class="size-3" />
						</button>
					</div>
				</div>
				<span class="font-mono text-[9.5px] text-secondary-foreground">
					{a.indicator.name}({a.timeframe}) {a.condition.field}
					{comparatorLabel(a.condition.comparator)}
					{a.condition.threshold}
				</span>
				{#if a.last_fired_at}
					<span class="font-mono text-[8.5px] text-dim">
						fired {a.fire_count > 1 ? `${a.fire_count}× · ` : ''}last at {a.last_fired_value}
					</span>
				{:else if !canRun}
					<!-- The server already knows this alert will never fire this
					     session (`senken_alerts::engine` logs exactly why) — this
					     says the same thing as product copy, not `ARMED` claiming
					     the opposite. -->
					<span class="font-mono text-[8.5px] text-dim">this venue has no live feed and will not run</span>
				{/if}
			</div>
		{/each}
		{#if alerts.length === 0}
			<p class="py-4 text-center font-mono text-[9px] tracking-[0.2em] text-dim">NO ALERTS YET</p>
		{/if}
		{#if deleteError}
			<p class="text-[11px] text-destructive">{deleteError}</p>
		{/if}
		<p class="text-center font-mono text-[8.5px] tracking-[0.1em] text-dim">
			{alerts.length} of {total}
		</p>
	{/if}
</div>
