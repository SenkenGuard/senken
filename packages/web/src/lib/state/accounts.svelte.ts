// Reactive accounts list for the trade engine page.
//
// `mock/engine.ts`'s `ACCOUNTS` (a plain `const` fixture, added in P4) has
// no way to grow at runtime, but the reference's "ATTACH ACCOUNT" /
// "CONNECT ADAPTER" flow (`attachAccount`, lines 1595-1609) does exactly
// that: it pushes a new account onto the list and switches to it. Wiring
// that trigger for real needs a
// mutable, reactive list, so this module lifts `ACCOUNTS` into `$state` and
// re-derives the id sequence from it, with `mock/engine.ts`'s own
// functions taking the list as an optional parameter (defaulting to the
// static fixture, so nothing outside the engine page had to change).
import { ACCOUNTS as SEED_ACCOUNTS, DEFAULT_ACCOUNT_ID, adapterOf, type Account } from '$lib/mock/engine';

class AccountsStore {
	list = $state<Account[]>(SEED_ACCOUNTS.map((a) => ({ ...a })));
	activeId = $state<string>(DEFAULT_ACCOUNT_ID);
}

export const accountsStore = new AccountsStore();

let seq = 100;

/** reference: `attachAccount`, lines 1595-1609 — attaches a new demo/live
 * account for `adapterKey` and makes it the active one. Returns the new
 * account's id. */
export function attachAccount(adapterKey: string): string {
	const a = adapterOf(adapterKey);
	const n = accountsStore.list.filter((x) => x.adapter === adapterKey).length + 1;
	const id = `${adapterKey}-${seq++}`;
	const live = a.key !== 'sim';
	const account: Account = {
		id,
		adapter: adapterKey,
		label:
			a.name.split(' ')[0].toUpperCase() +
			' · ' +
			(live ? String(8800000 + Math.floor(Math.random() * 99999)) : `SANDBOX ${n}`),
		mode: live ? 'LIVE' : 'DEMO',
		ccy: adapterKey === 'binance' || adapterKey === 'bybit' ? 'USDT' : 'USD',
		balance: 25000,
		equity: 25000,
		leverage: live ? '1:100' : '1:10',
		margin: 0
	};
	accountsStore.list = [...accountsStore.list, account];
	accountsStore.activeId = id;
	return id;
}
