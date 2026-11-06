# AGENTS.md — crates/prompts

## Purpose

Model-facing text as files (`include_str!` defaults under `prompts/*.md`):
- IDs: `ponytail`, `caveman`, `policy`, `compress`, `decompress`, `search_context`, `acp_status.description`, `compress.prompt`, the `tool.*.description` ids, and `nudge.first`, `nudge.pressure`, `nudge.emergency`, `nudge.tier2`, `nudge.tier3`.
- `PromptStore`: embedded default, override `<config root>/prompts/<id>.md`, empty override falls back to the default.
- `{{ name }}` placeholders validated at load against `project_name`, `project_root`, `level`, `date`, `session_id`, and the nudge-only `context_breakdown`, `since_last_nudge`, `compressible_ranges`; unknown names fail.
- `PinnedPrompts::pin`: renders and hashes every id once per Session; a Session never reads a changed prompt again. The nudge ids pin the raw template (their per-request lines are not known at pin time) and the proxy renders it with `render_text` for each nudge.
- `nudge.emergency` has no file of its own: its default is the `nudge.pressure` body (the reference wording is identical for both bands).
- `PromptWatcher`: polls the override directory, keeps the last good text on a broken file, reports errors to the callback.
- `filter_skill`: strips a skill's frontmatter and keeps only the requested level's intensity row and example lines.
- `ponytail_text`: that filter for the Session's `ponytail_level`; `off` yields empty text.
- `caveman_text`: the emberloop-go voice register, always ultra: banner, the filtered `caveman.md` (a byte-for-byte copy of the `caveman` plugin's `SKILL.md`), then its one-line reminder.

## Attribution

`nudge.first.md` and `nudge.pressure.md` adapt the nudge wording of billion-context (MIT) and its kernel acp-kernel (MIT), taken from the commit in `crates/proxy/parity/BASELINE`. The protocol paragraph and the range and breakdown lines are rewritten for turn indices and placeholders. The licence notice is the reference repository's `LICENSE`; the .md files carry no notice.

## Tests

`src/tests.rs` covers override precedence, deletion restoring the default, placeholder rejection, last-good on broken files, pinning across reloads, one hash per id, Ponytail and caveman level filtering and watcher reload, the nudge ids with their placeholders, and that a pinned nudge keeps its raw template.

## Child DOX Index

(none)
