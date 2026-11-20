# AGENTS.md — crates/prompts

## Reviewer subagents

`reviewer.spec` and `reviewer.standards` are the authoritative pinned reviewer briefs, adapted from the original `code-review` skill. The skill dispatches assignments without duplicating these prompts. The five `tool.{spawn,steer,stop,result,submit}.description` ids describe the shared Subagent tools (`spec`, `standards`, `worker`, `scout`); register every prompt in `PROMPT_IDS` and `default_text`.


## Purpose

Model-facing text as files (`include_str!` defaults under `prompts/*.md`):
- IDs: `ponytail`, `caveman`, `superpowers`, `policy`, `compress`, `decompress`, `search_context`, `acp_status.description`, `acp_status.report`, `todo.reminder`, `compress.prompt`, the `tool.*.description` ids, `skill.catalog`, and `nudge.first`, `nudge.pressure`, `nudge.emergency`, `nudge.tier2`, `nudge.tier3`.
- `PromptStore`: embedded default, override `<config root>/prompts/<id>.md`, empty override falls back to the default.
- `{{ name }}` placeholders validated at load against `project_name`, `project_root`, `level`, `date`, `session_id`, the nudge-only `context_breakdown`, `since_last_nudge`, `compressible_ranges`, the `acp_status.report` `status_*` figures, and the `todo.reminder` `reminder_sent`, `reminder_max`, `open_todos`; unknown names fail.
- `PinnedPrompts::pin`: renders and hashes every id once per live Session runtime. A daemon restart pins the current prompt store again; persisted hashes do not restore old prompt text. The nudge, `acp_status.report` and `todo.reminder` ids pin the raw template (their per-request values are not known at pin time) and render it with `render_text` per request: the proxy for the nudge and the status report, the daemon for the reminder, which reads the live store because the daemon holds no Session pin.
- Skill id: `skill.catalog` (heading and instruction of the frozen Session catalog; the daemon appends the skill lines). It takes no placeholders. `tool.skill_search.description` describes the skill tool; its name is in `OFFERED_TOOLS`.
- `nudge.emergency`: its default is `nudge.emergency.md` (the emergency header, which says the proxy truncated tool results) followed by the `nudge.pressure` body, joined in `defaults.rs`. An override file replaces the whole text.
- `PromptWatcher`: polls the override directory, keeps the last good text on a broken file, reports errors to the callback.
- `filter_skill`: strips a skill's frontmatter and keeps only the requested level's intensity row and example lines.
- `superpowers`: a pointer, not a copy, at the installed ember skill pack (`el://skills/ember-flow/SKILL.md`, which names the `ember-<stage>` skill to read next; source in `vendor/ember/`, installed by `scripts/install-ember`). The id keeps its old name. The skill is the single source of truth, so a skill update needs no prompt change; it also says a skill's scripts run through `bash` with their absolute path, because the shell starts at the project root. Wire order is `policy`, `ponytail`, `caveman`, `superpowers`, then the Session's frozen skill catalog. `skill.codebase-memory` was retired: code discovery is `find` (graph-assisted `architecture` and `trace`) with `execute` as the complex-query escape hatch.
- `ponytail_text`: that filter for the global `ponytail_default_level` setting; `off` yields empty text.
- `caveman_text`: the emberloop-go voice register, always ultra: banner, the filtered `caveman.md` (a byte-for-byte copy of the `caveman` plugin's `SKILL.md`), then its one-line reminder.
- `policy` owns the shared edit, required-check and spill-search workflow: hashline edits for existing files, sequential patches after batched reads, preserved check exit status, and `ctx_search` (spilled tool output) versus `search_context` (compressed history), the search-before-`decompress` rule and the protected-content rule for `compress` (system prompts and `<system-reminder>` content never fold), which only `policy` states. `tool.edit` demonstrates valid replacement syntax, the range-body counting rule, re-reading after each patch and whole-function repair of a damaged span; `tool.execute` routes mutations through `tools.edit` and distinguishes Python aliases from fully qualified MCP dispatch names. `compress.prompt` also tells the summary to keep the pending step, files still to edit, proven commands and open questions.
- `policy` also has `Showing files`: agents name a file as a `file://` Markdown link or image so the web UI renders it (`crates/api/src/files.rs` serves it from the Project root or Session directory only).
- `tool.spawn` and `policy` teach `spawn(batch=...)`: siblings in one batch report together behind one `Batch "<name>" completed (n/n):` notice and one wake. `ARGUMENTS` in `src/tests.rs` includes `batch`.

## Attribution

`nudge.first.md` and `nudge.pressure.md` adapt the nudge wording of billion-context (MIT) and its kernel acp-kernel (MIT), taken from the commit in `crates/proxy/parity/BASELINE`. The protocol paragraph and the range and breakdown lines are rewritten for turn indices and placeholders. The cache-cost line in `nudge.first.md` is new text: a fold restarts the provider prompt cache, so the model learns to space folds by about 50,000 tokens of new context. The question-list clauses are new text too: `compress.md` and both nudge templates say a numbered assistant question list that a later turn answers by number keeps its question texts with the numbers and the answers beside them, and the `DROP` bullet says such a list is not back-and-forth discussion. `nudge.emergency.md` is new text, and the `nudge.pressure` body it concatenates carries the same clauses. The licence notice is the reference repository's `LICENSE`; the .md files carry no notice.

## Tests

`src/tests.rs` covers override precedence, deletion restoring the default, placeholder rejection, last-good on broken files, pinning across reloads, one hash per id, Ponytail and caveman level filtering and watcher reload, the nudge ids with their placeholders, a pinned nudge keeping its raw template, and edit/graph workflow guidance. It also pins the offered tool list (`OFFERED_TOOLS`, mirroring the tools crate's private `tool_specs`, which this crate cannot depend on): each offered tool must be backticked in `policy` or have a `tool.<name>.description` id, and backticked bare identifiers in model text must name a tool, a proxy tool, a `compress`/`search_context` argument or a listed shell command. A new harness tool goes into that list.

## Child DOX Index

(none)
