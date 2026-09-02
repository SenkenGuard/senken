<script lang="ts">
	// Open positions widget body. Real, across every owned account —
	// `dashboardPositions` (`$lib/trade/view.ts`) builds the same `EngineTable`
	// shape the engine page's own POSITIONS tab uses, so this renders through
	// `execution-grid.svelte` rather than a second table implementation.
	// Read-only: every row's `tradable` is `false` (see that function's own
	// docs), so no actions column ever appears here.
	import ExecutionGrid from './execution-grid.svelte';
	import type { EngineTable } from '$lib/trade/view';

	let { table }: { table: EngineTable } = $props();
</script>

<ExecutionGrid {table} />
