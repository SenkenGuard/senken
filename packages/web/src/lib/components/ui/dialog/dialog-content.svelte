<script lang="ts">
	import { Dialog as DialogPrimitive } from "bits-ui";
	import XIcon from '@lucide/svelte/icons/x';
	import { Button } from "$lib/components/ui/button/index.js";
	import { cn, type WithoutChildrenOrChild } from "$lib/utils.js";
	import * as Dialog from "./index.js";
	import DialogPortal from "./dialog-portal.svelte";
	import type { Snippet } from "svelte";
	import type { ComponentProps } from "svelte";

	let {
		ref = $bindable(null),
		class: className,
		portalProps,
		children,
		showCloseButton = true,
		...restProps
	}: WithoutChildrenOrChild<DialogPrimitive.ContentProps> & {
		portalProps?: WithoutChildrenOrChild<ComponentProps<typeof DialogPortal>>;
		children: Snippet;
		showCloseButton?: boolean;
	} = $props();
</script>

<DialogPortal {...portalProps}>
	<Dialog.Overlay />
	<DialogPrimitive.Content
		bind:ref
		data-slot="dialog-content"
		class={cn(
			// No `sm:max-w-md` (shadcn's stock cap): every dialog in this app
			// sets its own `w-[...]` (chart settings, the layer
			// dialog and the command palette all specify a real width, e.g.
			// `w-[592px]`), but a caller's plain `max-w-[...]` never overrides a
			// `sm:`-scoped rule — different modifier chain, so tailwind-merge
			// can't dedupe them — and the desktop shell is always past the `sm`
			// breakpoint. The result was every dialog silently clamped to 28rem
			// (448px) regardless of its own width class, confirmed via
			// `getBoundingClientRect` on chart settings (592px requested,
			// 448px rendered) — which is what made its content pane end up
			// narrower than its own left nav column.
			"grid max-w-[calc(100%-2rem)] gap-6 rounded-xl bg-popover p-6 text-sm text-popover-foreground ring-1 ring-foreground/10 duration-100 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95 fixed top-1/2 left-1/2 z-50 w-full -translate-x-1/2 -translate-y-1/2 outline-none",
			className
		)}
		{...restProps}
	>
		{@render children?.()}
		{#if showCloseButton}
			<DialogPrimitive.Close data-slot="dialog-close">
				{#snippet child({ props })}
					<Button variant="ghost" class="absolute top-4 right-4" size="icon-sm" {...props}>
						<XIcon  />
						<span class="sr-only">Close</span>
					</Button>
				{/snippet}
			</DialogPrimitive.Close>
		{/if}
	</DialogPrimitive.Content>
</DialogPortal>
