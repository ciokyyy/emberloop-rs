---
name: ember-slice
description: Use when an approved design or a confirmed decisions.md must become work items. Cuts the work into vertical tracer-bullet tickets with blocking edges, agrees the cut with the user, and writes .scratch/<slug>/issues/NN-<slug>.md files that a fresh reviewer can check a diff against.
metadata:
  origin: agent
---

# Ember slice

Input: `.scratch/<slug>/design.md` (approved) or, for a small change, `.scratch/<slug>/decisions.md`. Output: one file per ticket in `.scratch/<slug>/issues/`.

## Cut vertical slices

- Each ticket is a tracer bullet: a thin path through every layer it needs (storage, logic, API, UI, docs), finished and demoable alone. Do not cut by layer ("all the schema", then "all the API").
- Prefactor first. When the change is hard to make, the first ticket makes it easy without changing behaviour; the next ticket makes the easy change.
- Wide refactor exception, expand and contract: one ticket adds the new form beside the old one; migration tickets move callers in batches, ordered by blast radius, each blocked by the expand ticket; one contract ticket deletes the old form, blocked by every migration ticket.
- One ticket must fit one review: a reviewer reads its diff in one sitting.
- Assign every file in the design's implementation plan to the slices. Docs and the nearest `AGENTS.md` change in the ticket whose behaviour they describe, not in a docs ticket at the end. A doc whose new text names a feature a later ticket adds moves to that later ticket, or it ships a false sentence. When the planning Session pre-wrote texts (skills, docs) under `.scratch/<slug>/` in a tree that mirrors their target paths, each owning ticket gets a criterion "copied as given from `<mirror path>`". When two slices necessarily touch the same shared file (such as an API types module, prompt contract test, or nearest `AGENTS.md`), chain them with a `Blocked by` edge so implementations and reviews never race on one file.
- Release, install and live-check steps are not a ticket: they leave no diff for the reviewer pair to read. Keep them in the design's runbook after the last ticket (see Hand off).
- A design that lives outside `.scratch/<slug>/` (for example at the repo root after an outside review pipeline) stays where it is; the ticket `Design:` line names its absolute path. Moving it breaks the review reports that cite it.

## Agree the cut

Show the proposed list in chat, one line per ticket: number, title, blocked by. Then ask in one numbered round, each with your recommendation: granularity (merge or split any?), order and blocking edges, anything missing. Write the files only after the user accepts. Write the round to `.scratch/<slug>/questions.md` first. When the user already said to accept every recommended answer through slicing, that is the acceptance: write the tickets with your picks and say so in the hand-off.

## Ticket file

Path: `.scratch/<slug>/issues/<NN>-<ticket-slug>.md`, numbered in dependency order (`01`, `02`, ...).

```markdown
# <NN>: <title>

- Status: todo | in progress | in review | done
- Starting point: <SHA, written by ember-build before the first edit>
- Blocked by: <NN, NN> | none
- Design: ../design.md, sections "<heading>", "<heading>"; decisions D2, D5

## What to build
<The end-to-end behaviour this ticket adds, in the glossary's words. What a user or caller can do after it that they cannot do before.>

## Acceptance criteria
- [ ] <observable, checkable result>
- [ ] <test that proves it>
- [ ] <doc or AGENTS.md change>

## Out of scope
- <what a reviewer must not expect here; which later ticket does it>
```

Rules:

- The ticket must stand alone for a fresh reviewer: what to build, how to tell it is done, and what is not in it. Point at design sections for the how; do not copy them.
- No file paths or code in "What to build" except a short snippet that carries a decision (an agreed signature or schema).
- Use glossary terms and respect the ADRs.

## Hand off

When preparing the plan for single-Session execution, remove the design's phase/step list and add a top-level `## Session Runbook` section: place it at H2 after `## Detailed implementation plan` when the removed list was an `###` subsection so sibling `###` headings (`### Target files`, `### Verification plan`) keep their parent. Include pre-edit steps (open decisions to ask, `AGENTS.md` files to read, pre-existing dirty files to exclude), per-ticket numbered `ember-build` steps (starting point, red test command and expected failure, edit, rerun, docs, regenerate, full check, review pair, commit on the user's word, compress), and ship steps. Each step names its command and expected result so the implementer makes no new decision.

Link the issues folder. Compress the slicing turns; keep the folder path and the ticket list with blocking edges. Next stage: `ember-build`, starting with the lowest unblocked ticket.
