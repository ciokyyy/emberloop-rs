import adapter from "@sveltejs/adapter-static";
import { sveltekit } from "@sveltejs/kit/vite";
import { svelteTesting } from "@testing-library/svelte/vite";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

// `EMBERLOOP_DEV_API=http://127.0.0.1:8790 npm run dev` proxies the daemon, so the dev server needs no CORS.
const devApi = (
  globalThis as { process?: { env: Record<string, string | undefined> } }
).process?.env.EMBERLOOP_DEV_API;

export default defineConfig({
  plugins: [
    tailwindcss(),
    sveltekit({
      alias: { $lib: "src/lib" },
      adapter: adapter({ fallback: "index.html" }),
      preprocess: vitePreprocess(),
    }),
    svelteTesting(),
  ],
  server: devApi ? { proxy: { "/api": { target: devApi, ws: true }, "/figcraft": devApi, "/health": devApi } } : {},
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts"],
    setupFiles: ["src/test-setup.ts"],
    // Threads instead of the default forked processes: the suite is Svelte compile, and a fork per
    // worker file re-pays it (138 s forked, 106 s threaded on the development host).
    pool: "threads",
  },
});
