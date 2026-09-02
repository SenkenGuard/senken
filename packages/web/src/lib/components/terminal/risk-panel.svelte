<script lang="ts">
	// Risk widget body: margin used against margin available per account,
	// then each held instrument's notional as a share of total equity. Both
	// are real inputs off the wire (`dashboardRisk`, `$lib/trade/view.ts`) —
	// nothing here is an invented exposure gauge.
	import { MeterBar } from '$lib/components/ui/meter-bar/index.js';
	import type { DashboardRiskData } from '$lib/trade/view';

	let { data }: { data: DashboardRiskData } = $props();

	const empty = $derived(data.margin.length === 0 && data.exposure.length === 0);
</script>

<div class="flex h-full flex-col gap-[13px] overflow-auto">
	{#if empty}
		<div class="flex flex-1 items-center justify-center font-mono text-[10px] tracking-[0.18em] text-dim">
			NO ACCOUNTS ATTACHED
		</div>
	{:else}
		{#each data.margin as m (m.accountId)}
			{#if m.hasMargin}
				<MeterBar
					label={`MARGIN · ${m.accountLabel.toUpperCase()}`}
					value={m.label}
					fraction={m.usedPct / 100}
				/>
			{:else}
				<div
					data-account-margin={m.accountId}
					class="flex items-baseline justify-between font-mono text-[8.5px] tracking-[0.22em] text-dim"
				>
					<span>MARGIN · {m.accountLabel.toUpperCase()}</span>
					<span class="text-secondary-foreground">NO MARGIN ON THIS ACCOUNT</span>
				</div>
			{/if}
		{/each}
		{#each data.exposure as e (e.instrument)}
			{#if e.pct === null}
				<!-- More than one currency in view: a position's notional is in
				     its instrument's quote currency and there is no rate here to
				     bring them onto one denominator, so the amount is shown and
				     the bar is not. A bar that filled anyway would be claiming a
				     proportion nothing supports. -->
				<MeterBar label={e.instrument.toUpperCase()} value={e.notional} fraction={0} />
			{:else}
				<MeterBar
					label={e.instrument.toUpperCase()}
					value={`${e.notional} · ${e.pct}%`}
					fraction={e.pct / 100}
				/>
			{/if}
		{/each}
	{/if}
</div>
