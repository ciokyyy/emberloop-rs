# AGENTS.md — web

## Purpose

SvelteKit SPA for phase 1: Projects sidebar, Session feed, composer and Settings. Built with `adapter-static` (fallback `index.html`, SSR off), Tailwind CSS v4 and shadcn-svelte-style components over `bits-ui`. The daemon serves the static build at its own origin; `VITE_API_BASE` overrides the API origin in development.

## Layout

- `src/routes/+layout.svelte`: the app shell — Session selection, SSE stream, prompt/steer/cancel, Settings save. The URL owns what it shows, so a refresh, a pasted link and Back all land on the same screen: `/p/<project>/s/<session>`, `/p/<project>/settings`, `/p/<project>/providers`, `/p/<project>/usage`, and `/<view>` when there is no Project to hang them on. Every other route file holds a comment only; `src/lib/route.ts` owns the URL grammar. SSE events are buffered and folded into one update per animation frame: a replay arrives as hundreds of frames in one task, and applying each one stalls the main thread on a long Session.
- `src/lib/api/index.ts`: typed `fetch` client for the routes in `crates/api`, prompts included. `types.ts` mirrors the DTOs; `schema.d.ts` is generated from `openapi.json` by `npm run generate:api`.
- `src/lib/components/Providers.svelte`: connected providers (Antigravity and ChatGPT sign-in flows, saved keys with Remove key) and a searchable Add provider list of the API-key providers (`ProviderView.auth === 'key'`), each with a password key input that saves through `PUT /api/providers/:id/key`. `Usage.svelte` lists connected providers; key providers show no usage numbers.
- `src/lib/feed.ts`: pure `SessionEvent` to `FeedState` reducer, plus `applySnapshot` for the server's open snapshot (status, usage and the todo list the tail window may not hold), covered by `feed.test.ts`. The `todos` event and the snapshot both carry `{items:[{id, content, status}]}` and touch no feed row. Paging older events never resets status, usage or todos. A `todo` tool row keeps only the first line of its result, so `set` and `read` never print the whole list in the feed.
- `src/lib/toolfeed.ts`, `compactFeed.ts`, `highlight.ts`: typed ports of the Go app's tool-row model (hues, exit codes, error tails, Explored grouping, diff stats, tokenizers). `toolRuns.ts` folds consecutive tool and thinking items into runs; `contextMeter.ts` computes the context ring percent and tone.
- `src/lib/motion.ts`: Svelte transition helpers (`rise`, `slideUp`, `slideDown`, `fadeAt`, `collapse`, `flipAnimation`) ported from the Go app. They read the `--dur-*` and `--ease-smooth` tokens in `app.css` with their unit (the minifier ships `.18s`, not `180ms`) and go to zero duration under reduced motion. Transitions on an element inside an `{#if}` inside an `{#each}` need `|global`.
- `src/lib/components/PlanCard.svelte`: the plan card above the composer — collapsed to `done/total`, the active item and a progress bar; click or `p` toggles; the expanded list caps at 8 rows with `+N more` and repeats the active item when it falls outside them; `[ ]`, `[~]`, `[x]`, `[–]` with resolved rows struck through; opens for 2 s on a create or replace when `todo_auto_expand` is on and fades after the last item resolves when `todo_hide_when_done` is on (fake timers in `PlanCard.test.ts`). A todo reminder and the cap note render as dashed notice rows.
- `src/lib/components/`: Sidebar (one Project's Sessions when the URL names one), Feed (a queued steer stays pinned at the bottom, inside the log's column, until delivery puts it back in the log; a virtual row whose block is gone is skipped, because a shrinking feed reaches the virtualizer one flush before its count does), ToolFeed (compact tool rows, Explored groups, thinking blocks), ToolBody, ContextRing, WorkingIndicator (the Go app's activity line, in the composer's column directly above the card), Icon, Composer (context ring, thinking-level pill, attach, send/stop; remounted per Session so a draft never leaks across), Settings (Proxy, Context: bands, floors, tiers, protected tools and the two inject switches, Todo), Prompts, ModelPicker and NewSessionDialog (`newProject` asks for a path only; the dialog opens for every new Session, capped to the viewport and scrolling inside it, with the URL's Project preselected by `newSessionProjects` in `route.ts`); `ui/` holds the local shadcn-svelte-style Button and Switch wrappers.
- `src/app.css`: dark-only tokens shared with the mock and the Go app.

## Event data convention

`feed.ts` reads these `SessionEvent.data` keys. Changing the API event payload requires this file and `feed.ts` together.

- `status`: `state`
- `prompt`: `text`, `imagesCount`
- `assistant_chunk`, `thinking_chunk`: `text`
- `tool_started`: `id`, `name`, `input`, `display`; `tool_finished`: `id`, `name`, `output`, `status`, `display`
- `steer_queued`: `id`, `text`; `steer_delivered`: `id`, `route`
- `notice`: `id`, `text`, `route` (`"cap"` marks the todo cap note)
- `todos`: `items` (the whole list, `{id, content, status}`)
- `compression`: `text`; `action` is `compress`, `decompress`, `nudge` or `preflight`, and a nudge also carries `kind`, `tokens` and `sinceLast`
- `usage`: `inputTokens`, `contextWindow`
- `error`: `message`

## Workflow

- `npm run check`, `npm test`, `npm run build`. The root `scripts/check` runs the first two when `web/` exists.
- Tests are Vitest; jsdom is the environment. Reducer, image limits, tool hues, Markdown sanitising, composer routing and the model picker are covered.
- The static output is `web/build/`, which the future embedded-assets crate consumes.
