import { describe, expect, test } from 'bun:test';
import {
	DRAWING_KIND_FOR_TOOL,
	TOOL_ANCHOR_COUNT,
	drawingFromDto,
	drawingToInput,
	neighborInGroup,
	objectTree,
	swapById,
	type DrawingRuntime,
	type LayerRuntime,
	drawingsForInstrument
} from './pane-runtime';
import type { DrawingDto } from '$lib/api/types';

function baseDto(overrides: Partial<DrawingDto>): DrawingDto {
	return {
		id: 'd1',
		position: 0,
		visible: true,
		color: '#7aa7e8',
		width: 2,
		line_style: 'SOLID',
		kind: { kind: 'horizontal_line', price: 100 },
		...overrides
	};
}

describe('drawingFromDto — new schema-v8 kinds round-trip through drawingToInput', () => {
	test('a horizontal_line carries the server\'s real `visible`, not a hardcoded default', () => {
		const hidden = drawingFromDto(baseDto({ visible: false }));
		expect(hidden.visible).toBe(false);
		const shown = drawingFromDto(baseDto({ visible: true }));
		expect(shown.visible).toBe(true);
	});

	test('a ray reads start/end and converts wire nanoseconds to chart seconds', () => {
		const dto = baseDto({
			kind: { kind: 'ray', start: { time: 60_000_000_000, price: 10 }, end: { time: 120_000_000_000, price: 20 } }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'ray',
			start: { time: 60, price: 10 },
			end: { time: 120, price: 20 }
		});
	});

	test('a fib_retracement reads start/end the same way a ray does', () => {
		const dto = baseDto({
			kind: { kind: 'fib_retracement', start: { time: 60_000_000_000, price: 100 }, end: { time: 120_000_000_000, price: 200 } }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'fib_retracement',
			start: { time: 60, price: 100 },
			end: { time: 120, price: 200 }
		});
	});

	test('a text_note reads `at` into `start`, plus its text and anchor', () => {
		const dto = baseDto({
			kind: { kind: 'text_note', at: { time: 60_000_000_000, price: 42 }, text: 'supply zone', anchor: 'below' }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'text_note',
			start: { time: 60, price: 42 },
			text: 'supply zone',
			anchor: 'below'
		});
	});

	test('every new kind survives a round trip back through drawingToInput', () => {
		const dtos: DrawingDto[] = [
			baseDto({ kind: { kind: 'ray', start: { time: 60_000_000_000, price: 10 }, end: { time: 120_000_000_000, price: 20 } } }),
			baseDto({
				kind: { kind: 'fib_retracement', start: { time: 60_000_000_000, price: 100 }, end: { time: 120_000_000_000, price: 200 } }
			}),
			baseDto({ kind: { kind: 'text_note', at: { time: 60_000_000_000, price: 42 }, text: 'note', anchor: 'center' }, visible: false })
		];
		for (const dto of dtos) {
			const input = drawingToInput(drawingFromDto(dto));
			expect(input.kind).toEqual(dto.kind);
			expect(input.visible).toBe(dto.visible);
		}
	});
});

describe('objectTree — a drawing group uses each drawing\'s own `visible`, not a hardcoded true', () => {
	function drawing(overrides: Partial<DrawingRuntime>): DrawingRuntime {
		return {
			id: 'd1',
			position: 0,
			kind: 'horizontal_line',
			visible: true,
			price: 100,
			color: '#7aa7e8',
			width: 2,
			lineStyle: 'SOLID',
			...overrides
		};
	}

	test('a hidden drawing reports visible: false in the DRAWINGS group', () => {
		const groups = objectTree([], [drawing({ visible: false })]);
		const drawings = groups.find((g) => g.title === 'DRAWINGS');
		expect(drawings?.items).toEqual([
			{ id: 'd1', kind: 'drawing', name: 'HORIZONTAL LINE', params: '100.00', visible: false, canConfigure: true }
		]);
	});

	test('a shown drawing still reports visible: true — proving this is the real field, not always-false either', () => {
		const groups = objectTree([], [drawing({ visible: true })]);
		const drawings = groups.find((g) => g.title === 'DRAWINGS');
		expect(drawings?.items[0]?.visible).toBe(true);
	});
});

describe('objectTree — rows carry the identity and capability the panel acts on', () => {
	function overlayLayer(overrides: Partial<LayerRuntime>): LayerRuntime {
		return { id: 'l1', position: 0, kind: 'overlay_instrument', visible: true, instrument: 'binance-spot:ETHUSDT', params: {}, ...overrides };
	}
	function indicatorLayer(overrides: Partial<LayerRuntime>): LayerRuntime {
		return { id: 'l2', position: 1, kind: 'indicator_overlay', visible: true, indicatorName: 'Ema', params: { period: 20 }, ...overrides };
	}
	function drawing(overrides: Partial<DrawingRuntime>): DrawingRuntime {
		return { id: 'd1', position: 0, kind: 'trend_line', visible: true, color: '#7aa7e8', width: 2, lineStyle: 'SOLID', ...overrides };
	}

	test('an overlay_instrument row carries the layer id and cannot be configured, since it has no settings dialog', () => {
		const groups = objectTree([overlayLayer({})], []);
		const item = groups.find((g) => g.key === 'instruments')?.items[0];
		expect(item).toMatchObject({ id: 'l1', kind: 'layer', canConfigure: false });
	});

	test('an indicator layer row carries the layer id and can be configured', () => {
		const groups = objectTree([indicatorLayer({})], []);
		const item = groups.find((g) => g.key === 'indicators')?.items[0];
		expect(item).toMatchObject({ id: 'l2', kind: 'layer', canConfigure: true });
	});

	test('a drawing row carries the drawing id, is tagged kind: "drawing", and can be configured', () => {
		const groups = objectTree([], [drawing({})]);
		const item = groups.find((g) => g.key === 'drawings')?.items[0];
		expect(item).toMatchObject({ id: 'd1', kind: 'drawing', canConfigure: true });
	});

	test('items keep whatever order layers/drawings arrive in — the same position-ascending order paneFromDto already sorts them into', () => {
		const layers = [indicatorLayer({ id: 'later', position: 1 }), indicatorLayer({ id: 'earlier', position: 0 })];
		const groups = objectTree(layers, []);
		expect(groups.find((g) => g.key === 'indicators')?.items.map((i) => i.id)).toEqual(['later', 'earlier']);

		const drawings = [drawing({ id: 'first', position: 0 }), drawing({ id: 'second', position: 1 })];
		const drawingGroups = objectTree([], drawings);
		expect(drawingGroups.find((g) => g.key === 'drawings')?.items.map((i) => i.id)).toEqual(['first', 'second']);
	});

	test('a group with nothing in it still comes back with its title and an empty item list', () => {
		const groups = objectTree([], []);
		expect(groups.find((g) => g.key === 'instruments')).toMatchObject({ title: 'INSTRUMENTS', items: [] });
		expect(groups.find((g) => g.key === 'indicators')).toMatchObject({ title: 'INDICATORS', items: [] });
		expect(groups.find((g) => g.key === 'drawings')).toMatchObject({ title: 'DRAWINGS', items: [] });
	});
});

describe('the new tools are wired through the same data tables as the original three', () => {
	test('ray, fib_retracement and text_note each have an anchor count and a DrawingKindTag', () => {
		expect(TOOL_ANCHOR_COUNT.ray).toBe(2);
		expect(TOOL_ANCHOR_COUNT.fib).toBe(2);
		expect(TOOL_ANCHOR_COUNT.text).toBe(1);
		expect(DRAWING_KIND_FOR_TOOL.ray).toBe('ray');
		expect(DRAWING_KIND_FOR_TOOL.fib).toBe('fib_retracement');
		expect(DRAWING_KIND_FOR_TOOL.text).toBe('text_note');
	});
});

describe('neighborInGroup — the object tree panel reorders within its own group, never across it', () => {
	// A realistic pane: one overlay instrument (position 0) then two
	// indicators (positions 1, 2). The INDICATORS group's own ordered ids —
	// what `object-tree.svelte` actually passes — never includes the
	// instrument, which is the whole point: a direction-plus-index computed
	// against the pane's *whole* `layers` array would see the instrument as
	// the indicator's "previous" row and let it move there, which is
	// exactly the bug this replaces.
	const indicatorGroupIds = ['indicator-a', 'indicator-b'];

	test('the first row in a group has no neighbor moving up, even though another kind of row precedes it in the full pane array', () => {
		expect(neighborInGroup(indicatorGroupIds, 'indicator-a', 'up')).toBeNull();
	});

	test('the last row in a group has no neighbor moving down', () => {
		expect(neighborInGroup(indicatorGroupIds, 'indicator-b', 'down')).toBeNull();
	});

	test('a middle move resolves to the id directly above or below in that group\'s own order', () => {
		expect(neighborInGroup(indicatorGroupIds, 'indicator-b', 'up')).toBe('indicator-a');
		expect(neighborInGroup(['a', 'b', 'c'], 'b', 'down')).toBe('c');
	});

	test('an id that is not in the group has no neighbor in either direction', () => {
		expect(neighborInGroup(indicatorGroupIds, 'overlay-instrument', 'up')).toBeNull();
		expect(neighborInGroup(indicatorGroupIds, 'overlay-instrument', 'down')).toBeNull();
	});
});

describe('swapById — swaps two named rows and renumbers position to match, regardless of what sits between them', () => {
	function layer(id: string, position: number): LayerRuntime {
		return { id, position, kind: 'indicator_overlay', visible: true, indicatorName: 'Ema', params: {} };
	}

	test('swaps two adjacent items and renumbers both', () => {
		const items = [layer('a', 0), layer('b', 1)];
		const result = swapById(items, 'a', 'b');
		expect(result.map((x) => [x.id, x.position])).toEqual([
			['b', 0],
			['a', 1]
		]);
	});

	test('swaps two items with another kind of row sitting between them in the full array, unaffected', () => {
		// Mirrors the real shape: an overlay_instrument (position 1) sits
		// between the two indicators being swapped — `swapById` finds each by
		// id, not by adjacency, so the row between them keeps its own position.
		const overlay: LayerRuntime = { id: 'ovl', position: 1, kind: 'overlay_instrument', visible: true, instrument: 'x:Y', params: {} };
		const items = [layer('a', 0), overlay, layer('b', 2)];
		const result = swapById(items, 'a', 'b');
		expect(result.map((x) => [x.id, x.position])).toEqual([
			['b', 0],
			['ovl', 1],
			['a', 2]
		]);
	});

	test('an unknown id leaves the array unchanged', () => {
		const items = [layer('a', 0), layer('b', 1)];
		expect(swapById(items, 'a', 'missing')).toBe(items);
	});

	test('swapping an item with itself is a no-op', () => {
		const items = [layer('a', 0), layer('b', 1)];
		expect(swapById(items, 'a', 'a')).toBe(items);
	});

	test('accepts an already out-of-order array — sorts by position before swapping', () => {
		const items = [layer('b', 1), layer('a', 0), layer('c', 2)];
		const result = swapById(items, 'a', 'c');
		expect(result.map((x) => [x.id, x.position])).toEqual([
			['c', 0],
			['b', 1],
			['a', 2]
		]);
	});
});


describe('a drawing belongs to the instrument it was drawn against', () => {
	const drawing = (id: string, instrument?: string): DrawingRuntime => ({
		id,
		position: 0,
		kind: 'horizontal_line',
		visible: true,
		price: 1,
		instrument,
		color: '#fff',
		width: 1,
		lineStyle: 'SOLID'
	});

	test('a pane shows the drawings made on what it is currently displaying', () => {
		const drawings = [drawing('eth', 'binance-spot:ETHUSDT'), drawing('sol', 'binance-spot:SOLUSDT')];
		expect(drawingsForInstrument(drawings, 'binance-spot:ETHUSDT').map((d) => d.id)).toEqual(['eth']);
	});

	// The failure this exists to stop: a horizontal line at 2450 drawn on
	// Ethereum means nothing on a market trading at 0.4, and painting it
	// there is a claim nobody made.
	test('another instrument’s drawings are not painted over these candles', () => {
		const drawings = [drawing('eth', 'binance-spot:ETHUSDT')];
		expect(drawingsForInstrument(drawings, 'binance-spot:SOLUSDT')).toEqual([]);
	});

	// Switching symbol hides them; switching back brings them straight back.
	// The pane never loses them, which is why this is a filter and not a
	// delete.
	test('switching away and back shows the same drawings again', () => {
		const drawings = [drawing('eth', 'binance-spot:ETHUSDT')];
		expect(drawingsForInstrument(drawings, 'binance-spot:SOLUSDT')).toEqual([]);
		expect(drawingsForInstrument(drawings, 'binance-spot:ETHUSDT').map((d) => d.id)).toEqual(['eth']);
	});

	// Nothing ever recorded what these were drawn against. Hiding them would
	// silently lose work; attributing them to whatever is on screen would
	// invent a fact.
	test('a drawing with no recorded instrument is shown whatever the pane displays', () => {
		const drawings = [drawing('legacy')];
		expect(drawingsForInstrument(drawings, 'binance-spot:ETHUSDT').map((d) => d.id)).toEqual(['legacy']);
		expect(drawingsForInstrument(drawings, 'okx-spot:BTCUSDT').map((d) => d.id)).toEqual(['legacy']);
	});
});
