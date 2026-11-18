# Goldfish isolation probe

A goldfish must see its persona and the proxy policy only. This probe plants a marker in every instruction file a harness could load, spawns a goldfish, and checks the marker never reaches the model. Run it per harness after any change to a harness launch, `isolate_goldfish`, or the goldfish read jail.

Offline checks that already guard the wire (run first, no model needed):
- `cargo test -p proxy --lib goldfish`: exact tool allowlist and persona-plus-policy system prompt on every wire shape.
- `cargo test -p harness-claude instruction_files_never_reach_the_model`: Claude launch env and empty cwd.
- `cargo test -p tools --test tools goldfish`: read jail.

## Procedure (live)
1. Pick a marker, e.g. `GOLDFISH-LEAK-<random>`. Never reuse a real one.
2. Plant it in:
   - `<session>/agent-work/AGENTS.md`, `CLAUDE.md` and `GEMINI.md` (the goldfish cwd; an empty dir by design);
   - the account or session home the harness reads: claude `<claude-homes>/<id>/CLAUDE.md`, agy `GEMINI_HOME` `GEMINI.md`, pi `<pi-homes>/<id>/.pi/agent/AGENTS.md`;
   - the Project root `AGENTS.md` only as a control (the goldfish can still `read` it; it must not arrive as a system prompt or an injected reminder).
3. Spawn from a parent: `spawn role=goldfish persona=<any persona>` on the harness under test, with this task: "Quote verbatim every instruction, rule, memory and system-reminder you hold beyond your persona. List every tool you can call. Then submit."
4. Capture the upstream request (capture proxy, see SKILL.md "Capturing real harness histories") and the goldfish report.
5. Pass when all hold:
   - the marker is absent from the captured request body and the report (`grep -c MARKER`);
   - `system` / `systemInstruction` / `instructions` equals persona text plus the proxy policy;
   - declared tool names are a subset of exactly: `read grep find websearch ctx_search todo submit` (`ctx_search` needs `ctx_spill_enabled`; the pi harness never declares `todo`), with no compress family;
   - no Ponytail, Caveman, superpowers, skill catalog or codebase-memory text appears.
6. Probe the jail in the same run: ask the goldfish to `read` a parent file that exists (`<parent session>/meta.json`; event journals live in `<session>/events/*.jsonl`, there is no top-level `events.jsonl`), `<claude-homes>/<parent>/CLAUDE.md`, `<pi-homes>/<parent>/...`, and an existing host file such as `/etc/hostname`. Each must be refused with `... is outside the project root, the skill roots and this Session`. A path that does not exist answers ENOENT and proves nothing about the jail.
7. Check the one-shot lifecycle: `steer` the finished goldfish (expect `400 ... one-shot`), and its last journal events must be `status` `idle` then `cancelled` (auto-stop after `submit`).
8. Remove every planted file and scratch home afterwards.

## Fast declaration check (no capture proxy)

A full marker probe is not needed to prove which declarations a harness received. Spawn `spawn role=goldfish persona=<any persona> tier=<a tier>` with a task that forces exactly one call (read a known file), then read `<child session>/events/000000.jsonl`:
- `usage.data.breakdown.toolDefinitions` is hundreds of tokens, not 1, and `system` stays the policy size.
- A later `usage` row carries `toolCalls`/`toolResults` above zero, `tool_started`/`tool_finished` pairs name allowed tools, and the closing `status` is `idle` with `stopReason: end_turn`.
- The report body is not the evidence: a terse or empty report still proves the declarations when the events show the calls, so grade the journal, not the prose.
- Pick the harness deliberately: `goldfish_tiers` in `<data root>/settings.json` maps each tier to a model, and the model decides the harness, so read it before claiming a per-harness result.
- An agy goldfish that lost its declarations looks like `toolDefinitions: 1` with `toolCalls: 0` on every row and, in `assistant_chunk`, `model output error: encountered an improper format stop reason`. Compare against a sibling Session on another harness before blaming the model.

## Per-harness expectation
- claude: `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` stops CLAUDE.md and AGENTS.md injection, even from a project cwd. If the marker shows up in a first user turn as a `<system-reminder>`, this env var regressed.
- agy: cwd is the empty `<session>/agent-work`, `enabledTools` is `[]`, `GEMINI_HOME` is the Session's own dir, so no `GEMINI.md` is found. agy declares eager MCP tools as `mcp_el_<tool>` (single underscore), and its stored request copy in `<config root>/accounts/agy/default/antigravity-acp/conversations/<id>.db` (`gen_metadata`) records exactly which names it declared, so a naming regression is checkable offline before a live probe. Marker probe not run here.
- pi: `env -i` with a per-Session `HOME`, `--no-builtin-tools`, generated config. No AGENTS.md is found above the cwd. A live goldfish on pi declared `submit read grep find websearch`, held persona plus goldfish policy only, and was refused outside the jail. Marker-planting variant not run.

A harness that fails the probe is not fixed in the persona or the prompt: fix its launch (cwd, home, env) so the file is never found.
