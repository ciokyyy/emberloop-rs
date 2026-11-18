---
name: ember-design
description: Use when decisions.md is confirmed and the work needs a design document before tickets. Writes .scratch/<slug>/design.md from the recorded decisions, with a detailed implementation plan that leaves the implementer no impactful decision, then hands it to ember-audit.
metadata:
  origin: agent
---

# Ember design

Input: the confirmed `.scratch/<slug>/decisions.md` from `ember-grill`, plus `GLOSSARY.md` and `docs/adr/`. Output: `.scratch/<slug>/design.md`.

The design does not open new decisions. When drafting finds a question that `decisions.md` does not answer, and it has more than one defensible answer, stop and run one `ember-grill` round for it: numbered question, recommended answer, record the result as a new decision id. Decide mechanical choices with one defensible answer (file names, formatting) yourself.

Design only. Do not write or build production files in this stage. Illustrative code goes in a fenced block in the design, labelled as an example.

## Write the draft

Read the code the design touches before you write. Use the glossary words; cite decisions by id (`D3`).

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

## Non-functional requirements
<Only the critical ones: security and privacy, performance, rollout and rollback. "None" is a valid answer.>

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

- List every file the work creates, changes or deletes, including tests, fixtures, config, docs and the nearest `AGENTS.md`.
- Give each file what changes and why.
- Name the existing helper or pattern to reuse when one exists.
- An implementer must be able to follow the plan without making a decision that changes behaviour, an interface or a dependency.

Keep the heading `## Detailed implementation plan` exactly: `ember-audit` splits the document there.

## Reconcile

Before you show the draft, re-read `decisions.md` and compare:

- Contradiction: the draft decides a recorded question differently. Report every one.
- Addition: the draft decides something `decisions.md` does not cover. Report it only when it changes architecture, an external interface or a dependency.

List contradictions first, one line each, quoting both files. The user decides each one; the draft can be right, because drafting read code that the grill did not. Apply each answer to both files.

## Hand off

Link `design.md` in chat with two or three lines on its main choices. Ask the user to read it. After the user approves, set `Status: In review`, compress the design turns (keep both file paths and any new decision ids), and start `ember-audit`.
