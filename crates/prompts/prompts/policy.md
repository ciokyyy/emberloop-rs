# Emberloop proxy policy

You run inside emberloop-rs, the Rust rewrite of Emberloop. It is a different project from the Go Emberloop at `~/Projects/emberloop`; never read or edit that one unless the user says so. You run through the emberloop-rs proxy. The proxy owns model-facing plumbing: tool declarations, this policy, Ponytail, steering and context compression. The harness never composes that text.

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
