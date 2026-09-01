import { describe, expect, test } from 'bun:test';
import { hasBarSource, hasBookFeed, hasLiveFeed, hasQuoteFeed, sourceIdOf } from './source-capability';
import type { SourceCapabilityDto } from './types';

function sourcesMap(rows: SourceCapabilityDto[]): Map<string, SourceCapabilityDto> {
	return new Map(rows.map((s) => [s.id, s]));
}

describe('sourceIdOf', () => {
	test('splits the wire form on the first colon', () => {
		expect(sourceIdOf('okx-spot:BTCUSDT')).toBe('okx-spot');
	});

	test('an instrument with no colon is its own source id rather than throwing', () => {
		expect(sourceIdOf('not-an-instrument')).toBe('not-an-instrument');
	});
});

describe('hasLiveFeed / hasBarSource — "never true without bars", read from the client side', () => {
	const sources = sourcesMap([
		{ id: 'okx-spot', name: 'OKX Spot', bars: true, live: true, quotes: true, book: { supported: true } },
		{ id: 'binance-spot', name: 'Binance Spot', bars: true, live: false, quotes: false, book: { supported: false } },
		{ id: 'whitebit', name: 'WhiteBIT', bars: false, live: false, quotes: false, book: { supported: false } }
	]);

	test('a source with both a bar source and a live pool reports both', () => {
		expect(hasBarSource(sources, 'okx-spot:BTCUSDT')).toBe(true);
		expect(hasLiveFeed(sources, 'okx-spot:BTCUSDT')).toBe(true);
		expect(hasQuoteFeed(sources, 'okx-spot:BTCUSDT')).toBe(true);
	});

	test('a source that can chart but cannot stream reports bars without live', () => {
		expect(hasBarSource(sources, 'binance-spot:BTCUSDT')).toBe(true);
		expect(hasLiveFeed(sources, 'binance-spot:BTCUSDT')).toBe(false);
		expect(hasQuoteFeed(sources, 'binance-spot:BTCUSDT')).toBe(false);
	});

	test('a source with neither reports neither', () => {
		expect(hasBarSource(sources, 'whitebit:ABTC')).toBe(false);
		expect(hasLiveFeed(sources, 'whitebit:ABTC')).toBe(false);
		expect(hasQuoteFeed(sources, 'whitebit:ABTC')).toBe(false);
	});

	// The defensive case the current doc calls out: an id absent
	// from the map at all (not yet loaded, or not a registered source)
	// must never be read as "has a feed" — the countdown/green-red line
	// must never show on the strength of an absent answer.
	test('an unregistered or not-yet-loaded source id defaults to neither, never to a false positive', () => {
		const empty = sourcesMap([]);
		expect(hasBarSource(empty, 'okx-spot:BTCUSDT')).toBe(false);
		expect(hasLiveFeed(empty, 'okx-spot:BTCUSDT')).toBe(false);
		expect(hasQuoteFeed(empty, 'okx-spot:BTCUSDT')).toBe(false);
	});
});

describe('hasBookFeed — the order-book panel\'s own capability, nested under `book.supported`', () => {
	const sources = sourcesMap([
		{ id: 'okx-spot', name: 'OKX Spot', bars: true, live: true, quotes: true, book: { supported: true } },
		{ id: 'binance-spot', name: 'Binance Spot', bars: true, live: false, quotes: false, book: { supported: false } }
	]);

	test('a source whose nested book capability reports supported: true', () => {
		expect(hasBookFeed(sources, 'okx-spot:BTCUSDT')).toBe(true);
	});

	test('a source whose nested book capability reports supported: false', () => {
		expect(hasBookFeed(sources, 'binance-spot:BTCUSDT')).toBe(false);
	});

	test('an unregistered or not-yet-loaded source id defaults to false, never to a false positive', () => {
		expect(hasBookFeed(sourcesMap([]), 'okx-spot:BTCUSDT')).toBe(false);
	});
});
