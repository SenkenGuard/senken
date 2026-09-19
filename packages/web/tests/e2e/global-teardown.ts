// Kills the server `global-setup.ts` spawned and removes its temporary
// data directory. `pgrep -fl senken` should report empty only after this
// has actually run.
import { readFile, rm, unlink } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

const SERVER_STATE_FILE = join(import.meta.dirname, '.server.json');

function isAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

async function waitForExit(pid: number, deadline: number): Promise<void> {
	while (isAlive(pid)) {
		if (Date.now() > deadline) throw new Error(`senken serve (pid ${pid}) did not exit within the deadline`);
		await new Promise((resolve) => setTimeout(resolve, 100));
	}
}

export default async function globalTeardown(): Promise<void> {
	if (!existsSync(SERVER_STATE_FILE)) return;
	const { pid, dataDir } = JSON.parse(await readFile(SERVER_STATE_FILE, 'utf8')) as { pid: number; dataDir: string };

	if (isAlive(pid)) {
		process.kill(pid, 'SIGTERM');
		await waitForExit(pid, Date.now() + 10_000);
	}

	await rm(dataDir, { recursive: true, force: true });
	await unlink(SERVER_STATE_FILE).catch(() => {});
}
