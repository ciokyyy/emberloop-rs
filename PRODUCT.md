# Product

<!-- impeccable:product-schema 1 -->

<!-- Facts marked (inferred) come from README.md, GLOSSARY.md, AGENTS.md and 189 stored Sessions, not from an interview. The owner confirms or corrects them. -->

## Platform

web

## Users

The owner runs coding-agent Sessions on their own machine and watches them from a browser, often on a phone, often while the agent works alone for minutes. (inferred)

Job: see what the agent does right now, trust it, steer it, and read back later what it did. The Session feed is the main reading surface. A person scans it far more often than they read it.

## Product Purpose

emberloop-rs is a local daemon with a web UI. It runs agent Sessions through several harnesses behind one proxy that owns the model wire: translation, context compression, steering and usage metering. Success: the owner can leave a Session running, return, and understand its state in seconds without opening raw logs.

## Positioning

One UI and one tool surface for every model family (claude, agy/gemini, pi/deepseek, codex). The proxy and tool server are shared, so the feed must read the same whatever model produced the events. A single-vendor client cannot copy that.

## Operating Context

- Loopback daemon, no auth. Projects hold Sessions. Sessions spawn Subagents (Worker, Scout, Reviewer; see GLOSSARY.md).
- Tools are served over MCP. `execute` runs Python that calls the other tools over a bridge and is the most used tool in long Sessions.
- Event log per Session on disk (`events/*.jsonl`): `thinking_chunk`, `assistant_chunk`, `tool_started`, `tool_finished`, `usage`, `compression`, `steer_*`.
- Model families differ in what they emit (measured on 189 Sessions):
  - pi: plain-prose thinking, almost no headers.
  - claude: 64% of thinking spans are empty text.
  - agy: no thinking events, full tool arguments, intent text (`toolAction`, `toolSummary`, ACP `title`) usually absent.
- Tool calls stream as many partial `tool_started` events before one `tool_finished`.

## Capabilities and Constraints

- Web UI: SvelteKit, Tailwind v4, shadcn-svelte over bits-ui, bun only. Every control is a shadcn-svelte component.
- Design source of truth: `design.figcraft` (mirrored from a live FigCraft document) and `design.md`. A redesign is drawn there first, then built.
- Tokens only: `surface-0..3`, `fg-1..4`, `status-*`, `tool-*`, motion `--dur-*` and `--ease-smooth`. No new colours.
- Pre-0.1: stored Sessions may be dropped between releases, but the feed must still render old Sessions that lack new event fields.
- Undecided: whether nested `execute` calls get their own events or a `display.children` summary on `tool_finished`.

## Brand Commitments

Name: Emberloop (this repo: emberloop-rs). Dark UI ("Astryx dark" boards). No logo or voice rules beyond that are recorded.

## Evidence on Hand

- 189 real Sessions under `~/.local/share/emberloop-rs/sessions/` (pi 81, claude 62, agy 46).
- `design.figcraft` pages: Session dialogs, Chrome, Components, Skills, Usage/Providers/Settings, Status. No tool-feed mock exists yet.
- No users other than the owner, no testimonials, no benchmarks. Do not invent any.

## Product Principles

1. The feed answers "what is the agent doing now, and did it work" before it shows any detail.
2. Every row says what it did in the agent's or the tool's own words, never in an implementation name (`import tools, json` is not a description).
3. Same grammar for every model family; the family only changes which fields fill the slots.
4. Detail is one tap away and closed by default. Only a failure opens by itself.
5. Quiet by default: the screen shows motion only for work that is running.

## Accessibility & Inclusion

Usable one-handed on a phone. Text contrast and focus rules follow the existing tokens. Row state must not depend on colour alone. (inferred)
