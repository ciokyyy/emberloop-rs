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

## Ticket P2-2: claude harness over raw stream-json

Harness #4: `crates/harness-claude` implements the `Harness` trait by running the real `claude` binary as `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages` and speaking newline-delimited JSON on its stdin and stdout. No ACP adapter, no Node, no SDK crate between the daemon and `claude`. This is the phase 2 scope decision (item 2), re-confirmed against the candidates below on 2026-11-06.

### Research (root Research first rule; sources read 2026-11-06)

| Candidate | Fit | Reasoning | Last release | Cost |
|---|---|---|---|---|
| **Raw stream-json, our own `harness-claude`** | 9 | Fewest layers (daemon, `claude`). Mid-turn stdin messages are native, so steer and wake map directly. We pin fixtures to the CLI version. | `claude` 2.1.319 installed, moves weekly | We own the parser and its drift; about 600 lines, no new dependency beyond `tokio`/`serde_json` |
| Fork `soddygo/claude-code-acp-rs` | 4 | Reuses `harness-acp`. But it is ACP-over-`claude-code-agent-sdk`-over-CLI: three layers, and it hides the stdin queue and the process flags we need. | Crate 0.1.22 on 2026-02-13; last push 2026-04-20; 5 stars, 1 maintainer, 18 open issues, MIT | `sacp` 11 plus an unstable-feature schema vendored via `[patch]`; we would own a fork with a stale SDK underneath |
| Use `claude-code-agent-sdk` / `claude-agent-sdk-rs` directly | 5 | Saves the parser, keeps the stream-json layer. | 0.1.39 on 2026-02-13 / 0.6.4 on 2026-02-09 (both MIT, same description, one maintainer each) | Six-to-nine months behind a CLI that adds protocol fields often; their types would sit in our trait boundary |
| Official `@agentclientprotocol/claude-agent-acp` (Go Emberloop's way, pinned 0.88.0) | 5 | Proven in the Go repo. | Not re-checked | Needs Node at runtime; ACP hides the stdin queue |
| `agent-client-protocol` Rust crate (5.0.0, 2026-11-02, Apache-2.0) | n/a | A protocol library, not a claude driver. Useful only if an adapter is chosen. | 2026-11-02 | n/a |

**Recommendation:** build `harness-claude` ourselves on stream-json. A fork was rated lower because it adds layers the daemon would have to see through, not because the code is bad.

**Not verified from a primary source:** the stdin message shape and the exact `stream_event` and `control_request` shapes (the official doc page names the event types and `system/init`, but the community pages disagree on the `initialize` payload); whether `--mcp-config` accepts an `http` MCP server with the options we need; what the fork's SDK layer does with queued mid-turn messages; the dependency graph of `claude-agent-sdk-rs`.

### Scope and design

- **Spawn:** `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages` in the Session's stable `workdir`, with a per-Session `CLAUDE_CONFIG_DIR` the daemon owns, `--setting-sources` limited to that user dir, `--strict-mcp-config --mcp-config <json>` naming only our MCP server, `--permission-mode bypassPermissions` with `--allowedTools mcp__el`, `--permission-prompts none`, `--model`, `--effort` from `thought_level`, `--resume <session_id>` from `resume_token`. Pinned Go-repo lesson: a load without the stripping flags re-asks permission, so one builder serves new and resume.
- **Native tools off:** every built-in (Bash, Read, Edit, Write, Grep, Glob, WebFetch, WebSearch, Task/Agent, TodoWrite, plan and question tools, MCP resource tools) is removed with `--tools` and a deny list; the model gets only our MCP tools, including `todo` (ticket 19). The deny list is a fixture tested against the `tools` array in `system/init`, so a CLI that adds a tool fails the test, not a Session.
- **System prompt:** the one policy text from `crates/prompts`, passed with `--system-prompt` (or `--append-system-prompt` if the spike shows removing the default prompt breaks tool use). Never composed in the harness.
- **Event mapping** (parser is a pure function over lines, tested from recorded fixtures): `stream_event` text delta to `AssistantChunk`, thinking delta to `ThoughtChunk`, an `assistant` `tool_use` block to `ToolStarted`, a `user` `tool_result` block to `ToolFinished`, `result` to `TurnFinished` with usage and `stop_reason`, `system/api_retry` to a status note, anything unknown to a debug log and never a Session error. `system/init` gives the model, tools, `mcp_servers` and `capabilities`; the harness refuses to start a Session if our MCP server is not `connected` in it.
- **Steer and wake:** a mid-turn user message is written to stdin as a user message; `claude` queues it into the running turn. `wake(text)` after `TurnFinished` is the same write. This is the whole harness side of steering. Route 1 splice injection does not exist for this harness (see the open conflict).
- **Cancel:** SIGINT to end the turn (documented); the spike also tries the `interrupt` control request and keeps whichever leaves the process reusable.
- **Resume and forget:** `session_id` from `system/init` is the `resume_token`; `resumed()` is true when `--resume` loaded it. `forget` deletes the transcript file in the owned config dir.
- **Process life:** one `claude` process per live Session; an unexpected exit emits `Error` and the next `send` respawns with `--resume`. Idle exit after the configured timeout is allowed. Stdin closing ends the process cleanly; the harness waits at most a short, configurable time for it.
- **Models:** the fixed aliases `opus`, `sonnet`, `haiku` as the Go repo, as a catalogue `Source` entry marked `harness: claude`.
- **Subscription rule:** the daemon never reads or holds the Claude subscription token; `claude` uses its own login in the owned config dir (the Providers panel gets a "Log in with claude" action that runs `claude auth login` in that dir). `ANTHROPIC_BASE_URL` is never set for this path.

### Open conflict (decide before the proxy-features step)

The root rule says one proxy owns the model wire, and also that Claude subscription traffic stays on the real `claude` binary. Both hold, so a subscription Session **bypasses the proxy**: no billion context, no Ponytail, no Route 1 steering, no compression, and the context meter reads usage from `result` instead of the proxy. Two ways out, both outside this ticket's first step:

1. Ship subscription Sessions without proxy features and say so in the UI.
2. Add an API-key mode that sets `ANTHROPIC_BASE_URL` to the proxy's Anthropic wire (P2-1 step 3 builds that codec); only this mode gets proxy features.

### Order (each step test-first, lands only with `scripts/check` green)

1. **Spike (throwaway), then fixtures:** run the real binary with a throwaway prompt and record, as files under `crates/harness-claude/tests/fixtures/`, the full stream for: a plain turn, a tool round trip through a fake MCP server, a mid-turn stdin message, an interrupt, a `--resume`, an `api_retry`, and a refused permission. Confirm the stdin message shape, the `http` MCP config, the `--tools` set, and whether a system-prompt replace keeps tools working. Findings go into this section; unconfirmed items stay listed as not verified.
2. Line parser and event mapping from the fixtures, no process.
3. `harness-claude` against an in-process fake `claude` (a test binary that replays a fixture and echoes stdin): send, steer, wake, cancel, resume, crash and respawn, init refusal.
4. Spawn builder with flag tests (the flag list is data; a test pins it).
5. Hook-up: workspace member, daemon registration, catalogue entry, Providers login action.
6. One `#[ignore]` live test with the User's login, never in the check.
7. Proxy features for the API-key mode, only after the conflict above is decided.

### Tests first

Failing before implementation: the parser maps each fixture to the exact `HarnessEvent` list; an unknown event type is skipped without an error; a mid-turn message reaches stdin before `TurnFinished` and the fake echoes it into the same turn; `wake` after the turn writes one message; cancel leaves the process reusable for the next `send`; a crash emits `Error` once and the next `send` respawns with `--resume`; `resumed()` reflects the init result; the builder output contains none of the subscription token variables and no `ANTHROPIC_BASE_URL`; the deny list covers every tool in the recorded `system/init`; the Session refuses to start when our MCP server is not `connected`; the whole set runs in the loop-check budget with no real sleep and no real `claude`.

### Risks

The CLI is not a stable API and ships often; fixtures pin one version and the live test is the early warning. The Go repo's pinned ACP adapter remains the fallback, with the same trait, if the stream proves too unstable.

## P2-UI: the Go Session surface, taken over

Phase 1 ships the simple SPA of its mock. Phase 2 replaces its feed and input with the Go design, **copied and not redrawn**. The source of truth is the Go UI (`emberloop-web/web/src/lib`) and its FigCraft reference (`designs/exports/tool-feed`).

- **Tool calls and outputs, fully taken over:** `ToolFeed.svelte`, `ToolStrip.svelte`, `ToolBody.svelte`, `ToolImage.svelte`, `ExecuteCode.svelte`, `CompressionCard.svelte`, with `toolfeed.js`, `toolstrip.js`, `compactFeed.js` (counted Explored groups, quiet finished rows), `toolFeedMode.js`, `toolText.js`, `events.js`, `eventStore.svelte.js`, `diff.js`, `highlight.js`, `highlighter.js`, `markdown.js`, `Markdown.svelte`, `MessageActions.svelte` and `images.js`, with their tests. Flat rows, hue glyphs, diff bodies, collapsed groups and the Classic switch behave as in Go.
- **Input, fully taken over:** `Composer.svelte` and `composer.js` with their tests (multiline, image paste, drop and pick, Stop, drafts), **except the switcher**. The input row carries exactly two indicators: the **context indicator** and the **model and harness indicator**. Both are display only: pressing them does nothing. The mid-Session model, effort and harness picks (`ModelSelect.svelte`, the swap pickers) are not taken over, and no model switcher is implemented in phase 2; its redesign comes later.
- **Not taken over:** the background-tool indicator and Processes panel, notice cards (Subagent report notices and the other notices), and the git panel.
- Anything the Go components import that Emberloop-rs lacks (Runs, quota, Ladders, groups) is cut at the import, not stubbed with fake data, and the ticket lists every cut.

**Tests first:** the copied Go component tests pass unchanged against the new event wire (a fixture feed renders the same rows, groups and diff bodies as Go); compact and Classic modes both render; the composer sends, stops and attaches as in Go; the context and model/harness indicators render and a press changes nothing; no switcher, background-tool indicator, notice card or git panel is in the DOM.

**Open conflict to settle before this ticket starts:** phase 1's simple feed already shows steer and background-exit notice rows (acceptance step 4 needs the User to see how a notice was delivered). If phase 2 drops notices from the UI, that information needs a different home.
