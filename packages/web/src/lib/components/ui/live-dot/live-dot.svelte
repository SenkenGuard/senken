<script lang="ts">
	// Generic pulsing status dot ("live-dot — pulsing status
	// dot, `senkenPulse` keyframes"). The reference applies `senkenPulse` via
	// an inline `animation` style rather than a class — this mirrors that exact technique, the same way
	// `ui/ticker-tape` mirrors the reference's marquee keyframe. Color is
	// intentionally left to the caller via `class` (e.g. `bg-gain`,
	// `bg-loss`) so this stays a plain generic primitive, not a
	// Senken-specific one — it renders a dot, not a "connection" or
	// "session" concept.
	import { cn } from '$lib/utils.js';

	let {
		pulse = false,
		durationSeconds = 2.4,
		class: className
	}: {
		/** Animate with `senkenPulse`. Off by default — most dots in the
		 * reference (e.g. the adapter account rows) are static; only a live
		 * marker or an active replay indicator pulses. */
		pulse?: boolean;
		durationSeconds?: number;
		class?: string;
	} = $props();
</script>

<div
	data-slot="live-dot"
	class={cn('size-[5px] flex-none rounded-full', className)}
	style={pulse ? `animation: senkenPulse ${durationSeconds}s infinite;` : undefined}
></div>

<style>
	@keyframes senkenPulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.25;
		}
	}
</style>
