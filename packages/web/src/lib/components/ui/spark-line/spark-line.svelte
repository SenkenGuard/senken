<script lang="ts">
	// Generic inline SVG sparkline. Renders one or two polylines
	// (a primary series and an optional dashed benchmark) inside a
	// `viewBox="0 0 300 100"`, matching the reference's equity-card spark. Knows nothing about equity,
	// symbols, or any other app concept — only numbers.
	import { cn } from '$lib/utils.js';

	let {
		values,
		benchmark,
		min,
		max,
		class: className,
		strokeClass = 'stroke-foreground',
		benchmarkStrokeClass = 'stroke-ink/28'
	}: {
		/** Primary series, plotted left to right across the full width. */
		values: number[];
		/** Optional second series, drawn dashed behind the primary line. */
		benchmark?: number[];
		/** Value domain. Defaults to the min/max across both series, so the
		 * line always fills the available height. Pass explicit bounds to
		 * keep two sparklines on the same scale. */
		min?: number;
		max?: number;
		class?: string;
		strokeClass?: string;
		benchmarkStrokeClass?: string;
	} = $props();

	const domain = $derived.by(() => {
		if (min !== undefined && max !== undefined) return { min, max };
		const all = benchmark ? [...values, ...benchmark] : values;
		return { min: Math.min(...all), max: Math.max(...all) };
	});

	function toPoints(series: number[], lo: number, hi: number): string {
		const range = hi - lo || 1;
		return series
			.map((v, i) => {
				const x = (i * 300) / Math.max(series.length - 1, 1);
				const y = 100 - ((v - lo) / range) * 100;
				return `${x.toFixed(1)},${y.toFixed(1)}`;
			})
			.join(' ');
	}

	const points = $derived(toPoints(values, domain.min, domain.max));
	const benchmarkPoints = $derived(benchmark ? toPoints(benchmark, domain.min, domain.max) : undefined);
</script>

<svg
	viewBox="0 0 300 100"
	preserveAspectRatio="none"
	class={cn('block', className)}
	data-slot="spark-line"
>
	{#if benchmarkPoints}
		<polyline
			points={benchmarkPoints}
			fill="none"
			class={cn('stroke-1', benchmarkStrokeClass)}
			style="stroke-dasharray: 2 3;"
			vector-effect="non-scaling-stroke"
		/>
	{/if}
	<polyline
		points={points}
		fill="none"
		class={cn('stroke-[1.2px]', strokeClass)}
		vector-effect="non-scaling-stroke"
	/>
</svg>
