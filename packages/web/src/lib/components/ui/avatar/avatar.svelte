<script lang="ts">
	import { Avatar as AvatarPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		loadingStatus = $bindable("loading"),
		size = "default",
		class: className,
		...restProps
	}: AvatarPrimitive.RootProps & {
		size?: "default" | "sm" | "lg";
	} = $props();
</script>

<AvatarPrimitive.Root
	bind:ref
	bind:loadingStatus
	data-slot="avatar"
	data-size={size}
	class={cn(
		// `rounded-full` removed here and across the avatar family (see
		// switch.svelte for the same rationale): it's a fixed Tailwind
		// utility, not derived from `--radius`, so the square,
		// no-radius reference can't fix it at the token level.
		"size-8 data-[size=lg]:size-10 data-[size=sm]:size-6 group/avatar relative flex shrink-0 select-none after:absolute after:inset-0 after:border after:border-border after:mix-blend-darken dark:after:mix-blend-lighten",
		className
	)}
	{...restProps}
/>
