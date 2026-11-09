# P2-2: Skills (lazy catalog, `skill_search`, `skill_manage`, background review)

## Status (2026-11-08)

Implemented end to end, steps 1 to 7 in order, in one pass. The per-step User review gate was not applied: the User asked for the whole ticket. Where things landed:

- Setting `skills_review_interval` (default 5): `crates/api/src/state.rs`; `openapi.json` and `web/src/lib/api/schema.d.ts` regenerated.
- Global root: `Config.global_skills`, `--global-skills` on `mcp`, `ToolConfig.global_skills`, `ProjectJail::with_extra_root`.
- `skill_search`: `crates/tools/src/skillsearch.rs`, scoring in `bm25.rs`. `ctx_search` output and tests unchanged.
- `skill_manage`: `crates/tools/src/skillmanage.rs`, file helpers in `skillstore.rs`; `view` log in `read.rs`.
- Discovery and catalog: `crates/tools/src/skills.rs`; frozen by `SessionRunner::freeze_catalog`; injected by `SessionRuntime` and `prompt_text`.
- Review: `crates/proxy/src/review.rs` and `SseState::review_check` in `server.rs`; the executor in `crates/daemon/src/review.rs`.
- Prompt ids: `skill.catalog`, `review.skills`, `tool.skill_search.description`, `tool.skill_manage.description`.

Deviations from the text below:

- The review trigger lives in the proxy and reads `skills_review_interval` from Settings.
- `discover(global, project)` takes the project directory and joins `.agents/skills`. It landed with `skill_search` (step A), since search needs it.
- A `create` or `write_file` marks the bytes it wrote as seen, so a patch right after it needs no `read`. A file the Session did not write needs one `read` first.
- The `skill_manage` description drops "delete": v1 has no delete op.
- The `review.skills` prompt keeps the Hermes wording, with `skill_view` changed to `read`, `skills_list` to `skill_search`, the curator-lock clause removed, and non-agent skills never patched.
- The usage line per `skill_manage` call takes the name and op of its first op.

Live test: `crates/daemon/tests/skills_live.rs` (`#[ignore]`). Run it with `LIVE_CLAUDE_HOME=<logged-in claude config dir> cargo test -p emberloop-daemon --test skills_live -- --ignored`. It passed 3 of 3 runs. The test copies the login files into a temp config dir and never touches `~/.agents/skills`.

Out of scope, unchanged: curator, install-time scan, install command, BM25 top-N catalog, dense search, UI.


## Goal

Give a Session reusable skills (SKILL.md folders) that cost almost nothing until used, and let the agent improve the skills it wrote itself. Prior art chosen by the User: Hermes Agent (`NousResearch/hermes-agent`, studied at commit 8422e54d, read only, never run) for self-improvement; the agentskills.io client guide for discovery and lazy loading. "Build ourselves, copying the Hermes design" is the decision; no new dependency.

## Evidence that shapes the design

- SkillsBench (arXiv 2602.12670): curated skills help (+16.6 pp), self-generated skills hurt (-8 to -11 pp), 2-3 focused skills beat 4+. So: small focused skills, and the review must prefer patching an existing skill over creating one.
- Vercel eval: an unprompted skill is never invoked in 56% of cases; a one-line instruction plus an always-visible index fixes most of it. So: a frozen catalog in the system prompt plus one instruction line.
- arXiv 2608.06196: BM25 alone gets hit@5 50%; fine for a library under ~100 skills as a fallback to the catalog, not as the only path.
- SoK Agentic Skills (arXiv 2602.20867): marketplace skills are an attack surface. So: origin tiers, and the agent never edits skills it did not write.
- All of the above are secondary-source summaries read via the web; the arXiv papers were not read in full.

## Decisions (User approved 2026-11-08)

1. `skill_search` and `skill_manage` are normal MCP tools in `crates/tools`. The proxy never runs them for the model's main turn.
2. Review trigger is the Hermes counter, interval 5 (Settings field `skills_review_interval`, 0 disables), not every turn.
3. Only skills with `metadata.origin: agent` in frontmatter are writable by `skill_manage`. A skill with no origin is human. Downloaded skills carry `origin: downloaded`. Human and downloaded skills change only through the normal `edit` tool when the User asks.
4. The review is a hidden fork issued by the proxy, not an extra Session turn. The proxy sees every request and response and holds the client's auth headers while a request is in flight, so it can send the fork; the daemon cannot (it holds no credentials). The proxy crate has no tools dependency: the daemon injects an executor (`ReviewTools`) that runs the whitelisted tool calls. Root `AGENTS.md` states this exception. The fork reuses the last request body, so the provider cache applies to the prefix.

## Layout and format

- Global: `~/.agents/skills/<name>/SKILL.md`. Project: `<project>/.agents/skills/<name>/SKILL.md`. Optional `references/`, `templates/`, `scripts/`, `assets/`. Project wins on a name collision; log a warning.
- Frontmatter: `name` (lowercase `[a-z0-9][a-z0-9._-]*`, max 64), `description` (max 150 chars in the catalog; the file may hold up to 1024, the catalog clips), `metadata.origin`.
- Limits copied from Hermes: SKILL.md 100,000 chars, supporting file 1 MiB.
- `skill_manage` creates under global by default; `scope: "project"` creates under the project.

## Pieces

### A. `skill_search` (crates/tools)

- Move `tokens` and the BM25 scoring in `crates/tools/src/ctxsearch.rs` into a shared private module (`bm25.rs`) generic over documents; `ctx_search` behaviour and tests stay byte-identical.
- New `skillsearch.rs`: one document per skill = name + description + body headings + first 2 KB of body. Args: `query` (string), `limit` (default 5, max 20). Output per hit: `name`, origin, clipped description, absolute path of SKILL.md (the model then calls `read`).
- Scans both skill roots on every call (no cache; library is small). Reuse the discovery function from piece C.
- Prompt text `tool.skill_search.md` in `crates/prompts/prompts`; register in `defaults.rs`, `lib.rs`, `mcp.rs` specs and `lib.rs` dispatch, like `ctx_search`.

### B. `skill_manage` (crates/tools)

Port of `tools/skill_manager_tool.py`. Ops array, one call may hold several ops, applied atomically with rollback:

- `create` (full SKILL.md; forces `metadata.origin: agent`), `patch` (`old_string`/`new_string`, must match exactly once unless `replace_all`; whole-file `content` as last resort), `write_file`, `remove_file`. Supporting-file paths: relative, first segment one of `references`, `templates`, `scripts`, `assets`, no `..`, no symlink escape.
- No `delete` in v1 (the Hermes review prompt also says never delete).
- Refusal rules: target skill origin is not `agent`; invalid name or frontmatter; size over limit; description over 1024.
- Per-skill exclusive lock (`File::lock` on `<skills root>/.locks/<sha256(name)>.lock`, outside the skill dir), held across the whole read-modify-write.
- Backup: before any change copy the previous file to `<skills root>/.backup/<name>/<unix-seconds>-<file>`; keep the last 10 per skill.
- Read-before-write guard: `patch` and `write_file` over an existing file are refused unless the sha256 of that file's current bytes is in the Session seen store (`Toolbox::seen().unseen(path, hash, &[])` returns `Some`; `crate::tag::content_hash` computes the hash). `read`, `grep`, `find` and `edit` already record views there, so a `read` of the file satisfies it, and a file changed since the view is refused. This is the same stale-view rule `edit` uses; no new bookkeeping. Refusal text: `Read <path> with read before changing it.`
- Usage log: append one JSON line per `skill_manage` call to `<skills root>/.usage.jsonl` (time, name, op, result). `read.rs` also appends a `view` line when the resolved path is a `SKILL.md` under a skill root. The catalog order (piece C) reads these lines; a later curator can too (the curator itself is out of scope).
- Tool description copied from Hermes `_skill_manage_description`, with the "Existing skills are modified wherever they live" sentence removed and the origin rule added.

### C. Discovery and frozen catalog (crates/tools + daemon + proxy + prompts)

- `skills::discover(global_root, project_root) -> Vec<Skill>` in `crates/tools`; skips unreadable or malformed skills with a warning, never fails the Session. It follows symlinked skill directories (tools such as `npx skills` install by symlink).
- Catalog text: the `skill.catalog` prompt (new id in `crates/prompts`, no placeholders; heading plus one instruction: check the catalog before starting a task, read the SKILL.md of a match with `read`, use `skill_search` when nothing matches), then one line per skill, `- <name>: <description clipped to 150>`, project skills first, then by most recent `view` in the usage log, capped at 50 lines; a final line `N more skills: use skill_search` when over the cap. No skills gives an empty file and nothing is injected.
- Frozen: `SessionRunner::ensure` (`crates/daemon/src/runner.rs:176`) writes `<session dir>/skill-catalog.md` before it spawns the harness, only when the file is absent (resume keeps it). The rendered text, including the prompt wording, is the file. A Session created before this feature gets its file on its next `ensure` and pays one prompt-cache miss, once.
- Injection: `SessionRuntime::new` (`crates/proxy/src/server.rs:197`) reads `splices_file.with_file_name("skill-catalog.md")` into a `catalog: String` field, the same pattern as `nudge.json`. `wire::prompt_text` and `inject_prompts_for_wire` (`crates/proxy/src/wire.rs:40,59`) take a `catalog: &str` and push it after the `SKILL_CODEBASE_MEMORY` part, so it lands in the top-level system field of all four wires. The runtime is per Session (token to session id to `ProxyState::runtime`), so nothing global is added. The catalog is never edited after the runtime reads it, so the system prompt is byte-stable across requests and across a daemon restart. System text is protected content (`protect.rs`), so compression never touches it.
- A harness never composes the text. A `read` result of a SKILL.md is an ordinary tool result: it can be folded by `compress` like any other, and the model re-reads the skill when it needs it again. Protecting it would need a per-path rule that `protect.rs` and `CompressPolicy.protectedTools` (tool names only) do not have, and it is out of scope.

### E. Access to the global skills root (crates/tools + daemon)

- Problem: `jail.rs` refuses any path outside the Project root, so `read`, `grep`, `find`, `edit` and `write` cannot reach `~/.agents/skills`. Second problem: the MCP process runs with the harness's pi-home as `HOME` (`crates/daemon/src/lib.rs:672` comment), so the tools cannot derive the real home themselves.
- `Config.global_skills` in the daemon (default `$HOME/.agents/skills` of the daemon process, computed next to `default_config_root`, `lib.rs:242`). The daemon passes it to the MCP subcommand as `--global-skills <path>` in `harness_wiring` (`lib.rs:566`) and to `run_mcp` (`lib.rs:670`); `toolbox_for` (`lib.rs:529`) and `SessionRunner` get it too. Standalone `emberloop-mcp` without the flag has no global root.
- `ToolConfig.global_skills: Option<PathBuf>` (default `None`). `ProjectJail::with_extra_root` accepts a path when its resolved form is under the Project root, or under the canonical global skills root, or under the canonical target of a direct child of that root (a symlinked skill). The children are listed per call; the library is small. Everything else stays refused, symlinks included.
- `skill_manage` and `discover` take the roots from the `Toolbox` and do their own path checks; they do not rely on the jail.

### D. Background skill review (proxy fork, daemon executor)

Port of `agent/turn_iteration_prep.py:166`, `agent/turn_finalizer.py:864`, `agent/background_review.py`, with decision 4 (a proxy-issued fork).

- New `crates/proxy/src/review.rs`: `ReviewState { count, reviewing }` with `round_ended(forwarded_names, interval) -> bool` and `finished()`, the `ReviewTools` trait, and per-wire helpers (`offers_skill_tool`, `fork_request`, `calls`, `append_round`, `text`). The state lives in memory on `SessionRuntime`; a daemon restart resets the count.
- Counting: `SseState` records the names of the harness calls it forwards in each round. A round that forwards a call adds 1; a round with a `skill_manage` call resets the count. A round that forwards nothing is a final answer: it fires when `skills_review_interval > 0`, the count reached the interval, no review runs, the request's tools offer `skill_manage` (side requests such as title generation do not), and the daemon injected an executor. Counting rounds matches Hermes.
- Fork: the exact body of the last upstream request plus one user message with the `review.skills` prompt, `stream` false (Google: `:generateContent`, `alt=sse` removed). Up to 16 rounds. Calls run through `ReviewTools`; the proxy appends the assistant message and results in each wire's native shape. The client's auth headers are held in memory for the task only and dropped after.
- Executor: `crates/daemon/src/review.rs` builds a `Toolbox` for the Session and allows only `skill_manage`, `skill_search`, `read`, `grep` and `find`. Any other call returns `Background review denied non-whitelisted tool: {name}. Only skill tools and read-only file tools are allowed.`
- Output: nothing reaches the Session history or the UI, except one `compression` event with action `skill-review` when a `skill_manage` call succeeded. Usage per round goes to `<session>/review.jsonl`.
- Review prompt `review.skills` (`crates/prompts/prompts/review.skills.md`, no placeholders): copy `_SKILL_REVIEW_PROMPT` from `agent/background_review.py:456` plus `_LESSON_LAYER_BLOCK` and `_DO_NOT_CAPTURE_BLOCK`, then change: open with "Skill review. Use only skill_manage, skill_search and read. Do not use any other tool, and do not continue the user's task."; remove the clause that lets the reviewer edit any skill; skills whose origin is not `agent` are never patched, put a suggested change in the reply instead; keep "prefer patch over create", "class-level names only" and "Nothing to save." as a real option. The final reply is one or two lines.
- Not ported: the curator, the cache-scope plumbing of Hermes (the fork reuses the request bytes, so the provider cache applies without it).
- Setting: `Settings.skills_review_interval: u32`, default 5 (`crates/api/src/state.rs`, next to `todo_reminders_max`). Adding it changes the generated files: run `cargo run -p api --bin dump_openapi` and `bun run generate:api` in `web/` (the `generated` stage of `scripts/check` diffs `openapi.json` and `web/src/lib/api/schema.d.ts`), add the field to `web/src/lib/api/types.ts` and the default in `web/src/routes/+layout.svelte`. No settings UI control in v1.
- Prompt-crate bookkeeping: add `skill.catalog`, `review.skills`, `tool.skill_search.description` and `tool.skill_manage.description` to `crates/prompts/src/lib.rs` (ids list, match), `defaults.rs`, and add `skill_search` and `skill_manage` to `OFFERED_TOOLS` in `crates/prompts/src/tests.rs`; update `crates/prompts/AGENTS.md`.

## Order (each step test-first; lands only with `scripts/check` green; User reviews each step before the next)

1. Settings field `skills_review_interval` and the generated files (the small, mechanical change that proves the loop).
2. E: `Config.global_skills`, `--global-skills`, `ToolConfig.global_skills`, jail extra root, with jail tests.
3. A: `bm25.rs` extraction (existing `ctxsearch` tests must pass unchanged), then `skill_search` and its prompt, MCP spec, dispatch, `OFFERED_TOOLS`.
4. B: `skill_manage`, one op at a time, each with its failing test first; `read.rs` view log.
5. C: `discover`, catalog file in `ensure`, proxy field and `prompt_text` change, prefix-stability test.
6. D: `review.rs` in the proxy (rules and fork loop) and in the daemon (executor), then the prompt.
7. Docs: update the owning `AGENTS.md` files and indexes for every changed crate (tools, daemon, proxy, prompts, api, root rule text unchanged); update this ticket's facts.

## Tests first

- BM25: `ctx_search` outputs unchanged; `skill_search` ranks a rare-term skill above a common-term one; empty query errors; both roots scanned; project overrides global on name collision.
- `skill_manage`: create forces origin agent; patch on a human or downloaded skill is refused and leaves the file byte-identical; patch with zero or two matches is refused; `..`, absolute and symlink-escape paths are refused; name and size limits; a failing second op rolls back the first; two concurrent patches to one skill both land or one is refused, never a torn file; backup file exists and keeps only the last 10; patch without a prior `read` is refused and succeeds after the `read`.
- Catalog: 51 skills give 50 lines plus the "N more" line; description clipped at 150; a skill created mid-Session is absent from the catalog file and from every later request of that Session (prefix-stability test in `crates/proxy/src/tests.rs`), present in the next Session; a malformed skill is skipped with a warning.
- Review: `ReviewState::round_ended` adds 1 per round that forwards a harness call, resets on `skill_manage`, and fires at a final answer once the count reaches the interval; a request whose tools do not offer `skill_manage` never fires; the fork body keeps the `system`, `tools` and `messages` prefix byte for byte, adds one user message, and sets `stream` false; a `bash` call in the fork is denied with the whitelist message and a `skill_manage` call runs through the executor (`crates/proxy/src/tests.rs`, `crates/daemon/src/review.rs`).
- Jail and roots: a path under the global skills root and a symlinked skill whose target is elsewhere are readable; a sibling directory of the global root, `..` escapes and a symlink to an unrelated directory are refused; `run_mcp` with `--global-skills` exposes the root, without it does not.

## Out of scope (v1)

Curator (stale and archive pass), install-time security scan of downloaded skills, a skill install command, BM25 top-N catalog by first message, dense or hybrid search, a UI for skills.

## Risks

- Origin lives in frontmatter, so a model with the `edit` tool could forge `origin: agent` on a human skill. Accepted for v1; the backup folder limits damage.
- Self-generated skills can lower quality (SkillsBench). The usage log exists so a later curator can find skills never viewed.
- The review is one extra model turn after every five tool calls that end a turn normally, sitting in history; interval 0 turns it off. The reviewer can in principle call a non-skill tool because no whitelist exists; the prompt forbids it and the origin gate protects human skills.
- The tool-call count approximates Hermes's iteration count.
- Descriptions of other agents' skills in the global root are read and clipped by `discover`; their bodies are only read on demand. Install-time scanning of downloaded skills is out of scope, so a downloaded skill is trusted as much as any file the User put there.
