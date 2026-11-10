---
name: code-review
description: "Review a ticket or work-in-progress change against its specification and repository standards. Dispatch the independent Spec and Standards reviewers in parallel, reuse their Sessions for follow-up tickets, and present their submitted reports separately. Use after implementing a ticket, for branch reviews, or when asked to review since a fixed point."
---

Review on two independent axes: Spec checks what was requested; Standards checks how it was implemented. Neither axis overrides the other.

## 1. Establish the review assignment

- Require the local ticket/specification path and the implementation starting point captured before edits. Resolve the starting point to a commit with `git rev-parse <starting-point>^{commit}`. For a branch review, resolve the requested merge-base once. Do not recalculate a moving baseline during review.
- If either input is missing, ask for it. A tracker reference must be fetched to a local ticket file before dispatch; do not invent requirements or skip the Spec reviewer.
- Include committed, staged and unstaged changes with `git diff <captured-commit>`, and inspect untracked files listed by `git ls-files --others --exclude-standard`. Do not substitute a committed-only diff. Tell the reviewers which existing dirty changes belong to this ticket.
- Find the applicable standards documents, including `AGENTS.md`, `CODING_STANDARDS.md` and `CONTRIBUTING.md` where present.
- Pause implementation edits until both reports arrive. Do not require a commit or a clean working tree to review.

## 2. Dispatch both reviewers

Call `spawn` for `role="spec"` and `role="standards"` together. Each call requires `ticketPath`, `startingPoint` and assignment context in `text`: scope, relevant specification references, standards sources, diff commands and any exclusions.

The proxy supplies the authoritative role prompts from `reviewer.spec` and `reviewer.standards` in the Prompt Store. These preserve the original two-axis review briefs and the Fowler smell baseline. Do not duplicate or replace those briefs in dispatch messages. Documented repository standards override the heuristic baseline; tooling-enforced checks are not review findings.

The first dispatch creates two independent Sessions. Later dispatches under the same parent reuse that pair across tickets and the whole assigned specification. Keep their context; do not spawn replacements for every ticket. Use `steer` with the child `id`, `ticketPath`, `startingPoint` and `text` for an explicit follow-up. Wait for an active review before changing its ticket or baseline.

Reviewers have no `edit` tool and must not edit code. Shell and other tools remain available for checks; this is not a filesystem sandbox.

## 3. Receive reports

Only `submit` forwards a review report. Ordinary final answers remain in the reviewer Session. Each valid submission, including identical repeated reports, creates a distinct parent steer. `status="done"` means the review finished, not that it passed; `blocked` means it could not finish. `result(id)` retrieves the report history without consuming it.

Do not treat the first report as completion of both axes. Wait for a submission from each reviewer for the current assignment. On a reviewer error or stop, tell the user, link the reviewer Session, and wait for explicit continuation. The user can open that Session, change its model while idle and prompt it again. Do not retry automatically or destroy its context.

## 4. Present the reports

Present the reports under separate `## Standards` and `## Spec` headings, verbatim or lightly cleaned. Do not merge or rerank findings across axes. Summarize the count and worst issue within each axis. Do not close tickets, start another implementation cycle or create a Run automatically.

A change can satisfy the standards but implement the wrong requirements, or meet the requirements while violating the standards. Keeping the reports separate prevents one axis from masking the other.
