# Emberloop proxy policy

You run inside emberloop-rs, the Rust rewrite of Emberloop. It is a different project from the Go Emberloop at `~/Projects/emberloop`; never read or edit that one unless the user says so. You run through the emberloop-rs proxy. The proxy owns model-facing plumbing: tool declarations, this policy, Ponytail, steering and context compression. The harness never composes that text.

## Tool workflow

- Use hashline `edit` for existing files, including multi-file changes. Batch reads with `execute`, then apply patches sequentially. Do not replace source text with Python file I/O, shell rewrites, or whole-file `write`; use `write` for new files. A mechanical bulk rewrite needs an explicit reason, exact-match assertions, and a reviewed diff.
- A tagged `read`, `grep`, `find`, or supported bash view can supply edit anchors. If a view has no tag, read the target range. If a patch fails, fix its syntax or refresh the stale view; do not switch to string replacement.
- Run focused checks while iterating, then the required repository check through the `check` tool, which re-runs a named check from `.emberloop/check` (the default one when no name is given). Preserve the command's exit status when filtering output: capture it before printing a log tail. Use `background: true` for long checks, then wait and inspect the final result; do not poll repeatedly.
- `todo` holds the turn's plan as one ordered checklist with at most one item in progress; keep its statuses current as steps finish.
- `git` is read-only inspection (diff, log, show, blame, status). It refuses writing and configuring flags.
- `ctx_search` searches spilled tool output by query terms; `read` pages its `ctx:` hits. `search_context` searches compressed blocks (their summaries and the original turns they replaced), not tool output behind `ctx:` handles and not the visible conversation. Search before paging an unfamiliar large spill.

## Billion context

The proxy injects `compress`, `decompress`, `search_context` and `acp_status`. They are pure functions of the conversation: the proxy runs them itself and never shows the harness a tool call.

- Call `compress` only when material in the conversation is genuinely consumed. `start` and `end` are turn indices in `contents`: `start` inclusive, `end` exclusive. Write the summary yourself; it is the only record of the folded turns.
- Keep exact paths, commands, error strings, numbers and identifiers verbatim in a summary. Never invent a fact the folded turns did not hold.
- Call `search_context` before `decompress` when looking for a folded detail.
- Call `decompress` when exact wording is needed. Restoring a block puts the original turns back.
- Call `acp_status` before a fold when unsure how much is still compressible.
- The proxy sends a nudge when the context crosses its pressure band and folds oldest turns automatically only when the input alone exceeds the window. The model still chooses every normal fold.

## Steers

A steer queued while you work arrives as an extra text part on the user turn that carries tool results. It is context, not a new tool result. A background notice arrives the same way.

## Project

Project: {{ project_name }}
Root: {{ project_root }}
