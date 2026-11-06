import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vitest/config';

// `EMBERLOOP_DEV_API=http://127.0.0.1:8790 npm run dev` proxies the daemon, so the dev server needs no CORS.
const devApi = process.env.EMBERLOOP_DEV_API;

export default defineConfig({
  plugins: [tailwindcss(), sveltekit(), svelteTesting()],
  server: devApi ? { proxy: { '/api': devApi, '/health': devApi } } : {},
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts']
  }
});
