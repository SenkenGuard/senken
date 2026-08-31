import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	server: {
		// In dev the UI is served by Vite (hot reload), while `/api` still
		// belongs to the Rust server — so proxy it across. Without this the
		// page would fetch `/api/health` from Vite and get its SPA fallback
		// HTML instead of JSON. Port 4190 is what the root `dev` script
		// starts `senken serve` on; change both together.
		proxy: {
			'/api': {
				target: 'http://127.0.0.1:4190',
				changeOrigin: true
			}
		}
	},
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) => filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// SPA mode: `index.html` is the fallback for every
			// path axum's SPA handler doesn't recognise as a built asset —
			// see crates/api/src/assets.rs.
			adapter: adapter({ fallback: 'index.html' })
		})
	]
});
