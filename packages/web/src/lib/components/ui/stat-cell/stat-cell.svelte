<script lang="ts" module>
	import { type VariantProps, tv } from "tailwind-variants";

	export const statCellVariants = tv({
		base: "font-mono text-xs font-medium",
		variants: {
			tone: {
				bright: "text-foreground",
				gain: "text-gain",
				loss: "text-loss",
				dim: "text-muted-foreground",
			},
		},
		defaultVariants: {
			tone: "bright",
		},
	});

	export type StatCellTone = VariantProps<typeof statCellVariants>["tone"];
</script>

<script lang="ts">
	import { cn } from "$lib/utils.js";

	let {
		label,
		value,
		tone = "bright",
		class: className,
	}: {
		label: string;
		value: string;
		tone?: StatCellTone;
		class?: string;
	} = $props();
</script>

<div data-slot="stat-cell" class={cn("flex flex-col justify-center gap-[3px]", className)}>
	<div class="font-mono text-[7.5px] tracking-[0.26em] text-dim">{label}</div>
	<div class={statCellVariants({ tone })}>{value}</div>
</div>
