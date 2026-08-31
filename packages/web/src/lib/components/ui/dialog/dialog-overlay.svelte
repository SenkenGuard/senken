<script lang="ts">
	import { Dialog as DialogPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	// `bg-ink-shadow/72` replaces the registry's `bg-black/10` +
	// `backdrop-blur-xs` (layout.css's `--color-ink-shadow` comment): the reference's own dialogs (layer, chart settings, command
	// palette) all use `rgba(var(--ink-shadow),0.72)` with no blur, which
	// reads much darker than shadcn's default overlay.
	let {
		ref = $bindable(null),
		class: className,
		...restProps
	}: DialogPrimitive.OverlayProps = $props();
</script>

<DialogPrimitive.Overlay
	bind:ref
	data-slot="dialog-overlay"
	class={cn("bg-ink-shadow/72 duration-100 data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0 fixed inset-0 isolate z-50", className)}
	{...restProps}
/>
