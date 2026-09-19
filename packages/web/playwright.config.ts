import { defineConfig } from '@playwright/test';

export default defineConfig({
	testDir: './tests/e2e',
	timeout: 30_000,
	fullyParallel: false,
	workers: 1,
	use: {
		baseURL: process.env.SENKEN_E2E_URL ?? 'http://127.0.0.1:4390',
		viewport: { width: 1440, height: 900 },
		trace: 'retain-on-failure'
	},
	globalSetup: './tests/e2e/global-setup.ts',
	globalTeardown: './tests/e2e/global-teardown.ts'
});
