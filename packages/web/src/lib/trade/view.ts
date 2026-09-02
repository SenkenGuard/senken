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
import { formatInstant } from '$lib/time';
import type {
	AccountAccessDto,
	AdapterDto,
	AdapterHealthDto,
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
	/** Present only when `value` is a mixed-currency placeholder (`"3
	 * accounts · 2 currencies"`) rather than a real total — one formatted
	 * line per currency, meant to be shown under the headline value so the
	 * figure is not simply missing. Absent for every other stat. */
	detail?: string[];
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

/** One control in a row's trailing actions column. `key` is what
 * `onaction` is called with — `'close' | 'cancel' | 'amend'` today. */
export interface TableRowAction {
	key: string;
	label: string;
	tone: 'default' | 'destructive';
}

export interface TableRow {
	/** Identifies the resource this row is drawn from — `accountId:orderId`
	 * for an order or a fill, `accountId:instrument` for a position — so a
	 * click on one of `actions` can be traced back to what to act on
	 * without the caller re-deriving it from the rendered cells. */
	key: string;
	cells: TableCell[];
	/** Optional per-row controls, rendered in a trailing actions column.
	 * Absent on every row means no actions column at all. */
	actions?: TableRowAction[];
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
	/** `true` when at least one account in scope has no portfolio fetched
	 * yet at all — as distinct from a fetch that came back with genuinely
	 * nothing. An empty table renders one or the other, never a blank box
	 * that could be either. */
	loading: boolean;
}

export interface ExposureItem {
	icon: Component;
	name: string;
	equity: string;
	/** The currency this row's equity is in. One row per adapter *per
	 * currency*, because a share only means anything within one. */
	currency: string;
	/** 0-100, of the total equity **in this row's own currency**. */
	pct: number;
	highlighted: boolean;
}

/** A status dot's colour plus the reason worth showing beside it —
 * `'unknown'` while nothing has answered yet or the account's adapter is
 * not installed, kept distinct from `'disconnected'` so a card never
 * claims a venue rejected credentials it never got to ask. */
export type HealthState = 'connected' | 'degraded' | 'disconnected' | 'unknown';

/** The Tailwind class for a health dot, shared by every place one is drawn
 * so the account card and the account chip can never disagree on what
 * "degraded" looks like. Degraded is still answering and worth watching, so
 * it is the one state a caller may also choose to pulse; disconnected sits
 * solid — a connection not coming back on its own is a different fact from
 * one that might. */
export const healthDotClass: Record<HealthState, string> = {
	connected: 'bg-gain',
	degraded: 'bg-loss',
	disconnected: 'bg-loss',
	unknown: 'bg-ink/25'
};

export interface AccountCardData {
	id: string;
	label: string;
	mode: string;
	icon: Component;
	adapterName: string;
	/** `false` when the account's `adapter_id` no longer resolves to an
	 * installed adapter — the account row outlives the plugin. `adapterName`
	 * still carries some text in that case (the raw id), but a card must say
	 * plainly that the adapter is gone rather than let a raw id read as a
	 * real name. */
	adapterInstalled: boolean;
	/** `''` while balances have not loaded, so a caller can tell "no
	 * currency yet" from "an adapter that reports none". */
	currency: string;
	equity: string;
	pnl: string;
	pnlTone: 'gain' | 'loss';
	selected: boolean;
	/** `true` when this account's resolved access is `read_only` — a
	 * MetaTrader 5 investor login, an exchange key with no trade scope.
	 * `false` both when the account trades and while its access has not
	 * loaded yet, since a badge that flickered on and off during every load
	 * would be worse than one that is a beat late to appear. */
	readOnly: boolean;
	health: HealthState;
	/** The adapter's own words for a degraded or disconnected state, `null`
	 * otherwise (including `'unknown'`, which has nothing to explain). */
	healthReason: string | null;
	/** This account's most recent portfolio fetch failure, `null` when the
	 * last fetch succeeded or none has completed yet. Shown on the card
	 * itself, not only in the Accounts view's own banner, so a failure is
	 * visible from the strip that lists every account. */
	portfolioError: string | null;
}

export interface AdapterAccountRow {
	id: string;
	label: string;
	equity: string;
	/** `''` while balances have not loaded. */
	currency: string;
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

/** Equity, balance and open PnL, summed separately per currency. There is no
 * FX rate anywhere in this system, so an account priced in USD and one
 * priced in USDT must never land in the same total — the same rule
 * `addScaled` already enforces for two different *scales* of the same unit,
 * applied here to two different units. One entry per currency actually seen,
 * in the order its first account appears; an account whose balances have not
 * loaded yet (or which answers with no currency at all) contributes to
 * none. */
export function sumByCurrency(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): CurrencyTotal[] {
	const order: string[] = [];
	const totals = new Map<string, CurrencyTotal>();
	// Margin is optional per adapter, and a partial sum is worse than none:
	// adding the one account that reports it to the one that does not
	// produces a figure that looks like the whole and understates it. Once
	// any contributing account is silent, that currency's margin is unknown
	// and stays unknown.
	const marginUnknown = new Set<string>();
	const availableUnknown = new Set<string>();
	for (const account of accounts) {
		const balances = portfolios[account.id]?.balances;
		if (!balances || !balances.currency) continue;
		const currency = balances.currency;
		let entry = totals.get(currency);
		if (!entry) {
			entry = {
				currency,
				equity: null,
				balance: null,
				pnl: null,
				marginUsed: null,
				marginAvailable: null
			};
			totals.set(currency, entry);
			order.push(currency);
		}
		entry.equity = addScaled(entry.equity, balances.equity);
		entry.balance = addScaled(entry.balance, balances.balance);
		entry.pnl = addScaled(entry.pnl, balances.unrealized_pnl ?? null);
		if (balances.margin_used) {
			entry.marginUsed = addScaled(entry.marginUsed, balances.margin_used);
		} else {
			marginUnknown.add(currency);
		}
		if (balances.margin_available) {
			entry.marginAvailable = addScaled(entry.marginAvailable, balances.margin_available);
		} else {
			availableUnknown.add(currency);
		}
	}
	return order.map((currency) => {
		const total = totals.get(currency)!;
		return {
			...total,
			marginUsed: marginUnknown.has(currency) ? null : total.marginUsed,
			marginAvailable: availableUnknown.has(currency) ? null : total.marginAvailable
		};
	});
}

/** One currency's totals across whichever accounts share it, from
 * `sumByCurrency`. */
export interface CurrencyTotal {
	currency: string;
	equity: ScaledDto | null;
	balance: ScaledDto | null;
	pnl: ScaledDto | null;
	/** `null` when at least one account in this currency does not report
	 * margin at all — see `sumByCurrency` for why that is not summed
	 * around. */
	marginUsed: ScaledDto | null;
	/** `null` on the same rule as `marginUsed`. */
	marginAvailable: ScaledDto | null;
}

/** `a * b`, exactly — the scales add rather than widen, because that is what
 * multiplying two decimals actually produces (`1.5` at scale 1 times `2.25`
 * at scale 2 is `3.375` at scale 3, not scale 2). Used for a position's
 * notional (`quantity * mark_price`), which is still money and must not go
 * through a double just because it is derived rather than wired straight
 * off an adapter. */
export function mulScaled(a: ScaledDto, b: ScaledDto): ScaledDto {
	return { scale: a.scale + b.scale, value: (BigInt(a.value) * BigInt(b.value)).toString() };
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
	const totals = sumByCurrency(accounts, portfolios);
	const positions = accounts.reduce((n, a) => n + (portfolios[a.id]?.positions.length ?? 0), 0);
	const working = accounts.reduce(
		(n, a) => n + (portfolios[a.id]?.orders.filter((o) => isWorking(o)).length ?? 0),
		0
	);
	return [
		moneyStat('TOTAL EQUITY', accounts.length, totals, (t) => t.equity, false),
		moneyStat('TOTAL BALANCE', accounts.length, totals, (t) => t.balance, false),
		moneyStat('OPEN PNL', accounts.length, totals, (t) => t.pnl, true),
		{ label: 'POSITIONS', value: String(positions), tone: 'fg2' },
		{ label: 'WORKING ORDERS', value: String(working), tone: 'fg2' }
	];
}

/** One money stat in a totals strip, built from `sumByCurrency`'s grouping.
 * When every account in view shares a currency (or there are none loaded at
 * all) this is an ordinary total with its unit alongside it — `totals` has
 * at most one entry. When they do not, summing would silently add USD to
 * USDT, so the headline becomes the count of accounts and currencies
 * instead, and each currency's own subtotal is carried in `detail` rather
 * than lost. */
function moneyStat(
	label: string,
	accountCount: number,
	totals: CurrencyTotal[],
	pick: (total: CurrencyTotal) => ScaledDto | null,
	signed: boolean
): StatItem {
	const format = signed ? formatSigned : formatScaledForDisplay;
	if (totals.length <= 1) {
		const total = totals[0];
		const value = total ? pick(total) : null;
		const text = format(value);
		return {
			label,
			value: total ? `${text} ${total.currency}` : text,
			tone: signed ? toneOf(value) : 'fg'
		};
	}
	return {
		label,
		value: `${accountCount} account${accountCount === 1 ? '' : 's'} · ${totals.length} currencies`,
		tone: 'fg2',
		detail: totals.map((total) => `${format(pick(total))} ${total.currency}`)
	};
}

/** One HUD cell in the shell's top bar, from `hudStats`. `key` is what the
 * bar's own visibility toggles are keyed on, so a stat can be turned off
 * without depending on its label text. */
export interface HudStat extends StatItem {
	key: string;
}

/** The shell top bar's stat strip, across every account the shared trade
 * store currently holds.
 *
 * Every cell here is read from an adapter's own answer. There is
 * deliberately no "PNL 24h", no "open risk" and no "buying power": nothing
 * in this system records an equity history to difference, no adapter
 * reports an R-multiple, and "buying power" would be a friendlier name for
 * available margin that not every venue reports at all. A number in the
 * chrome of every screen is the one a user glances at instead of checking,
 * so it is either something an adapter said or it is not shown.
 *
 * With no accounts attached (or none loaded yet) the money cells read as
 * `formatScaledForDisplay`'s placeholder rather than as zero — an account
 * whose balances have not arrived has not told us it is empty.
 */
export function hudStats(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): HudStat[] {
	const totals = sumByCurrency(accounts, portfolios);
	const positions = accounts.reduce((n, a) => n + (portfolios[a.id]?.positions.length ?? 0), 0);
	const working = accounts.reduce(
		(n, a) => n + (portfolios[a.id]?.orders.filter((o) => isWorking(o)).length ?? 0),
		0
	);
	return [
		{ key: 'equity', ...moneyStat('EQUITY', accounts.length, totals, (t) => t.equity, false) },
		{ key: 'pnl', ...moneyStat('OPEN PNL', accounts.length, totals, (t) => t.pnl, true) },
		{ key: 'positions', label: 'POSITIONS', value: String(positions), tone: 'fg2' },
		{ key: 'working', label: 'WORKING ORDERS', value: String(working), tone: 'fg2' },
		{ key: 'balance', ...moneyStat('BALANCE', accounts.length, totals, (t) => t.balance, false) },
		{
			key: 'margin',
			...moneyStat('MARGIN USED', accounts.length, totals, (t) => t.marginUsed, false)
		},
		{
			key: 'available',
			...moneyStat('MARGIN FREE', accounts.length, totals, (t) => t.marginAvailable, false)
		}
	];
}

/** Which of `hudStats` the top bar shows before anyone changes it. */
export const DEFAULT_HUD_KEYS = ['equity', 'pnl', 'positions', 'working'];

/** Whether an order can still fill — the same partition
 * `senken_trade::OrderStatus::is_open` draws. */
export function isWorking(order: OrderDto): boolean {
	return ['pending', 'open', 'partially_filled'].includes(order.status);
}

/** Whether row-level actions (CLOSE, CANCEL, AMEND) may be offered for
 * `account` at all: owned, enabled, and not read-only. Absence of an access
 * entry (not yet loaded) reads as unrestricted, the same rule
 * `state/trade.svelte.ts` documents for every other access check. */
export function isAccountTradable(
	account: TradeAccountDto,
	access: Record<string, AccountAccessDto>
): boolean {
	return account.owned && account.enabled && access[account.id]?.level !== 'read_only';
}

/** The Accounts view's per-account stat strip. */
export function accountStats(portfolio: Portfolio | undefined): StatItem[] {
	const balances = portfolio?.balances ?? null;
	// One account, so one currency — never at risk of the blending
	// `aggregateStats` guards against, just missing its unit until now.
	const currency = balances?.currency;
	const withCurrency = (text: string): string => (currency ? `${text} ${currency}` : text);
	return [
		{
			label: 'BALANCE',
			value: withCurrency(formatScaledForDisplay(balances?.balance)),
			tone: 'fg'
		},
		{ label: 'EQUITY', value: withCurrency(formatScaledForDisplay(balances?.equity)), tone: 'fg' },
		{
			label: 'OPEN PNL',
			value: withCurrency(formatSigned(balances?.unrealized_pnl)),
			tone: toneOf(balances?.unrealized_pnl)
		},
		{
			label: 'MARGIN USED',
			value: withCurrency(formatScaledForDisplay(balances?.margin_used)),
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
	// Grouped by adapter **and currency**, and each share is of its own
	// currency's total. A single "equity by adapter" bar spanning a USD
	// account and a USDT one would be a percentage of a number that does
	// not exist: there is no rate anywhere in this system, and inventing
	// one to make a bar fill is the same class of error as adding two
	// scaled integers at different scales, which `addScaled` already
	// refuses.
	const totalsByCurrency = new Map<string, ScaledDto | null>();
	for (const account of accounts) {
		const balances = portfolios[account.id]?.balances;
		if (!balances) continue;
		totalsByCurrency.set(
			balances.currency,
			addScaled(totalsByCurrency.get(balances.currency) ?? null, balances.equity)
		);
	}

	const groups = new Map<string, { adapterId: string; currency: string; equity: ScaledDto | null }>();
	for (const account of accounts) {
		const balances = portfolios[account.id]?.balances;
		if (!balances) continue;
		const key = `${account.adapter_id}\u0000${balances.currency}`;
		const existing = groups.get(key);
		groups.set(key, {
			adapterId: account.adapter_id,
			currency: balances.currency,
			equity: addScaled(existing?.equity ?? null, balances.equity)
		});
	}

	return Array.from(groups.values())
		.map((group) => {
			const adapter = adapters.find((candidate) => candidate.id === group.adapterId);
			return {
				icon: iconForKind(adapter?.kind ?? 'exchange'),
				name: adapter?.name ?? group.adapterId,
				equity: formatScaledForDisplay(group.equity),
				currency: group.currency,
				pct: percentOf(group.equity, totalsByCurrency.get(group.currency) ?? null),
				// The largest share, set after sorting rather than pinned to
				// whichever account happened to be opened first.
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
	activeId: string | null,
	access: Record<string, AccountAccessDto> = {},
	health: Record<string, AdapterHealthDto> = {}
): AccountCardData[] {
	return accounts.map((account) => {
		const adapter = adapters.find((candidate) => candidate.id === account.adapter_id);
		const adapterInstalled = adapter !== undefined;
		const balances = portfolios[account.id]?.balances;
		const pnl = balances?.unrealized_pnl ?? null;
		const resolved = healthOf(adapterInstalled, health[account.id]);
		return {
			id: account.id,
			label: account.label,
			mode: modeOf(adapter),
			icon: iconForKind(adapter?.kind ?? 'exchange'),
			adapterName: adapterInstalled
				? (adapter?.name ?? account.adapter_id).toUpperCase()
				: 'ADAPTER NOT INSTALLED',
			adapterInstalled,
			currency: balances?.currency ?? '',
			equity: formatScaledForDisplay(balances?.equity),
			pnl: formatSigned(pnl),
			pnlTone: toneOf(pnl),
			selected: account.id === activeId,
			readOnly: access[account.id]?.level === 'read_only',
			health: resolved.state,
			healthReason: resolved.reason,
			portfolioError: portfolios[account.id]?.error ?? null
		};
	});
}

/** Maps a resolved account health to the dot state and reason a card shows.
 * `'unknown'` — no dot claiming a fact nobody has checked — covers both an
 * account whose adapter is no longer installed (there is nothing to ask)
 * and one whose health simply has not answered yet, which are different
 * situations `adapterInstalled` on the card itself already distinguishes. */
function healthOf(
	adapterInstalled: boolean,
	health: AdapterHealthDto | undefined
): { state: HealthState; reason: string | null } {
	if (!adapterInstalled || !health) return { state: 'unknown', reason: null };
	return { state: health.state, reason: health.state === 'connected' ? null : (health.reason ?? null) };
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
				currency: portfolios[account.id]?.balances?.currency ?? '',
				selected: account.id === activeId,
				dotTone: account.enabled ? 'gain' : 'loss'
			}))
		};
	});
}

const L = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'left' });
const R = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'right' });

/** A short clock-only time, for a nanosecond timestamp, rendered in `zoneId`
 * — the reader's chosen display zone (`userZoneStore.zone`), never the
 * browser's own, the same `formatInstant(...).text.slice(11)` idiom
 * `top-bar.svelte`'s own clock uses. */
export function formatTime(nanos: number, zoneId: string): string {
	return formatInstant(nanos, zoneId).text.slice(11);
}

/** Builds the POSITIONS / ORDERS / EXECUTIONS table.
 *
 * `scope` is `'all'` when several accounts are in view, which adds an
 * ACCOUNT column and changes the empty-state copy — "nothing across your
 * accounts" and "nothing on this account" are different statements. */
export function engineTable(
	tab: EngineTableTab,
	scope: 'all' | 'account',
	positions: (PositionDto & { accountId: string; accountLabel: string; tradable: boolean })[],
	orders: (OrderDto & { accountId: string; accountLabel: string; tradable: boolean })[],
	fills: (FillDto & { accountId: string; accountLabel: string })[],
	/** The reader's chosen display zone, for every `TIME` cell — see
	 * `formatTime`. No default: a table rendered with nobody's zone is
	 * exactly the ambiguity `$lib/time` exists to rule out. */
	zoneId: string,
	/** `true` when at least one account in scope has no portfolio fetched
	 * yet — see `EngineTable.loading`. Defaults to `false` for callers (like
	 * a fixture-driven test) that have no loading state to report. */
	loading = false
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
				key: `${order.accountId}:${order.id}`,
				cells: [
					L(formatTime(order.submitted_at, zoneId), 'dim2'),
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
				],
				actions:
					order.tradable && isWorking(order)
						? [
								{ key: 'cancel', label: 'CANCEL', tone: 'destructive' },
								{ key: 'amend', label: 'AMEND', tone: 'default' }
							]
						: undefined
			})),
			emptyLabel: accColumn
				? 'NO ORDERS ACROSS ACCOUNTS — PLACE ONE FROM THE CHART'
				: 'NO ORDERS ON THIS ACCOUNT — PLACE ONE FROM THE CHART',
			loading
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
				key: `${fill.accountId}:${fill.id}`,
				cells: [
					L(formatTime(fill.executed_at, zoneId), 'dim2'),
					L(fill.instrument, 'fg'),
					...accCell(fill.accountLabel),
					L(fill.side.toUpperCase(), fill.side === 'buy' ? 'gain' : 'loss'),
					R(formatScaledForDisplay(fill.quantity, 8)),
					R(formatScaledForDisplay(fill.price, 8)),
					R(`${formatScaledForDisplay(fill.fee, 4)} ${fill.fee_currency}`, 'dim2')
				]
			})),
			emptyLabel: accColumn ? 'NO EXECUTIONS ACROSS ACCOUNTS' : 'NO EXECUTIONS ON THIS ACCOUNT',
			loading
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
			key: `${position.accountId}:${position.instrument}`,
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
			],
			actions: position.tradable
				? [{ key: 'close', label: 'CLOSE', tone: 'destructive' }]
				: undefined
		})),
		emptyLabel: accColumn ? 'NO OPEN POSITIONS ACROSS ACCOUNTS' : 'NO OPEN POSITIONS ON THIS ACCOUNT',
		loading
	};
}

// ----------------------------------------------------------------------
// The home page's account widgets. Three of the dashboard's widgets are
// about the trade engine; these build their view models the same way the
// engine page builds its own — real DTOs in, no fetching — so the two
// screens can never quietly disagree about what an account's numbers are.
// ----------------------------------------------------------------------

/** One account's row in the `equity` widget. */
export interface AccountEquityRow {
	id: string;
	label: string;
	/** `''` while the account's balances have not loaded yet. */
	currency: string;
	equity: string;
	balance: string;
	pnl: string;
	pnlTone: Tone;
	/** `false` for a spot venue that genuinely reports no margin (`margin_used:
	 * null`) — never for an account whose balances just have not loaded, so a
	 * widget can tell "no margin" from "no data" apart. */
	hasMargin: boolean;
	marginUsed: string | null;
	marginAvailable: string | null;
}

/** One currency's subtotal, shown under the headline when the accounts on
 * screen do not all share one currency. */
export interface DashboardCurrencySubtotal {
	currency: string;
	equity: string;
	balance: string;
	pnl: string;
	pnlTone: Tone;
}

export interface DashboardEquityTotals {
	/** `false` when every account with loaded balances shares one currency
	 * (or there is at most one) — `equity`/`balance`/`pnl` below are then a
	 * real sum, with its unit attached. `true` when they do not: summing
	 * would silently add USD to USDT, so those three instead hold a
	 * `"N accounts · M currencies"` placeholder and `perCurrency` carries
	 * the real numbers. */
	mixedCurrency: boolean;
	equity: string;
	balance: string;
	pnl: string;
	pnlTone: Tone;
	/** Always populated with at least the entries `sumByCurrency` found —
	 * one entry exactly matching `equity`/`balance`/`pnl` above when
	 * `!mixedCurrency` and at least one currency loaded, so a caller does
	 * not need a second branch just to show a per-currency line. */
	perCurrency: DashboardCurrencySubtotal[];
}

export interface DashboardEquityData {
	rows: AccountEquityRow[];
	/** `null` when there is nothing to total — no accounts at all. */
	totals: DashboardEquityTotals | null;
}

/** The `equity` widget's view model: equity, balance, open PnL and margin,
 * per account and totalled, from whatever each account's adapter most
 * recently answered. No curve — Senken reads balances on request and keeps
 * no history to draw one from, so there is nothing honest to plot yet.
 *
 * The totals are built with `sumByCurrency`, never a blind `sumScaled`
 * across every account: there is no FX rate anywhere in this system, so an
 * account priced in USD and one priced in USDT are never added together —
 * see `DashboardEquityTotals.mixedCurrency`. */
export function dashboardEquity(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): DashboardEquityData {
	const rows: AccountEquityRow[] = accounts.map((account) => {
		const balances = portfolios[account.id]?.balances ?? null;
		const pnl = balances?.unrealized_pnl ?? null;
		return {
			id: account.id,
			label: account.label,
			currency: balances?.currency ?? '',
			equity: formatScaledForDisplay(balances?.equity),
			balance: formatScaledForDisplay(balances?.balance),
			pnl: formatSigned(pnl),
			pnlTone: toneOf(pnl),
			hasMargin: balances?.margin_used != null,
			marginUsed: balances?.margin_used ? formatScaledForDisplay(balances.margin_used) : null,
			marginAvailable: balances?.margin_available
				? formatScaledForDisplay(balances.margin_available)
				: null
		};
	});
	if (accounts.length === 0) return { rows, totals: null };
	const grouped = sumByCurrency(accounts, portfolios);
	const perCurrency: DashboardCurrencySubtotal[] = grouped.map((total) => ({
		currency: total.currency,
		equity: formatScaledForDisplay(total.equity),
		balance: formatScaledForDisplay(total.balance),
		pnl: formatSigned(total.pnl),
		pnlTone: toneOf(total.pnl)
	}));
	if (grouped.length <= 1) {
		const only = perCurrency[0];
		return {
			rows,
			totals: {
				mixedCurrency: false,
				equity: only ? `${only.equity} ${only.currency}` : '—',
				balance: only ? `${only.balance} ${only.currency}` : '—',
				pnl: only ? only.pnl : '—',
				pnlTone: only ? only.pnlTone : 'gain',
				perCurrency
			}
		};
	}
	return {
		rows,
		totals: {
			mixedCurrency: true,
			equity: `${accounts.length} accounts · ${grouped.length} currencies`,
			balance: `${accounts.length} accounts · ${grouped.length} currencies`,
			pnl: `${grouped.length} currencies`,
			pnlTone: 'gain',
			perCurrency
		}
	};
}

/** The `positions` widget: every owned account's open positions, scoped
 * `'all'` so the empty state reads "across accounts" and each row carries
 * its account label — built with `engineTable` rather than a second
 * formatter. Every row's `tradable` is `false`: this widget is a read-only
 * summary with no `onaction` wired to it, and `engineTable` only offers CLOSE
 * on a row marked tradable — leaving that on here would render a button that
 * looks live and does nothing when clicked. */
export function dashboardPositions(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>,
	/** Threaded through to `engineTable` for parity with its other callers,
	 * even though the `positions` tab this widget always renders has no
	 * `TIME` column of its own to use it on. */
	zoneId: string
): EngineTable {
	const positions = accounts.flatMap((account) =>
		(portfolios[account.id]?.positions ?? []).map((row) => ({
			...row,
			accountId: account.id,
			accountLabel: account.label,
			tradable: false
		}))
	);
	return engineTable(
		'positions',
		'all',
		positions,
		[],
		[],
		zoneId,
		anyPortfolioLoading(accounts, portfolios)
	);
}

/** Whether at least one of `accounts` has no portfolio fetched at all
 * yet — `tradeStore.portfolios` has no entry for it, as opposed to an entry
 * whose arrays are genuinely empty. The distinction an empty table alone
 * cannot make. */
export function anyPortfolioLoading(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): boolean {
	return accounts.some((account) => portfolios[account.id] === undefined);
}

/** One account's margin gauge in the `risk` widget. */
export interface RiskMarginRow {
	accountId: string;
	accountLabel: string;
	/** `false` when the adapter reports no margin at all (a spot venue) —
	 * the widget shows a sentence instead of a bar sitting at 0%, which would
	 * otherwise read as "no risk" rather than "not applicable". */
	hasMargin: boolean;
	/** 0-100, meaningful only when `hasMargin`. */
	usedPct: number;
	/** `"12,000.00 / 50,000.00 USD"`, `''` when `!hasMargin`. */
	label: string;
}

/** One instrument's share of total equity in the `risk` widget. */
export interface RiskExposureRow {
	instrument: string;
	notional: string;
	/** 0-100, of total equity — or `null` when the accounts in view hold
	 * more than one currency.
	 *
	 * A position's notional is in its instrument's quote currency, which
	 * need not be the account's currency either, so once more than one
	 * currency is in play there is no single denominator this share could
	 * honestly be of. The amount is still shown; the bar is not, because a
	 * bar that filled anyway would be claiming a proportion nothing
	 * supports. */
	pct: number | null;
}

export interface DashboardRiskData {
	margin: RiskMarginRow[];
	exposure: RiskExposureRow[];
}

/** The `risk` widget's view model: margin used against margin available per
 * account, and each held instrument's notional as a share of total equity.
 * Both are real inputs already on the wire — nothing here is invented. */
export function dashboardRisk(
	accounts: TradeAccountDto[],
	portfolios: Record<string, Portfolio>
): DashboardRiskData {
	const margin: RiskMarginRow[] = accounts
		.filter((account) => portfolios[account.id]?.balances)
		.map((account) => {
			// The filter above guarantees `balances` is loaded for every
			// account reaching this point.
			const balances = portfolios[account.id]!.balances!;
			const used = balances.margin_used ?? null;
			const hasMargin = used !== null;
			const total = addScaled(used, balances.margin_available ?? null);
			return {
				accountId: account.id,
				accountLabel: account.label,
				hasMargin,
				usedPct: hasMargin ? percentOf(used, total) : 0,
				label: hasMargin
					? `${formatScaledForDisplay(used)} / ${formatScaledForDisplay(total)} ${balances.currency}`
					: ''
			};
		});

	const currencies = new Set(
		accounts
			.map((account) => portfolios[account.id]?.balances?.currency)
			.filter((currency): currency is string => currency !== undefined)
	);
	// One currency in view, one honest denominator. See `RiskExposureRow.pct`.
	const totalEquity =
		currencies.size === 1
			? sumScaled(accounts.map((a) => portfolios[a.id]?.balances?.equity))
			: null;
	const notionalByInstrument = new Map<string, ScaledDto>();
	for (const account of accounts) {
		for (const position of portfolios[account.id]?.positions ?? []) {
			// No mark price, no notional — an instrument an adapter cannot
			// mark is left out of the exposure bars rather than guessed at.
			if (!position.mark_price) continue;
			const notional = mulScaled(position.quantity, position.mark_price);
			const running = notionalByInstrument.get(position.instrument);
			notionalByInstrument.set(
				position.instrument,
				running ? (addScaled(running, notional) as ScaledDto) : notional
			);
		}
	}
	const exposure: RiskExposureRow[] = Array.from(notionalByInstrument.entries())
		.map(([instrument, notional]) => ({
			instrument,
			notional: formatScaledForDisplay(notional),
			pct: totalEquity ? percentOf(notional, totalEquity) : null
		}))
		.sort((a, b) => (b.pct ?? 0) - (a.pct ?? 0));

	return { margin, exposure };
}
