# AGENTS.md — crates/prompts

## Reviewer subagents

`reviewer.spec` and `reviewer.standards` are the authoritative pinned reviewer briefs, adapted from the original `code-review` skill. The skill dispatches assignments without duplicating these prompts. The five `tool.{spawn,steer,stop,result,submit}.description` ids describe the shared Session tools; register every prompt in `PROMPT_IDS` and `default_text`.


## Purpose

Model-facing text as files (`include_str!` defaults under `prompts/*.md`):
- IDs: `ponytail`, `caveman`, `policy`, `compress`, `decompress`, `search_context`, `acp_status.description`, `acp_status.report`, `todo.reminder`, `compress.prompt`, the `tool.*.description` ids, `skill.codebase-memory`, `skill.catalog`, `review.skills`, and `nudge.first`, `nudge.pressure`, `nudge.emergency`, `nudge.tier2`, `nudge.tier3`.
- `PromptStore`: embedded default, override `<config root>/prompts/<id>.md`, empty override falls back to the default.
- `ProposalStore` (`proposals.rs`): the User-approval gate for model-written prompt edits. One pending `Proposal { id, text, reason, base_hash }` per prompt id in `<config root>/prompt-proposals/<id>.json` (a sibling of `prompts/`); a newer proposal replaces the older. `propose` refuses an unknown id (`NotFound`), empty text or reason, text equal to the active text (`Invalid`) and bad placeholders; it never touches the override. `approve` calls `set_override` then drops the proposal; `Proposal::is_stale` is true when the active hash no longer equals `base_hash`, and a stale proposal still approves. A running Session keeps its pinned text. `tool.prompt_propose.description` describes the `prompt_propose` tool that writes proposals.
- `{{ name }}` placeholders validated at load against `project_name`, `project_root`, `level`, `date`, `session_id`, the nudge-only `context_breakdown`, `since_last_nudge`, `compressible_ranges`, the `acp_status.report` `status_*` figures, and the `todo.reminder` `reminder_sent`, `reminder_max`, `open_todos`; unknown names fail.
- `PinnedPrompts::pin`: renders and hashes every id once per live Session runtime. A daemon restart pins the current prompt store again; persisted hashes do not restore old prompt text. The nudge, `acp_status.report` and `todo.reminder` ids pin the raw template (their per-request values are not known at pin time) and render it with `render_text` per request: the proxy for the nudge and the status report, the daemon for the reminder, which reads the live store because the daemon holds no Session pin.
- Skill ids: `skill.catalog` (heading and instruction of the frozen Session catalog; the daemon appends the skill lines) and `review.skills` (the background skill review prompt; the proxy renders it for the fork). Neither takes placeholders. `tool.skill_search.description` and `tool.skill_manage.description` describe the two skill tools; both names are in `OFFERED_TOOLS`.
- `nudge.emergency`: its default is `nudge.emergency.md` (the emergency header, which says the proxy truncated tool results) followed by the `nudge.pressure` body, joined in `defaults.rs`. An override file replaces the whole text.
- `PromptWatcher`: polls the override directory, keeps the last good text on a broken file, reports errors to the callback.
- `skill.codebase-memory`: bounded search and caller-tracing guidance for the five graph tools, with fully qualified dispatch names and on-demand `tools.help` instead of duplicated schemas. The proxy appends it only when the graph server is usable; the tools crate fills in `project`. Static results require verification against current files.
- `filter_skill`: strips a skill's frontmatter and keeps only the requested level's intensity row and example lines.
- `ponytail_text`: that filter for the global `ponytail_default_level` setting; `off` yields empty text.
- `caveman_text`: the emberloop-go voice register, always ultra: banner, the filtered `caveman.md` (a byte-for-byte copy of the `caveman` plugin's `SKILL.md`), then its one-line reminder.
- `policy` owns the shared edit, required-check and spill-search workflow: hashline edits for existing files, sequential patches after batched reads, preserved check exit status, and `ctx_search` (spilled tool output) versus `search_context` (compressed history), the search-before-`decompress` rule and the protected-content rule for `compress` (system prompts and `<system-reminder>` content never fold), which only `policy` states. `tool.edit` demonstrates valid replacement syntax, the range-body counting rule, re-reading after each patch and whole-function repair of a damaged span; `tool.execute` routes mutations through `tools.edit` and distinguishes Python aliases from fully qualified MCP dispatch names. `compress.prompt` also tells the summary to keep the pending step, files still to edit, proven commands and open questions.

## Attribution

`nudge.first.md` and `nudge.pressure.md` adapt the nudge wording of billion-context (MIT) and its kernel acp-kernel (MIT), taken from the commit in `crates/proxy/parity/BASELINE`. The protocol paragraph and the range and breakdown lines are rewritten for turn indices and placeholders. The cache-cost line in `nudge.first.md` is new text: a fold restarts the provider prompt cache, so the model learns to space folds by about 50,000 tokens of new context. `nudge.emergency.md` is new text. The licence notice is the reference repository's `LICENSE`; the .md files carry no notice.

## Tests

`src/tests.rs` covers override precedence, deletion restoring the default, placeholder rejection, last-good on broken files, pinning across reloads, one hash per id, Ponytail and caveman level filtering and watcher reload, the nudge ids with their placeholders, a pinned nudge keeping its raw template, and edit/graph workflow guidance. It also covers proposals: store, replace, refusals, staleness, approval (override written, proposal removed, a pinned Session unchanged) and not-found errors. It also pins the offered tool list (`OFFERED_TOOLS`, mirroring the tools crate's private `tool_specs`, which this crate cannot depend on): each offered tool must be backticked in `policy` or have a `tool.<name>.description` id, and backticked bare identifiers in model text must name a tool, a proxy tool, a `compress`/`search_context` argument or a listed shell command. A new harness tool goes into that list.

## Child DOX Index

(none)
