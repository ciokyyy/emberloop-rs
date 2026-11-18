---
name: ember-flow
description: Use when starting any Session, when the user asks to build, plan, design or change something, when fixing a bug, or when work needs decisions, a design, tickets, subagents or a review. Routes work through the ember skills - grill, design, audit, slice, build, review - in one long-lived Session.
metadata:
  origin: agent
---

# Ember flow

<SUBAGENT-STOP>
If you were spawned as a subagent (worker, scout, reviewer or goldfish) to do one task, ignore this skill and do the task.
</SUBAGENT-STOP>

Ember is one pipeline. Each stage has one job and leaves a file on disk:

| Stage | Skill | Job | Leaves |
| --- | --- | --- | --- |
| 1 | `ember-grill` | Human and agent mean the same thing. Every decision is the user's and is written down. | `GLOSSARY.md`, `docs/adr/`, `.scratch/<slug>/decisions.md` |
| 2 | `ember-design` | Turn the decisions into a design that an implementer can follow without new decisions. | `.scratch/<slug>/design.md` |
| 3 | `ember-audit` | Fresh-context goldfish reviewers find the holes in the design. | `.scratch/<slug>/audit/*.md`, fixed design |
| 4 | `ember-slice` | Cut the design into vertical tickets with blocking edges. | `.scratch/<slug>/issues/NN-*.md` |
| 5 | `ember-build` | Per ticket: test first, root cause before fixes, evidence before claims. | code, tests |
| 6 | `ember-review` | Per ticket: spec and standards reviewers check the diff. Commit only when the user says so. | review reports, commit |

Read a stage skill whole with `read(path="el://skills/ember-<stage>/SKILL.md")` when you enter that stage. Do not work from memory of it.

## Pick the entry point

- Bug, failing test or wrong behaviour: `ember-build`, section "Debugging". Grill first only when the fix needs a decision.
- Small change that the user stated plainly (one place, no open decision): `ember-build` directly. A direct user instruction wins over every gate here.
- Change with open decisions but small scope: `ember-grill`, then `ember-slice` from `decisions.md`. Skip design and audit.
- Feature, refactor across modules, new interface, or anything costly to undo: all six stages.

When unsure, recommend an entry point in one line and let the user correct it.

## The elephant and the goldfish

This Session is the elephant. It holds why each decision was made, so it stays alive from grill to commit. Do not hand the work to a fresh Session to "clear context". Fold finished work with `compress` instead.

Goldfish are fresh-context subagents: `ember-audit` goldfish, spec and standards reviewers, and scouts. They never see this conversation. That is their value: they read only what is on disk, so they find what the disk does not say.

## Billion-context rules

- Disk is the contract. Every decision, design, ticket and report lives in a file under `.scratch/<slug>/` (or `GLOSSARY.md` and `docs/adr/`). A subagent must be able to do its job from files alone.
- Compress at every boundary: after each stage and after each ticket. Keep verbatim in the summary: artifact paths, decision ids, ticket path, starting-point SHA, open findings, check commands and their last result.
- Search before you re-read. Use `search_context` for a folded detail, then `decompress` only when you need the exact words.
- Reuse subagents. Steer a long-lived worker or reviewer pair across tickets; do not spawn a new one per ticket.
- Use tier `mid` for every worker, scout and goldfish. Use `top` only when the user allows it.
- Reports arrive as steers. Do not poll, sleep or loop on `result`.

## Rules for every stage

- Decisions belong to the user; facts belong to you. Look facts up in code and docs, or send a scout, instead of asking.
- Critique first. Lead with what is wrong, missing or risky. No flattery, no apology; state the corrected logic.
- Plain English. No promotional adjectives (seamless, robust) and no decorative adverbs (simply, obviously).
- Write artifacts to files and link them. Do not paste a draft, plan or report into chat.
- Contradiction stop: when a new request contradicts a recorded decision, edit no file. Quote both in one line each and ask which wins.
- Verification belongs to the repository: run the check command its `AGENTS.md` names, and keep the exit status.

## Credits

Ember merges ideas from Matt Pocock's skills (grilling, domain modeling, tickets, implement, code review), obra/superpowers (MIT) and Google's Elephant-Goldfish Model (Apache-2.0). Sources, commits and licenses: `UPSTREAM.md` in the emberloop-rs repository at `vendor/ember/UPSTREAM.md`.
