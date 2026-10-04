import { defineConfig } from 'vitest/config';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';

// During development, `/graphql` is proxied to the CodeAtlas server so the UI
// and API share an origin. Override the target with CODEATLAS_API_URL.
const apiTarget = process.env.CODEATLAS_API_URL ?? 'http://127.0.0.1:8080';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// Single-page app: every route falls back to the client-side router.
			adapter: adapter({ fallback: '200.html' })
		})
	],
	server: {
		proxy: {
			'/graphql': { target: apiTarget, changeOrigin: true },
			'/health': { target: apiTarget, changeOrigin: true }
		}
	},
	test: {
		expect: { requireAssertions: true },
		projects: [
			{
				extends: './vite.config.ts',
				test: {
					name: 'unit',
					environment: 'node',
					include: ['src/**/*.{test,spec}.{js,ts}'],
					exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
				}
			}
		]
	}
});
