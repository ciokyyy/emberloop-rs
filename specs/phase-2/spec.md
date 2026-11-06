# emberloop-rs, phase 2

**Status:** spec only. Starts after phase 1 is accepted **and** the measured phase 1 check meets the budgets of phase 1 decision O.

## Scope

The second harness and the tools phase 1 left out.

1. **pi-rust harness** (below).
2. `claude` harness over raw stream-json (decision: no ACP adapter), subscription traffic on the real `claude` binary only.
3. `scout` and `worker` subagents and their Settings forms.
4. `execute` (a `python3 -I` subprocess with a socket bridge, saved scripts, `tools.parallel`, store/load, 500-call cap), `readurl`, `websearch`, `find` (lexical, no index), `edit` stale-tag rebase.
5. MCP client and OAuth for external servers (spike `rmcp` first).
6. Tauri shell.
7. **UI takeover from emberloop-go** (below).

Every item starts with the Research first rule and tests first, and every ticket keeps the check within budget.

## Ticket P2-1: pi-rust harness

Harness #3, replacing the Go pig harness. Candidate: `yannelli/pi-rust`, run as the subprocess `pi --mode acp`, pinned to an exact release (v0.12.11 was tested on 2026-11-05; the project ships almost daily from a single maintainer). Fit rating 8/10 under the Research first rule.

**Probed 2026-11-05 with the verified linux musl binary, a clean environment, a fake key and local capture servers:**

- ACP: `initialize` about 0 s, `session/new` 60 to 100 ms; idle RSS 33 MB with extensions off, 41 MB with them on. Capabilities: `loadSession`, image input, session `list` and `delete`; `mcpCapabilities` has `http: false` and `sse: false`.
- A provider `baseUrl` with a path prefix is honoured: `http://127.0.0.1:18765/p/SESSIONTOKEN/v1` produced `POST /p/SESSIONTOKEN/v1/chat/completions` (OpenAI-compatible wire only). The ACP prompt ended `end_turn` with usage numbers. Config options: `model`, `thought_level`, `mode` (auto, ask, bypass).
- Native tools (`read`, `bash`, `edit`, `write`) vanish from the request with `--no-builtin-tools` or `--no-tools`.
- An HTTP MCP server in `session/new` fails with `-32602 ... HTTP transport is not supported`. The same server in pi's own `mcp.json` (`{"mcpServers":{"ours":{"url":"...","exposure":"direct"}}}`) connected and its tool reached the model as `mcp__ours__echo`. MCP is a built-in extension, so `--no-extensions` turns it off.

**Design:** the `Harness` plugin writes a per-Session pi config directory (`models.json` with the proxy base URL, `mcp.json` with the in-daemon MCP URL and `exposure: direct`), starts `pi --mode acp --no-builtin-tools --offline` with that `HOME`, and maps `forget(resume_token)` to ACP `session/delete`. Credentials are passed explicitly; the daemon never lets pi inherit the User's environment keys (`pi doctor` showed it reads `DEEPSEEK_API_KEY` from the environment).

**Not yet verified, and the ticket's spike must cover:** Anthropic and Google wire formats behind the proxy; real tool-call round trips and the `mcp__` names in transcripts; `session/load` resume and what the sessions directory holds; memory over a long Session; what `exposure: deferred` (the default) does; pi's Google Antigravity provider (`subscription-providers.md`); behaviour across releases.

**Tests first:** a fake pi (an in-process ACP fake) drives the harness plugin in the check; the live probe above becomes a recorded fixture plus an `#[ignore]` live test.

**Fallback:** the Go pig adapter over ACP, same trait.

## P2-UI: the Go Session surface, taken over

Phase 1 ships the simple SPA of its mock. Phase 2 replaces its feed and input with the Go design, **copied and not redrawn**. The source of truth is the Go UI (`emberloop-web/web/src/lib`) and its FigCraft reference (`designs/exports/tool-feed`).

- **Tool calls and outputs, fully taken over:** `ToolFeed.svelte`, `ToolStrip.svelte`, `ToolBody.svelte`, `ToolImage.svelte`, `ExecuteCode.svelte`, `CompressionCard.svelte`, with `toolfeed.js`, `toolstrip.js`, `compactFeed.js` (counted Explored groups, quiet finished rows), `toolFeedMode.js`, `toolText.js`, `events.js`, `eventStore.svelte.js`, `diff.js`, `highlight.js`, `highlighter.js`, `markdown.js`, `Markdown.svelte`, `MessageActions.svelte` and `images.js`, with their tests. Flat rows, hue glyphs, diff bodies, collapsed groups and the Classic switch behave as in Go.
- **Input, fully taken over:** `Composer.svelte` and `composer.js` with their tests (multiline, image paste, drop and pick, Stop, drafts), **except the switcher**. The input row carries exactly two indicators: the **context indicator** and the **model and harness indicator**. Both are display only: pressing them does nothing. The mid-Session model, effort and harness picks (`ModelSelect.svelte`, the swap pickers) are not taken over, and no model switcher is implemented in phase 2; its redesign comes later.
- **Not taken over:** the background-tool indicator and Processes panel, notice cards (Subagent report notices and the other notices), and the git panel.
- Anything the Go components import that Emberloop-rs lacks (Runs, quota, Ladders, groups) is cut at the import, not stubbed with fake data, and the ticket lists every cut.

**Tests first:** the copied Go component tests pass unchanged against the new event wire (a fixture feed renders the same rows, groups and diff bodies as Go); compact and Classic modes both render; the composer sends, stops and attaches as in Go; the context and model/harness indicators render and a press changes nothing; no switcher, background-tool indicator, notice card or git panel is in the DOM.

**Open conflict to settle before this ticket starts:** phase 1's simple feed already shows steer and background-exit notice rows (acceptance step 4 needs the User to see how a notice was delivered). If phase 2 drops notices from the UI, that information needs a different home.
