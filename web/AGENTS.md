# AGENTS.md — web

## Purpose

SvelteKit SPA for phase 1: Projects sidebar, Session feed, composer and Settings. Built with `adapter-static` (fallback `index.html`, SSR off), Tailwind CSS v4 and shadcn-svelte-style components over `bits-ui`. The daemon serves the static build at its own origin; `VITE_API_BASE` overrides the API origin in development.

## Layout

- `src/routes/+layout.svelte`: the app shell — Session selection, SSE stream, prompt/steer/cancel, Settings save. The URL owns what it shows, so a refresh, a pasted link and Back all land on the same screen: `/p/<project>/s/<session>`, `/p/<project>/settings`, `/p/<project>/providers`, and `/<view>` when there is no Project to hang them on. Every other route file holds a comment only; `src/lib/route.ts` owns the URL grammar.
- `src/lib/api/index.ts`: typed `fetch` client for the routes in `crates/api`, prompts included. `types.ts` mirrors the DTOs; `schema.d.ts` is generated from `openapi.json` by `npm run generate:api`.
- `src/lib/feed.ts`: pure `SessionEvent` to `FeedState` reducer, covered by `feed.test.ts`.
- `src/lib/toolfeed.ts`, `compactFeed.ts`, `highlight.ts`: typed ports of the Go app's tool-row model (hues, exit codes, error tails, Explored grouping, diff stats, tokenizers). `toolRuns.ts` folds consecutive tool and thinking items into runs; `contextMeter.ts` computes the context ring percent and tone.
- `src/lib/motion.ts`: Svelte transition helpers (`rise`, `slideUp`, `slideDown`, `fadeAt`, `collapse`, `flipAnimation`) ported from the Go app. They read the `--dur-*` and `--ease-smooth` tokens in `app.css` with their unit (the minifier ships `.18s`, not `180ms`) and go to zero duration under reduced motion. Transitions on an element inside an `{#if}` inside an `{#each}` need `|global`.
- `src/lib/components/`: Sidebar, Feed (a queued steer stays pinned at the bottom until delivery puts it back in the log), ToolFeed (compact tool rows, Explored groups, thinking blocks), ToolBody, ContextRing, WorkingIndicator (the Go app's activity line above the composer), Icon, Composer (context ring, thinking-level pill, attach, send/stop), Settings, Prompts, ModelPicker and NewSessionDialog; `ui/` holds the local shadcn-svelte-style Button and Switch wrappers.
- `src/app.css`: dark-only tokens shared with the mock and the Go app.

## Event data convention

`feed.ts` reads these `SessionEvent.data` keys. Changing the API event payload requires this file and `feed.ts` together.

- `status`: `state`
- `prompt`: `text`, `imagesCount`
- `assistant_chunk`, `thinking_chunk`: `text`
- `tool_started`: `id`, `name`, `input`, `display`; `tool_finished`: `id`, `name`, `output`, `status`, `display`
- `steer_queued`: `id`, `text`; `steer_delivered`: `id`, `route`
- `notice`: `id`, `text`, `route`
- `compression`: `text`
- `usage`: `inputTokens`, `contextWindow`
- `error`: `message`

## Workflow

- `npm run check`, `npm test`, `npm run build`. The root `scripts/check` runs the first two when `web/` exists.
- Tests are Vitest; jsdom is the environment. Reducer, image limits, tool hues, Markdown sanitising, composer routing and the model picker are covered.
- The static output is `web/build/`, which the future embedded-assets crate consumes.
