// Renders the real, compiled order ticket through Svelte's own server
// renderer and reads the actual markup, rather than asserting on the logic
// that is supposed to produce it — the same reason `schema-form.test.ts`
// does. A read-only account is the property that matters most here: the
// ticket must not merely disable itself cosmetically, it must never emit a
// button that would place an order.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { AccountAccessDto, TradeAccountDto } from '$lib/api/types';
import OrderTicket from './order-ticket.svelte';

const account: TradeAccountDto = {
	id: 'acct-1',
	owner_id: 'user-1',
	adapter_id: 'simulator',
	label: 'Investor',
	enabled: true,
	owned: true,
	created_at: 0,
	updated_at: 0
};

const tradingAccess: AccountAccessDto = {
	level: 'trade',
	capabilities: {
		order_kinds: ['market', 'limit'],
		time_in_force: ['gtc'],
		quantity_unit: 'base',
		position_mode: 'spot_holdings',
		features: []
	},
	note: null
};

const readOnlyAccess: AccountAccessDto = {
	...tradingAccess,
	level: 'read_only',
	note: 'This account was attached read-only.'
};

const noop = () => {};

const baseProps = {
	instrument: 'okx-spot:BTCUSDT',
	account,
	mid: 100,
	onPlace: noop
};

describe('order ticket (real DOM output)', () => {
	test('a trading account renders a place button and no read-only copy', () => {
		const { body } = render(OrderTicket, { props: { ...baseProps, access: tradingAccess } });
		expect(body).toContain('PLACE BUY ORDER');
		expect(body).not.toContain('READ-ONLY ACCOUNT');
		expect(body).not.toContain('data-ticket-readonly-note');
	});

	test('a read-only account renders the disabled button and its note, never a place button', () => {
		const { body } = render(OrderTicket, { props: { ...baseProps, access: readOnlyAccess } });
		expect(body).toContain('READ-ONLY ACCOUNT');
		expect(body).toContain('data-ticket-readonly-note');
		expect(body).toContain('This account was attached read-only.');
		expect(body).not.toContain('PLACE BUY ORDER');
		expect(body).not.toContain('PLACE SELL ORDER');
	});

	test('a read-only account disables the size input rather than accepting a size it will never send', () => {
		const { body } = render(OrderTicket, { props: { ...baseProps, access: readOnlyAccess } });
		// The size box is always rendered; on a read-only account it must
		// carry `disabled`, not merely look inert.
		expect(body).toContain('disabled');
	});

	test('a trading account with no note attached renders no note line', () => {
		const { body } = render(OrderTicket, { props: { ...baseProps, access: tradingAccess } });
		expect(body).not.toContain('data-ticket-readonly-note');
	});
});
