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

Spawn both in the same turn: `spawn role=spec` and `spawn role=standards`, each with `ticketPath`, `startingPoint` and a `text` that gives:

- the scope: ticket number, title, and which design sections and decisions it implements;
- the diff commands: `git diff <startingPoint>` (covers staged and unstaged) and `git ls-files --others --exclude-standard` for new files. When the tree already carries uncommitted work, `git diff <startingPoint>` shows all of it, so never send that bare diff: scope it with a pathspec to the ticket's files, and name the pre-existing hunks inside a shared file so a reviewer does not report them as ticket findings;
- pre-existing dirty files that are not part of the ticket;
- the check commands, to run themselves; a reviewer must not trust your result;
- what is out of scope, from the ticket.

Then pause. Make no edit and run no build until both reports arrive: a build can change shared generated output the reviewers are reading. Reports arrive as steers; do not poll.

## Read the reports

- `done` means the review finished, not that it passed.
- A reviewer that errors or stops did not review. Tell the user; do not retry on your own.
- Match each report to its ticket by the ticket named in it, not by arrival order.
- Present the two reports apart, under `## Spec` and `## Standards`. Do not merge or re-rank them.

## Act on findings

For each finding, check it against the current file before you act:

1. Restate it in one line. If any finding is unclear, ask about all unclear ones before you fix any.
2. Verify it in the code. Is it true here? Does the fix break a caller? Is there a reason for the current code?
3. Fix it, or push back with evidence: a file and line, a test, or a recorded decision. "It works" is not evidence.
4. Fix one finding at a time and run its test after each. Order: blocking first, then simple, then complex.
5. "Implement it properly" for unused code added or touched by this ticket: grep for callers first. No callers means delete, not extend; if it is pre-existing dead code outside scope, note it instead of deleting unasked.

No performative agreement and no thanks; state the fix or the reason. When your pushback turns out wrong, say so in one line and fix it.

## Re-review

Steer the same reviewers, with the same `ticketPath` and `startingPoint`. List each fix with its file and line, quote doc changes verbatim, and ask them to re-check only the open blockers. A steer right after a report can answer `409 Conflict: Wait for this review before changing its ticket or starting point`: wait until the reviewer is idle, then retry once.

Do not auto-loop: after one re-review with findings still open, bring them to the user. Do not close the ticket yourself; the user decides.

## Next ticket

Keep the pair. For the next ticket, steer both reviewers with the new `ticketPath` and `startingPoint`. After a commit is amended, state the new SHA. Then compress the review turns and return to `ember-build`.

## Credits

The two-axis review follows Matt Pocock's `code-review` skill; acting on findings follows obra/superpowers `receiving-code-review` (MIT). Rewritten for Emberloop's spec and standards reviewer roles.
