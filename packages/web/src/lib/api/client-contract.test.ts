// Closes a real class of bug found by hand: `DELETE /api/registry/indicators/{name}` was fully typed all the
// way through `generated.ts`, but nothing in the frontend ever called
// it — no UI control could ever have reached it. A pure Bun test (no DOM
// needed) reads `generated.ts` and the frontend source tree as text and
// checks that every operation the server actually exposes has at least one
// literal `/api/...` reference somewhere the app could run it from, so a
// future gap like that one fails a test instead of shipping silently.
//
// The property is "does *some* client-side code reference this operation",
// not "does `client.ts` alone" — `apiClient.request(...)` is the one
// funnel (`client.ts`'s own header comment), but a few modules call it
// directly with their own literal path instead of going through a
// `client.ts` method: `dashboard/api.ts` owns the dashboard-workspace and
// widget-plugin endpoints, `websocket.ts` owns the ticket exchange. Reading
// only `client.ts` would report both as gaps they are not.
import { describe, expect, test } from 'bun:test';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const GENERATED_PATH = join(HERE, 'generated.ts');
const GENERATED_SRC = readFileSync(GENERATED_PATH, 'utf8');

const HTTP_METHODS = ['get', 'post', 'put', 'patch', 'delete'] as const;
type HttpMethod = (typeof HTTP_METHODS)[number];

interface Operation {
	method: HttpMethod;
	path: string;
}

/** Any `{...}` path parameter, named or from a template interpolation,
 * collapses to the same placeholder — `generated.ts`'s `{alert_id}` and a
 * caller's `${encodeURIComponent(id)}` describe the same route. */
function normalizePlaceholders(path: string): string {
	return path.replace(/\{[^}]*\}/g, '{}');
}

/** Walks the `paths` interface `generated.ts` (openapi-typescript's own
 * output shape) line by line, tracking which `"/api/..."` block is current
 * and collecting every direct `get|post|put|patch|delete: operations[...]`
 * under it — never the `get?: never;` stubs the same block lists for every
 * method a path does not support. */
function extractGeneratedOperations(src: string): Operation[] {
	const start = src.indexOf('export interface paths {');
	const end = src.indexOf('export interface components {');
	const body = start >= 0 && end > start ? src.slice(start, end) : src;

	const pathHeader = /^\s*"(\/api\/[^"]+)":\s*\{/;
	const methodLine = /^\s*(get|post|put|patch|delete):\s*operations\[/;

	const operations: Operation[] = [];
	let currentPath: string | null = null;
	for (const line of body.split('\n')) {
		const pathMatch = line.match(pathHeader);
		if (pathMatch) {
			currentPath = pathMatch[1];
			continue;
		}
		if (!currentPath) continue;
		const methodMatch = line.match(methodLine);
		if (methodMatch) {
			operations.push({ method: methodMatch[1] as HttpMethod, path: normalizePlaceholders(currentPath) });
		}
	}
	return operations;
}

/** Every `.ts`/`.svelte` file under the app's own source (not
 * `node_modules`, not `.svelte-kit`, not this file's own test siblings) —
 * the surface a UI control could plausibly reach an endpoint from. */
function appSourceFiles(): string[] {
	const roots = [join(HERE, '..'), join(HERE, '..', '..', 'routes')];
	const files: string[] = [];
	for (const root of roots) {
		for (const entry of readdirSync(root, { recursive: true }) as string[]) {
			if (!/\.(ts|svelte)$/.test(entry)) continue;
			if (entry.includes('node_modules') || entry.includes('.svelte-kit')) continue;
			if (/\.(test|browser-test)\.ts$/.test(entry)) continue;
			if (entry.endsWith('generated.ts')) continue;
			const full = join(root, entry);
			if (statSync(full).isFile()) files.push(full);
		}
	}
	return files;
}

/** Every `/api/...` literal referenced anywhere in the app's own source, as
 * a plain string or a template — normalized the same way as `generated.ts`'s
 * paths (path parameters collapsed to `{}`) and with any query string
 * dropped, since a generated path never carries one. This does not attempt
 * to pair each literal back to a specific HTTP method; see this file's own
 * header comment on the property this proves. */
function extractAppPaths(files: string[]): Set<string> {
	const literal = /`(\/api\/[^`]*)`|'(\/api\/[^']*)'/g;
	const paths = new Set<string>();
	for (const file of files) {
		const src = readFileSync(file, 'utf8');
		let match: RegExpExecArray | null;
		while ((match = literal.exec(src))) {
			let path = match[1] ?? match[2];
			path = path.split('?')[0];
			path = path.replace(/\$\{[^}]*\}/g, '{}');
			path = normalizePlaceholders(path);
			paths.add(path);
		}
	}
	return paths;
}

/** Endpoints nothing in the frontend calls yet. Each needs its own reason,
 * named here rather than left as a silent gap (`AGENTS.md`'s "Zero lint
 * suppressions" spirit applied to test exceptions: an unexplained one is
 * exactly as unwelcome as an unexplained `#[allow]`). Found by first
 * running this test scoped to `client.ts` alone, which over-reported nine
 * gaps that turned out to be `dashboard/api.ts` and `websocket.ts` calling
 * the funnel directly — those are not listed here because they are not
 * gaps. What is left after that correction is real. */
const KNOWN_GAPS = new Set<string>([
	// Minute-bar backfill for replay/simulation is a real capability with
	// no UI trigger yet — the charts page's own replay feature
	// (`toggleReplay`) works from bars already loaded and never calls this.
	'post /api/bars/m1-download',
	// Per-plugin permission grants on a role or a user: typed and routed,
	// but the admin screen that would grant/revoke one by name does not
	// exist yet.
	'post /api/roles/{}/plugin-grants',
	'post /api/roles/{}/plugin-grants/revoke',
	'post /api/users/{}/plugin-grants',
	'post /api/users/{}/plugin-grants/revoke',
	// Superseded by the unified `POST /api/plugins` (installs a package
	// from a zip archive or wraps a bare `.wasm` upload automatically),
	// `POST /api/plugins/{id}/enabled`, `DELETE /api/plugins/{id}` and
	// `POST /api/plugins/refresh` — the Plugins page drives those instead
	// now. The routes below stay mounted so nothing that already automated
	// against them breaks, but nothing in this app calls them any more;
	// removing them from the server is a later, separate change. Their own
	// `GET` siblings (`/api/indicators/plugins`, `/api/widget-plugins`) are
	// not listed here — this page still reads both, purely for the
	// runtime health, log, description and manifest-digest detail the
	// unified `GET /api/plugins/{id}` does not carry yet.
	'post /api/indicators/plugins/{}/enabled',
	'post /api/widget-plugins/{}/enabled',
	'delete /api/widget-plugins/{}',
	'post /api/widget-plugins/refresh'
]);

const generatedOperations = extractGeneratedOperations(GENERATED_SRC);
const appPaths = extractAppPaths(appSourceFiles());

describe('every operation generated.ts describes is referenced somewhere in the app', () => {
	test('generated.ts actually yielded operations to check', () => {
		// A sanity floor, not a magic number: if this collapses to a
		// handful, the extraction above broke silently and every test below
		// would pass for the wrong reason (nothing left to check).
		expect(generatedOperations.length).toBeGreaterThan(100);
	});

	for (const operation of generatedOperations) {
		const key = `${operation.method} ${operation.path}`;
		test(key, () => {
			if (KNOWN_GAPS.has(key)) return;
			expect(appPaths.has(operation.path)).toBe(true);
		});
	}

	test('every declared exception is still actually missing from the app', () => {
		// An exception nobody needs any more is a gap that quietly stopped
		// being tracked, not a gap that closed — the app gaining a real
		// reference should make this fail so the entry gets deleted, not
		// linger forever "just in case".
		for (const key of KNOWN_GAPS) {
			const path = key.split(' ').slice(1).join(' ');
			expect(appPaths.has(path)).toBe(false);
		}
	});
});
