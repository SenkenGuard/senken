// Boots one real `senken serve` against a temporary data directory for the
// whole e2e run — never a mock. `senken-*` fixtures elsewhere in this repo
// are recorded venue responses; this is the one place the client meets the
// actual Rust server it ships with.
import { spawn, type ChildProcess } from 'node:child_process';
import { openSync } from 'node:fs';
import { mkdtemp, writeFile, mkdir, readFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { crc32 } from 'node:zlib';

const REPO_ROOT = join(import.meta.dirname, '..', '..', '..', '..');
const SENKEN_BIN = join(REPO_ROOT, 'target', 'debug', 'senken');
const PORT = 4390;
const BASE_URL = `http://127.0.0.1:${PORT}`;
const SERVER_STATE_FILE = join(import.meta.dirname, '.server.json');
const AUTH_STATE_FILE = join(import.meta.dirname, '.auth.json');
const ARTIFACTS_STATE_FILE = join(import.meta.dirname, '.artifacts.json');
const ADMIN_EMAIL = 'admin@mail.com';
const ADMIN_PASSWORD = 'e2e-pass-123456';

async function waitForHealth(deadline: number): Promise<void> {
	for (;;) {
		try {
			const response = await fetch(`${BASE_URL}/api/health`);
			if (response.ok) return;
		} catch {
			// Not listening yet — retry until the deadline.
		}
		if (Date.now() > deadline) throw new Error(`senken serve did not answer /api/health within the deadline`);
		await new Promise((resolve) => setTimeout(resolve, 200));
	}
}

/** A minimal "stored" (uncompressed) ZIP writer for exactly the two small
 * files a plugin package needs. Reaching for a dependency for this would
 * add a devDependency this project's own pinned dependency list does not
 * name, for a format simple enough to write directly: no compression, so no need for
 * `zlib.deflateRawSync` either — `node:zlib`'s built-in `crc32` is the only
 * primitive this needs. */
function buildStoredZip(entries: { name: string; data: Buffer }[]): Buffer {
	const localParts: Buffer[] = [];
	const centralParts: Buffer[] = [];
	let offset = 0;

	for (const { name, data } of entries) {
		const nameBuf = Buffer.from(name, 'utf8');
		const crc = crc32(data) >>> 0;

		const localHeader = Buffer.alloc(30);
		localHeader.writeUInt32LE(0x04034b50, 0);
		localHeader.writeUInt16LE(20, 4); // version needed
		localHeader.writeUInt16LE(0, 6); // flags
		localHeader.writeUInt16LE(0, 8); // method: stored
		localHeader.writeUInt16LE(0, 10); // mod time
		localHeader.writeUInt16LE(0x21, 12); // mod date
		localHeader.writeUInt32LE(crc, 14);
		localHeader.writeUInt32LE(data.length, 18); // compressed size
		localHeader.writeUInt32LE(data.length, 22); // uncompressed size
		localHeader.writeUInt16LE(nameBuf.length, 26);
		localHeader.writeUInt16LE(0, 28); // extra field length
		localParts.push(localHeader, nameBuf, data);

		const centralHeader = Buffer.alloc(46);
		centralHeader.writeUInt32LE(0x02014b50, 0);
		centralHeader.writeUInt16LE(20, 4); // version made by
		centralHeader.writeUInt16LE(20, 6); // version needed
		centralHeader.writeUInt16LE(0, 8); // flags
		centralHeader.writeUInt16LE(0, 10); // method: stored
		centralHeader.writeUInt16LE(0, 12); // mod time
		centralHeader.writeUInt16LE(0x21, 14); // mod date
		centralHeader.writeUInt32LE(crc, 16);
		centralHeader.writeUInt32LE(data.length, 20);
		centralHeader.writeUInt32LE(data.length, 24);
		centralHeader.writeUInt16LE(nameBuf.length, 28);
		centralHeader.writeUInt16LE(0, 30); // extra field length
		centralHeader.writeUInt16LE(0, 32); // comment length
		centralHeader.writeUInt16LE(0, 34); // disk number start
		centralHeader.writeUInt16LE(0, 36); // internal attributes
		centralHeader.writeUInt32LE(0, 38); // external attributes
		centralHeader.writeUInt32LE(offset, 42); // relative offset of local header
		centralParts.push(centralHeader, nameBuf);

		offset += localHeader.length + nameBuf.length + data.length;
	}

	const centralDirectory = Buffer.concat(centralParts);
	const centralDirectoryOffset = offset;

	const end = Buffer.alloc(22);
	end.writeUInt32LE(0x06054b50, 0);
	end.writeUInt16LE(0, 4); // disk number
	end.writeUInt16LE(0, 6); // disk with central directory
	end.writeUInt16LE(entries.length, 8); // records on this disk
	end.writeUInt16LE(entries.length, 10); // total records
	end.writeUInt32LE(centralDirectory.length, 12);
	end.writeUInt32LE(centralDirectoryOffset, 16);
	end.writeUInt16LE(0, 20); // comment length

	return Buffer.concat([...localParts, centralDirectory, end]);
}

/** Zips a compiled `venue-example` fixture component into an installable
 * plugin package, the shape the Settings → Plugins install flow expects —
 * built only if `senken-plugin-host`'s own tests have already produced the
 * `.wasm` (this file never invokes `cargo` itself, to honor this machine's
 * one-`cargo`-at-a-time rule). */
async function packageVenueExampleArtifact(): Promise<string | null> {
	const wasmPath = join(REPO_ROOT, 'target', 'fixture-wasm', 'wasm32-wasip2', 'debug', 'fixture_venue_example.wasm');
	if (!existsSync(wasmPath)) return null;

	const artifactsDir = join(import.meta.dirname, 'artifacts');
	await mkdir(artifactsDir, { recursive: true });
	const zipPath = join(artifactsDir, 'venue-example.zip');

	const manifest = {
		schema_version: 1,
		id: 'venue-example',
		name: 'Venue Example',
		version: '0.0.0',
		contributes: [{ point: 'venue', venue: { entry: 'venue.wasm' } }]
	};

	const zipBytes = buildStoredZip([
		{ name: 'senken-plugin.json', data: Buffer.from(JSON.stringify(manifest, null, 2)) },
		{ name: 'venue.wasm', data: await readFile(wasmPath) }
	]);
	await writeFile(zipPath, zipBytes);
	return zipPath;
}

export default async function globalSetup(): Promise<void> {
	if (!existsSync(SENKEN_BIN)) {
		throw new Error(`build the server first: cargo build --bin senken (expected ${SENKEN_BIN})`);
	}

	const dataDir = await mkdtemp(join(tmpdir(), 'senken-e2e-'));

	// Keep the server's own log. Discarding it costs a whole round trip
	// whenever a scenario fails for a reason the server already explained —
	// a warning about a component that would not load, say, which no
	// assertion can see.
	const artifacts = join(import.meta.dirname, 'artifacts');
	await mkdir(artifacts, { recursive: true });
	const serverLog = openSync(join(artifacts, 'server.log'), 'w');
	const child: ChildProcess = spawn(SENKEN_BIN, ['serve', '--data-dir', dataDir, '--port', String(PORT)], {
		stdio: ['ignore', serverLog, serverLog],
		detached: false,
		env: { ...process.env, RUST_LOG: process.env.RUST_LOG ?? 'senken_runtime=debug,senken_api=info' }
	});
	if (child.pid === undefined) throw new Error('senken serve did not start (no pid)');

	await writeFile(SERVER_STATE_FILE, JSON.stringify({ pid: child.pid, dataDir }));

	await waitForHealth(Date.now() + 30_000);

	const setPasswordResponse = await fetch(`${BASE_URL}/api/set-password`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ email: ADMIN_EMAIL, new_password: ADMIN_PASSWORD })
	});
	if (setPasswordResponse.status !== 204) {
		throw new Error(`POST /api/set-password returned ${setPasswordResponse.status}, expected 204`);
	}

	const loginResponse = await fetch(`${BASE_URL}/api/login`, {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ email: ADMIN_EMAIL, password: ADMIN_PASSWORD })
	});
	if (!loginResponse.ok) throw new Error(`POST /api/login returned ${loginResponse.status}`);
	const { token } = (await loginResponse.json()) as { token: string };
	await writeFile(AUTH_STATE_FILE, JSON.stringify({ token }));

	const venueExampleZip = await packageVenueExampleArtifact();
	await writeFile(ARTIFACTS_STATE_FILE, JSON.stringify({ venueExampleZip }));
	if (venueExampleZip === null) {
		console.log(
			'[e2e global-setup] fixture_venue_example.wasm not found under target/fixture-wasm — ' +
				'plugin-install tests that need it will skip with a printed reason (run `cargo test -p senken-plugin-host` first if you want to cover them).'
		);
	}
}
