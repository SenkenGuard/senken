// Turning what the trade API returns into what the engine page draws.
//
// Every function here is pure: DTOs in, view model out, no fetching and no
// state. That is what makes the page's numbers testable without a server,
// and it is why the arithmetic that matters — summing equity across
// accounts, counting open orders — lives here rather than inside a
// component.
//
// # Money is summed as strings, never as numbers
//
// A total is built with `addScaled`, which does the addition on the digits.
// `Number(a.value) + Number(b.value)` would be shorter and would be wrong
// in exactly the cases that matter least often and cost most.

import type { Component } from 'svelte';
import CpuIcon from '@lucide/svelte/icons/cpu';
import LandmarkIcon from '@lucide/svelte/icons/landmark';
import CoinsIcon from '@lucide/svelte/icons/coins';
import type {
	AdapterDto,
	BalancesDto,
	FillDto,
	OrderDto,
	PositionDto,
	ScaledDto,
	TradeAccountDto
} from '$lib/api/types';
import { formatScaledForDisplay, formatSigned, toneOf } from './scaled';

/** Text-colour keys, mapped to Tailwind classes by whatever renders them. */
export type Tone = 'fg' | 'fg2' | 'dim' | 'dim2' | 'gain' | 'loss' | 'inv';

export interface StatItem {
	label: string;
	value: string;
	tone: Tone;
}

export interface TableColumn {
	label: string;
	align: 'left' | 'right';
}

export interface TableCell {
	value: string;
	tone: Tone;
	align: 'left' | 'right';
}

export interface TableRow {
	cells: TableCell[];
}

export type EngineTableTab = 'positions' | 'orders' | 'exec';

export interface TableTabDef {
	key: EngineTableTab;
	label: string;
	count: number;
}

export interface EngineTable {
	tabs: TableTabDef[];
	columns: TableColumn[];
	rows: TableRow[];
	emptyLabel: string;
}

export interface ExposureItem {
	icon: Component;
	name: string;
	equity: string;
	pct: number;
	highlighted: boolean;
}

export interface AccountCardData {
	id: string;
	label: string;
	mode: string;
	icon: Component;
	adapterName: string;
	equity: string;
	pnl: string;
	pnlTone: 'gain' | 'loss';
	selected: boolean;
}

export interface AdapterAccountRow {
	id: string;
	label: string;
	equity: string;
	selected: boolean;
	dotTone: 'gain' | 'loss';
}

export interface AdapterCardData {
	key: string;
	icon: Component;
	name: string;
	kind: string;
	status: string;
	note: string;
	connected: boolean;
	coverage: string;
	symbolChips: string[];
	attachLabel: string;
	accounts: AdapterAccountRow[];
}

/** Everything one account's panels are drawn from, as the page last read it
 * from that account's adapter. `null` while it is still loading, or when
 * the adapter could not answer. */
export interface Portfolio {
	balances: BalancesDto | null;
	positions: PositionDto[];
	orders: OrderDto[];
	fills: FillDto[];
	error: string | null;
}

/** An empty portfolio, for an account whose adapter has not answered yet. */
export function emptyPortfolio(): Portfolio {
	return { balances: null, positions: [], orders: [], fills: [], error: null };
}

/** The icon for an adapter kind. Presentational only — nothing branches on
 * this. */
export function iconForKind(kind: string): Component {
	switch (kind) {
		case 'simulation':
			return CpuIcon;
		case 'broker':
			return LandmarkIcon;
		default:
			return CoinsIcon;
	}
}

/** `DEMO` for a simulated account, `LIVE` for one that reaches a real
 * venue. The one badge a user must be able to trust at a glance. */
export function modeOf(adapter: AdapterDto | undefined): string {
	return adapter?.trades_real_money ? 'LIVE' : 'DEMO';
}

/** `a + b`, on the digits. Both are re-expressed at the wider scale first,
 * so adding a scale-2 balance to a scale-8 one is exact rather than
 * approximately right. */
export function addScaled(a: ScaledDto | null, b: ScaledDto | null): ScaledDto | null {
	if (!a) return b;
	if (!b) return a;
	const scale = Math.max(a.scale, b.scale);
	return { scale, value: (widen(a, scale) + widen(b, scale)).toString() };
}

/** The digits of `value` at `scale`, as a `bigint` — arbitrary precision,
 * so nothing is lost on the way through. */
function widen(value: ScaledDto, scale: number): bigint {
	return BigInt(value.value) * 10n ** BigInt(scale - value.scale);
}

/** Sums a column of scaled values. */
export function sumScaled(values: (ScaledDto | null | undefined)[]): ScaledDto | null {
	return values.reduce<ScaledDto | null>((total, value) => addScaled(total, value ?? null), null);
}

/** The percentage `part` is of `whole`, rounded to a whole number, or `0`
 * when `whole` is zero. Only ever used for a bar's width. */
export function percentOf(part: ScaledDto | null, whole: ScaledDto | null): number {
	if (!part || !whole) return 0;
	const scale = Math.max(part.scale, whole.scale);
	const denominator = widen(whole, scale);
	if (denominator === 0n) return 0;
	return Number((widen(part, scale) * 100n) / denominator);
}

/** The Overview view's top stat strip, across every visible account. */
export function aggregateStats(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): StatItem[] {
	const equity = sumScaled(accounts.map((a) => portfolios[a.id]?.balances?.equity));
	const balance = sumScaled(accounts.map((a) => portfolios[a.id]?.balances?.balance));
	const pnl = sumScaled(accounts.map((a) => portfolios[a.id]?.balances?.unrealized_pnl));
	const positions = accounts.reduce((n, a) => n + (portfolios[a.id]?.positions.length ?? 0), 0);
	const working = accounts.reduce(
		(n, a) => n + (portfolios[a.id]?.orders.filter((o) => isWorking(o)).length ?? 0),
		0
	);
	return [
		{ label: 'TOTAL EQUITY', value: formatScaledForDisplay(equity), tone: 'fg' },
		{ label: 'TOTAL BALANCE', value: formatScaledForDisplay(balance), tone: 'fg' },
		{ label: 'OPEN PNL', value: formatSigned(pnl), tone: toneOf(pnl) },
		{ label: 'POSITIONS', value: String(positions), tone: 'fg2' },
		{ label: 'WORKING ORDERS', value: String(working), tone: 'fg2' }
	];
}

/** Whether an order can still fill — the same partition
 * `senken_trade::OrderStatus::is_open` draws. */
export function isWorking(order: OrderDto): boolean {
	return ['pending', 'open', 'partially_filled'].includes(order.status);
}

/** The Accounts view's per-account stat strip. */
export function accountStats(portfolio: Portfolio | undefined): StatItem[] {
	const balances = portfolio?.balances ?? null;
	return [
		{
			label: 'BALANCE',
			value: formatScaledForDisplay(balances?.balance),
			tone: 'fg'
		},
		{ label: 'EQUITY', value: formatScaledForDisplay(balances?.equity), tone: 'fg' },
		{
			label: 'OPEN PNL',
			value: formatSigned(balances?.unrealized_pnl),
			tone: toneOf(balances?.unrealized_pnl)
		},
		{
			label: 'MARGIN USED',
			value: formatScaledForDisplay(balances?.margin_used),
			tone: 'fg2'
		}
	];
}

/** "Equity by adapter" bars. */
export function exposureByAdapter(
	accounts: TradeAccountDto[],
	adapters: AdapterDto[],
	portfolios: Record<string, Portfolio>
): ExposureItem[] {
	const total = sumScaled(accounts.map((a) => portfolios[a.id]?.balances?.equity));
	const keys = Array.from(new Set(accounts.map((a) => a.adapter_id)));
	return keys
		.map((key) => {
			const adapter = adapters.find((candidate) => candidate.id === key);
			const equity = sumScaled(
				accounts
					.filter((account) => account.adapter_id === key)
					.map((account) => portfolios[account.id]?.balances?.equity)
			);
			return {
				icon: iconForKind(adapter?.kind ?? 'exchange'),
				name: adapter?.name ?? key,
				equity: formatScaledForDisplay(equity),
				pct: percentOf(equity, total),
				// The largest share, computed after sorting rather than
				// pinned to whichever account happened to be opened first.
				highlighted: false
			};
		})
		.sort((a, b) => b.pct - a.pct)
		.map((item, index) => ({ ...item, highlighted: index === 0 }));
}

/** The Accounts view's card strip. */
export function accountCards(
	accounts: TradeAccountDto[],
	adapters: AdapterDto[],
	portfolios: Record<string, Portfolio>,
	activeId: string | null
): AccountCardData[] {
	return accounts.map((account) => {
		const adapter = adapters.find((candidate) => candidate.id === account.adapter_id);
		const pnl = portfolios[account.id]?.balances?.unrealized_pnl ?? null;
		return {
			id: account.id,
			label: account.label,
			mode: modeOf(adapter),
			icon: iconForKind(adapter?.kind ?? 'exchange'),
			adapterName: (adapter?.name ?? account.adapter_id).toUpperCase(),
			equity: formatScaledForDisplay(portfolios[account.id]?.balances?.equity),
			pnl: formatSigned(pnl),
			pnlTone: toneOf(pnl),
			selected: account.id === activeId
		};
	});
}

/** How an adapter's coverage reads on its card. */
export function coverageLabel(adapter: AdapterDto): string {
	switch (adapter.coverage.coverage) {
		case 'universal':
			return 'ALL INSTRUMENTS';
		case 'sources':
			return `${adapter.coverage.source_ids.length} VENUES`;
		default:
			return `${adapter.coverage.instruments.length} INSTRUMENTS`;
	}
}

/** The chips under an adapter's header. */
export function coverageChips(adapter: AdapterDto): string[] {
	switch (adapter.coverage.coverage) {
		case 'universal':
			return ['EVERY INSTRUMENT ON SENKEN'];
		case 'sources':
			return adapter.coverage.source_ids.map((id) => id.toUpperCase());
		default:
			return adapter.coverage.instruments.slice(0, 12);
	}
}

/** The Adapters view's card grid. */
export function adapterCards(
	adapters: AdapterDto[],
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>,
	activeId: string | null
): AdapterCardData[] {
	return adapters.map((adapter) => {
		const attached = accounts.filter((account) => account.adapter_id === adapter.id);
		return {
			key: adapter.id,
			icon: iconForKind(adapter.kind),
			name: adapter.name,
			kind: adapter.kind.toUpperCase(),
			// "Registered" is the honest word: an adapter being installed
			// says nothing about whether any account on it authenticates.
			// That is per-account health, shown on the account itself.
			status: attached.length > 0 ? 'IN USE' : 'AVAILABLE',
			note: adapter.description,
			connected: attached.length > 0,
			coverage: coverageLabel(adapter),
			symbolChips: coverageChips(adapter),
			attachLabel: 'ATTACH ACCOUNT',
			accounts: attached.map((account) => ({
				id: account.id,
				label: account.label,
				equity: formatScaledForDisplay(portfolios[account.id]?.balances?.equity),
				selected: account.id === activeId,
				dotTone: account.enabled ? 'gain' : 'loss'
			}))
		};
	});
}

const L = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'left' });
const R = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'right' });

/** A short local time, for a nanosecond timestamp. */
export function formatTime(nanos: number): string {
	return new Date(nanos / 1_000_000).toLocaleTimeString([], {
		hour: '2-digit',
		minute: '2-digit',
		second: '2-digit'
	});
}

/** Builds the POSITIONS / ORDERS / EXECUTIONS table.
 *
 * `scope` is `'all'` when several accounts are in view, which adds an
 * ACCOUNT column and changes the empty-state copy — "nothing across your
 * accounts" and "nothing on this account" are different statements. */
export function engineTable(
	tab: EngineTableTab,
	scope: 'all' | 'account',
	positions: (PositionDto & { accountLabel: string })[],
	orders: (OrderDto & { accountLabel: string })[],
	fills: (FillDto & { accountLabel: string })[]
): EngineTable {
	const accColumn = scope === 'all';
	const accHead: TableColumn[] = accColumn ? [{ label: 'ACCOUNT', align: 'left' }] : [];
	const accCell = (label: string): TableCell[] => (accColumn ? [L(label, 'dim2')] : []);
	const tabs: TableTabDef[] = [
		{ key: 'positions', label: 'POSITIONS', count: positions.length },
		{ key: 'orders', label: 'ORDERS', count: orders.length },
		{ key: 'exec', label: 'EXECUTIONS', count: fills.length }
	];

	if (tab === 'orders') {
		return {
			tabs,
			columns: [
				{ label: 'TIME', align: 'left' },
				{ label: 'INSTRUMENT', align: 'left' },
				...accHead,
				{ label: 'SIDE', align: 'left' },
				{ label: 'TYPE', align: 'left' },
				{ label: 'QTY', align: 'right' },
				{ label: 'PRICE', align: 'right' },
				{ label: 'STATUS', align: 'right' }
			],
			rows: orders.map((order) => ({
				cells: [
					L(formatTime(order.submitted_at), 'dim2'),
					L(order.instrument, 'fg'),
					...accCell(order.accountLabel),
					L(order.side.toUpperCase(), order.side === 'buy' ? 'gain' : 'loss'),
					L(order.kind.replace('_', '-').toUpperCase(), 'dim2'),
					R(formatScaledForDisplay(order.quantity, 8)),
					R(
						formatScaledForDisplay(
							order.limit_price ?? order.trigger_price ?? order.average_price,
							8
						)
					),
					R(
						order.status.replace('_', ' ').toUpperCase(),
						order.status === 'filled' ? 'gain' : isWorking(order) ? 'fg' : 'dim'
					)
				]
			})),
			emptyLabel: accColumn
				? 'NO ORDERS ACROSS ACCOUNTS — PLACE ONE FROM THE CHART'
				: 'NO ORDERS ON THIS ACCOUNT — PLACE ONE FROM THE CHART'
		};
	}

	if (tab === 'exec') {
		return {
			tabs,
			columns: [
				{ label: 'TIME', align: 'left' },
				{ label: 'INSTRUMENT', align: 'left' },
				...accHead,
				{ label: 'SIDE', align: 'left' },
				{ label: 'QTY', align: 'right' },
				{ label: 'FILL', align: 'right' },
				{ label: 'FEE', align: 'right' }
			],
			rows: fills.map((fill) => ({
				cells: [
					L(formatTime(fill.executed_at), 'dim2'),
					L(fill.instrument, 'fg'),
					...accCell(fill.accountLabel),
					L(fill.side.toUpperCase(), fill.side === 'buy' ? 'gain' : 'loss'),
					R(formatScaledForDisplay(fill.quantity, 8)),
					R(formatScaledForDisplay(fill.price, 8)),
					R(`${formatScaledForDisplay(fill.fee, 4)} ${fill.fee_currency}`, 'dim2')
				]
			})),
			emptyLabel: accColumn ? 'NO EXECUTIONS ACROSS ACCOUNTS' : 'NO EXECUTIONS ON THIS ACCOUNT'
		};
	}

	return {
		tabs,
		columns: [
			{ label: 'INSTRUMENT', align: 'left' },
			...accHead,
			{ label: 'SIDE', align: 'left' },
			{ label: 'QTY', align: 'right' },
			{ label: 'ENTRY', align: 'right' },
			{ label: 'MARK', align: 'right' },
			{ label: 'PNL', align: 'right' }
		],
		rows: positions.map((position) => ({
			cells: [
				L(position.instrument, 'fg'),
				...accCell(position.accountLabel),
				L(position.side.toUpperCase(), position.side === 'long' ? 'gain' : 'loss'),
				R(formatScaledForDisplay(position.quantity, 8)),
				R(formatScaledForDisplay(position.average_entry, 8), 'dim2'),
				// A position with no mark shows a dash, not the entry
				// price: pretending it is marked at cost would report a
				// real position as flat.
				R(formatScaledForDisplay(position.mark_price, 8)),
				R(
					position.unrealized_pnl ? formatSigned(position.unrealized_pnl) : '—',
					toneOf(position.unrealized_pnl)
				)
			]
		})),
		emptyLabel: accColumn ? 'NO OPEN POSITIONS ACROSS ACCOUNTS' : 'NO OPEN POSITIONS ON THIS ACCOUNT'
	};
}
