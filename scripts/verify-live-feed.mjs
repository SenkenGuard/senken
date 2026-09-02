// Watches live price ticks arrive from several venues at once, against a
// real server and real venue sockets — the property a unit test cannot
// show, because every protocol in this repository decodes a *recorded*
// frame there and a recording proves nothing about whether the venue is
// still sending that shape today.
//
// Not part of `bun test`: it needs a built binary and live venue traffic.
// Run it by hand, and do not leave it running.
//
//   cargo build --bin senken
//   ./target/debug/senken serve --port 4197 --data-dir /tmp/senken-feed-check &
//   node scripts/verify-live-feed.mjs
//
// Prints one row per source: how many sources declare a live feed, and
// which of them actually produced a tick inside the watch window. A source
// with a feed that stays silent is not necessarily broken — a quiet venue
// looks the same — so read the rows, not a pass/fail.

const BASE = process.env.SENKEN_URL ?? 'http://127.0.0.1:4197';
const EMAIL = process.env.SENKEN_EMAIL ?? 'admin@mail.com';
const PASSWORD = process.env.SENKEN_PASSWORD ?? 'a-very-long-password';
/** Long enough for a quiet venue to print something, short enough to be
 * polite to every venue at once. */
const WATCH_MS = Number(process.env.SENKEN_WATCH_MS ?? 25000);

const json = async (res) => {
	const text = await res.text();
	try {
		return JSON.parse(text);
	} catch {
		throw new Error(`${res.status} ${text.slice(0, 200)}`);
	}
};

async function token() {
	// A fresh data directory has an admin account with no password yet,
	// which `set-password` claims; an existing one just logs in.
	const claim = await fetch(`${BASE}/api/set-password`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify({ email: EMAIL, new_password: PASSWORD })
	});
	if (!claim.ok) await claim.text();
	const login = await fetch(`${BASE}/api/login`, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify({ email: EMAIL, password: PASSWORD })
	});
	return (await json(login)).token;
}

const auth = await token();
const headers = { authorization: `Bearer ${auth}` };

const sources = (await json(await fetch(`${BASE}/api/sources`, { headers }))).sources ?? [];
const live = sources.filter((s) => s.live);
console.log(
	`sources: ${sources.length}  bars: ${sources.filter((s) => s.bars).length}` +
		`  book: ${sources.filter((s) => s.book?.supported).length}  live: ${live.length}`
);

// One instrument per live source, picked from the venue's own catalog
// rather than guessed: each venue writes its main BTC market differently
// (XBTUSD on Kraken, KRWBTC on Upbit, BTCPERPETUAL on Deribit).
const PREFERRED = [
	'BTCUSDT',
	'BTCUSD',
	'XBTUSD',
	'BTCUSDC',
	'KRWBTC',
	'BTCPERPETUAL',
	'BTCUSDTM',
	'BTCUSDTPERP'
];
// The search grammar takes `source:term`, so each source is asked about
// its own catalog and only exact symbol matches count — a fuzzy hit like
// `ABTC` is a different market, not this venue's BTC one.
const topics = [];
for (const source of live) {
	const found = [];
	for (const candidate of PREFERRED) {
		const page = await json(
			await fetch(
				`${BASE}/api/instruments?q=${encodeURIComponent(`${source.id}:${candidate}`)}&limit=50`,
				{ headers }
			)
		);
		const hit = (page.rows ?? []).find(
			(row) => row.source_id === source.id && row.symbol === candidate
		);
		// Every BTC market a source lists, not just the first: a venue can
		// carry BTC/USDT and BTC/USD and have one of them near-idle, and a
		// silent idle pair says nothing about the protocol.
		if (hit) found.push(hit.symbol);
	}
	for (const symbol of found) topics.push({ source: source.id, topic: `${source.id}:${symbol}` });
	if (!found.length) console.log(`${source.id}: no BTC market in the catalog to watch`);
}

const ticket = (
	await json(await fetch(`${BASE}/api/ws/ticket`, { method: 'POST', headers }))
).ticket;
const ws = new WebSocket(`${BASE.replace('http', 'ws')}/api/ws?ticket=${ticket}`);
const ticks = new Map();
const unavailable = new Set();

ws.onopen = () => {
	for (const { topic } of topics) {
		ws.send(JSON.stringify({ type: 'subscribe', topic }));
	}
};
ws.onmessage = (event) => {
	const frame = JSON.parse(event.data);
	if (frame.type === 'price') {
		const seen = ticks.get(frame.topic) ?? { count: 0 };
		seen.count += 1;
		seen.price = frame.price / 10 ** frame.price_scale;
		ticks.set(frame.topic, seen);
	}
	if (frame.type === 'unavailable' || frame.type === 'no_feed') unavailable.add(frame.topic);
};

await new Promise((resolve) => setTimeout(resolve, WATCH_MS));
ws.close();

console.log(`\nwatched ${topics.length} topics for ${WATCH_MS / 1000}s\n`);
for (const { source, topic } of topics.sort((a, b) => a.source.localeCompare(b.source))) {
	const seen = ticks.get(topic);
	const status = seen ? `${String(seen.count).padStart(4)} ticks  last ${seen.price}` : '   — silent';
	console.log(`${source.padEnd(18)} ${topic.padEnd(34)} ${status}`);
}
console.log(`\nsources that produced a tick: ${ticks.size} of ${topics.length}`);
if (unavailable.size) console.log(`reported unavailable: ${[...unavailable].join(', ')}`);
