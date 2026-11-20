---
name: ember-design
description: Use when decisions.md is confirmed and the work needs a design document before tickets. Writes .scratch/<slug>/design.md from the recorded decisions, with a detailed implementation plan that leaves the implementer no impactful decision, then hands it to ember-audit.
metadata:
  origin: agent
---

# Ember design

Input: the confirmed `.scratch/<slug>/decisions.md` from `ember-grill`, plus `GLOSSARY.md` and `docs/adr/`. Output: `.scratch/<slug>/design.md`.

The design does not open new decisions. When drafting finds a question that `decisions.md` does not answer, and it has more than one defensible answer, stop and run one `ember-grill` round for it: numbered question, recommended answer, record the result as a new decision id. Decide mechanical choices with one defensible answer (file names, formatting) yourself.

Check every new user-facing name a decision introduces (skill, slash command, CLI subcommand, setting, route) against existing names in every place it resolves, such as both skill roots, because a project copy shadows a global one and the grill rarely looks. A clash is a grill question.

Design only. Do not write or build production files in this stage. Illustrative code goes in a fenced block in the design, labelled as an example.

## Untangle check

Read the code the design touches first (plus direct callers and any duplicated copies). Ask: **should these pieces change together?**

Check only what this change touches for:
- **Hidden links**: duplicated logic or values, implicit call order, shared globals or files, one concept split across layers, or lifecycle invariants enforced at N call sites instead of inside the single state-transition mutator.
- **Needless links**: callers tied to internals, stable code importing volatile code, caller-specific flags or parameters on a general function, shallow modules.
- **Surplus and special cases**: pass-through wrappers (deletion test), speculative config or duplicate enable flags, shorter outer timeouts that abort a callee's handshake mid-flight, dead code, one-off `if` branches, caller-only params, `sleep`/retry, swallowed errors.

If the touched seams are clean, skip untangling and draft directly.

If friction exists, combine `untangle-codebase` synthesis with `codebase-design` (`module`, `interface`, `depth`, `seam`, `adapter`, `locality`):
- Look for the **one structural fix** behind multiple problems rather than patching special cases one by one.
- Keep internal cleanups in `## Technical plan` so `ember-slice` can cut a zero-behaviour-change prefactor ticket (`01`) ahead of feature tickets.
- Run one `ember-grill` round first if untangling changes an external interface, removes user-visible config/behaviour, or widens scope.
- Do not sweep unrelated files, strip comments repo-wide, or rewrite unrelated tests here; use `ember-untangling` when the user asks for whole-codebase cleanup.

## Write the draft

Use the glossary words; cite decisions by id (`D3`).

When a recorded decision leaves a mechanism to an unverified external fact (a vendor event that may not fire), choose the mechanism that does not depend on that fact, name the choice under Reconcile as an addition, and keep the fact as a verification step.

When a design must deviate from a decision for a code reason (a live-data rename that would corrupt rows), keep the decision's intent, state the deviation in Reconcile, and let the user decide.

Write the whole draft in one pass with this layout:

```markdown
# <Feature>

- Status: Draft | In review | Approved
- Decisions: ./decisions.md

## Overview
<What this changes and why, in one paragraph.>

## The problem
## Goals
<Specific and checkable.>
## Non-goals
## Open questions
<Kept until answered; remove each one when answered.>

## Critical user journeys
### CUJ 1: <title>
- Given: <starting state>
- When: <action or event>
- Then: <observable result>

## Technical plan
<Prose: components, data flow, interfaces, what changes and what stays. One ASCII or Mermaid diagram when there are three or more parts.>

## Alternatives considered
### <Alternative>
- Pros / Cons
- Rejected by author: <technical reason>

## Risks & Limits
<Mandatory for `ember-audit`: potential failure modes, security and privacy implications, scalability limits, rollout/rollback, and how each is mitigated.>

## Detailed implementation plan
### Target files
#### 1. `<path>` (Create | Modify | Delete)
- Scope: <module, test, config, doc>
- Changes: <what changes and why. APIs, signatures, schemas and pseudocode are allowed; production code is not.>

### Verification plan
- Build, tests, lint: <the repository's check commands>
- New tests: <behaviour each test proves>
- Manual or live check: <only when tests cannot prove it>
```

Rules for the implementation plan:

- List every repository file the work creates, changes or deletes, including tests, fixtures, config, docs and the nearest `AGENTS.md`. Keep host-local binary installs and release steps in a separate runbook or verification section rather than `Target files`, because `Target files` maps repo paths one-to-one onto reviewable tickets.
- Give each file what changes and why in one to three concise lines, keeping interface contracts, struct fields, and invariants once in `## Technical plan` rather than duplicating them in `Target files`.
- Name the existing helper or pattern to reuse when one exists. When a helper is called from another crate or package, give its full signature and public visibility, and check that its inputs carry everything the new caller needs (for example module data the old caller fetched separately); a crate-private helper does not compile from outside.
- Name deleted tests by function name, not by line range: a range sweeps unrelated tests and the helpers other tests still call.
- Run every grep gate and removal search in the Verification plan against the live tree before handoff. Every matching file is either a Target file or an explicit glob exclusion; a gate that matches an unlisted file cannot pass, and "mark it historical" keeps the match. Write the gate as `git grep --untracked -nE '<pattern>' -- . ':!<excluded path>/**' ...`: it searches hidden folders such as `.agents/` (which `rg` skips by default) and the new files the tickets create, and the pathspec excludes name every untracked report or tool cache that matches. Record the file count it matched at design time.
- When generalizing a feature across more roles or kinds, rename misleading narrow identifiers in code, types, and UI labels unless an external wire contract forbids it; keeping a legacy single-role name causes future callers to mistake scope.
- An implementer must be able to follow the plan without making a decision that changes behaviour, an interface or a dependency.

Keep the heading `## Detailed implementation plan` exactly: `ember-audit` splits the document there.

## Reconcile

Before you show the draft, re-read `decisions.md` and compare:

- Contradiction: the draft decides a recorded question differently. Report every one.
- Addition: the draft decides something `decisions.md` does not cover. Report it only when it changes architecture, an external interface or a dependency.

List contradictions first, one line each, quoting both files. The user decides each one; the draft can be right, because drafting read code that the grill did not. Apply each answer to both files.

## Hand off

Link `design.md` in chat with two or three lines on its main choices. Ask the user to read it. After the user approves, set `Status: In review`, compress the design turns (keep both file paths and any new decision ids), and start `ember-audit`.
