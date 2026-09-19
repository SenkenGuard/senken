import { defineConfig, mergeConfig } from 'vitest/config';
import { playwright } from '@vitest/browser-playwright';
import viteConfig from './vite.config.ts';

export default mergeConfig(
	viteConfig,
	defineConfig({
		test: {
			// Only files written for the browser. `bun test` still owns every
			// plain *.test.ts.
			include: ['src/**/*.browser-test.ts'],
			setupFiles: ['./src/test-support/browser-setup.ts'],
			browser: {
				enabled: true,
				provider: playwright(),
				instances: [{ browser: 'chromium' }],
				headless: true
			}
		}
	})
);
