# AGENTS.md — crates/prompts

## Purpose

Model-facing text as files (`include_str!` defaults under `prompts/*.md`):
- IDs: `ponytail`, `caveman`, `policy`, `compress`, `decompress`, `search_context`, `acp_status.description`, `compress.prompt`, the `tool.*.description` ids, `skill.codebase-memory`, and `nudge.first`, `nudge.pressure`, `nudge.emergency`, `nudge.tier2`, `nudge.tier3`.
- `PromptStore`: embedded default, override `<config root>/prompts/<id>.md`, empty override falls back to the default.
- `{{ name }}` placeholders validated at load against `project_name`, `project_root`, `level`, `date`, `session_id`, and the nudge-only `context_breakdown`, `since_last_nudge`, `compressible_ranges`; unknown names fail.
- `PinnedPrompts::pin`: renders and hashes every id once per live Session runtime. A daemon restart pins the current prompt store again; persisted hashes do not restore old prompt text. The nudge ids pin the raw template (their per-request lines are not known at pin time) and the proxy renders it with `render_text` for each nudge.
- `nudge.emergency` has no file of its own: its default is the `nudge.pressure` body (the reference wording is identical for both bands).
- `PromptWatcher`: polls the override directory, keeps the last good text on a broken file, reports errors to the callback.
- `skill.codebase-memory`: bounded search and caller-tracing guidance for the five graph tools, with fully qualified dispatch names and on-demand `tools.help` instead of duplicated schemas. The proxy appends it only when the graph server is usable; the tools crate fills in `project`. Static results require verification against current files.
- `filter_skill`: strips a skill's frontmatter and keeps only the requested level's intensity row and example lines.
- `ponytail_text`: that filter for the Session's `ponytail_level`; `off` yields empty text.
- `caveman_text`: the emberloop-go voice register, always ultra: banner, the filtered `caveman.md` (a byte-for-byte copy of the `caveman` plugin's `SKILL.md`), then its one-line reminder.
- `policy` owns the shared edit/check/spill-search workflow: hashline edits for existing files, sequential patches after batched reads, preserved check exit status, and `ctx_search` versus compressed-summary `search_context`. `tool.edit` demonstrates valid replacement syntax and recovery; `tool.execute` routes mutations through `tools.edit` and distinguishes Python aliases from fully qualified MCP dispatch names.

## Attribution

`nudge.first.md` and `nudge.pressure.md` adapt the nudge wording of billion-context (MIT) and its kernel acp-kernel (MIT), taken from the commit in `crates/proxy/parity/BASELINE`. The protocol paragraph and the range and breakdown lines are rewritten for turn indices and placeholders. The licence notice is the reference repository's `LICENSE`; the .md files carry no notice.

## Tests

`src/tests.rs` covers override precedence, deletion restoring the default, placeholder rejection, last-good on broken files, pinning across reloads, one hash per id, Ponytail and caveman level filtering and watcher reload, the nudge ids with their placeholders, a pinned nudge keeping its raw template, and edit/graph workflow guidance.

## Child DOX Index

(none)
