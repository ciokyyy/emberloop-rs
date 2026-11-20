---
name: ember-review
description: Use when a ticket's diff is ready, or the user asks for a code review. Starts the persistent spec and standards reviewer pair on the ticket path and captured starting point, waits for both reports, checks each finding against the code, and fixes or pushes back with evidence. Reuses the pair across tickets.
metadata:
  origin: agent
---

# Ember review

Two reviewers, two questions:

- Spec: does the diff build what the ticket says, all of it, and nothing it excludes?
- Standards: is the diff built the way this repository requires (`AGENTS.md`, `CODING_STANDARDS.md`, `CONTRIBUTING.md`, the nearest `AGENTS.md`)?

The proxy gives each reviewer its role prompt. Do not repeat that prompt in your text.

## Start the pair

Requirements before you spawn:

- `ticketPath`: the ABSOLUTE path of a ticket file on disk. When the ticket is a section of a larger document, pass that document and quote the section heading and lines in `text`.
- `startingPoint`: the SHA captured before the first edit, one short line. A multi-sentence value is refused (`400 Bad Request: A ticket path and captured implementation starting point are required`). No commits: use the empty tree `4b825dc642cb6eb9a060e54bf8d69288fbee4904` and say so in `text`.
- Pass both as top-level `spawn`/`steer` arguments. Naming them only inside `text` is refused with the same 400, so the reviewer never starts.

Spawn both in the same turn: `spawn role=spec` and `spawn role=standards`, each with `ticketPath`, `startingPoint` and a `text` that gives:

- the scope: ticket number, title, and which design sections and decisions it implements;
- the diff commands: `git diff <startingPoint>` (covers staged and unstaged) and `git ls-files --others --exclude-standard` for new files. When the tree already carries uncommitted work, `git diff <startingPoint>` shows all of it, so never send that bare diff: scope it with a pathspec to the ticket's files, and name the pre-existing hunks inside a shared file so a reviewer does not report them as ticket findings;
- pre-existing or earlier-ticket dirty files that are not part of this ticket;
- the check commands, to run themselves; a reviewer must not trust your result. When a repo check already fails at `startingPoint` on unrelated items, name those baseline findings so the reviewer verifies they are unchanged instead of treating them as ticket blockers;
- what is out of scope, from the ticket.

Then pause. Make no edit and run no build until both reports arrive: a build can change shared generated output the reviewers are reading. Reports arrive as steers; do not poll.

## Read the reports

- `done` means the review finished, not that it passed.
- A reviewer that errors or stops did not review. Tell the user and do not retry on your own. A reviewer whose turn ends without a submission (`blocked`, "ended its turn without calling submit") is a different case: tell the user, then steer it once with a narrower brief — at most three files, the ticket delta quoted inline instead of a diff command, and "end with the submit call" — because an over-broad reading turn is the usual cause. One narrowed steer is the only retry; a second silent turn ends the attempt, and a steer that asks only for the missing report still counts as that retry. On a re-review, a silent turn does not erase that role's earlier complete report for the code it read: record the failed attempt under `## Review`, do not claim that role re-reviewed, and treat the earlier verdict as covering only the pre-fix lines. Every steer you send the pair repeats `ticketPath` and `startingPoint`, not only the spawn and the re-review: a steer without them is refused with `400 Bad Request: A ticket path and captured implementation starting point are required`, and that refusal does not resume the reviewer. When that stuck turn will not take the steer, `stop` it and spawn a fresh reviewer of the same role with the same narrowed brief. When a fresh reviewer of that role also ends silently, that is the user's decision, not another steer loop: stop steering, write the role and the attempt count into the ticket's `## Review` section, and say which axis the ticket is missing, so the next reader knows that half is unreviewed.
- Match each report to its ticket by the ticket named in it and by the paths it says it checked, not by arrival order: a report that names this ticket but examined another ticket's files credits that other ticket, and this ticket still lacks that axis.
- Present the two reports apart, under `## Spec` and `## Standards`. Do not merge or re-rank them.

## Act on findings

For each finding, check it against the current file before you act:

1. Restate it in one line. If any finding is unclear, ask about all unclear ones before you fix any.
2. Verify it in the code. Is it true here? Does the fix break a caller? Is there a reason for the current code?
3. Fix it, or push back with evidence: a file and line, a test, or a recorded decision. "It works" is not evidence.
4. Fix one finding at a time and run its test after each. Order: blocking first, then simple, then complex.
5. "Implement it properly" for unused code added or touched by this ticket: grep for callers first. No callers means delete, not extend; if it is pre-existing dead code outside scope, note it instead of deleting unasked.

A non-blocking finding, such as a missing test for a coverage bullet the design lists, is closed in the same pass with its test; then move on. A re-review is for open blockers only, so do not spend a reviewer turn on nits you already closed.

No performative agreement and no thanks; state the fix or the reason. When your pushback turns out wrong, say so in one line and fix it.

## Re-review

Steer the same reviewers, with the same `ticketPath` and `startingPoint`. List each fix with its file and line, quote doc changes verbatim, and ask them to re-check only the open blockers. A steer right after a report can answer `409 Conflict: Wait for this review before changing its ticket or starting point`: wait until the reviewer is idle, then retry once.

When a gate fix (a repo check or lint finding) clears an acceptance criterion that the first review left unchecked, reopen that ticket (`Status: in progress`), tick the criterion with the fresh gate output, and record the gate fix under `## What changed` and `## Evidence` so the extra paths have a stated scope. Then re-steer both reviewers with the same `ticketPath` and original `startingPoint`. Add the gate-fix paths to the scoped diff and give the current HEAD in `text`; a later commit does not reset the starting point. Re-review only that ticket; the other tickets' reviews stay valid. When both reports pass, restore `Status: done` and append both re-review reports to `## Review`.

Do not auto-loop: after one re-review with findings still open, bring them to the user. Do not close a ticket that has an open finding or a pending re-review of one; `ember-build` sets `Status: done` once both reports are in and no blocker remains, uncommitted state included. When neither report had a blocker, those reports already close the ticket: a re-review you started anyway and that ends silently is recorded, not retried.

## Next ticket

Keep the pair. For the next ticket, steer both reviewers with the new `ticketPath`, `startingPoint`, scoped pathspec, and any prior-ticket dirty files still sharing that `startingPoint`. After a commit is amended, state the new SHA. Then compress the review turns and return to `ember-build`.

## Credits

The two-axis review follows Matt Pocock's `code-review` skill; acting on findings follows obra/superpowers `receiving-code-review` (MIT). Rewritten for Emberloop's spec and standards reviewer roles.
