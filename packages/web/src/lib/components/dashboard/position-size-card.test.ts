// Renders the real, compiled widget through Svelte's own server renderer —
// the same reason `equity-card.test.ts` and `order-ticket.test.ts` do it
// this way. The property under test: the widget shows an honest empty
// state until every required field is filled (never a zero that could pass
// for an answer), a saved config round-trips back into the form, and a
// zero stop distance renders its rejection message rather than
// `Infinity`/`NaN`.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import PositionSizeCard from './position-size-card.svelte';

describe('position-size-card (real DOM output)', () => {
	test('shows the waiting state and no result before anything is typed', () => {
		const { body } = render(PositionSizeCard, { props: {} });
		expect(body).toContain('Enter a balance, a risk percent, an entry price and a stop price.');
		expect(body).not.toContain('Infinity');
		expect(body).not.toContain('NaN');
	});

	test('a saved config round-trips into the visible fields and computes the same answer', () => {
		const config = JSON.stringify({
			balance: '10000',
			riskPercent: '1',
			entryPrice: '100',
			stopPrice: '95',
			sizeDecimals: '2'
		});
		const { body } = render(PositionSizeCard, { props: { config } });
		expect(body).toContain('value="10000"');
		expect(body).toContain('value="95"');
		expect(body).toContain('100.00');
		expect(body).toContain('20.00');
	});

	test('an equal entry and stop price renders the rejection message, never Infinity or NaN', () => {
		const config = JSON.stringify({
			balance: '10000',
			riskPercent: '1',
			entryPrice: '100',
			stopPrice: '100',
			sizeDecimals: '2'
		});
		const { body } = render(PositionSizeCard, { props: { config } });
		expect(body).toContain('there is no distance to risk against');
		expect(body).not.toContain('Infinity');
		expect(body).not.toContain('NaN');
	});

	test('malformed saved config falls back to the empty form rather than throwing', () => {
		const { body } = render(PositionSizeCard, { props: { config: 'not json' } });
		expect(body).toContain('Enter a balance, a risk percent, an entry price and a stop price.');
	});
});
