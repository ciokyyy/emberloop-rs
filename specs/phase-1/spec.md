# emberloop-rs, phase 1: agy + Sessions + core tools + one proxy

**Status:** tickets 01-20 built, `scripts/check` green; ticket 04 has two untested edge cases, ticket 11's live acceptance run is not complete, ticket 12 misses the prompt-error event, and ticket 21 is open (see each issue). Source: the 2026-11-05 grilling Session (`f88dc96e8e01e07bafdf370e16de1feb`) and its continuation.

This repo, written in Rust. It is a different product, not a port: the paradigm changes to **MCP for tools and one proxy for everything on the model wire**, and every harness plugs into both through one small trait. Phase 1 proves the whole shape with `agy` only.

## Problem Statement

The Go Daemon carries Runs, Ladders, Users, Memory, quota and a per-harness copy of most behaviour. The User wants a smaller, faster core in Rust where adding a harness means implementing a few functions, not rebuilding the plugin set, and where compression ("billion context"), skills text and steering live once, in a proxy every harness goes through.

## Phase 1 deliverable

One binary, a trimmed copy of today's UI, and `agy` working end to end:

- Projects, each owning its own Sessions (a Session belongs to exactly one Project and runs in its root), and Sessions (create, list, prompt with text and images, cancel, resume, delete).
- `read`, `edit` (hashline only), `bash` with `background`, `background`, `grep`.
- Background command notices delivered as steers.
- Steer: queued, injected by the proxy at the next request, or sent as a prompt when idle.
- Billion context through the proxy (compress, decompress, search_context), model-driven only. There is no automatic compression and no threshold setting.
- Ponytail injected as skill text by the proxy.
- A Settings tab (proxy, Ponytail level; Subagents section present but disabled).
- No auth, bound to loopback.

## Out of scope for phase 1

`claude` and `pi-rust` harnesses, `scout`/`worker`, `execute`, `readurl`, `websearch`, MCP client and OAuth, saved scripts, Tauri, voice, Runs, Ladders, Memory, git tools, Users, quota, the Babysitter, a codebase index, `find`. (`find` is lexical-only and lands in phase 2 with the other tools.)

## Decisions

### A. Stack

- Cargo workspace. Crates: `core` (Context tree, event bus, ids), `store` (Projects, Sessions, event log), `api` (axum, `utoipa`), `harness` (trait) and `harness-agy`, `proxy`, `tools` (MCP server and tools), `web` (embedded assets), `daemon` (the binary).
- Plugins are compiled-in crates registered on a Cordis-style Context tree: services injected by type, scoped disposal through `CancellationToken`, a typed event bus. No dylib, WASM or runtime loading.
- Frontend: SvelteKit with `adapter-static`, SSR off, Tailwind CSS v4 and shadcn-svelte (bits-ui), copied from `emberloop-web/web/src` and cut down. Same tokens, same flat tool rows, same event wire shape. Embedded with `rust-embed`.
- RPC: JSON over HTTP, SSE for the feed, `utoipa` generates `openapi.json`, `openapi-typescript` generates the client types. CI fails when the generated file is stale.
- Data root is new and separate from the Go Daemon's. No import of old Sessions.

### B. Projects, Sessions, storage

- A Project is `{id, name, root}`. A Session belongs to one Project; its work dir is the Project root.
- Layout: `<data root>/projects.json`, `<data root>/sessions/<id>/{meta.json, events.jsonl, images/, spill/}`.
- `events.jsonl` is append-only; each event carries `v`, `seq`, `sessionId`, `kind`, `at`. The SSE endpoint replays from `?after=<seq>`.
- No auth. The daemon binds `127.0.0.1`. A non-loopback bind needs `--allow-remote` and logs a warning at start.

### C. Harness trait

The only per-harness code. Phase 1 implements it once, for agy.

```
spawn(session, wiring) -> Process
send(prompt, images)
events() -> Stream<HarnessEvent>
cancel()
resume(token)
wake(text)                  # send a prompt when idle (steer path 2)
wiring: base_url_env(proxy_url), mcp_config(mcp_url), disable_native_tools()
```

- agy runs over the official `agent-client-protocol` crate, confined like the Go `agy` harness (`enabledTools: []`, empty work dir, eager MCP). ADR 0016 in the Go repo explains the confinement.
- A harness never composes model-facing text. The proxy and the policy text own it.

### D. Proxy

- Route `/p/<session-token>/…`. The token is HMAC-signed and carries the Session id, so the proxy knows the Session without a header.
- It forwards model traffic, counts usage for the context meter from the last request's input tokens (not a running sum), and records spliced content.
- **Billion context:** the proxy injects `compress`, `decompress` and `search_context`, runs them itself and never shows the harness a proxy tool call. It does this through one `Compressor` trait (`compress(range, summary)`, `decompress(block)`, `search_context(query)`, `status()`), so the proxy owns the wire and the compressor owns the blocks.
  - **Phase 1 backend: native Rust.** The model writes the summary; the compressor swaps the chosen message range for a stored block, keeps the originals in the Session's `spill/`, restores them on `decompress` and searches them for `search_context`. Tiers, nudges, `acp_status`, `acp_rule` and the preflight backstop follow billion-context 0.1.212 under ticket 20, proved by golden fixtures; the model still decides every fold, there is no automatic compression.
  - **One proxy.** bili (the `billion-context` npm package) is not run. A `bili` backend that talks to a supervised bili process, pinned to an exact version with `autoUpdate: false`, a start-time health and manifest check and its version recorded in `meta.json`, is a later optional ticket, and only for lanes where it works (agy through bili is unproven; see the Go repo's `agy-bili-lane`).
  - **Parity by fixtures.** The message shapes the native compressor must preserve are fixtures taken from the Go repo's bili tests and probe scripts.
- **Ponytail:** one embedded `SKILL.md`, filtered by `ponytail_level` (`off`, `lite`, `full`, `ultra`), appended to the system prompt. No skill registry.
- **Caveman:** the `caveman` plugin's `SKILL.md`, filtered to its `ultra` row and examples, always on: banner, ruleset, one-line reminder, appended to the system prompt after Ponytail. Copied from the Go repo's `el/voice.go`; there is no level switch and no second delivery owner.
- **Stored splices:** anything the proxy adds to history (a steer, a compress marker) is stored keyed by the id of the tool-result turn it attached to, and re-attached byte-identically on every later request. If the harness drops that turn, the splice is dropped with it. Thought signatures and other opaque fields are never re-serialized.
- **Gate:** if Ticket 1 shows agy cannot work behind the proxy, phase 1 ships with Ponytail through `systemPrompt.append`, steer path 2 only, and compression deferred.

### E. Steer

- One queue per Session. An entry is `{id, kind: steer | notice, text, state}`.
- The queue is drained by whichever comes first:
  1. the next model request: the proxy appends the entry as a text part in the user turn that carries the tool results, then marks it delivered when the upstream request succeeds (requeued by id on failure);
  2. turn end with no tool call: the daemon calls `wake(text)`.
- An idle Session takes path 2 immediately. There is no Follow-up. Background notices enter the same queue with `kind: notice`.
- The feed shows pending, then delivered, with the route taken (`injected` or `sent at idle`).

### F. Tools (MCP server in the daemon)

Shared rules: every path resolves inside the Project root after symlink resolution; output over the overflow threshold spills to `spill/` and returns a head/tail summary with a handle; each call emits `tool_started` and `tool_finished` events.

- **`read`**: `path`, `offset`, `limit`. Plain text only, 2 MiB cap, numbered lines. Records the lines shown in the seen-line store and returns `[path#TAG] (lines A-B)`. Directories return a listing.
- **`edit`**: hashline only, ported from the Go grammar: header `[path#TAG]`, ops `PUT N.=M:`, `PUT <N:`, `PUT >N:`, `PUT >$:`, `CUT N`, `CUT N.=M`, body lines start with `+`. The tag is the first two bytes of the file's sha256 as four hex digits. A patch applies against the original in one pass, anchors only on lines the tag's view showed, and returns the new tag with the changed lines in context. A stale tag is refused with the current lines in phase 1; rebase is phase 2.
- **`bash`**: `command`, `timeout` (default 120 s, max 600 s), `background`. Own process group, merged and capped output, kill the group on timeout. `cat`/`head`/`tail`/`sed -n`/`grep -n` views of files feed the seen-line store. With `background: true` it starts a background command and answers with the handle and its first output.
- **`background`**: `list`, `read` (offset into the log file, answers with the chunk and the next offset), `wait` (one call, answers `exited` or `timeout`), `stop`. The daemon owns the process; the log is a file in `spill/`, and a handle survives a daemon restart because the daemon adopts the pid.
- **`grep`**: ripgrep crates, `.gitignore` aware. `pattern`, `path`, `glob`, `case_insensitive`, `context`, `max_results`, files-only and count modes. Hits feed the seen-line store.
- `bash` is not a sandbox. The jail constrains tool path arguments only. The docs say so.

### G. API

`GET/POST /api/projects`, `PATCH/DELETE /api/projects/:id`, `GET/POST /api/sessions`, `PATCH/DELETE /api/sessions/:id`, `POST /api/sessions/:id/prompt` (`{text, images}`), `POST /api/sessions/:id/steer`, `POST /api/sessions/:id/cancel`, `GET /api/sessions/:id/events?after=`, `GET/PUT /api/settings`, `GET /openapi.json`.

Event kinds: `status`, `prompt`, `assistant_chunk`, `tool_started`, `tool_finished`, `steer_queued`, `steer_delivered`, `notice`, `compression`, `usage`, `error`, `timing`. `usage` carries the last request's input tokens and the model's window; the proxy publishes it, the harness never does.

### H. UI

The mock is `mock/index.html`. Phase 1 UI:

- Sidebar: Projects with their Sessions, status dot plus a word on hover, new Session dialog (Project, model, Ponytail level), per-row rename and delete, Settings entry. A Project URL scopes the list to that Project and its New session button opens the dialog with that Project chosen; a second button opens the same dialog for a new Project by path. The dialog always asks, because the model is one of its choices.
- Session view: header with title, model, context meter and status; the feed; the composer.
- Feed: user and assistant rows (Markdown), flat tool rows with hue glyphs (`bash` green, `read` blue, `edit` amber, `grep` violet), expandable bodies (diff for `edit`, highlighted text for `read`, command and output for `bash`, hits for `grep`), a compression card, steer and notice rows, error rows.
- Composer: multiline, Enter sends, image paste/drop/pick with thumbnails (10 MB each, 5 per prompt), Stop while working, a steer box while working.
- Settings tab: Proxy (billion context on/off), Ponytail default level, Subagents (scout, worker: model, effort, enabled, extra instruction; disabled in phase 1).
- Left out: Dashboard, quota cards, Memory, git panel, processes panel, groups, pinning.
- Scroll follows the tail, pauses when the User scrolls up, and shows a jump-to-latest pill.

### I. Prompts as files

- Every model-facing text the daemon owns is a file, not a constant: the Ponytail skill, the caveman skill, the policy text, the compress/decompress/search_context tool descriptions and prompts, the MCP tool descriptions, and later the subagent prompts. Each has a stable id (`ponytail`, `policy`, `tool.edit.description`, ...).
- Two layers, lowest first: the default embedded in the binary (`include_str!`), then an override file in `<config root>/prompts/<id>.md`. A missing or empty override means the default. No rebuild is needed to change a prompt.
- Text may use `{{ name }}` placeholders filled by the daemon (Project name, root, level, date); unknown placeholders are an error at load, not at request time.
- A watcher (`notify`) reloads changed files; a file that fails to parse keeps the last good text and emits an `error` event.
- **Cache rule:** a prompt is pinned per Session when the Session starts (id to content hash). A running Session never loads a changed prompt, so its cache and stored splices stay valid; only Sessions created after the change use it. There is no apply-now action. Tool descriptions follow the same rule.
- Each Session's `meta.json` records the hash of every prompt it uses, so a transcript can be traced to its exact prompt text.
- `GET /api/prompts` lists ids with default, override, active hash and whether the override differs; `PUT/DELETE /api/prompts/:id` writes or removes an override. The Settings tab gets a Prompts section (list, edit, reset to default, diff against the default).

### J. Efficiency budgets

- **Segmented event log.** `sessions/<id>/events/NNNNNN.jsonl` segments of about 4 MB plus a small sidecar index (turn number to segment and byte offset). A tail read seeks to the last turns and parses nothing older; its cost scales with the window, not the Session's age.
- **Coalesce before writing.** Assistant text is buffered 100 to 250 ms, or until a tool starts or the turn ends, and written as one event. A tool call writes `started` and `finished` only; progress goes out over SSE and is not stored. Big bodies go to content-addressed blobs and the event keeps a head, a tail and a handle.
- **The daemon holds no history.** Per Session it keeps metadata, the last `seq` and the segment index. Live fan-out uses a small bounded ring (about 200 events); replay of anything older reads the file.
- **Session list reads metadata only**, never an event file.
- **Frontend.** The first request returns the Session meta and the last turn or two (about 50 events). The feed is virtualized and older pages load on scroll up with the scroll position held. Client state is kept for the current Session and the previous two only. Markdown and highlighting render for visible rows only.
- **Spawn.** The MCP tool server runs in the daemon, so a Session spawns no tool process. A small warm pool (the last 2 adapters, short TTL) plus prewarm on open and on composer focus. Every stage (`spawn`, `initialize`, `session/load`, first token) is logged as a `timing` event.
- Budgets are tests (ticket 13): opening a Session with 1M events reads under a fixed byte count; the idle daemon stays under an RSS budget with 1,000 Sessions on disk (a target to set when measured, not a known figure); a Session list request touches no event file; coalescing keeps the line count within a set ratio of tokens streamed; tail time does not grow with log size.

### K. Model catalogue

- One `ModelCatalogue` service with a `Source` trait (`fetch() -> Vec<Entry>`); each harness plugin contributes its own source. An entry carries `id`, `provider`, `label`, `kind`, `efforts`, `image_input`, `context_window`, `source`, `stale`, `updated_at`.
- Sources merge by entry key. A failing source keeps its last good entries marked `stale` with its error and time; nothing empties.
- The snapshot persists atomically at `<data root>/models/catalogue.json`, so a restart serves the last list before the first refresh. A refresher runs at boot and every 10 minutes; `POST /api/models/refresh` forces it.
- Phase 1 has one source: agy, discovered over ACP at `session/new`. Ladders, Relace, OpenRouter, Groq, claude discovery and decisions models are out of scope and arrive later as new `Source` impls.
- `GET /api/models` serves the list; one picker component feeds the new Session dialog and the Subagents settings.

### L. Storage ownership and archival

- **Two stores, disjoint jobs.** Our event log holds what the UI shows. The harness's own store holds what the model sees and is needed to resume. We never copy the harness's transcript; `meta.json` keeps only its resume token. The proxy's `spill/` holds only unique data (originals of compressed ranges), never a copy of either store.
- **Content-addressed blobs.** Images and big tool output are stored once as `blobs/<sha256>`; events point at them.
- **Delete means delete everywhere.** The harness trait gains `forget(resume_token)`; deleting a Session also deletes the harness's copy.
- **Lifecycle: active → archived → restored, and nothing is ever deleted.** A sweep runs every 24 hours (`archive_sweep_hours`). Each Session has a `lastUsedAt` and a state in `meta.json`.
  1. **Active.** Used within the last 3 days (`archive_after_days`).
  2. **Archived.** At a sweep, an active Session idle for 3 days or more (no live process, no running turn, no queued steer) is compressed into an archive file: text only, zstd at the highest practical level, long-window mode. The write goes to a temporary file and is renamed, and the uncompressed log is removed only after the archive is verified, so an interrupted run leaves the original intact. Already compressed media (png, jpeg, webp) is left alone. The harness copy is kept, so the Session can still resume. `meta.json` records `archivedAt`; the Session list and `meta.json` stay uncompressed.
  3. **Restored.** Opening an archived Session decompresses it into a normal hot copy, marks it current (`restoredAt`, `lastUsedAt` now) and serves reads from the hot copy. **The archive file stays on disk.**
  4. **The next sweep decides, once the Session has been restored for 24 hours or more** (`restore_keep_hours`) and has no live process:
     - **Resumed** (a prompt was sent, so new events exist after the restore): it goes back to Active with its clock from the last use, the now-stale archive file is deleted, and the loop continues as normal: it is compressed again after 3 idle days.
     - **Only viewed** (nothing new was written): the hot copy is deleted and the archive stays. No recompression is needed, so a look at an old Session costs only temporary disk.
- Each sweep logs what it compressed and what hot copies it removed (Session id, title, size). A Session is only ever removed by the User deleting it, which also calls `forget` on the harness copy.
- The sweep runs at low priority, one Session at a time, skips when the daemon is busy, and is idempotent: a second sweep over the same state changes nothing.
- Compression uses plain `zstd` with long-window mode. Restore decompresses the whole file, so random access is not needed and a seekable format is not used (it resets the window per frame). Ticket 14 confirms the crate and its long-window support before depending on it.

### M. Hardening decisions from the 2026-11-05 review

A subagent reviewed the plans and the code so far. These decisions close its findings; each lands with failing tests first.

- **Check is hard.** `scripts/check` has no `|| true` and no "if the tool exists" guards. It runs `cargo fmt --check`, `cargo clippy --workspace --all-targets` with `[workspace.lints]` (`unwrap_used` and `expect_used` outside tests, `await_holding_lock`, `large_futures`, `cognitive_complexity`) under `-D warnings`, `cargo nextest run --workspace` plus `cargo test --doc`, `cargo deny check`, `cargo machete`, and in `web/`: `npm ci`, `svelte-check`, a blocking `fallow health`, and the web tests. It then dumps `openapi.json`, regenerates the TypeScript client and fails on `git diff --exit-code`. Missing `web/node_modules` is a failure, not a skip. The Go repo's complexity thresholds are read from `.emberloop/go-complexity` and copied, not assumed.
- **Current stack, pinned.** axum 0.8, utoipa 6, utoipa-axum 0.3, `agent-client-protocol` pinned exactly, `rmcp` for the MCP server. Bump before more routes exist.
- **No hand-rolled protocol.** agy runs over the `agent-client-protocol` crate, as decision C says. The MCP server uses `rmcp` only if ticket 05 first confirms it serves streamable HTTP with cancellation; otherwise the hand-rolled server stays, with tests for sessions and cancellation.
- **Splices are persisted.** `sessions/<id>/splices.jsonl`, append-only, keyed only by the id of the tool-result turn they attach to (no other anchor kind). Tests: byte-identical replay across a daemon restart; a turn the harness re-serialises with different ids drops its splice instead of misapplying; double-apply is impossible (an applied splice is detected by its marker, not a heuristic).
- **The streaming path.** The proxy forwards model SSE through. When a proxy-tool `functionCall` appears, it withholds that part, runs the tool, issues the follow-up upstream request and continues streaming into the same client response. A turn with only proxy calls loops (cap 8 round trips, then an `error` event). A mixed turn (proxy and harness calls) is held, the proxy calls are resolved upstream, and the harness sees a rewritten turn with only its own calls; the proxy pair is stored as a splice. A client disconnect cancels the upstream request and any running proxy tool; each round trip has a timeout. Each case gets a fake-SSE-upstream acceptance test.
- **Steer delivery is exactly-once by definition.** An entry is delivered when the upstream request returns a 2xx response head, not when the stream ends; a stream that dies afterwards still counts as delivered because the model saw it, and the stored splice replays it. An entry is requeued only on a non-2xx or a connection failure before the response head. Turn end and an in-flight request race through one per-Session lock: `wake` fires only when no proxy request is in flight and the queue is not empty, and one wake drains the whole queue as a single prompt, so a notice and a steer never trigger two wakes. The queue is persisted in the Session directory.
- **Background adoption.** The registry records pid, process start time (`/proc/<pid>/stat`) and boot id. Adopt or `stop` only if all three match; otherwise the handle is marked lost and nothing is killed. An adopted process's exit code is unknown and is reported as `exited (code unknown)`. An exit that happened while the daemon was down still produces its notice exactly once.
- **Seen-line store is per Session and persisted** in `sessions/<id>/seen/`, so a restart does not make every `edit` refuse. The displayed tag stays four hex digits, and the store keeps the full sha256 and checks it too, so a 16-bit collision cannot pass a stale edit.
- **Housekeeping.** `crates/spike` is excluded from the workspace once its findings are fixtures. Every workspace member exists and every endpoint in decision G has a `#[utoipa::path]`. The proxy token check uses a constant-time compare and a per-daemon-start key.

### N. Extensibility: a Cordis-shaped Context tree

Cordis (cordiverse, MIT, README read; its source was not) gives the shape; we take the parts that fit a compiled Rust daemon and leave the rest. Not everything is a plugin: the core (store, event log, API shell) is plain code, and only things that vary are plugins.

| Cordis | Here |
|---|---|
| `Context`, `ctx.extend()` child | `Context`, cheap to clone (Arc), `ctx.child()` shares parent services and can override some |
| services (`ctx.database`) | typed services: `ctx.provide::<T>(Arc<T>)`, `ctx.get::<T>() -> Option<Arc<T>>`, `ctx.require::<T>()` |
| plugin = function, class or object with `apply` | `trait Plugin { fn name(); fn deps() -> &[ServiceId]; fn apply(&self, ctx: &Context) }`, registered with `ctx.plugin(P)` which returns a `Scope` |
| `dispose()` on the fork scope | `Scope` is RAII: dropping or `scope.dispose()` runs the plugin's disposers in reverse order and cancels its `CancellationToken` child |
| `inject` (hard dependency): wait until present, unload when it goes away, reload when it returns | `deps()`: a plugin is applied only once every listed service exists, disposed when one is removed, and re-applied when it returns |
| events `emit`, `parallel`, `bail`, `serial` | typed events on the bus: `emit` (all, sync), `parallel` (all, async), `bail` (first non-`None` wins, a veto), `serial` (ordered async); every `on` returns a disposer tied to the plugin's scope |
| `isolate(keys)` | a child Context that overrides chosen services: **a Session is a child Context** with its own steer queue, compressor, seen-line store and prompt pins, while the harness registry and model catalogue stay shared |
| lifecycle `ready` / `dispose` | `Ready` and `Dispose` events |

**Left out on purpose:** property-style dynamic injection, filters and `intercept`, reusable forks, runtime plugin loading (Rust has no stable ABI; see decision A), automatic reload from config files.

**Extension points** are service types holding a registry, and a plugin registers its implementation at apply time and removes it on dispose:
- `Harness` (agy now; claude and pi-rust later),
- `ToolProvider` (adds tools to the MCP server),
- `ModelSource` (decision K),
- `Compressor` (decision D),
- `PromptSource` (decision I),
- `RouteContributor` (adds axum routes and OpenAPI paths),
- event hooks, including `bail` hooks that may veto (for example before a tool call).

**The extensibility test:** adding a harness is one new crate and one `ctx.plugin(...)` line in the daemon composition root, with no change to any core crate. A fake harness in the test suite proves it.

The core crate stays small (target under 600 lines) and carries tests for: dependency wait, unload and re-apply; disposal order; `bail` and `serial` ordering; a disposer removed with its scope; a child Context overriding one service while sharing the rest; a panicking plugin not taking down the tree.

### O. Fast tests and a check budget

The Go repo's full check took about 7 minutes. Its test timings show why: 1,776 tests summed to 669 s, 110 tests of 1 s or more were 60% of that time, and 4 tests of 10 s or more were 27% (one alone ran 114 s, spawning real processes). The root package was also one test binary that package parallelism could not split. Rules here:

- **Budgets:** the loop check (changed crates only) finishes in 30 s or less; the full check in 3 minutes or less on this 4-core host. `scripts/check` prints per-stage wall time and fails when a budget is exceeded. Budgets are recorded in `scripts/check-budget.tsv` and a change to them needs a reason in the commit.
- **Hard per-test cap:** nextest `slow-timeout` terminates any test that runs past 4 s, so a slow test is a failure, not a trend.
- **No real time, no real harnesses in the check.** Tests use paused tokio time and a fake clock, never `sleep` (clippy `disallowed-methods` forbids `std::thread::sleep` and `tokio::time::sleep` outside the fake clock module). Harness, model and process tests use an in-process fake (an ACP fake, the fake SSE upstream) built once. Live tests are `#[ignore]` or behind a feature and never run in `scripts/check`.
- **Small crates, small test binaries.** A crate whose tests take more than 20 s is split. No crate may become the new single root package.
- **Stages run in parallel.** fmt, clippy, deny, machete, the web checks and the tests run concurrently where they do not share a build lock; tests are built once by nextest and reused.
- **Build speed:** the mold linker, `debug = "line-tables-only"` for dev and test profiles, dependencies at a light optimization level, sccache in CI.
- **Changed-only loop:** `scripts/check-changed` runs the tests of changed crates plus their reverse dependencies (from `cargo metadata`), and hands off to the full check when the diff touches the workspace manifest, `scripts/`, `deny.toml` or `clippy.toml`.
- **Web:** Vitest runs only changed files in the loop; `svelte-check` and Fallow run on the full tree in the full check.
- **A slow-test report** (top 10 by time) is printed by every full check, and any new test over 1 s needs a comment saying why.

### P. Providers and the agy login

- **A Providers entry in the sidebar footer, beside Settings,** opens a Providers page. Phase 1 lists one provider, agy, with its Account (label, signed in or out, tier, last login) and a Sign in button. More providers and several Accounts per provider come later.
- **The login is a daemon flow, as in the Go Daemon:** spawn the pinned agy ACP server for the Account's own `GEMINI_HOME` (with `--uid=`, found in the spike), send ACP `authenticate` with `methodId: "oauth-personal"`, read the sign-in URL the server prints on stderr, and show it in the panel (open and copy buttons). The User signs in, the browser lands on a loopback address that does not load on a remote machine, the User pastes that address into the panel, and the daemon GETs it to complete the exchange. The Account is kept only after the server answers with its tier; a failed, timed-out or cancelled login removes the Account and its home.
- **No secret passes through emberloop-rs.** The server owns its OAuth client and writes the token file in `<data root>/accounts/agy/<id>/`; the daemon never reads, logs or returns its contents. Sign out deletes the Account and its home.
- **One login at a time** (a second start answers 409); a login times out and kills its server.
- **Endpoints:** `GET /api/providers` (providers with Account status), `POST /api/providers/agy/login` (`{label}`), `GET` and `DELETE /api/providers/agy/login/:id`, `POST /api/providers/agy/login/:id/complete` (`{address}`), `DELETE /api/providers/agy/accounts/:id`. All in the OpenAPI document.
- **A Session on a signed-out Account** fails at start with an error row that links to the Providers page.

## Tickets

Order follows `Blocked by`. Acceptance for the whole phase is Ticket 11.

1. **Spike: agy behind the proxy.** Throwaway Rust proxy; agy → proxy → model; inject one tool, run it, store the turn, re-splice byte-exact on request 2, thought signatures survive. Also pins the compression trigger rules from the existing proxy. Blocks 6, 8, 9. Failure rewrites D and E.
2. **Workspace, hard `scripts/check` (decision M), (fmt, clippy with the complexity lint, tests, `svelte-check`, Fallow, OpenAPI freshness; see root `AGENTS.md`), Context tree, data root, Projects and Sessions store, event log.**
3. **agy ACP driver** behind the harness trait, with `resume`, `cancel`, `wake`. Blocked by 2.
4. **HTTP API, SSE, OpenAPI, generated TypeScript client in CI.** Blocked by 2 and 13 (the log it serves).
5. **MCP server with `read`, `grep`, overflow store, jail.** Blocked by 2.
6. **Hashline `edit`, seen-line store.** Blocked by 5; port the Go tests as the cases.
7. **`bash` with views, `background` tool and daemon-owned processes that survive restart.** Blocked by 5.
8. **Proxy: routing, usage meter, Ponytail, stored splices, and the `Compressor` trait with the native backend (compress/decompress/search_context).** Blocked by 1, 3, 12. Tests first, from the bili fixtures.
9. **Steer queue and both delivery routes, notices from background exits.** Blocked by 7, 8.
10. **SPA (blocked also by 12, 13 and 15 for the Prompts section, the tail load and the model picker): copy, trim, Tailwind v4 and shadcn-svelte, Projects, feed, composer with images, Settings tab.** Blocked by 4.
11. **Acceptance run** (below). Blocked by all.
12. **Prompts as files** (decision I): ids, embedded defaults, override layer, placeholders, watcher reload, per-Session pinning and hashes, API, Settings section. Blocked by 2; tickets 8 and 10 consume it, so 8 reads Ponytail and the compression prompts through it.
13. **Efficiency (reworks the log ticket 02 built; ticket 03's consumer adapts): segmented event log, coalescing, tail reads, no-history daemon, timing events, budgets as tests** (decision J). Blocked by 2; do it before 3 and 4 build on the log.
14. **Archival sweep, content-addressed blobs and harness `forget`** (decision L). Blocked by 13 and 3. Adds `forget` to the harness trait after the agy driver shipped, so ticket 03's driver is updated in this ticket.
15. **Model catalogue** with the agy source (decision K). Blocked by 3.

## Testing Decisions

- Unit: hashline grammar and tag rules from the Go tests; jail and symlink escapes; overflow handles; steer queue transitions including requeue after an upstream failure; splice replay is byte-identical across requests; the meter uses the last request's input.
- Integration with a fake model server (the Go repo's `fakellm` is the model for it): the proxy swallows its own tool calls, injects a steer into a tool-result turn, and drops a splice when the harness drops the turn.
- Live probe, outside the check: one agy Session through every phase 1 feature.

### Acceptance (Ticket 11)

1. Create a Project and a Session; send text plus an image; the agent answers.
2. The agent reads a file, greps, edits through a hashline patch, runs `bash`; each shows a tool row.
3. A stale tag edit is refused and the refusal names the current lines.
4. The agent starts `bash(background: true)`; the command exits; a notice reaches the model as a steer on its next request, or as a prompt if the turn was over. The feed shows which.
5. A steer sent mid-turn is injected at the next request and appears again, unchanged, in later requests.
6. In a long Session the agent calls `compress`, the history shrinks, and `decompress` restores a turn.
7. Edit `<config root>/prompts/ponytail.md`; a new Session uses the new text, a running Session keeps its pinned text, and `meta.json` shows both hashes.
8. Restart the daemon; the Session resumes and a running background command is still readable.
9. Opening a Session with a large log reads only its tail. A Session idle for 3 days is compressed by the sweep. Opening it restores the same feed; if it was only viewed, the next sweep after 24 hours removes the hot copy and keeps the archive; if it was resumed, the archive is replaced and the loop restarts.

## Open Questions

- Whether agy's `CLOUD_CODE_URL` seam works in the confined harness (Ticket 1).
- Whether the proxy stays in-process (assumed) or becomes its own process.
- The `edit` rebase and `find` land in phase 2.
16. **Context tree v2** (decision N): typed services, `Plugin` with `deps`, RAII `Scope`, the four event modes, child Contexts, the extension-point registries, and a fake harness plugin proving the extensibility test. Reworks `core/context.rs`; the agy driver, tools, proxy and prompts re-register as plugins. Blocked by 2. Do it before ticket 4 and before any further plugin code.
17. **Fast checks and budgets** (decision O): nextest config with the 4 s cap, `disallowed-methods`, `scripts/check` stage timing and budgets, `scripts/check-changed`, `scripts/check-budget.tsv`, slow-test report, mold and profile settings. Blocked by 2. **Phase 2 does not start until the measured phase 1 check meets the budgets.**
18. **Providers page and agy login** (decision P): the login flow, the endpoints, the Providers page and its sign-in panel, the signed-out Session error. Blocked by 3, 4 and 10.
