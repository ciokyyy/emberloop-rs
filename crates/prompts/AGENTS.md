# AGENTS.md — crates/prompts

## Purpose

Model-facing text as files (`include_str!` defaults under `prompts/*.md`):
- IDs: `ponytail`, `policy`, `compress`, `decompress`, `search_context`, `compress.prompt`, and the `tool.*.description` ids.
- `PromptStore`: embedded default, override `<config root>/prompts/<id>.md`, empty override falls back to the default.
- `{{ name }}` placeholders validated at load against `project_name`, `project_root`, `level`, `date`, `session_id`; unknown names fail.
- `PinnedPrompts::pin`: renders and hashes every id once per Session; a Session never reads a changed prompt again.
- `PromptWatcher`: polls the override directory, keeps the last good text on a broken file, reports errors to the callback.
- `filter_ponytail`: keeps only the table row and example lines for the Session's `ponytail_level`; `off` yields empty text.

## Tests

`src/tests.rs` covers override precedence, deletion restoring the default, placeholder rejection, last-good on broken files, pinning across reloads, one hash per id, Ponytail levels and watcher reload.

## Child DOX Index

(none)
