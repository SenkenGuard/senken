<script lang="ts">
	import { DropdownMenu as DropdownMenuPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		inset,
		variant = "default",
		...restProps
	}: DropdownMenuPrimitive.ItemProps & {
		inset?: boolean;
		variant?: "default" | "destructive";
	} = $props();

	// `leading-none`, not shadcn's stock line-height: every
	// caller in this app puts its own mono caption in a nested span (8-11px,
	// never shadcn's `text-sm`) but never restated `text-sm`'s paired 20px
	// line-height here, so that line box kept padding the row from the
	// inside regardless of the label's real font-size. That read as uneven
	// top/bottom space (a small glyph sitting high in a tall, untouched line
	// box), not as an obviously-wrong number — confirmed via
	// `getComputedStyle` (row height 31px against an 8px/12px `padding` that
	// was already correct and even).
</script>

<DropdownMenuPrimitive.Item
	bind:ref
	data-slot="dropdown-menu-item"
	data-inset={inset}
	data-variant={variant}
	class={cn(
		"gap-2 rounded-sm px-2 py-1.5 text-sm leading-none focus:bg-accent focus:text-accent-foreground not-data-[variant=destructive]:focus:**:text-accent-foreground data-inset:pl-8 data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 data-[variant=destructive]:focus:text-destructive dark:data-[variant=destructive]:focus:bg-destructive/20 [&_svg:not([class*='size-'])]:size-4 data-[variant=destructive]:*:[svg]:text-destructive group/dropdown-menu-item relative flex cursor-default items-center outline-hidden select-none data-[inset]:pl-8 data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
		className
	)}
	{...restProps}
/>
