---
name: ember-grill
description: Use when a request has open decisions, fuzzy terms or unstated scope, before any design or code. Grills the user in numbered rounds until human and agent share one understanding, and records every decision in GLOSSARY.md, ADRs and .scratch/<slug>/decisions.md.
metadata:
  origin: agent
---

# Ember grill

Goal: you and the user mean the same thing by every term and every decision. The grill ends with written decisions, not with a feeling of agreement.

## The design tree

Treat the request as a tree of decisions. A decision is on the frontier when every decision it depends on is settled.

Work in rounds:

1. Ask the whole frontier in one message, as a numbered list.
2. Give each question your recommended answer and its reason. Word it so that "yes" accepts the recommendation.
3. Hold back a question whose answer depends on another open question. It goes in a later round.
   Put each question in its own short block at the end of the reply: what the system does today, why that blocks the change, then lettered options (a, b, c) with your pick marked. Say how to answer ("reply like 1a, 2 keep"). A question that sits inside a long analysis or a summary of findings gets missed, and the user has to ask what they were supposed to answer.
   When the user asks a follow-up about a decision that is still open, answer it in two or three lines, then restate the open question with its options. Do not start a long analysis of each way the follow-up could be read.
4. After the user answers, record the answers (see "Outputs"), recompute the frontier, and start the next round.

Facts are your job; decisions are the user's. Find facts in the code, docs and git history yourself, or spawn a `scout` (tier `mid`) for a slow lookup. Do not hold the round for a fact; ask the questions that do not depend on it.

Stress-test each answer. State the strongest counter-argument and the case where the chosen option fails. Agree only when the option survives that.

## Grill against the docs

Read these before round 1, if they exist: `GLOSSARY.md` (or `GLOSSARY-MAP.md` and the per-context `GLOSSARY.md` it points to), `docs/adr/`, and the nearest `AGENTS.md`.

- Challenge each term against the glossary. When the user uses a word in a different sense, or uses a word that `_Avoid_` lists, say so and ask which meaning holds.
- Sharpen fuzzy words. "Fast", "simple", "the session" each get a concrete meaning or a number.
- Use concrete scenarios. For each rule, give one edge case ("a user with two open Sessions closes one: what happens to X?") and ask what must happen.
- Cross-check with the code. When the user's description and the code disagree, show the file and line and ask which is right.
- Update `GLOSSARY.md` as each term resolves, in the same round. Create the file lazily, on the first resolved term.
- Offer an ADR only when a decision meets all three: hard to reverse, surprising without the context, a real trade-off. Typical cases: architectural shape, integration pattern, lock-in technology, scope boundary, a deliberate deviation from convention, an invisible constraint, a non-obvious rejected alternative.

## Outputs

Write `.scratch/<slug>/decisions.md` and keep it current after every round:

```markdown
# <Feature>: decisions

## Problem
<2-4 sentences: who has the problem, what fails today.>

## Decisions
- D1: <decision in one line>. Why: <reason>. Rejected: <option, reason>.
- D2: ...

## Non-goals
- <what this work will not do>

## Open questions
- <only what is still unanswered>

## Docs touched
- GLOSSARY.md: <terms added or changed>
- docs/adr/NNNN-<slug>.md: <title>
```

Number decisions once and never renumber them; later stages cite `D3`. When the user changes a decision, edit it in place and add `(changed: <reason>)`.

## Exit

The grill is done when the frontier is empty. Then play the understanding back: one short paragraph in your own words, plus the decision list by id. Ask the user to confirm or correct it. Do not start design or code before the user confirms.

Then compress the grill turns. Keep the path of `decisions.md` and the decision ids in the summary. Next stage: `ember-design`, or `ember-slice` for a small change.

## Formats

Glossary (`GLOSSARY.md`), only terms specific to this project, no implementation details:

```markdown
# <Context name>
<1-2 sentences: what this context is.>

## Language
**Term**: <1-2 sentences: what it IS.>
_Avoid_: <synonyms that must not be used>
```

Be opinionated: pick one word per concept and list the rest under `_Avoid_`. Group related terms under `###` subheadings when the list grows. A repository with several contexts has a `GLOSSARY-MAP.md` at the root that links one `GLOSSARY.md` and one `docs/adr/` per context.

ADR (`docs/adr/NNNN-<slug>.md`, number = highest existing + 1, start at `0001`):

```markdown
# <Decision title>
<1-3 sentences: the context, the decision, and why.>
```

Add these only when they carry information: `Status:` (proposed, accepted, deprecated, superseded by ADR-NNNN), `## Considered options`, `## Consequences`.
