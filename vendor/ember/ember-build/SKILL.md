---
name: ember-build
description: Use when implementing a ticket, fixing a bug or making any code change. Runs the per-ticket loop in one long-lived Session - capture the starting point, test first, find the root cause before any fix, prove every claim with fresh command output - then hands the diff to ember-review.
metadata:
  origin: agent
---

# Ember build

Three laws hold for every change:

1. No production code without a failing test first.
2. No fix without a root cause first.
3. No claim of "done", "fixed" or "passing" without fresh command output that shows it.

A ticket that ships an operator script or runbook also reads `references/operator-scripts-and-runbooks.md` before writing either.

## The ticket loop

For each ticket, lowest unblocked number first:

1. Start. Record the ticket's absolute path. Capture the starting point before any edit: `git rev-parse HEAD` (no commits yet: use the empty tree `4b825dc642cb6eb9a060e54bf8d69288fbee4904`). Record pre-existing dirty files from `git status --short`; they are not part of this ticket. Set the ticket `Status: in progress`. When the ticket already claims `done` or `in progress`, prove that claim before editing: run the repository's check command and `git log --oneline` over the ticket's paths. Green code with a committed diff means the remaining work is the record, not the code.
2. Build it test first (below). Change only what the ticket needs: minimum code that solves it, no speculative abstractions, configurability or error handling for impossible scenarios, and no dead code "for the next ticket". Keep edits surgical: do not reformat, refactor or rewrite comments on untouched code; remove only imports, variables or helpers orphaned by your edit; report pre-existing dead code instead of deleting it unless the ticket asks. Patch by syntactic unit: when a range replaces an import, include every import it held; when it replaces a block interior, account for the closing line so no orphan brace survives; a single-line range on the head of a multi-line expression drops its continuation lines. Read the range back and run the type gate after each patch, because these break only at the gate; after an insert, read the line after the range too, because a body that restates the old last line leaves a duplicate. A hunk that deletes a field or builder call can also swallow an unrelated adjacent line (a bootstrap `ensure`, a default): read the hunk's neighbours, because the type gate and tests stay green without a deleted side-effect call. Format only with the repository's own formatter, run through its gate command; a one-off `npx prettier` or package-wide formatter over an edited file reformats untouched lines, buries the ticket diff, and undoing it means reverting the whole file and reapplying your hunks from the ticket, because `git checkout -- <file>` takes every uncommitted hunk with it.
3. Update the docs the ticket lists, including the nearest `AGENTS.md`. Correct the sentence that is now false instead of appending a new one beside it: a line that says a case cannot happen must name the new exception. When the ticket adds user-facing copy, use the glossary's preferred terms and grep the new strings for its `Avoid` list; a banned synonym is a review blocker.
4. Verify (below): focused tests, then the repository's full check command once.
5. Review: run `ember-review` with the ticket path and starting point. Pause all edits and builds until both reports arrive.
6. Set the ticket `Status: done` once both reports arrive with no open blocker and every acceptance criterion is checked, whether or not a commit exists; record "uncommitted" in the ticket and your report, because the commit is a separate decision only the user makes. A criterion another ticket owns, or a gate left red on a pre-existing finding, stays unchecked with a one-line pointer to the owning ticket or the pending user decision, and `Status: in progress` stays so the ticket still shows the open work; a tick that hides a deferral costs the next reader the whole recheck. A criterion that counts call sites or quotes line numbers can go stale after earlier tickets: recount the current tree, and annotate the new count in the ticket instead of forcing the code to match the old number. When the user authorizes it and the checks pass, commit with files staged by name; never `git add -A` on your own judgement. Before a commit-and-push request, check `git remote -v` and the branch upstream: with no remote there is no push, so report that with the commit instead of promising both. Stage only the ticket's paths, and when a shared file's hunks mix another stream's uncommitted work, name which hunks are the ticket's and ask for the scope: committing that file whole publishes the other stream, and the ticket's paths alone may not build until that stream commits. When the user answers an explicit whole-tree commit ("commit all"), that answer is the scope decision: `git add -A` runs then, and the commit body names the other streams it carries plus any build risk, because the commit is no longer the ticket alone. A phase commit after every ticket is done is also a scope decision: verify every `git status --short` entry belongs to the phase, stage those paths, and keep ignored secret files and tracked local telemetry (for example `.agents/skills/.usage.jsonl`) out of the index; confirm with `git diff --cached --name-only`.
7. Compress the ticket's turns. Keep verbatim: ticket path, starting point, commit SHA, check command and result, any open finding. Then take the next ticket.

When the build stops before the ticket is done (the user interrupts or redirects, or a blocker appears), stop editing at once and write the state into the ticket file: keep `Status: in progress`, tick only the criteria that hold, add `## Implemented so far (uncommitted)` listing each file and what changed, and `## Still to do` with the last check result and the next concrete step. The ticket is the handoff record; the user reads it instead of the diff.

## Closing tickets whose code already landed

A request to finish a phase or implement every ticket can arrive after the code landed and only the record went stale: unchecked criteria, `Status: in progress`, and caveat lines naming a red gate or an `uncommitted` tree. Triage before editing: list the unchecked criteria across the ticket files, run the gate they name, and match the ticket paths against `git log --oneline`. Costly work is done when the gate is green and the diff is committed; the tickets then need their evidence recorded, not their code rebuilt.

- Tick a criterion only on fresh output from this session, and rewrite the caveat beside it to the same result. A green tick left next to "blocked only by" text sends the next reader into a recheck.
- Map the design's item numbers to the ticket files before declaring a phase complete: a design says "item 20 first", and the ticket that carries it can have a different number.
- When one batch patch touches the same criterion line in many ticket files, find each anchor below the header lines, because a status or summary line can contain the same words, and two ops on one line are refused as overlapping. Strip the read view's `N→` gutter before matching line text.
- Record the phase's state once in the plan file the phase came from: a line naming the commits, the check result, and what is left. Operator and live checks belong there as remaining work, not as code.

The Session stays alive across tickets. It remembers why each decision was made; `compress` keeps it lean. Do not start a new Session per ticket.

## Test first

- Red: write one test for one behaviour, with a name that states the behaviour. Use real code; mock only what you cannot run. Assert exactly the contract the ticket or design records; a stricter rule you add (a minimum length, a format check) is an unrecorded decision and its failure message names the wrong cause.
- A changed default or config value: test the behaviour it drives, read through the production mapping (for example the private settings-to-policy function, reached from the module's own test child), not `Default::default().x == N`. An equality test only restates the number; the behaviour test proves the value reaches the code that uses it, and still fails before the edit.
- Verify red: run it. It must fail for the expected reason, not from a typo, a compile error or a missing fixture. A test that passes at once proves nothing; fix the test. A red run that only reports a missing new symbol (an undefined task name, a module that does not exist, an unimplemented export) proves the wiring, not the behaviour: add that symbol alone, run again, and make the behavioural assertion fail before implementing the behaviour. When an earlier layer already removes the trigger (a filter, allowlist or upstream guard the test cannot reach), a named acceptance test cannot go red: keep it as the end-to-end pin, state which new line it leaves unproven, and pass that gap to review instead of faking a seam.
- Green: write the least code that passes it.
- Verify green: run the test, then the suites the change can reach. Report every failure by name, even one you think is unrelated.
- Refactor with tests green, then run them again.

Code written before its test is deleted and rewritten test first. Skip the test only when the user agrees: throwaway prototypes, generated code, pure config. Hard to test means the design needs a seam; say so instead of skipping. For a bug, the first test reproduces the bug and fails.

Good tests (details in `references/writing-good-tests.md`): name the production change that would make the test fail; derive expected values independently (never recompute them with code under test); assert behaviour, not that a mock was called; keep test-only code out of production; know a dependency's side effects before you mock it. A privacy boundary needs one test per path that carries the field: the realtime row's hidden list and the server serializer's allowlist each get an assertion, including an unknown key so the serializer fails closed. A counter assertion starts from the fixture's seed, not zero: read the fixture's initial value, then state only the increments the production path adds, or the test fails on the fixture instead of the behaviour. A test for an absence (no marker, no stored record, no file) also drives the reader of that state to its not-found result: a bare `!contains` assertion passes whenever the code fails before the write it should have made.

## Renames and bulk rewrites

A pure identifier rename is one mechanical rewrite, not a patch per site: `grep -rl` the OLD name across code, tests, fixtures and docs, assert the exact occurrence count per file, replace, assert zero remain, then run the formatter and read the diff yourself. A ticket or design list of files is a starting point, not the whole set; construction sites in test files and fixtures are what a rename misses. When a count differs, read the file and correct the expectation before rewriting; never drop the assertion. Run the formatter after the rename, because new names reorder imports.

## Debugging

Use this for any bug, failing test or surprise, before you change code. Redact any secret as `<REDACTED>` in commands, logs and outputs you show.

1. Feedback loop first. Build a tight red signal before reading code to theorize: a failing test, curl/HTTP probe, CLI snapshot diff, headless browser script, replayed trace, fuzz loop or bisection harness. Watch it fail with the exact symptom reported.
2. Investigate and minimize. Read the whole error, stack trace and recent changes (`git log`, `git diff`). Shrink the repro until every remaining input, config and step is load-bearing. Log what enters and leaves boundaries with a unique prefix (`[DEBUG-<id>]`) so one `grep` deletes all probes later, and trace the bad value backwards (`references/root-cause-tracing.md`). For performance regressions, measure a baseline and bisect instead of logging.
3. Compare and hypothesize. Compare against similar working code and list every difference. Generate 3-5 ranked falsifiable hypotheses ("if X is the cause, changing Y does Z") before testing any; show the list when the user is present so domain knowledge can re-rank it. Test one variable at a time.
4. Fix. If a regression test would sit on too shallow a seam, flag the missing seam instead of trusting a false-confidence test. Otherwise turn the minimized repro into a failing test at the real call-site seam, make one fix at the root cause, and verify against the original un-minimized scenario. Add checks at each layer the bad value crossed when that prevents a repeat (`references/defense-in-depth.md`). Replace sleeps and timeouts in tests with waits on the real condition (`references/condition-based-waiting.md`). Find a test that pollutes shared state with `references/find-polluter.sh`.
5. Clean up. Confirm every `[DEBUG-<id>]` probe and throwaway harness is removed, and state the confirmed root cause in the commit or report.

After three failed fixes, stop. The architecture is the likely problem; bring the evidence to the user before a fourth attempt. Red flags that mean go back to step 1: theorizing before a red loop exists, "quick fix for now", several changes at once, a fix you cannot explain.

## Verify before you claim

Before any claim of status: name the command that proves it, run it fresh and in full, read the output and the exit status, then state the result with the evidence.

Capture the status before you filter the output: `cmd > /tmp/out.log 2>&1; status=$?; tail -20 /tmp/out.log`. In a pipeline such as `cmd | tail`, `$?` is the viewer's status, so a failing check reads as a pass. A check run as a background job must end with `exit $status`: a trailing `echo status=$?` leaves the job's own exit status 0, so the runner reports success for a failed check. Capture with `cmd; status=$?`, not `${PIPESTATUS[0]}`: the harness shell is `sh`, where that expansion is a syntax error after the command already ran, so a successful commit reports failure.

| Claim | Needs |
| --- | --- |
| Tests pass | Test output with 0 failures, this run |
| Build or lint clean | Exit status 0, this run |
| Bug fixed | The test of the original symptom passes |
| Regression test works | It failed without the fix and passes with it |
| Subagent finished | You read the diff yourself; its report is not evidence |
| Ticket done | Each acceptance criterion checked line by line |

Words such as "should", "probably" or "seems to" mean you have not verified. Run the command.

A check that fails only on its total time budget while every stage reports green is a cold-build overrun: rerun it once warm and report the overrun. Never raise, skip or soften the budget.

A single test that times out in an otherwise green `nextest` run usually hit parallel-host load, not a regression: rerun that one by name and, when it passes in isolation, report both runs as a flake instead of claiming a clean pass or a broken build.

A gate stage that regenerates committed artifacts and diffs them against `HEAD` (an OpenAPI snapshot, a generated schema) fails while those artifacts are uncommitted, however correct the content: prove the regeneration stage itself is green and idempotent, record the caveat in the ticket, and let the commit close it. Never "fix" it by regenerating again or hand-editing `HEAD`'s copy.

A gate stage that fetches a floating tool (`bunx <tool>` with no pinned version) can go red on findings that pre-date the ticket when a newer release adds checks. Prove a finding is pre-existing before attributing it: `git status --short` shows its file unmodified and `git show <startingPoint>:<file>` matches. Record the command and the finding list in the ticket; leave out-of-scope files alone, because editing them to buy a green gate mixes another owner's work into the ticket diff. When the user authorizes clearing those findings, fix each at its root (delete an unused export or drop `export` when the symbol is internal, move a test-only package to `devDependencies` and refresh the lockfile, point an alias import at a relative path), rerun the same gate, and record the finding list in the ticket.

On a Rust change, run the crate's clippy before the full check so a lint failure costs seconds, not the gate. A new lint such as `clippy::cognitive_complexity` means the branch belongs in a named helper that returns the decision: extract it and re-run instead of adding an `allow`.

## Delegating to workers

Write the code yourself by default. Use workers when the user asks for it, or for disjoint tickets that can run at once.

- Tier `mid` unless the user allows `top`. One owner per file or ticket; two workers never edit the same file.
- The worker reads the ticket file and this skill; its brief names the ticket path, starting point, files it owns, the focused check command, and "end with a submit call".
- Keep a worker alive across tickets with `steer`; one long-lived worker running tickets in sequence beats a new spawn per ticket.
- Workers run scoped checks; you run the full check once, after their diffs land. Serialize heavy builds if the repository has a lock (for example `flock /tmp/<repo>-build.lock <command>`).
- Read every worker diff before review. A worker report that says "done" proves nothing.

## Credits

The test-first, debugging and verification rules come from obra/superpowers, MIT, copyright Jesse Vincent, https://github.com/obra/superpowers at commit 22f0b46. The `references/` files and `references/LICENSE-superpowers` are unmodified copies. The ticket loop, tight debugging loop and tautological-test rule draw from Matt Pocock's `implement`, `diagnosing-bugs` and `tdd` skills; surgical-change and simplicity rules draw from `ForrestChang/andrej-karpathy-skills` (MIT), rewritten for Emberloop.
